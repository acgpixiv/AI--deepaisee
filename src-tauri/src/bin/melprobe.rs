// 预处理对齐探针：把 Rust 与 Python 的 mel/文本向量逐项对比。
//
// 用法：melprobe <模型根目录> <音频文件>
use std::path::Path;

use deepaisee_lib::ai::{clap::ClapModel, clip::ClipModel};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法：melprobe <模型根目录> <音频文件>");
        std::process::exit(2);
    }
    let root = Path::new(&args[1]);
    let audio = Path::new(&args[2]);

    let clap = ClapModel::load(&root.join("clap"), 8).expect("CLAP 未加载");
    let pcm = deepaisee_lib::media::load_audio_48k(audio, 10.0).expect("读不出音频");
    println!("pcm_len={}", pcm.len());
    let mel = clap.debug_mel(&pcm);
    println!("mel_len={}", mel.len());
    let mut sum = 0f64;
    for v in &mel {
        sum += *v as f64;
    }
    println!("mel_sum={sum:.6}");
    // 打印几帧的若干 bin，便于和 Python 精确对齐
    for frame in [0usize, 1, 500, 1000] {
        let base = frame * 64;
        if base + 8 <= mel.len() {
            print!("frame{frame}:");
            for i in 0..8 {
                print!(" {:.5}", mel[base + i]);
            }
            println!();
        }
    }

    let prompts = ["rain falling", "piano music", "a person speaking", "birds chirping"];
    for p in prompts {
        let v = clap.text_vec(p).expect("文本编码失败");
        print!("text[{p}] n={} first:", v.len());
        for i in 0..5 {
            print!(" {:.6}", v[i]);
        }
        println!();
    }
    println!("--- tokens ---");
    for p in prompts {
        let ids = deepaisee_lib::ai::bpe::debug_tokens(p, 12);
        println!("{p} => {ids:?}");
    }

    let av = clap.audio_vec(audio).expect("音频编码失败");
    print!("audio_vec first:");
    for i in 0..5 {
        print!(" {:.6}", av[i]);
    }
    println!();
    for p in prompts {
        let tv = clap.text_vec(p).unwrap_or_default();
        let c: f32 = av.iter().zip(&tv).map(|(a, b)| a * b).sum();
        println!("cos({p}) = {c:.6}");
    }

    if let Some(clip) = ClipModel::load(&root.join("clip"), 8) {
        let tv = clip.text_vec("一只黑色的狗").unwrap_or_default();
        print!("clip_text first:");
        for i in 0..5 {
            print!(" {:.6}", tv[i]);
        }
        println!();
    }
}
