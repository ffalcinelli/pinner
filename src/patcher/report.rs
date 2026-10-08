//! Rendering of `pinner verify` results.
//!
//! Verification is split into two steps: the pipeline classifies every dependency
//! into a [`VerifyFinding`], and this module renders the findings for each output
//! format. Renderers return strings so they can be tested without capturing stdio.

use crate::core::UpdateTask;
use colored::Colorize;
use std::fmt::Write;

/// Outcome of verifying a single dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyStatus {
    /// Referenced by a mutable tag or branch.
    Unpinned,
    /// Listed as compromised, or flagged by OSV as malicious/hijacked.
    Compromised,
    /// OSV reports ordinary vulnerabilities for the pinned commit.
    Vulnerable,
    /// Pinned image with no cosign signature. Fails only in strict mode.
    Unsigned,
    /// Pinned but not in the vetted list (strict mode only).
    NotVetted,
    /// Pinned; not vetted, but strict mode is off.
    Pinned,
    /// Pinned and explicitly vetted.
    Vetted,
}

impl VerifyStatus {
    /// Returns true if this status makes verification fail.
    pub fn is_failure(self, strict: bool) -> bool {
        match self {
            Self::Unpinned | Self::Compromised | Self::Vulnerable | Self::NotVetted => true,
            Self::Unsigned => strict,
            Self::Pinned | Self::Vetted => false,
        }
    }
}

/// A dependency together with its verification outcome.
#[derive(Debug, Clone)]
pub struct VerifyFinding {
    /// The scanned dependency.
    pub task: UpdateTask,
    /// Its verification outcome.
    pub status: VerifyStatus,
    /// OSV advisory identifiers behind a `Compromised` or `Vulnerable` status.
    pub advisories: Vec<String>,
}

impl VerifyFinding {
    /// The reference as written in the file, or `latest` when none is given.
    pub fn reference(&self) -> &str {
        self.task.current_tag.as_deref().unwrap_or("latest")
    }

    /// ` (ID-1, ID-2)` when OSV advisories are attached, otherwise empty.
    fn advisory_suffix(&self) -> String {
        if self.advisories.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.advisories.join(", "))
        }
    }

    fn location(&self) -> String {
        format!(
            "{}:{}:{}",
            self.task.path.display(),
            self.task.line,
            self.task.column
        )
    }

    /// One-sentence description used by the GitHub and JUnit reports.
    fn message(&self) -> String {
        let action = &self.task.action;
        let reference = self.reference();
        match self.status {
            VerifyStatus::Unpinned => format!(
                "Dependency {} is not pinned to an immutable hash (found tag: {})",
                action, reference
            ),
            VerifyStatus::Compromised => format!(
                "Dependency {}@{} is COMPROMISED (Supply Chain Attack)!{}",
                action,
                reference,
                self.advisory_suffix()
            ),
            VerifyStatus::Vulnerable => format!(
                "Dependency {}@{} has known vulnerabilities{}",
                action,
                reference,
                self.advisory_suffix()
            ),
            VerifyStatus::Unsigned => format!(
                "Image {}@{} has no cosign signature; its provenance cannot be verified",
                action, reference
            ),
            VerifyStatus::NotVetted => format!(
                "Dependency {}@{} is pinned but not vetted (strict mode enabled)",
                action, reference
            ),
            VerifyStatus::Pinned | VerifyStatus::Vetted => {
                format!("Dependency {}@{} is pinned", action, reference)
            }
        }
    }
}

/// Counts of each failing category, used in summaries.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct VerifySummary {
    pub unpinned: usize,
    pub compromised: usize,
    pub vulnerable: usize,
    pub unsigned: usize,
    pub non_vetted: usize,
}

impl VerifySummary {
    pub fn from_findings(findings: &[VerifyFinding]) -> Self {
        let mut summary = Self::default();
        for f in findings {
            match f.status {
                VerifyStatus::Unpinned => summary.unpinned += 1,
                VerifyStatus::Compromised => summary.compromised += 1,
                VerifyStatus::Vulnerable => summary.vulnerable += 1,
                VerifyStatus::Unsigned => summary.unsigned += 1,
                VerifyStatus::NotVetted => summary.non_vetted += 1,
                VerifyStatus::Pinned | VerifyStatus::Vetted => {}
            }
        }
        summary
    }
}

