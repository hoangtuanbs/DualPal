// Performance baselines (Phase 8, L9: PERF-001/002). Run with `cargo bench`.

use criterion::{criterion_group, criterion_main, Criterion};
use dualpal_core::compare::glob_match;

fn glob_bench(c: &mut Criterion) {
    let names: Vec<String> = (0..1000).map(|i| format!("report_{i:04}.pdf")).collect();
    let patterns = ["*.pdf", "report_0*", "*.jpg; *.png", "a?b*"];
    c.bench_function("glob_match over 1000 names", |b| {
        b.iter(|| {
            for p in patterns {
                for n in &names {
                    criterion::black_box(glob_match(p, n));
                }
            }
        });
    });
}

criterion_group!(benches, glob_bench);
criterion_main!(benches);
