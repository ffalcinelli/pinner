## 2024-05-17 - Secure File Creation on Unix
**Vulnerability:** TOCTOU race condition causing sensitive configuration files (e.g. `.pinner.toml`) to briefly exist with globally readable permissions before `fs::set_permissions` restricts them.
**Learning:** Using `fs::write` and subsequently restricting permissions via `fs::set_permissions` leaves a window where the file is readable by others. This can expose sensitive secrets, like API tokens or OCI passwords.
**Prevention:** Use `std::fs::OpenOptions` in combination with `std::os::unix::fs::OpenOptionsExt` and `mode(0o600)` to restrict file permissions strictly at creation time, effectively eliminating the TOCTOU risk.