/// Renders the human-readable report written to stderr.
pub fn render_text(findings: &[VerifyFinding], strict: bool) -> String {
    let mut out = String::new();
    for f in findings {
        let label = match f.status {
            VerifyStatus::Unpinned => "[✗ unpinned]",
            VerifyStatus::Compromised => "[✗ compromised]",
            VerifyStatus::Vulnerable => "[✗ vulnerable]",
            VerifyStatus::NotVetted => "[✗ not vetted]",
            VerifyStatus::Unsigned => "[! unsigned]",
            VerifyStatus::Pinned | VerifyStatus::Vetted => continue,
        };
        let failing = f.status.is_failure(strict);
        let marker = if failing {
            "✗".red().bold()
        } else {
            "⚠".yellow().bold()
        };
        let reference = if f.status == VerifyStatus::Compromised {
            f.reference().red()
        } else {
            f.reference().yellow()
        };
        let _ = writeln!(
            out,
            "  {} {}@{} in {}:{}:{} {}{}",
            marker,
            f.task.action.to_string().yellow(),
            reference,
            f.task.path.display().to_string().cyan(),
            f.task.line.to_string().magenta(),
            f.task.column.to_string().magenta(),
            label,
            f.advisory_suffix()
        );
    }

    let summary = VerifySummary::from_findings(findings);
    if findings.iter().any(|f| f.status.is_failure(strict)) {
        let _ = writeln!(
            out,
            "\n{} Verification failed! Some dependencies are not pinned, are compromised or vulnerable, or are not vetted.",
            "✗".red().bold()
        );
        let _ = writeln!(
            out,
            "{} Run `pinner pin` to automatically secure your dependencies, or `pinner verify --help` for options.",
            "hint:".blue()
        );
    } else {
        let _ = writeln!(
            out,
            "\n{} Verification successful! All dependencies are pinned and secure.",
            "✔".green().bold()
        );
        if summary.unsigned > 0 {
            let _ = writeln!(
                out,
                "{} {} image(s) have no cosign signature. Use `--strict` to fail on unsigned images.",
                "note:".yellow(),
                summary.unsigned
            );
        }
    }
    out
}

/// Renders GitHub Actions workflow commands (`::error` / `::warning`).
pub fn render_github(findings: &[VerifyFinding], strict: bool) -> String {
    let mut out = String::new();
    for f in findings {
        let level = match f.status {
            VerifyStatus::Pinned | VerifyStatus::Vetted => continue,
            status if status.is_failure(strict) => "error",
            _ => "warning",
        };
        let _ = writeln!(
            out,
            "::{} file={},line={},col={}::{}",
            level,
            github_property(&f.task.path.display().to_string()),
            f.task.line,
            f.task.column,
            github_data(&f.message())
        );
    }
    out
}

/// Renders a Markdown table suitable for PR comments or job summaries.
pub fn render_markdown(findings: &[VerifyFinding], strict: bool) -> String {
    let mut out = String::new();
    out.push_str("## Pinner Verification Report\n\n");
    out.push_str("| Status | Dependency | Reference | Location | Details |\n");
    out.push_str("| :---: | :--- | :--- | :--- | :--- |\n");
    for f in findings {
        let (icon, details) = match f.status {
            VerifyStatus::Unpinned => ("❌", "Unpinned mutable dependency"),
            VerifyStatus::Compromised => ("🚨", "Compromised (Supply Chain Attack)"),
            VerifyStatus::Vulnerable => ("🐛", "Known vulnerabilities"),
            VerifyStatus::NotVetted => ("⚠️", "Not vetted (strict mode)"),
            VerifyStatus::Unsigned if strict => ("❌", "Unsigned image (strict mode)"),
            VerifyStatus::Unsigned => ("⚠️", "Unsigned image (no cosign signature)"),
            VerifyStatus::Pinned => ("ℹ️", "Pinned (not vetted)"),
            VerifyStatus::Vetted => ("✔", "Pinned & Vetted"),
        };
        let _ = writeln!(
            out,
            "| {} | `{}` | `{}` | `{}` | {}{} |",
            icon,
            markdown_cell(&f.task.action.to_string()),
            markdown_cell(f.reference()),
            markdown_cell(&f.location()),
            details,
            markdown_cell(&f.advisory_suffix())
        );
    }

    let s = VerifySummary::from_findings(findings);
    if findings.iter().any(|f| f.status.is_failure(strict)) {
        let _ = write!(
            out,
            "\n> **Result**: ❌ Verification failed ({} unpinned, {} compromised, {} vulnerable, {} non-vetted",
            s.unpinned, s.compromised, s.vulnerable, s.non_vetted
        );
        if strict {
            let _ = write!(out, ", {} unsigned", s.unsigned);
        }
        out.push_str("). Run `pinner pin` to automatically secure your dependencies.\n");
    } else {
        out.push_str(
            "\n> **Result**: ✔ All dependencies are pinned to immutable hashes and secure.\n",
        );
        if s.unsigned > 0 {
            let _ = writeln!(
                out,
                ">\n> ⚠️ {} image(s) have no cosign signature.",
                s.unsigned
            );
        }
    }
    out
}

