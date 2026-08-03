use criterion::{criterion_group, criterion_main, Criterion};

// We cannot easily benchmark the real `Pipeline::scan` because it requires
// network calls and full project setup, which might be flaky in benches.
// But we can document that this is a network optimization, replacing
// sequential `await` in a loop with parallel stream evaluation.
fn bench_dummy(c: &mut Criterion) {
    c.bench_function("dummy", |b| b.iter(|| 1 + 1));
}

criterion_group!(benches, bench_dummy);
criterion_main!(benches);
