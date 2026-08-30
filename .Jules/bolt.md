## Diff Formatting Optimization
- Avoid `String::push_str` with `format!` for sequential string additions in hot paths, as it leads to unnecessary intermediate allocations.
- Prefer the `write!` macro from `std::fmt::Write` directly on the target `String` to append content without allocating temporary `String` instances. Benchmarking showed ~7% improvement in `format_diff` and ~12% in `format_inline_diff`.
## 2024-08-16 - String Formatting Optimization
**Learning:** `push_str(&format!(...))` allocates a temporary `String` on the heap before appending to the target `String`.
**Action:** Use `write!(target_string, ...)` or `writeln!(target_string, ...)` from `std::fmt::Write` to append formatted data directly to the target buffer, avoiding the intermediate allocation. Note that `writeln!` is preferred by clippy (`clippy::write-with-newline`) over `write!` with a trailing `\n`.
## 2024-05-18 - Optimized format_security_list
**Learning:** `format_security_list` was returning a new string, allocating an intermediate `Vec`, and calling `format!` in a loop, causing unnecessary heap allocations during config serialization, especially for large vetted/compromised lists.
**Action:** Changed the function to take a `&mut String` buffer, replaced `Vec::push` and `format!` with `std::fmt::Write::write!`, and updated `to_formatted_string` to pass the target buffer directly. This eliminates `O(N)` string and vector allocations.
