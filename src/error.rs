//! 统一错误体系。全工程用 `Result<T>` 传播,杜绝无意义的 `unwrap`/`expect`。

use thiserror::Error;

/// 引擎统一错误类型。各模块或直接返回对应变体,或定义子错误再 `From` 转换。
#[derive(Debug, Error)]
pub enum QueryError {
    /// 词法分析错误,`pos` 为字节偏移。
    #[error("词法错误 (位置 {pos}): {msg}")]
    Lex { pos: usize, msg: String },

    /// 语法分析错误,`pos` 为字节偏移。
    #[error("语法错误 (位置 {pos}): {msg}")]
    Parse { pos: usize, msg: String },

    /// 绑定 / 语义分析错误(列不存在、类型不匹配、聚合非法、列名歧义等)。
    #[error("绑定错误: {0}")]
    Bind(String),

    /// 类型推导 / 类型检查错误。
    #[error("类型错误: {0}")]
    Type(String),

    /// 执行期错误。
    #[error("执行错误: {0}")]
    Execution(String),

    /// 底层 IO 错误。
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// CSV 解析 / 类型推断错误。
    #[error("CSV 错误: {0}")]
    Csv(String),
}

/// 引擎统一 `Result` 别名。
pub type Result<T> = std::result::Result<T, QueryError>;

impl QueryError {
    /// 构造绑定错误的便捷方法。
    pub fn bind(msg: impl Into<String>) -> Self {
        QueryError::Bind(msg.into())
    }

    /// 构造类型错误的便捷方法。
    pub fn type_err(msg: impl Into<String>) -> Self {
        QueryError::Type(msg.into())
    }

    /// 构造执行错误的便捷方法。
    pub fn exec(msg: impl Into<String>) -> Self {
        QueryError::Execution(msg.into())
    }
}
