//! 投影下推 / 列裁剪:计算每个 Scan 真正需要的列,下推为 `Scan.projection`,
//! 只读必要列(列存的核心价值),并把上方表达式的列索引按裁剪后位置重映射。
//!
//! 含 Join 的计划也支持:把所需列拆分到左右子树分别裁剪,并重映射 `on` 与输出索引。

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use super::OptimizerRule;
use crate::error::Result;
use crate::logical::{LogicalPlan, SortExpr};

pub struct ProjectionPushdown;

impl OptimizerRule for ProjectionPushdown {
    fn name(&self) -> &str {
        "ProjectionPushdown"
    }

    fn rewrite(&self, plan: LogicalPlan) -> Result<LogicalPlan> {
        let n = plan.schema().len();
        let required: BTreeSet<usize> = (0..n).collect();
        let (new_plan, _kept) = prune(plan, &required);
        Ok(new_plan)
    }
}

/// 返回(裁剪后的计划, 该计划实际输出的旧列索引列表[升序])。
/// `kept` 必然 ⊇ `required`,父节点据 `kept` 重映射自身列引用。
fn prune(plan: LogicalPlan, required: &BTreeSet<usize>) -> (LogicalPlan, Vec<usize>) {
    match plan {
        LogicalPlan::Scan {
            table_name,
            source,
            projection,
            projected_schema,
        } => {
            if projection.is_some() {
                let n = projected_schema.len();
                return (
                    LogicalPlan::Scan {
                        table_name,
                        source,
                        projection,
                        projected_schema,
                    },
                    (0..n).collect(),
                );
            }
            let mut kept: Vec<usize> = required.iter().copied().collect();
            if kept.is_empty() {
                kept.push(0); // 至少保留一列以维持基数(如 COUNT(*))
            }
            let new_schema = Arc::new(projected_schema.project(&kept));
            (
                LogicalPlan::Scan {
                    table_name,
                    source,
                    projection: Some(kept.clone()),
                    projected_schema: new_schema,
                },
                kept,
            )
        }

        LogicalPlan::Filter { predicate, input } => {
            let mut child_req = required.clone();
            predicate.collect_columns(&mut child_req);
            let (new_input, child_kept) = prune(*input, &child_req);
            let map = build_map(&child_kept);
            (
                LogicalPlan::Filter {
                    predicate: predicate.remap_columns(&map),
                    input: Box::new(new_input),
                },
                child_kept,
            )
        }

        LogicalPlan::Projection {
            exprs,
            schema,
            input,
        } => {
            let kept_positions: Vec<usize> = required.iter().copied().collect();
            let kept_exprs: Vec<_> = kept_positions.iter().map(|&i| exprs[i].clone()).collect();

            let mut child_req = BTreeSet::new();
            for e in &kept_exprs {
                e.collect_columns(&mut child_req);
            }
            let (new_input, child_kept) = prune(*input, &child_req);
            let map = build_map(&child_kept);

            let new_exprs: Vec<_> = kept_exprs.iter().map(|e| e.remap_columns(&map)).collect();
            let new_schema = Arc::new(schema.project(&kept_positions));
            (
                LogicalPlan::Projection {
                    exprs: new_exprs,
                    schema: new_schema,
                    input: Box::new(new_input),
                },
                kept_positions,
            )
        }

        LogicalPlan::Aggregate {
            group_expr,
            aggr_expr,
            schema,
            input,
        } => {
            // 不裁剪聚合自身输出;只把输入所需列(分组键 + 聚合参数)下推。
            let mut child_req = BTreeSet::new();
            for e in &group_expr {
                e.collect_columns(&mut child_req);
            }
            for e in &aggr_expr {
                e.collect_columns(&mut child_req);
            }
            let (new_input, child_kept) = prune(*input, &child_req);
            let map = build_map(&child_kept);
            let new_group = group_expr.iter().map(|e| e.remap_columns(&map)).collect();
            let new_aggr = aggr_expr.iter().map(|e| e.remap_columns(&map)).collect();
            let out: Vec<usize> = (0..schema.len()).collect();
            (
                LogicalPlan::Aggregate {
                    group_expr: new_group,
                    aggr_expr: new_aggr,
                    schema,
                    input: Box::new(new_input),
                },
                out,
            )
        }

        LogicalPlan::Sort { exprs, input } => {
            let mut child_req = required.clone();
            for s in &exprs {
                s.expr.collect_columns(&mut child_req);
            }
            let (new_input, child_kept) = prune(*input, &child_req);
            let map = build_map(&child_kept);
            let new_exprs = exprs
                .iter()
                .map(|s| SortExpr {
                    expr: s.expr.remap_columns(&map),
                    asc: s.asc,
                })
                .collect();
            (
                LogicalPlan::Sort {
                    exprs: new_exprs,
                    input: Box::new(new_input),
                },
                child_kept,
            )
        }

        LogicalPlan::Limit { skip, fetch, input } => {
            let (new_input, child_kept) = prune(*input, required);
            (
                LogicalPlan::Limit {
                    skip,
                    fetch,
                    input: Box::new(new_input),
                },
                child_kept,
            )
        }

        LogicalPlan::Join {
            left,
            right,
            on,
            join_type,
            schema,
        } => {
            let left_ncols = left.schema().len();
            // 需要的列 = 父需要的 ∪ on 引用的
            let mut needed = required.clone();
            on.collect_columns(&mut needed);
            // 拆分到左右子树(右侧索引减去 left_ncols 转到右算子列空间)
            let left_req: BTreeSet<usize> = needed
                .iter()
                .filter(|&&i| i < left_ncols)
                .copied()
                .collect();
            let right_req: BTreeSet<usize> = needed
                .iter()
                .filter(|&&i| i >= left_ncols)
                .map(|&i| i - left_ncols)
                .collect();

            let (new_left, left_kept) = prune(*left, &left_req);
            let (new_right, right_kept) = prune(*right, &right_req);

            // 合并空间 old→new 映射
            let mut map = HashMap::new();
            for (newpos, &old) in left_kept.iter().enumerate() {
                map.insert(old, newpos);
            }
            let lk = left_kept.len();
            for (k, &old) in right_kept.iter().enumerate() {
                map.insert(old + left_ncols, lk + k);
            }

            // kept(旧合并索引)= 左 kept ++ (右 kept + left_ncols)
            let kept: Vec<usize> = left_kept
                .iter()
                .copied()
                .chain(right_kept.iter().map(|&i| i + left_ncols))
                .collect();
            let new_schema = Arc::new(schema.project(&kept));

            (
                LogicalPlan::Join {
                    left: Box::new(new_left),
                    right: Box::new(new_right),
                    on: on.remap_columns(&map),
                    join_type,
                    schema: new_schema,
                },
                kept,
            )
        }
    }
}

fn build_map(kept: &[usize]) -> HashMap<usize, usize> {
    kept.iter()
        .enumerate()
        .map(|(new, &old)| (old, new))
        .collect()
}
