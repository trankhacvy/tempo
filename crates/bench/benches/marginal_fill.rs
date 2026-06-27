use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use tempo_math::clearing::compute_marginal_fill;

/// The marginal-tick allocation is O(1) per order regardless of book size.
fn bench_marginal_fill(c: &mut Criterion) {
    let mut g = c.benchmark_group("compute_marginal_fill");
    for &total in &[100u64, 10_000, 1_000_000] {
        g.bench_with_input(BenchmarkId::from_parameter(total), &total, |b, &total| {
            b.iter(|| {
                compute_marginal_fill(
                    black_box(total / 3),
                    black_box(total / 7),
                    black_box(total / 2),
                    black_box(total),
                )
                .unwrap()
            })
        });
    }
    g.finish();
}

criterion_group!(benches, bench_marginal_fill);
criterion_main!(benches);
