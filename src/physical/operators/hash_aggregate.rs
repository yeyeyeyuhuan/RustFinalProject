//! HashAggregate 算子(阻塞):哈希表 key 为分组列组合,value 为各聚合累加状态。
//! 消费完全部输入后一次性产出结果。
//!
//! 核心累加逻辑(`accumulate` / `build_output`)被串行与并行(partitioned)两版聚合共用。

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use crate::array::{Array, ArrayRef, RecordBatch};
use crate::error::Result;
use crate::physical::key::{KeyValue, partition_of, row_key};
use crate::physical::{PhysicalExpr, PhysicalOperator};
use crate::sql::ast::AggregateFunc;
use crate::types::{DataType, ScalarValue, Schema};

/// 单个聚合的物理规格。
#[derive(Clone)]
pub struct AggrSpec {
    pub func: AggregateFunc,
    pub arg: Option<PhysicalExpr>,
    pub out_type: DataType,
}

/// 一个分组的累加结果:分组键值 + 各聚合的累加状态。
pub(crate) struct GroupAccum {
    pub keys: Vec<ScalarValue>,
    pub accs: Vec<Acc>,
}

/// 哈希聚合算子。`parallel` 为真时按分组键哈希分区,多线程(std::thread + mpsc + Arc)并行累加。
pub struct HashAggregateExec {
    input: Box<dyn PhysicalOperator>,
    group_exprs: Vec<PhysicalExpr>,
    aggr_specs: Vec<AggrSpec>,
    schema: Arc<Schema>,
    parallel: bool,
    produced: bool,
}

impl HashAggregateExec {
    pub fn new(
        input: Box<dyn PhysicalOperator>,
        group_exprs: Vec<PhysicalExpr>,
        aggr_specs: Vec<AggrSpec>,
        schema: Arc<Schema>,
        parallel: bool,
    ) -> Self {
        HashAggregateExec {
            input,
            group_exprs,
            aggr_specs,
            schema,
            parallel,
            produced: false,
        }
    }

    fn aggregate_all(&mut self) -> Result<RecordBatch> {
        let mut batches = Vec::new();
        while let Some(b) = self.input.next_batch()? {
            batches.push(b);
        }
        let mut groups = if self.parallel {
            self.accumulate_parallel(batches)?
        } else {
            accumulate(&batches, &self.group_exprs, &self.aggr_specs, |_| true)?
        };
        fixup_empty_count(&mut groups, &self.group_exprs, &self.aggr_specs);
        build_output(
            &self.schema,
            self.group_exprs.len(),
            &self.aggr_specs,
            groups,
        )
    }

    /// Partitioned 并行累加:按分组键哈希分 P 区,每区一个线程独立累加,经 channel 汇总。
    /// 由于按完整分组键分区,同一分组只落在一个分区,各分区结果可直接拼接(无需跨区合并)。
    fn accumulate_parallel(&self, batches: Vec<RecordBatch>) -> Result<Vec<GroupAccum>> {
        let p = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .max(1);
        let batches = Arc::new(batches);
        let group_exprs = &self.group_exprs;
        let specs = &self.aggr_specs;

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            for pid in 0..p {
                let tx = tx.clone();
                let batches = Arc::clone(&batches);
                scope.spawn(move || {
                    let res = accumulate(&batches, group_exprs, specs, |key| {
                        if key.is_empty() {
                            pid == 0 // 无 GROUP BY:单组全归 0 号分区
                        } else {
                            partition_of(key, p) == pid
                        }
                    });
                    let _ = tx.send(res);
                });
            }
        });
        drop(tx);

        let mut groups = Vec::new();
        for res in rx {
            groups.extend(res?);
        }
        Ok(groups)
    }
}

impl PhysicalOperator for HashAggregateExec {
    fn schema(&self) -> Arc<Schema> {
        Arc::clone(&self.schema)
    }

    fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        if self.produced {
            return Ok(None);
        }
        self.produced = true;
        Ok(Some(self.aggregate_all()?))
    }
}

/// 对给定批序列做哈希聚合;`keep` 决定某分组键是否纳入(并行 partition 用)。
pub(crate) fn accumulate(
    batches: &[RecordBatch],
    group_exprs: &[PhysicalExpr],
    specs: &[AggrSpec],
    keep: impl Fn(&[KeyValue]) -> bool,
) -> Result<Vec<GroupAccum>> {
    let mut order: Vec<GroupAccum> = Vec::new();
    let mut index: HashMap<Vec<KeyValue>, usize> = HashMap::new();

    for batch in batches {
        let nrows = batch.num_rows();
        let group_cols: Vec<ArrayRef> = group_exprs
            .iter()
            .map(|e| e.evaluate(batch))
            .collect::<Result<_>>()?;
        let arg_cols: Vec<Option<ArrayRef>> = specs
            .iter()
            .map(|s| s.arg.as_ref().map(|e| e.evaluate(batch)).transpose())
            .collect::<Result<_>>()?;

        for row in 0..nrows {
            let row_vals: Vec<ScalarValue> = group_cols.iter().map(|c| c.value(row)).collect();
            let key = row_key(&row_vals);
            if !keep(&key) {
                continue;
            }
            let gi = match index.get(&key) {
                Some(&i) => i,
                None => {
                    let i = order.len();
                    order.push(GroupAccum {
                        keys: row_vals,
                        accs: specs.iter().map(|s| Acc::new(s.func)).collect(),
                    });
                    index.insert(key, i);
                    i
                }
            };
            for (j, _) in specs.iter().enumerate() {
                let v = arg_cols[j].as_ref().map(|c| c.value(row));
                order[gi].accs[j].update(v.as_ref());
            }
        }
    }
    Ok(order)
}

