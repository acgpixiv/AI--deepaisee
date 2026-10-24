#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // --selftest：无界面自检，结果写到 exe 同级 selftest.txt。
    // 打包版没有控制台，这是让用户/我确认「装好了能用」的唯一手段。
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--selftest") {
        std::process::exit(deepaisee_lib::selftest::run());
    }
    if args.iter().any(|a| a == "--cli") {
        std::process::exit(deepaisee_lib::cli::run(&args));
    }
    // 发布版没有控制台，panic 默认直接静默退出。装个钩子写文件，
    // 否则用户只会看到「双击后窗口闪一下就没了」。
    std::panic::set_hook(Box::new(|info| {
        let dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default();
        let mut msg = format!("panic: {info}\n");
        let bt = std::backtrace::Backtrace::force_capture();
        msg.push_str(&format!("{bt}\n"));
        let _ = std::fs::create_dir_all(dir.join("data"));
        let _ = std::fs::write(dir.join("data").join("panic.log"), &msg);
        let _ = std::fs::write(dir.join("panic.log"), &msg);
    }));
    deepaisee_lib::run()
}
