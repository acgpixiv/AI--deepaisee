use deepdark_oracle_lib::ai::build_embedder_with_index;
use deepdark_oracle_lib::config::{IndexConfig, ModelConfig};

fn main() -> Result<(), String> {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| r"E:\googleembeddinggamma-2\safetensors model".to_string());
    let mut cfg = ModelConfig::default();
    cfg.embed_model = dir;
    cfg.context_size = 8192;
    cfg.gpu_device = "CUDA|0000:01:00.0".into();
    let index = IndexConfig::default();
    let started = std::time::Instant::now();
    let model = build_embedder_with_index(&cfg, &index, 8)?;
    let info = model.info();
    println!("loaded backend={} dim={} device={} modalities={:?} elapsed={:.1}s", info.backend, info.dim, info.device, info.modalities, started.elapsed().as_secs_f64());

    let texts = vec![
        "Volkswagen ESP electronic stability program".to_string(),
        "汽车的电子稳定控制系统如何工作".to_string(),
    ];
    let vectors = model.embed_text_batch(&texts)?;
    println!("text batch count={} dims={:?}", vectors.len(), vectors.iter().map(Vec::len).collect::<Vec<_>>());

    let benchmark_count = std::env::var("DEEPAISEE_SAFETENSORS_BENCH_IMAGES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
        .clamp(0, 16);
    if benchmark_count > 0 {
        let images: Vec<image::DynamicImage> = (0..benchmark_count)
            .map(|_| image::DynamicImage::new_rgb8(1024, 1024))
            .collect();
        let serial_start = std::time::Instant::now();
        let mut serial = Vec::with_capacity(images.len());
        for image in &images {
            serial.push(model.embed_image_data(image)?);
        }
        let serial_elapsed = serial_start.elapsed();
        let refs: Vec<&image::DynamicImage> = images.iter().collect();
        let batch_start = std::time::Instant::now();
        let batched = model.embed_image_data_batch(&refs)?;
        let batch_elapsed = batch_start.elapsed();
        let min_cosine = serial
            .iter()
            .zip(&batched)
            .map(|(left, right)| left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>())
            .fold(1.0f32, f32::min);
        println!(
            "image benchmark count={} serial={:.3}s batch={:.3}s speedup={:.2}x min_cosine={:.6}",
            benchmark_count,
            serial_elapsed.as_secs_f64(),
            batch_elapsed.as_secs_f64(),
            serial_elapsed.as_secs_f64() / batch_elapsed.as_secs_f64().max(1e-9),
            min_cosine
        );
        return Ok(());
    }

    let images = vec![
        image::DynamicImage::new_rgb8(1024, 1024),
        image::DynamicImage::new_rgb8(1024, 1024),
    ];
    let image_refs: Vec<&image::DynamicImage> = images.iter().collect();
    let vectors = model.embed_image_data_batch(&image_refs)?;
    let norms: Vec<f32> = vectors
        .iter()
        .map(|vector| vector.iter().map(|value| value * value).sum::<f32>().sqrt())
        .collect();
    println!("image batch count={} dims={:?} norms={norms:?} elapsed={:.1}s", vectors.len(), vectors.iter().map(Vec::len).collect::<Vec<_>>(), started.elapsed().as_secs_f64());
    Ok(())
}