/// Renders a JUnit XML report with one test case per dependency.
pub fn render_junit(findings: &[VerifyFinding], strict: bool) -> String {
    let failures = findings
        .iter()
        .filter(|f| f.status.is_failure(strict))
        .count();

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        xml,
        "<testsuites name=\"Pinner Verification\" tests=\"{}\" failures=\"{}\" errors=\"0\" time=\"0.0\">",
        findings.len(),
        failures
    );
    let _ = writeln!(
        xml,
        "  <testsuite name=\"pinner.verify\" tests=\"{}\" failures=\"{}\" errors=\"0\" time=\"0.0\">",
        findings.len(),
        failures
    );
    for f in findings {
        let name = xml_escape(&f.task.action.to_string());
        let classname = xml_escape(&f.task.path.display().to_string());
        if f.status.is_failure(strict) {
            let short = match f.status {
                VerifyStatus::Unpinned => "Dependency is not pinned",
                VerifyStatus::Compromised => "Dependency is compromised",
                VerifyStatus::Vulnerable => "Dependency has known vulnerabilities",
                VerifyStatus::NotVetted => "Dependency is not vetted",
                _ => "Image is unsigned",
            };
            let _ = writeln!(
                xml,
                "    <testcase name=\"{}\" classname=\"{}\" time=\"0.0\">\n      <failure message=\"{}\">{} in {}</failure>\n    </testcase>",
                name,
                classname,
                short,
                xml_escape(&f.message()),
                xml_escape(&f.location())
            );
        } else {
            let _ = writeln!(
                xml,
                "    <testcase name=\"{}\" classname=\"{}\" time=\"0.0\"/>",
                name, classname
            );
        }
    }
    xml.push_str("  </testsuite>\n</testsuites>\n");
    xml
}

