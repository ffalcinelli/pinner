## Diff Formatting Optimization
- Avoid `String::push_str` with `format!` for sequential string additions in hot paths, as it leads to unnecessary intermediate allocations.
- Prefer the `write!` macro from `std::fmt::Write` directly on the target `String` to append content without allocating temporary `String` instances. Benchmarking showed ~7% improvement in `format_diff` and ~12% in `format_inline_diff`.
