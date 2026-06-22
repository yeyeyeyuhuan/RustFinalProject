//! 元命令处理:`.load` / `.schema` / `.tables` / `.explain` / `.help` / `.quit`。

use super::format;
use crate::execution::SessionContext;

/// 元命令处理结果。
pub enum MetaResult {
    Continue,
    Quit,
}

const HELP: &str = "\
可用命令:
  .load <csv文件> <表名>   把 CSV 加载为命名表
  .schema <表名>          查看表结构
  .tables                 列出已注册的表
  .explain <SQL>          打印该查询的逻辑计划树(优化前后)
  .parallel <on|off>      开启 / 关闭 partitioned 并行执行
  .help                   显示本帮助
  .quit                   退出
直接输入 SQL(以 SELECT 开头)即可执行查询。";

/// 处理一条以 `.` 开头的元命令。
pub fn handle_meta(ctx: &mut SessionContext, line: &str) -> MetaResult {
    let parts: Vec<&str> = line.split_whitespace().collect();
    match parts.as_slice() {
        [".quit"] | [".exit"] => return MetaResult::Quit,
        [".help"] => println!("{HELP}"),
        [".tables"] => {
            let names = ctx.table_names();
            if names.is_empty() {
                println!("(暂无已注册的表)");
            } else {
                println!("{}", names.join(", "));
            }
        }
        [".load", path, name] => match ctx.register_csv(name, path) {
            Ok(()) => println!("已加载表 `{name}`(来自 {path})"),
            Err(e) => eprintln!("加载失败: {e}"),
        },
        [".schema", name] => match ctx.table_schema(name) {
            Some(schema) => println!("{}", format::format_schema(name, &schema)),
            None => eprintln!("表 `{name}` 不存在"),
        },
        [".parallel", mode] => match *mode {
            "on" => {
                ctx.set_parallel(true);
                println!("并行执行:已开启");
            }
            "off" => {
                ctx.set_parallel(false);
                println!("并行执行:已关闭");
            }
            _ => eprintln!("用法: .parallel <on|off>"),
        },
        _ if line.starts_with(".explain") => {
            let sql = line[".explain".len()..].trim();
            match ctx.explain(sql) {
                Ok(plan) => println!("{plan}"),
                Err(e) => eprintln!("{e}"),
            }
        }
        _ => eprintln!("未知命令: {line}(输入 .help 查看帮助)"),
    }
    MetaResult::Continue
}