/// 无 GROUP BY 且输入为空时,补一个空分组(`SELECT COUNT(*) FROM 空表` 返回一行 0)。
pub(crate) fn fixup_empty_count(
    groups: &mut Vec<GroupAccum>,
    group_exprs: &[PhysicalExpr],
    specs: &[AggrSpec],
) {
    if group_exprs.is_empty() && groups.is_empty() {
        groups.push(GroupAccum {
            keys: Vec::new(),
            accs: specs.iter().map(|s| Acc::new(s.func)).collect(),
        });
    }
}

/// 把分组结果物化为列式 RecordBatch(分组列 ++ 聚合列)。
pub(crate) fn build_output(
    schema: &Arc<Schema>,
    group_n: usize,
    specs: &[AggrSpec],
    groups: Vec<GroupAccum>,
) -> Result<RecordBatch> {
    let ngroups = groups.len();
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(schema.len());

    for g in 0..group_n {
        let dt = schema.field(g).data_type;
        let vals: Vec<ScalarValue> = groups.iter().map(|gr| gr.keys[g].clone()).collect();
        columns.push(Array::from_scalars(dt, &vals));
    }
    for (j, spec) in specs.iter().enumerate() {
        let vals: Vec<ScalarValue> = (0..ngroups)
            .map(|gi| groups[gi].accs[j].finalize(spec.out_type))
            .collect();
        columns.push(Array::from_scalars(spec.out_type, &vals));
    }

    RecordBatch::try_new(Arc::clone(schema), columns)
}

/// 聚合累加状态。
pub(crate) enum Acc {
    Count(i64),
    Sum {
        int_sum: i64,
        float_sum: f64,
        seen: bool,
    },
    Avg {
        sum: f64,
        count: i64,
    },
    Min(Option<ScalarValue>),
    Max(Option<ScalarValue>),
}

impl Acc {
    pub(crate) fn new(func: AggregateFunc) -> Self {
        match func {
            AggregateFunc::Count => Acc::Count(0),
            AggregateFunc::Sum => Acc::Sum {
                int_sum: 0,
                float_sum: 0.0,
                seen: false,
            },
            AggregateFunc::Avg => Acc::Avg { sum: 0.0, count: 0 },
            AggregateFunc::Min => Acc::Min(None),
            AggregateFunc::Max => Acc::Max(None),
        }
    }

    pub(crate) fn update(&mut self, v: Option<&ScalarValue>) {
        match self {
            Acc::Count(n) => match v {
                None => *n += 1,                      // COUNT(*)
                Some(sv) if !sv.is_null() => *n += 1, // COUNT(col):非空计数
                _ => {}
            },
            Acc::Sum {
                int_sum,
                float_sum,
                seen,
            } => {
                if let Some(sv) = v {
                    match sv {
                        ScalarValue::Int64(x) => {
                            *int_sum = int_sum.wrapping_add(*x);
                            *float_sum += *x as f64;
                            *seen = true;
                        }
                        ScalarValue::Float64(x) => {
                            *float_sum += *x;
                            *seen = true;
                        }
                        _ => {}
                    }
                }
            }
            Acc::Avg { sum, count } => {
                if let Some(sv) = v
                    && let Some(f) = sv.as_f64()
                {
                    *sum += f;
                    *count += 1;
                }
            }
            Acc::Min(cur) => {
                if let Some(sv) = v
                    && !sv.is_null()
                    && (cur.is_none() || sv.order_cmp(cur.as_ref().unwrap()) == Ordering::Less)
                {
                    *cur = Some(sv.clone());
                }
            }
            Acc::Max(cur) => {
                if let Some(sv) = v
                    && !sv.is_null()
                    && (cur.is_none() || sv.order_cmp(cur.as_ref().unwrap()) == Ordering::Greater)
                {
                    *cur = Some(sv.clone());
                }
            }
        }
    }

    pub(crate) fn finalize(&self, out_type: DataType) -> ScalarValue {
        match self {
            Acc::Count(n) => ScalarValue::Int64(*n),
            Acc::Sum {
                int_sum,
                float_sum,
                seen,
            } => {
                if !seen {
                    ScalarValue::Null(out_type)
                } else if out_type == DataType::Int64 {
                    ScalarValue::Int64(*int_sum)
                } else {
                    ScalarValue::Float64(*float_sum)
                }
            }
            Acc::Avg { sum, count } => {
                if *count == 0 {
                    ScalarValue::Null(DataType::Float64)
                } else {
                    ScalarValue::Float64(sum / (*count as f64))
                }
            }
            Acc::Min(c) | Acc::Max(c) => c.clone().unwrap_or(ScalarValue::Null(out_type)),
        }
    }
}
