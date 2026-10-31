//! Building blocks shared by the text, vision and audio towers.

use candle::{DType, Module, Result, Tensor, D};
use candle_nn::{linear_no_bias, Linear, VarBuilder};

// A larger score tile gives CUDA a useful amount of work per kernel launch.
// The tile is still bounded (32M f32 scores ~= 128 MiB before allocator
// overhead), while the caller's attention budget controls how many sequences
// enter a forward pass. This keeps a 1 GiB VRAM safety margin practical for
// the integrated model and avoids the many tiny launches seen at 16M.
const MAX_ATTENTION_SCORE_ELEMENTS: usize = 1 << 25;

/// RMS normalization as used across Gemma 4 and EmbeddingGemma 2: computed in f32 and scaled
/// by `weight` directly (unlike earlier Gemma checkpoints, which store `weight - 1`).
#[derive(Debug, Clone)]
pub(crate) struct RmsNorm {
    weight: Option<Tensor>,
    eps: f64,
}

impl RmsNorm {
    pub(crate) fn new(dim: usize, eps: f64, vb: VarBuilder) -> Result<Self> {
        let weight = vb.get(dim, "weight")?.to_dtype(DType::F32)?;
        Ok(Self {
            weight: Some(weight),
            eps,
        })
    }

    pub(crate) fn without_scale(eps: f64) -> Self {
        Self { weight: None, eps }
    }
}

impl Module for RmsNorm {
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let dtype = xs.dtype();
        let xs = xs.to_dtype(DType::F32)?;
        let mean_squared = (xs.sqr()?.mean_keepdim(D::Minus1)? + self.eps)?;
        let xs = xs.broadcast_mul(&mean_squared.powf(-0.5)?)?;
        let xs = match &self.weight {
            Some(weight) => xs.broadcast_mul(weight)?,
            None => xs,
        };
        xs.to_dtype(dtype)
    }
}

pub(crate) fn rotate_half(xs: &Tensor) -> Result<Tensor> {
    let half = xs.dim(D::Minus1)? / 2;
    let x1 = xs.narrow(D::Minus1, 0, half)?;
    let x2 = xs.narrow(D::Minus1, half, half)?;
    Tensor::cat(&[&x2.neg()?, &x1], D::Minus1)
}

/// Repeats each key/value head `n_rep` times: `(b, kv_heads, t, d)` to `(b, kv_heads * n_rep, t, d)`.
pub(crate) fn repeat_kv(xs: Tensor, n_rep: usize) -> Result<Tensor> {
    if n_rep == 1 {
        return Ok(xs);
    }
    let (b, kv_heads, t, d) = xs.dims4()?;
    xs.unsqueeze(2)?
        .broadcast_as((b, kv_heads, n_rep, t, d))?
        .reshape((b, kv_heads * n_rep, t, d))
}

/// `softmax(q k^T + mask) v`, with the softmax taken in f32 whatever the working dtype.
/// No `1/sqrt(d)` factor: queries and keys are RMS normalized beforehand.
pub(crate) fn attend(q: &Tensor, k: &Tensor, v: &Tensor, mask: Option<&Tensor>) -> Result<Tensor> {
    attend_with_limit(q, k, v, mask, MAX_ATTENTION_SCORE_ELEMENTS)
}

fn attend_with_limit(
    q: &Tensor,
    k: &Tensor,
    v: &Tensor,
    mask: Option<&Tensor>,
    max_score_elements: usize,
) -> Result<Tensor> {
    let dtype = q.dtype();
    let (batch, heads, query_len, _) = q.dims4()?;
    let key_len = k.dim(2)?;
    let keys = k.transpose(2, 3)?.contiguous()?;
    let per_query = batch
        .saturating_mul(heads)
        .saturating_mul(key_len)
        .max(1);
    let chunk_len = (max_score_elements / per_query).clamp(1, query_len);
    let mut outputs = Vec::new();
    for start in (0..query_len).step_by(chunk_len) {
        let len = chunk_len.min(query_len - start);
        let query = q.narrow(2, start, len)?;
        let weights = query.matmul(&keys)?.to_dtype(DType::F32)?;
        let weights = match mask {
            Some(mask) if mask.dims().len() == 4
                && mask.dim(2)? == key_len
                && mask.dim(3)? == key_len => {
                let query_mask = mask.narrow(2, start, len)?;
                weights.broadcast_add(&query_mask)?
            }
            Some(mask) => weights.broadcast_add(mask)?,
            None => weights,
        };
        let weights = candle_nn::ops::softmax_last_dim(&weights)?.to_dtype(dtype)?;
        outputs.push(weights.matmul(v)?);
    }
    let output_refs: Vec<&Tensor> = outputs.iter().collect();
    Tensor::cat(&output_refs, 2)
}

