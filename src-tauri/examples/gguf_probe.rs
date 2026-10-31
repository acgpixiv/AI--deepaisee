use deepdark_oracle_lib::ai::llama::LlamaEmbedder;
use deepdark_oracle_lib::ai::Embedder;
use std::path::Path;

fn main() -> Result<(), String> {
    let model = r"E:\googleembeddinggamma-2\guff model\embeddinggemma-2-BF16.gguf";
    let started = std::time::Instant::now();
    let embedder = LlamaEmbedder::load(model, 8192, 0, "auto")?;
    let info = embedder.info();
    println!("loaded backend={} dim={} device={} elapsed={:.1}s", info.backend, info.dim, info.device, started.elapsed().as_secs_f64());
    println!("modalities={:?} detail={}", info.modalities, info.detail);
    let batch = vec![
        "Volkswagen ESP electronic stability program".to_string(),
        "ABS 与电子稳定控制系统".to_string(),
    ];
    let vectors = embedder.embed_text_batch(&batch)?;
    println!("text batch count={} dims={:?}", vectors.len(), vectors.iter().map(Vec::len).collect::<Vec<_>>());
    for (index, vector) in vectors.iter().enumerate() {
        report(&format!("text batch {index}"), vector);
        validate("text batch", vector)?;
    }
    let vector = embedder.embed_text(&batch[0], false)?;
    report("text", &vector);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(kind) = args.first() {
        if kind == "text-bench" {
            let count = args.get(1).and_then(|value| value.parse::<usize>().ok()).unwrap_or(32);
            let texts: Vec<String> = (0..count)
                .map(|index| format!("benchmark text document {index}: Volkswagen electronic stability program and braking control"))
                .collect();
            let serial_start = std::time::Instant::now();
            let mut serial_vectors = Vec::with_capacity(texts.len());
            for text in &texts {
                serial_vectors.push(embedder.embed_text(text, false)?);
            }
            let serial_elapsed = serial_start.elapsed();
            let batch_start = std::time::Instant::now();
            let batch = embedder.embed_text_batch(&texts)?;
            let batch_elapsed = batch_start.elapsed();
            println!(
                "text benchmark count={} serial={:.3}s batch={:.3}s speedup={:.2}x",
                count,
                serial_elapsed.as_secs_f64(),
                batch_elapsed.as_secs_f64(),
                serial_elapsed.as_secs_f64() / batch_elapsed.as_secs_f64().max(1e-9)
            );
            if batch.len() != count {
                return Err("text batch returned an unexpected number of vectors".into());
            }
            let minimum_cosine = serial_vectors
                .iter()
                .zip(batch.iter())
                .map(|(left, right)| {
                    left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>()
                })
                .fold(1.0f32, f32::min);
            println!("text benchmark minimum serial/batch cosine={minimum_cosine:.6}");
            return Ok(());
        }
        if kind == "image-batch" {
            let paths: Vec<_> = args.iter().skip(1).map(Path::new).collect();
            if paths.is_empty() {
                return Err("pass one or more image paths after image-batch".into());
            }
            let start = std::time::Instant::now();
            let vectors = embedder.embed_image_batch(&paths)?;
            println!("image batch count={} elapsed={:.3}s", vectors.len(), start.elapsed().as_secs_f64());
            if vectors.len() != paths.len() {
                return Err("image batch returned an unexpected number of vectors".into());
            }
            let mut minimum_cosine = 1.0f32;
            for (index, vector) in vectors.iter().enumerate() {
                report(&format!("image batch {index}"), vector);
                validate("image batch", vector)?;
                let serial = embedder.embed_image(paths[index])?;
                let cosine = serial.iter().zip(vector).map(|(a, b)| a * b).sum::<f32>();
                minimum_cosine = minimum_cosine.min(cosine);
            }
            println!("image batch minimum ordered serial/batch cosine={minimum_cosine:.6}");
            return Ok(());
        }
        if kind == "image-bench" {
            let paths: Vec<_> = args.iter().skip(1).map(Path::new).collect();
            if paths.is_empty() {
                return Err("pass one or more image paths after image-bench".into());
            }
            let serial_start = std::time::Instant::now();
            let mut serial_vectors = Vec::with_capacity(paths.len());
            for path in &paths {
                let vector = embedder.embed_image(path)?;
                validate("serial image", &vector)?;
                serial_vectors.push(vector);
            }
            let serial_elapsed = serial_start.elapsed();
            let batch_start = std::time::Instant::now();
            let vectors = embedder.embed_image_batch(&paths)?;
            let batch_elapsed = batch_start.elapsed();
            println!(
                "image benchmark count={} serial={:.3}s batch={:.3}s speedup={:.2}x",
                paths.len(),
                serial_elapsed.as_secs_f64(),
                batch_elapsed.as_secs_f64(),
                serial_elapsed.as_secs_f64() / batch_elapsed.as_secs_f64().max(1e-9)
            );
            let mut minimum_cosine = 1.0f32;
            for (index, (vector, serial)) in vectors.iter().zip(serial_vectors).enumerate() {
                validate(&format!("batch image {index}"), vector)?;
                let cosine = serial.iter().zip(vector).map(|(a, b)| a * b).sum::<f32>();
                minimum_cosine = minimum_cosine.min(cosine);
            }
            println!("image benchmark minimum ordered serial/batch cosine={minimum_cosine:.6}");
            return Ok(());
        }
        let path = args.get(1).ok_or("pass a media path after image/audio/video")?;
        match kind.as_str() {
            "image" => {
                let vector = embedder.embed_image(Path::new(path))?;
                report(kind, &vector);
                validate(kind, &vector)?;
            }
            "pdf-image" => {
                let image = image::open(path).map_err(|error| error.to_string())?;
                let vector = embedder.embed_pdf_image_data(&image)?;
                report(kind, &vector);
                validate(kind, &vector)?;
            }
            "audio" => {
                let vector = embedder.embed_audio(Path::new(path))?;
                report(kind, &vector);
                validate(kind, &vector)?;
            }
            "video" => {
                let vectors = embedder.embed_video_segments(Path::new(path))?;
                println!("video windows={}", vectors.len());
                for (index, vector) in vectors.iter().enumerate() {
                    report(&format!("video window {index}"), vector);
                    validate("video window", vector)?;
                }
            }
            other => return Err(format!("unknown probe mode: {other}")),
        }
    }
    Ok(())
}

fn report(name: &str, vector: &[f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    println!("{name} embedding dim={} norm={norm:.5}", vector.len());
}

fn validate(name: &str, vector: &[f32]) -> Result<(), String> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if vector.len() != 768 || vector.iter().any(|value| !value.is_finite()) || (norm - 1.0).abs() > 1e-3 {
        return Err(format!("{name} returned invalid 768-D unit-vector values (norm={norm})"));
    }
    Ok(())
}
