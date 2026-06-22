//! 并行一致性测试:同一 SQL 的串行与 partitioned 并行执行结果集相同。
//! (无 ORDER BY 时行顺序不保证,故按排序后的行集合比较。)

use rust_final_project::array::RecordBatch;
use rust_final_project::execution::SessionContext;

fn ctx() -> SessionContext {
    let mut ctx = SessionContext::new();
    ctx.register_csv("emp", "testdata/employees.csv").unwrap();
    ctx.register_csv("dept", "testdata/departments.csv")
        .unwrap();
    ctx
}

/// 排序后的行集合(消除并行带来的顺序差异)。
fn sorted_rows(batches: &[RecordBatch]) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for b in batches {
        for r in 0..b.num_rows() {
            rows.push(b.columns.iter().map(|c| c.value(r).to_string()).collect());
        }
    }
    rows.sort();
    rows
}

fn assert_consistent(ctx: &SessionContext, sql: &str) {
    let serial = sorted_rows(&ctx.sql_with_parallel(sql, false).unwrap());
    let parallel = sorted_rows(&ctx.sql_with_parallel(sql, true).unwrap());
    assert_eq!(serial, parallel, "串行与并行结果不一致: {sql}");
    assert!(!serial.is_empty(), "查询无结果: {sql}");
}

#[test]
fn parallel_aggregate_consistent() {
    let ctx = ctx();
    assert_consistent(&ctx, "SELECT dept_id, COUNT(*) FROM emp GROUP BY dept_id");
    assert_consistent(
        &ctx,
        "SELECT dept_id, COUNT(*), AVG(salary), MIN(age), MAX(salary), SUM(salary) \
         FROM emp GROUP BY dept_id",
    );
    assert_consistent(&ctx, "SELECT active, COUNT(*) FROM emp GROUP BY active");
}

#[test]
fn parallel_global_aggregate_consistent() {
    let ctx = ctx();
    // 无 GROUP BY 的全局聚合
    assert_consistent(&ctx, "SELECT COUNT(*), AVG(salary), MIN(salary) FROM emp");
}

#[test]
fn parallel_join_consistent() {
    let ctx = ctx();
    assert_consistent(
        &ctx,
        "SELECT e.name, d.name FROM emp AS e JOIN dept AS d ON e.dept_id = d.id",
    );
    assert_consistent(
        &ctx,
        "SELECT d.name, COUNT(*) FROM emp AS e JOIN dept AS d ON e.dept_id = d.id GROUP BY d.name",
    );
}

#[test]
fn parallel_matches_expected_values() {
    let ctx = ctx();
    // 并行聚合的具体数值正确性(配合 ORDER BY 固定顺序)
    let r = ctx
        .sql_with_parallel(
            "SELECT dept_id, COUNT(*) FROM emp GROUP BY dept_id ORDER BY dept_id",
            true,
        )
        .unwrap();
    let rows = sorted_rows(&r);
    assert_eq!(
        rows,
        vec![
            vec!["10".to_string(), "4".to_string()],
            vec!["20".to_string(), "3".to_string()],
            vec!["30".to_string(), "3".to_string()],
        ]
    );
}