#[cfg(test)]
mod tests {
    use super::attend_with_limit;
    use candle::{Device, Tensor};

    #[test]
    fn chunked_attention_matches_full_attention() -> candle::Result<()> {
        let device = Device::Cpu;
        let q = Tensor::from_vec(
            vec![1.0f32, 0.0, 0.0, 1.0, 1.0, 1.0],
            (1, 1, 3, 2),
            &device,
        )?;
        let k = Tensor::from_vec(
            vec![1.0f32, 0.0, 0.0, 1.0, 1.0, 1.0],
            (1, 1, 3, 2),
            &device,
        )?;
        let v = Tensor::from_vec(
            vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0],
            (1, 1, 3, 2),
            &device,
        )?;
        let full = attend_with_limit(&q, &k, &v, None, 1024)?
            .reshape((6,))?
            .to_vec1::<f32>()?;
        let chunked = attend_with_limit(&q, &k, &v, None, 3)?
            .reshape((6,))?
            .to_vec1::<f32>()?;
        for (expected, actual) in full.iter().zip(&chunked) {
            assert!((expected - actual).abs() < 1e-5);
        }

        let mask = Tensor::from_vec(
            vec![
                0.0f32, -1e4, -1e4,
                0.0, 0.0, -1e4,
                0.0, 0.0, 0.0,
            ],
            (1, 1, 3, 3),
            &device,
        )?;
        let full = attend_with_limit(&q, &k, &v, Some(&mask), 1024)?
            .reshape((6,))?
            .to_vec1::<f32>()?;
        let chunked = attend_with_limit(&q, &k, &v, Some(&mask), 3)?
            .reshape((6,))?
            .to_vec1::<f32>()?;
        for (expected, actual) in full.iter().zip(&chunked) {
            assert!((expected - actual).abs() < 1e-5);
        }
        Ok(())
    }
}

/// A bias-free linear layer that optionally clamps its input and its output to ranges stored
/// in the checkpoint. The clamps are left over from quantization aware training.
#[derive(Debug, Clone)]
pub(crate) struct ClippableLinear {
    linear: Linear,
    clip: Option<Clip>,
}

#[derive(Debug, Clone)]
struct Clip {
    input: (Tensor, Tensor),
    output: (Tensor, Tensor),
}

impl ClippableLinear {
    pub(crate) fn new(
        in_dim: usize,
        out_dim: usize,
        use_clipped_linears: bool,
        vb: VarBuilder,
    ) -> Result<Self> {
        let linear = linear_no_bias(in_dim, out_dim, vb.pp("linear"))?;
        let clip = if use_clipped_linears {
            let bound = |name: &str| vb.get((), name);
            Some(Clip {
                input: (bound("input_min")?, bound("input_max")?),
                output: (bound("output_min")?, bound("output_max")?),
            })
        } else {
            None
        };
        Ok(Self { linear, clip })
    }
}

fn clamp(xs: &Tensor, (min, max): &(Tensor, Tensor)) -> Result<Tensor> {
    xs.broadcast_maximum(min)?.broadcast_minimum(max)
}

impl Module for ClippableLinear {
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        match &self.clip {
            None => self.linear.forward(xs),
            Some(clip) => clamp(
                &self.linear.forward(&clamp(xs, &clip.input)?)?,
                &clip.output,
            ),
        }
    }
}