/// Escapes text for use in XML attributes and element content.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escapes the message part of a GitHub workflow command.
fn github_data(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escapes a property value (e.g. `file=`) of a GitHub workflow command.
fn github_property(s: &str) -> String {
    github_data(s).replace(':', "%3A").replace(',', "%2C")
}

/// Escapes a value placed inside a Markdown table cell.
fn markdown_cell(s: &str) -> String {
    s.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(action: &str, tag: Option<&str>, path: &str, status: VerifyStatus) -> VerifyFinding {
        VerifyFinding {
            task: UpdateTask {
                path: path.into(),
                action: action.into(),
                current_tag: tag.map(String::from),
                line: 3,
                column: 7,
                ..Default::default()
            },
            status,
            advisories: Vec::new(),
        }
    }

    #[test]
    fn test_status_failure_depends_on_strict() {
        assert!(VerifyStatus::Unpinned.is_failure(false));
        assert!(VerifyStatus::Compromised.is_failure(false));
        assert!(!VerifyStatus::Unsigned.is_failure(false));
        assert!(VerifyStatus::Unsigned.is_failure(true));
        assert!(!VerifyStatus::Pinned.is_failure(true));
        assert!(!VerifyStatus::Vetted.is_failure(true));
    }

    #[test]
    fn test_render_text_unsigned_is_a_warning() {
        colored::control::set_override(false);
        let findings = vec![finding(
            "alpine",
            Some("sha256:abc"),
            "ci.yml",
            VerifyStatus::Unsigned,
        )];

        let out = render_text(&findings, false);
        assert!(out.contains("⚠ alpine@sha256:abc in ci.yml:3:7 [! unsigned]"));
        assert!(out.contains("Verification successful"));
        assert!(out.contains("1 image(s) have no cosign signature"));

        let out = render_text(&findings, true);
        assert!(out.contains("✗ alpine@sha256:abc"));
        assert!(out.contains("Verification failed"));
    }

    #[test]
    fn test_render_github_levels_and_escaping() {
        let findings = vec![
            finding("a/b", Some("v1"), "dir,x/ci.yml", VerifyStatus::Unpinned),
            finding(
                "alpine",
                Some("sha256:abc"),
                "ci.yml",
                VerifyStatus::Unsigned,
            ),
            finding("c/d", Some("sha"), "ci.yml", VerifyStatus::Vetted),
        ];
        let out = render_github(&findings, false);
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "::error file=dir%2Cx/ci.yml,line=3,col=7::Dependency a/b is not pinned to an immutable hash (found tag: v1)"
        );
        assert!(lines[1].starts_with("::warning file=ci.yml,line=3,col=7::Image alpine@sha256:abc"));

        assert!(render_github(&findings, true)
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("::error"));
    }

    #[test]
    fn test_render_markdown() {
        let findings = vec![
            finding("a/b", Some("v1"), "ci.yml", VerifyStatus::Unpinned),
            finding("c|d", Some("sha"), "ci.yml", VerifyStatus::Vetted),
        ];
        let out = render_markdown(&findings, false);
        assert!(out.starts_with("## Pinner Verification Report"));
        assert!(out.contains("| ❌ | `a/b` | `v1` | `ci.yml:3:7` | Unpinned mutable dependency |"));
        assert!(out.contains("`c\\|d`"));
        assert!(out.contains("(1 unpinned, 0 compromised, 0 vulnerable, 0 non-vetted)"));
    }

    #[test]
    fn test_render_junit_escapes_xml() {
        let findings = vec![
            finding("a/b", Some("v1"), "R&D/<ci>.yml", VerifyStatus::Unpinned),
            finding("c/d", Some("sha"), "ci.yml", VerifyStatus::Pinned),
        ];
        let out = render_junit(&findings, false);
        assert!(out.contains("tests=\"2\" failures=\"1\""));
        assert!(out.contains("classname=\"R&amp;D/&lt;ci&gt;.yml\""));
        assert!(out.contains("in R&amp;D/&lt;ci&gt;.yml:3:7</failure>"));
        assert!(!out.contains("R&D"));
        assert!(out.contains("<testcase name=\"c/d\" classname=\"ci.yml\" time=\"0.0\"/>"));
    }

    #[test]
    fn test_summary_counts() {
        let findings = vec![
            finding("a", None, "f", VerifyStatus::Unpinned),
            finding("b", None, "f", VerifyStatus::Unsigned),
            finding("c", None, "f", VerifyStatus::Unsigned),
            finding("d", None, "f", VerifyStatus::Vetted),
        ];
        assert_eq!(
            VerifySummary::from_findings(&findings),
            VerifySummary {
                unpinned: 1,
                compromised: 0,
                vulnerable: 0,
                unsigned: 2,
                non_vetted: 0
            }
        );
    }

    #[test]
    fn test_vulnerable_findings_list_advisories() {
        colored::control::set_override(false);
        let mut f = finding("a/b", Some("sha"), "ci.yml", VerifyStatus::Vulnerable);
        f.advisories = vec!["GHSA-1".into(), "GHSA-2".into()];
        let findings = vec![f];

        assert!(VerifyStatus::Vulnerable.is_failure(false));
        assert!(render_text(&findings, false)
            .contains("a/b@sha in ci.yml:3:7 [✗ vulnerable] (GHSA-1, GHSA-2)"));
        assert!(render_github(&findings, false).contains(
            "::error file=ci.yml,line=3,col=7::Dependency a/b@sha has known vulnerabilities (GHSA-1, GHSA-2)"
        ));
        assert!(render_markdown(&findings, false).contains(
            "| 🐛 | `a/b` | `sha` | `ci.yml:3:7` | Known vulnerabilities (GHSA-1, GHSA-2) |"
        ));
        assert!(render_junit(&findings, false)
            .contains("<failure message=\"Dependency has known vulnerabilities\">"));
    }
}
