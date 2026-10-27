//! 视觉标签调试工具：给一批图片打印中文标签。
//!
//! 用法：`visdemo.exe <图片或目录>...`
//! 用于验证 `vision.rs` 的判定规则在真实照片上的效果。
use std::path::Path;

fn walk(p: &Path, out: &mut Vec<std::path::PathBuf>) {
    if p.is_file() {
        out.push(p.to_path_buf());
        return;
    }
    let Ok(rd) = std::fs::read_dir(p) else { return };
    for e in rd.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(&path, out);
        } else if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            if matches!(
                ext.to_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "bmp" | "webp" | "gif"
            ) {
                out.push(path);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: visdemo <图片或目录>... [--profile]");
        std::process::exit(2);
    }
    let show_profile = args.iter().any(|a| a == "--profile");
    let paths: Vec<&String> = args.iter().filter(|a| *a != "--profile").collect();
    let mut files = Vec::new();
    for a in paths {
        walk(Path::new(a), &mut files);
    }
    files.sort();
    println!("待分析 {} 个文件\n", files.len());
    for f in &files {
        let name = f.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        let tags = deepdark_oracle_lib::vision::describe_file(f, "image");
        if tags.is_empty() {
            println!("{name:<28} （无法读取）");
            continue;
        }
        println!("{name:<28} {}", tags.join(" / "));
        if show_profile {
            if let Ok(img) = image::open(f) {
                let p = deepdark_oracle_lib::vision::analyze(&img.to_rgb8());
                println!(
                    "{:<28} hue={:?} sat={:.2} val={:.2} top={:.2} bot={:.2} sky={} \
                     blue={:.2} green={:.2} warm={:.2} white={:.2} dark={:.2} gray={:.2} edge={:.2}",
                    "",
                    p.hue.map(|h| (h * 10.0).round() / 10.0),
                    p.sat,
                    p.val,
                    p.top_val,
                    p.bottom_val,
                    p.sky_layout,
                    p.blue,
                    p.green,
                    p.warm,
                    p.white,
                    p.dark,
                    p.gray,
                    p.edge
                );
            }
        }
    }
}
