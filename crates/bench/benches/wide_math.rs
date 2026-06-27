use criterion::{black_box, criterion_group, criterion_main, Criterion};

use tempo_math::wide::mul_div_floor;

/// The fast `u128` path (product fits) vs the 256-bit wide path (it overflows).
fn bench_mul_div(c: &mut Criterion) {
    let mut g = c.benchmark_group("mul_div_floor");
    g.bench_function("fast_path", |b| {
        b.iter(|| {
            mul_div_floor(
                black_box(1_000_000u128),
                black_box(250u128),
                black_box(10_000u128),
            )
        })
    });
    g.bench_function("wide_path", |b| {
        b.iter(|| {
            mul_div_floor(
                black_box(u128::MAX),
                black_box(u128::MAX / 2),
                black_box(u128::MAX / 4),
            )
        })
    });
    g.finish();
}

criterion_group!(benches, bench_mul_div);
criterion_main!(benches);
