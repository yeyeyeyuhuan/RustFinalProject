//! 列裁剪(去冗余投影):消除"恒等投影"——其输出恰为子节点全部列且顺序一致、无别名,
//! 这类 Projection 不改变数据,可直接去掉。

use super::{OptimizerRule, map_children};
use crate::error::Result;
use crate::logical::{Expr, LogicalPlan};
use crate::types::Schema;

pub struct ColumnPruning;

impl OptimizerRule for ColumnPruning {
    fn name(&self) -> &str {
        "ColumnPruning"
    }

    fn rewrite(&self, plan: LogicalPlan) -> Result<LogicalPlan> {
        let plan = map_children(plan, |c| self.rewrite(c))?;

        if let LogicalPlan::Projection { exprs, input, .. } = &plan
            && is_identity(exprs, &input.schema())
        {
            // 安全地取出 input
            if let LogicalPlan::Projection { input, .. } = plan {
                return Ok(*input);
            }
        }
        Ok(plan)
    }
}

/// 投影是否为"恒等":列数与子 Schema 相同,且第 i 项恰为 `Column(i)`(无别名 / 表达式 / 重排)。
fn is_identity(exprs: &[Expr], child_schema: &Schema) -> bool {
    exprs.len() == child_schema.len()
        && exprs
            .iter()
            .enumerate()
            .all(|(i, e)| matches!(e, Expr::Column(c) if *c == i))
}
