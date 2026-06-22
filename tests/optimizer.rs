//! 优化器测试:验证规则正确触发,且优化前后查询结果一致(等价性)。

use rust_final_project::array::RecordBatch;
use rust_final_project::execution::{SessionContext, collect};
use rust_final_project::physical::create_physical_plan;

fn ctx() -> SessionContext {
    let mut ctx = SessionContext::new();
    ctx.register_csv("emp", "testdata/employees.csv").unwrap();
    ctx.register_csv("dept", "testdata/departments.csv")
        .unwrap();
    ctx
}

fn rows(batches: &[RecordBatch]) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for b in batches {
        for r in 0..b.num_rows() {
            out.push(b.columns.iter().map(|c| c.value(r).to_string()).collect());
        }
    }
    out
}

/// 不经优化器直接执行(用于对比等价性)。
fn run_unoptimized(ctx: &SessionContext, sql: &str) -> Vec<Vec<String>> {
    let plan = ctx.logical_plan(sql).unwrap();
    let mut op = create_physical_plan(&plan).unwrap();
    rows(&collect(op.as_mut()).unwrap())
}

fn run_optimized(ctx: &SessionContext, sql: &str) -> Vec<Vec<String>> {
    rows(&ctx.sql(sql).unwrap())
}

#[test]
fn column_pruning_sets_scan_projection() {
    let ctx = ctx();
    let plan = ctx
        .explain("SELECT name FROM emp WHERE salary > 9000")
        .unwrap();
    // 优化后扫描应只读所需列(name, salary),体现在 Scan 投影列里
    let optimized = plan.split("== 优化后逻辑计划 ==").nth(1).unwrap();
    assert!(optimized.contains("投影列"));
    assert!(optimized.contains("name"));
    assert!(optimized.contains("salary"));
    // 不应包含未用到的列
    assert!(!optimized.contains("dept_id"));
}

#[test]
fn constant_folding_removes_trivial_filter() {
    let ctx = ctx();
    let plan = ctx.explain("SELECT id FROM emp WHERE 1 = 1").unwrap();
    let optimized = plan.split("== 优化后逻辑计划 ==").nth(1).unwrap();
    // 1 = 1 → true → Filter 被删除
    assert!(!optimized.contains("Filter"));
    // 结果应是全部 10 行
    assert_eq!(
        run_optimized(&ctx, "SELECT id FROM emp WHERE 1 = 1").len(),
        10
    );
}

#[test]
fn expr_simplify_and_true() {
    let ctx = ctx();
    // active = true AND 1 = 1 → active = true
    let r = run_optimized(&ctx, "SELECT name FROM emp WHERE active = true AND 1 = 1");
    assert_eq!(r.len(), 7);
}

#[test]
fn column_pruning_through_join() {
    let ctx = ctx();
    let plan = ctx
        .explain("SELECT e.name, d.name FROM emp AS e JOIN dept AS d ON e.dept_id = d.id")
        .unwrap();
    let optimized = plan.split("== 优化后逻辑计划 ==").nth(1).unwrap();
    // 两侧扫描都应被裁剪:emp 读 name+dept_id,dept 读 id+name
    assert!(optimized.contains("投影列"));
    assert!(!optimized.contains("salary")); // emp 未用列被裁掉
    assert!(!optimized.contains("budget")); // dept 未用列被裁掉
    // 结果仍正确
    let r = run_optimized(
        &ctx,
        "SELECT e.name, d.name FROM emp AS e JOIN dept AS d ON e.dept_id = d.id WHERE e.id = 1",
    );
    assert_eq!(r[0][0], "Alice");
    assert_eq!(r[0][1], "Engineering");
}

#[test]
fn optimization_preserves_results() {
    let ctx = ctx();
    let queries = [
        "SELECT name, salary FROM emp WHERE salary > 8000 ORDER BY salary",
        "SELECT dept_id, COUNT(*), AVG(salary) FROM emp GROUP BY dept_id ORDER BY dept_id",
        "SELECT id FROM emp WHERE active = true AND age > 30 ORDER BY id",
        "SELECT name FROM emp ORDER BY name LIMIT 4",
    ];
    for q in queries {
        assert_eq!(
            run_unoptimized(&ctx, q),
            run_optimized(&ctx, q),
            "优化前后结果不一致: {q}"
        );
    }
}

#[test]
fn explain_shows_before_and_after() {
    let ctx = ctx();
    let plan = ctx.explain("SELECT name FROM emp").unwrap();
    assert!(plan.contains("未优化逻辑计划"));
    assert!(plan.contains("优化后逻辑计划"));
}
