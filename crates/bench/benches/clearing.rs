use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use tempo_bench::synthetic_book;
use tempo_math::clearing::find_cross;

/// Runtime vs tick count (orders held at 4× ticks): the linear-in-ticks curve.
fn bench_find_cross_by_ticks(c: &mut Criterion) {
    let mut g = c.benchmark_group("find_cross/by_ticks");
    for &ticks in &[64usize, 256, 1024, 4096] {
        let (d, s) = synthetic_book(ticks, ticks * 4, 0xABCD);
        g.bench_with_input(BenchmarkId::from_parameter(ticks), &ticks, |b, _| {
            b.iter(|| find_cross(black_box(&d), black_box(&s)).unwrap())
        });
    }
    g.finish();
}

/// Runtime vs order count at FIXED ticks (256): the flat-in-orders curve.
fn bench_find_cross_by_orders(c: &mut Criterion) {
    let mut g = c.benchmark_group("find_cross/by_orders");
    for &orders in &[256usize, 1024, 4096, 16384] {
        let (d, s) = synthetic_book(256, orders, 0x1234);
        g.bench_with_input(BenchmarkId::from_parameter(orders), &orders, |b, _| {
            b.iter(|| find_cross(black_box(&d), black_box(&s)).unwrap())
        });
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_find_cross_by_ticks,
    bench_find_cross_by_orders
);
criterion_main!(benches);
