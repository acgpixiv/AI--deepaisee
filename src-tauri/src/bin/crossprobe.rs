// 跨模态探针：验证 CLIP/CLAP 在本机的真实检索质量。
//
// 用法：
//   crossprobe <模型根目录> <音频/图片目录>
//
// 会打印每个文件与每句描述之间的余弦相似度矩阵，用来判定
// 「文字找图/找声」到底有没有区分度，而不是凭主观感觉。
use std::path::Path;

use deepaisee_lib::ai::{clap::ClapModel, clip::ClipModel};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法：crossprobe <模型根目录> <媒体目录>");
        std::process::exit(2);
    }
    let root = Path::new(&args[1]);
    let media_dir = Path::new(&args[2]);

    println!("== CLIP ==");
    if let Some(clip) = ClipModel::load(&root.join("clip"), 8) {
        let prompts = [
            "一只黑色的狗",
            "海边的沙滩和海浪",
            "城市夜景的街道",
            "绿色的草地",
        ];
        let files: Vec<_> = std::fs::read_dir(media_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let p = e.path();
                matches!(
                    p.extension().and_then(|x| x.to_str()).map(|s| s.to_lowercase()).as_deref(),
                    Some("jpg") | Some("jpeg") | Some("png")
                )
            })
            .collect();
        println!("{:24}", "");
        for p in &prompts {
            print!("{:>14}", p);
        }
        println!();
        for f in &files {
            let p = f.path();
            let name = f.file_name().to_string_lossy().to_string();
            match clip.image_vec(&p) {
                Ok(iv) => {
                    print!("{:24}", &name[..name.len().min(22)]);
                    for q in &prompts {
                        let tv = clip.text_vec(q).unwrap_or_default();
                        print!("{:>14.4}", cosine(&iv, &tv));
                    }
                    println!();
                }
                Err(e) => println!("{name}: 失败 {e}"),
            }
        }
    } else {
        println!("CLIP 未加载（检查 {}/clip", root.display());
    }

    println!("\n== CLAP ==");
    if let Some(clap) = ClapModel::load(&root.join("clap"), 8) {
        let prompts = ["a dog barking", "rain", "piano music", "people talking"];
        let files: Vec<_> = std::fs::read_dir(media_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let p = e.path();
                matches!(
                    p.extension().and_then(|x| x.to_str()).map(|s| s.to_lowercase()).as_deref(),
                    Some("wav") | Some("mp3") | Some("m4a") | Some("flac")
                )
            })
            .collect();
        println!("{:24}", "");
        for p in &prompts {
            print!("{:>16}", p);
        }
        println!();
        for f in &files {
            let p = f.path();
            let name = f.file_name().to_string_lossy().to_string();
            match clap.audio_vec(&p) {
                Ok(av) => {
                    print!("{:24}", &name[..name.len().min(22)]);
                    for q in &prompts {
                        let tv = clap.text_vec(q).unwrap_or_default();
                        print!("{:>16.4}", cosine(&av, &tv));
                    }
                    println!();
                }
                Err(e) => println!("{name}: 失败 {e}"),
            }
        }
    } else {
        println!("CLAP 未加载（检查 {}/clap", root.display());
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return f32::NAN;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
