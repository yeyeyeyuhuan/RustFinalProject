//! 列式内存分析查询引擎(mini OLAP / DataFrame 内核)。
//!
//! 走完整查询处理链路:
//! `SQL 文本 → 词法/语法(AST)→ 绑定/语义分析 → 逻辑计划 → 查询优化
//!  → 物理计划 → 向量化执行(火山模型 + 并行)→ 结果输出`。
//!
//! 模块按职责分层,严格单向依赖(上层依赖下层,下层不反向依赖上层)。

pub mod array;
pub mod error;
pub mod types;

pub mod datasource;
pub mod logical;
pub mod optimizer;
pub mod physical;
pub mod sql;

pub mod execution;
pub mod repl;

pub use error::{QueryError, Result};
