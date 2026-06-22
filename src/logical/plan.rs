//! 逻辑计划:一棵描述"做什么"的算子树。

use std::sync::Arc;

use super::expr::Expr;
use crate::datasource::DataSource;
use crate::sql::ast::JoinType;
use crate::types::Schema;

/// ORDER BY 中的单个排序键。
#[derive(Debug, Clone)]
pub struct SortExpr {
    pub expr: Expr,
    /// true 升序,false 降序。
    pub asc: bool,
}

/// 逻辑计划节点(递归树)。
#[derive(Clone)]
pub enum LogicalPlan {
    /// 表扫描。`projection` 为下推的列裁剪(M8 优化器填充),None 表示读全部列。
    Scan {
        table_name: String,
        source: Arc<dyn DataSource>,
        projection: Option<Vec<usize>>,
        projected_schema: Arc<Schema>,
    },
    /// 过滤。
    Filter {
        predicate: Expr,
        input: Box<LogicalPlan>,
    },
    /// 投影(输出列集)。
    Projection {
        exprs: Vec<Expr>,
        schema: Arc<Schema>,
        input: Box<LogicalPlan>,
    },
    /// 分组聚合。输出 Schema = 分组列 ++ 聚合列。
    Aggregate {
        group_expr: Vec<Expr>,
        aggr_expr: Vec<Expr>,
        schema: Arc<Schema>,
        input: Box<LogicalPlan>,
    },
    /// 连接。输出 Schema = 左列 ++ 右列。`on` 为对合并 Schema 绑定的连接条件。
    Join {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        on: Expr,
        join_type: JoinType,
        schema: Arc<Schema>,
    },
    /// 排序。
    Sort {
        exprs: Vec<SortExpr>,
        input: Box<LogicalPlan>,
    },
    /// 取前 N 行(可带跳过)。
    Limit {
        skip: usize,
        fetch: Option<usize>,
        input: Box<LogicalPlan>,
    },
}

impl LogicalPlan {
    /// 该节点输出的 Schema。
    pub fn schema(&self) -> Arc<Schema> {
        match self {
            LogicalPlan::Scan {
                projected_schema, ..
            } => Arc::clone(projected_schema),
            LogicalPlan::Filter { input, .. } => input.schema(),
            LogicalPlan::Projection { schema, .. } => Arc::clone(schema),
            LogicalPlan::Aggregate { schema, .. } => Arc::clone(schema),
            LogicalPlan::Join { schema, .. } => Arc::clone(schema),
            LogicalPlan::Sort { input, .. } => input.schema(),
            LogicalPlan::Limit { input, .. } => input.schema(),
        }
    }

    /// 粗略输出行数估计(供代价优化)。未知返回 None。
    pub fn estimated_rows(&self) -> Option<usize> {
        match self {
            LogicalPlan::Scan { source, .. } => source.estimated_rows(),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Projection { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Sort { input, .. } => input.estimated_rows(),
            LogicalPlan::Limit { fetch, input, .. } => {
                let inner = input.estimated_rows();
                match (fetch, inner) {
                    (Some(f), Some(n)) => Some((*f).min(n)),
                    (Some(f), None) => Some(*f),
                    (None, n) => n,
                }
            }
            LogicalPlan::Join { left, right, .. } => {
                match (left.estimated_rows(), right.estimated_rows()) {
                    (Some(a), Some(b)) => Some(a.saturating_mul(b)),
                    _ => None,
                }
            }
        }
    }

    /// 子节点(用于优化器遍历)。
    pub fn children(&self) -> Vec<&LogicalPlan> {
        match self {
            LogicalPlan::Scan { .. } => vec![],
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Projection { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. } => vec![input.as_ref()],
            LogicalPlan::Join { left, right, .. } => vec![left.as_ref(), right.as_ref()],
        }
    }
}
