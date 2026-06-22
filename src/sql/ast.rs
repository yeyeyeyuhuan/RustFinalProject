//! 抽象语法树定义,以及运算符 / 聚合函数枚举(逻辑层与物理层共享)。

/// 二元运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

impl BinaryOp {
    /// 运算符符号文本(用于打印 / 列名生成)。
    pub fn symbol(&self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Eq => "=",
            BinaryOp::NotEq => "<>",
            BinaryOp::Lt => "<",
            BinaryOp::LtEq => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::GtEq => ">=",
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
        }
    }

    pub fn is_arithmetic(&self) -> bool {
        matches!(
            self,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
        )
    }

    pub fn is_comparison(&self) -> bool {
        matches!(
            self,
            BinaryOp::Eq
                | BinaryOp::NotEq
                | BinaryOp::Lt
                | BinaryOp::LtEq
                | BinaryOp::Gt
                | BinaryOp::GtEq
        )
    }

    pub fn is_logical(&self) -> bool {
        matches!(self, BinaryOp::And | BinaryOp::Or)
    }
}

/// 一元运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

impl UnaryOp {
    pub fn symbol(&self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "NOT ",
        }
    }
}

/// 聚合函数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl AggregateFunc {
    pub fn name(&self) -> &'static str {
        match self {
            AggregateFunc::Count => "COUNT",
            AggregateFunc::Sum => "SUM",
            AggregateFunc::Avg => "AVG",
            AggregateFunc::Min => "MIN",
            AggregateFunc::Max => "MAX",
        }
    }
}

/// 字面量。
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
}

/// 表达式 AST。
#[derive(Debug, Clone, PartialEq)]
pub enum AstExpr {
    Literal(Literal),
    /// 列引用,可带表限定(`table.col`)。
    Column {
        table: Option<String>,
        name: String,
    },
    /// `*` 通配(出现在 `COUNT(*)` 或裸 `SELECT *` 中)。
    Wildcard,
    Binary {
        left: Box<AstExpr>,
        op: BinaryOp,
        right: Box<AstExpr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<AstExpr>,
    },
    /// `expr IS [NOT] NULL`。
    IsNull {
        expr: Box<AstExpr>,
        negated: bool,
    },
    /// 聚合函数调用,`arg` 为 `Wildcard` 表示 `COUNT(*)`。
    Aggregate {
        func: AggregateFunc,
        arg: Box<AstExpr>,
    },
}

/// SELECT 投影项。
#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    /// `*`
    Wildcard,
    /// `<expr> [AS alias]`
    Expr {
        expr: AstExpr,
        alias: Option<String>,
    },
}

/// 表引用(表名 + 可选别名)。
#[derive(Debug, Clone, PartialEq)]
pub struct TableRef {
    pub name: String,
    pub alias: Option<String>,
}

/// JOIN 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
}

impl JoinType {
    pub fn name(&self) -> &'static str {
        match self {
            JoinType::Inner => "Inner",
            JoinType::Left => "Left",
        }
    }
}

/// 单个 JOIN 子句。
#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    pub table: TableRef,
    pub join_type: JoinType,
    pub on: AstExpr,
}

/// ORDER BY 项。
#[derive(Debug, Clone, PartialEq)]
pub struct OrderByExpr {
    pub expr: AstExpr,
    pub asc: bool,
}

/// SELECT 语句。
#[derive(Debug, Clone, PartialEq)]
pub struct SelectStmt {
    pub projections: Vec<SelectItem>,
    pub from: TableRef,
    pub joins: Vec<Join>,
    pub filter: Option<AstExpr>,
    pub group_by: Vec<AstExpr>,
    pub having: Option<AstExpr>,
    pub order_by: Vec<OrderByExpr>,
    pub limit: Option<u64>,
}

/// 顶层语句(当前仅 SELECT)。
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Select(SelectStmt),
}
