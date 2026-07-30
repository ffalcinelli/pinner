use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pinner::cli::OutputFormat;
use pinner::core::{DependencyRef, UpdateResult, UpdateTask};
use pinner::patcher::formatter::{Formatter, HashSecurityStatus};
use std::path::PathBuf;

fn bench_format_inline_diff(c: &mut Criterion) {
    let formatter = Formatter::new(OutputFormat::Text, false, vec![], vec![], true);

    let old = "actions/checkout@v2";
    let new = "actions/checkout@hash3";

    c.bench_function("format_inline_diff", |b| {
        b.iter(|| {
            formatter.format_inline_diff(
                black_box(old),
                black_box(new),
                black_box(HashSecurityStatus::NotChecked),
            )
        });
    });
}

fn bench_format_diff(c: &mut Criterion) {
    let formatter = Formatter::new(
        OutputFormat::Text,
        false,
        vec!["hash3".to_string()],
        vec![],
        true,
    );
    let old = "line1\nuses: actions/checkout@v2\n";
    let new = "line1\nuses: actions/checkout@hash3\n";

    let res = UpdateResult {
        task: UpdateTask::default(),
        action: "actions/checkout".into(),
        path: PathBuf::from("f.yml"),
        old_tag: Some("v2".to_string()),
        new_sha: DependencyRef::GitSha("hash3".to_string()),
        new_tag: Some("v2".to_string()),
    };

    let results = vec![res];

    c.bench_function("format_diff", |b| {
        b.iter(|| formatter.format_diff(black_box(old), black_box(new), black_box(&results)));
    });
}

criterion_group!(benches, bench_format_inline_diff, bench_format_diff);
criterion_main!(benches);
