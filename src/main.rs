//! 程序入口:启动交互式 REPL。

use rust_final_project::repl;

fn main() {
    if let Err(e) = repl::run() {
        eprintln!("致命错误: {e}");
        std::process::exit(1);
    }
}
