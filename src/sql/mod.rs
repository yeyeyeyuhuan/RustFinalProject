//! 解析前端:词法分析、语法分析、AST 定义。

pub mod ast;
mod lexer;
mod parser;
mod token;

pub use parser::parse;
pub use token::{Keyword, Token};
