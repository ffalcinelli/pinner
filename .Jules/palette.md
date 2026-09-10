## 2024-05-24 - [Format unhandled error with alternate Display format]
**Learning:** Using `{:?}` on an `anyhow::Error` prints the full backtrace and multi-line formatting which isn't idiomatic for CLI end-users. Using `{:#}` provides a much cleaner, single-line error chain.
**Action:** When printing top-level errors to users in CLI, prefer `{:#}` over `{:?}` for a cleaner error display.
