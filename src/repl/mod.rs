//! 交互式 REPL:输入 SQL 执行并表格化打印,或执行元命令。

mod command;
mod format;

use std::time::Instant;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::error::{QueryError, Result};
use crate::execution::SessionContext;

/// 启动 REPL 主循环。
pub fn run() -> Result<()> {
    let mut ctx = SessionContext::new();
    let mut editor =
        DefaultEditor::new().map_err(|e| QueryError::exec(format!("初始化 REPL 失败: {e}")))?;

    println!("列式分析查询引擎 REPL。输入 .help 查看命令,.quit 退出。");

    loop {
        match editor.readline("sql> ") {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);
                if line.starts_with('.') {
                    if let command::MetaResult::Quit = command::handle_meta(&mut ctx, line) {
                        break;
                    }
                } else {
                    run_sql(&ctx, line);
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("读取错误: {e}");
                break;
            }
        }
    }
    Ok(())
}

fn run_sql(ctx: &SessionContext, sql: &str) {
    let start = Instant::now();
    match ctx.sql(sql) {
        Ok(batches) => {
            println!("{}", format::format_batches(&batches));
            println!("耗时 {:?}", start.elapsed());
        }
        Err(e) => eprintln!("{e}"),
    }
}
