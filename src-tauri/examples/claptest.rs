//! CLAP 音文对齐质量实测：合成音频 × 中英文提示词，看余弦矩阵。
//!
//! 用法：`cargo run --bin claptest -- <clap模型目录> [音频目录]`
//! 音频目录留空则跳过音频实测，只打印模型加载与分词结果。
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(args.get(1).map(|s| s.as_str()).unwrap_or("models/clap"));
    eprintln!("dir={} exists={}", dir.display(), dir.exists());
    for f in ["audio_model.onnx", "text_model.onnx", "vocab.json", "merges.txt"] {
        eprintln!("  {} -> {}", f, dir.join(f).exists());
    }
    eprintln!("dll = {:?}", deepdark_oracle_lib::ai::clip::find_dll());
    // 分步复现 load() 内部，把吞掉的错误打出来
    {
        use deepdark_oracle_lib::ai::ort::Ort;
        let dll = deepdark_oracle_lib::ai::clip::find_dll();
        match dll.map(|d| Ort::load(&d, 8)) {
            None => eprintln!("!! 找不到 onnxruntime.dll"),
            Some(Err(e)) => eprintln!("!! Ort::load 失败：{e}"),
            Some(Ok(ort)) => {
                for f in ["audio_model.onnx", "text_model.onnx"] {
                    match ort.session(&dir.join(f)) {
                        Ok(_) => eprintln!("   session {f} OK"),
                        Err(e) => eprintln!("!! session {f} 失败：{e}"),
                    }
                }
            }
        }
        eprintln!("   bpe = {}", if deepdark_oracle_lib::ai::bpe::Bpe::load(dir).is_some() { "OK" } else { "None" });
    }
    let m = deepdark_oracle_lib::ai::clap::ClapModel::load(dir, 8).expect("加载 CLAP 失败");
    println!("CLAP 已加载，dim={}", m.dim());

    // 音频目录从第 2 个参数取；不给就只跑文本侧，音频实测自动跳过。
    let audio_dir = Path::new(args.get(2).map(|s| s.as_str()).unwrap_or(""));
    let files = ["piano.wav", "rain.wav", "speech.wav", "drum.wav"];
    let prompts_cn = ["钢琴声", "下雨的声音", "有人说话", "鼓点节奏"];
    let prompts_en = ["piano music", "rain falling", "people talking", "drum beats"];

    // 决定性实验：同一段音频喂两次、不同音频对比，看输出是否真的随输入变化。
    // 若不同音频的向量几乎重合（cos≈1），说明输入张量没有真正喂进去。
    {
        let a = deepdark_oracle_lib::media::load_audio_48k(&audio_dir.join("piano.wav"), 10.0);
        let b = deepdark_oracle_lib::media::load_audio_48k(&audio_dir.join("rain.wav"), 10.0);
        if let (Some(pa), Some(pb)) = (a, b) {
            println!("\n=== 原始张量诊断 ===");
            for (tag, pcm) in [("piano", &pa), ("rain", &pb)] {
                let f = m.debug_mel(pcm);
                let mn = f.iter().cloned().fold(f32::MAX, f32::min);
                let mx = f.iter().cloned().fold(f32::MIN, f32::max);
                let mean = f.iter().sum::<f32>() / f.len() as f32;
                println!("{tag} mel: min={mn:.4} max={mx:.4} mean={mean:.4}");
                // 真喂两次同一张量、再喂另一张，看输出是否变化
                let shape = [1i64, 1, 1001, 64];
                let o1 = m.raw_audio(&f, &shape);
                let o2 = m.raw_audio(&f, &shape);
                if let (Ok(x), Ok(y)) = (o1, o2) {
                    println!("  raw 两次同输入 cos={:.6} 前4={:?}", dot_n(&x, &y), &x[..4.min(x.len())]);
                }
            }
            // 直接把 piano 的 mel 换成 rain 的前后两半混合，输出应该不同
            let fa = m.debug_mel(&pa);
            let fb = m.debug_mel(&pb);
            let shape = [1i64, 1, 1001, 64];
            match (m.raw_audio(&fa, &shape), m.raw_audio(&fb, &shape)) {
                (Ok(x), Ok(y)) => println!("  piano vs rain 原始输出 cos={:.6}", dot_n(&x, &y)),
                (Err(e), _) => println!("  raw 失败：{e}"),
                _ => {}
            }
        }
    }

    let mut av = Vec::new();
    let have_audio = audio_dir.is_dir();
    if !have_audio {
        println!("（未提供音频目录，跳过音频实测；只验证文本塔）");
    }
    let audio_files: &[&str] = if have_audio { &files } else { &[] };
    for f in audio_files {
        let p = audio_dir.join(f);
        if let Some(pcm) = deepdark_oracle_lib::media::load_audio_48k(&p, 10.0) {
            let rms = (pcm.iter().map(|x| x * x).sum::<f32>() / pcm.len().max(1) as f32).sqrt();
            let peak = pcm.iter().fold(0f32, |a, b| a.max(b.abs()));
            println!("{f}: pcm={} 样本 rms={rms:.4} peak={peak:.4}", pcm.len());
        } else {
            println!("{f}: load_audio_48k 失败");
        }
        let t = std::time::Instant::now();
        match m.audio_vec(&audio_dir.join(f)) {
            Ok(v) => {
                let mn = v.iter().cloned().fold(f32::MAX, f32::min);
                let mx = v.iter().cloned().fold(f32::MIN, f32::max);
                let mean = v.iter().sum::<f32>() / v.len() as f32;
                println!(
                    "{} → {} 维，{:?}  min={mn:.4} max={mx:.4} mean={mean:.4}",
                    f,
                    v.len(),
                    t.elapsed()
                );
                av.push(v);
            }
            Err(e) => {
                println!("{} 失败：{}", f, e);
                return;
            }
        }
    }

    // 先看音频彼此之间像不像：如果这一段就全是 0.9+，说明特征提取没区分度，
    // 跟文本空间对不对齐无关。
    if have_audio {
        println!("\n=== 音频互相之间的余弦 ===");
        print!("{:<14}", "");
        for f in files {
            print!("{:>12}", trunc(f, 10));
        }
        println!();
        for (i, f) in files.iter().enumerate() {
            print!("{:<14}", f);
            for (j, _) in files.iter().enumerate() {
                print!("{:>12.4}", dot(&av[i], &av[j]));
            }
            println!();
        }
    }

    for (tag, prompts) in [("中文", prompts_cn), ("英文", prompts_en)] {
        let mut tv = Vec::new();
        for p in prompts {
            let t = std::time::Instant::now();
            match m.text_vec(p) {
                Ok(v) => {
                    println!("文本「{}」→ {} 维，{:?}", p, v.len(), t.elapsed());
                    tv.push(v);
                }
                Err(e) => {
                    println!("文本「{}」失败：{}", p, e);
                    return;
                }
            }
        }
        if !have_audio {
            // 没有音频就只打印「文本互相之间的余弦」，用来确认文本塔本身有区分度。
            println!("\n=== {} 文本互相之间的余弦 ===", tag);
            for (i, a) in tv.iter().enumerate() {
                for (j, b) in tv.iter().enumerate().skip(i + 1) {
                    println!("  {} ↔ {} = {:.4}", trunc(prompts[i], 10), trunc(prompts[j], 10), dot(a, b));
                }
            }
            continue;
        }
        println!("\n=== {} 余弦矩阵（行=音频，列=文本）===", tag);
        print!("{:<14}", "");
        for p in prompts {
            print!("{:>12}", trunc(p, 10));
        }
        println!();
        let mut hit = 0;
        for (i, f) in files.iter().enumerate() {
            print!("{:<14}", f);
            let mut best = (0usize, f32::MIN);
            for (j, _) in prompts.iter().enumerate() {
                let c = dot(&av[i], &tv[j]);
                print!("{:>12.4}", c);
                if c > best.1 {
                    best = (j, c);
                }
            }
            if best.0 == i {
                hit += 1;
            }
            println!("   → 最像「{}」{}", trunc(prompts[best.0], 10), if best.0 == i { " ✓" } else { "" });
        }
        println!("{} 命中 {}/4\n", tag, hit);
    }
}

fn trunc(s: &str, n: usize) -> String {
    let v: Vec<char> = s.chars().collect();
    if v.len() <= n {
        s.to_string()
    } else {
        v[..n].iter().collect()
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn dot_n(a: &[f32], b: &[f32]) -> f32 {
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot(a, b) / (na * nb)
}
