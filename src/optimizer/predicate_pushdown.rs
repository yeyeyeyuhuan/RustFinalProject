//! 谓词处理:删除恒真过滤、合并相邻过滤(把谓词尽量收拢到一个 Filter)。
//!
//! 说明:本引擎的绑定器已把 WHERE 直接放在扫描 / 连接之上(谓词天然靠下),
//! 故此规则主要做相邻 Filter 合并与恒真 Filter 消除。

use super::{OptimizerRule, map_children};
use crate::error::Result;
use crate::logical::{Expr, LogicalPlan};
use crate::sql::ast::BinaryOp;
use crate::types::ScalarValue;

pub struct PredicatePushdown;

impl OptimizerRule for PredicatePushdown {
    fn name(&self) -> &str {
        "PredicatePushdown"
    }

    fn rewrite(&self, plan: LogicalPlan) -> Result<LogicalPlan> {
        // 自底向上:先优化子树,再处理本层。
        let plan = map_children(plan, |c| self.rewrite(c))?;

        if let LogicalPlan::Filter { predicate, input } = plan {
            // 恒真谓词 → 直接去掉 Filter
            if is_true(&predicate) {
                return Ok(*input);
            }
            // Filter(p1) over Filter(p2) → Filter(p1 AND p2)
            if let LogicalPlan::Filter {
                predicate: inner_pred,
                input: inner_input,
            } = *input
            {
                return Ok(LogicalPlan::Filter {
                    predicate: Expr::Binary {
                        left: Box::new(predicate),
                        op: BinaryOp::And,
                        right: Box::new(inner_pred),
                    },
                    input: inner_input,
                });
            }
            return Ok(LogicalPlan::Filter { predicate, input });
        }
        Ok(plan)
    }
}

fn is_true(e: &Expr) -> bool {
    matches!(e, Expr::Literal(ScalarValue::Boolean(true)))
}
