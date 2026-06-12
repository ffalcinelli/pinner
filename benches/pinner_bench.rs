use criterion::{criterion_group, criterion_main, Criterion};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn create_test_files(dir: &Path, num_files: usize, uses_per_file: usize) {
    for i in 0..num_files {
        let mut content = String::new();
        for j in 0..uses_per_file {
            content.push_str(&format!("    uses: actions/checkout@v{}\n", j));
        }
        fs::write(dir.join(format!("file_{}.yml", i)), content).unwrap();
    }
}

fn bench_file_read(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let path = dir.path().to_path_buf();
    create_test_files(&path, 50, 10);

    // Simulating the read in lib.rs
    c.bench_function("simulated current approach", |b| {
        b.iter(|| {
            for i in 0..50 {
                let p = path.join(format!("file_{}.yml", i));
                let _content = fs::read_to_string(&p).unwrap(); // First read
                                                                // Process would happen here...
                let content = fs::read_to_string(&p).unwrap(); // Second read in apply
                let mut new_content = content.clone();
                new_content.push_str("test");
            }
        });
    });

    c.bench_function("simulated optimized approach", |b| {
        b.iter(|| {
            for i in 0..50 {
                let p = path.join(format!("file_{}.yml", i));
                let content = fs::read_to_string(&p).unwrap(); // Only read once
                                                               // Pass content through to apply phase
                let mut new_content = content.clone();
                new_content.push_str("test");
            }
        });
    });
}

criterion_group!(benches, bench_file_read);
criterion_main!(benches);
