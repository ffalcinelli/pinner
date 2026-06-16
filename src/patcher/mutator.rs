use crate::core::UpdateResult;
use crate::error::PinnerError;
use regex::Regex;
use std::sync::LazyLock;

static COMMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^#\s*(v\d[a-zA-Z0-9.\-_]*|main|\d[a-zA-Z0-9.\-_]*)\s*")
        .expect("Failed to compile COMMENT_REGEX")
});

/// Applies an update to the string content of a YAML file.
///
/// This function surgically modifies the source text at the precise byte offsets
/// identified during the scanning phase. It handles comment preservation, appending
/// the old tag as a comment if necessary, and formatting the dependency URI correctly
/// based on the CI provider key.
///
/// # Examples
///
/// ```
/// use pinner::core::{DependencyName, DependencyRef, UpdateResult, UpdateTask};
/// use pinner::patcher::mutator::apply_update;
/// use std::path::PathBuf;
///
/// let mut content = "uses: actions/checkout@v3 # keep me".to_string();
/// let res = UpdateResult {
///     action: DependencyName::from("actions/checkout"),
///     path: PathBuf::from("f.yml"),
///     old_tag: Some("v3".to_string()),
///     task: UpdateTask {
///         path: PathBuf::from("f.yml"),
///         start: 6,
///         end: 25,
///         line: 1,
///         column: 1,
///         action: DependencyName::from("actions/checkout"),
///         current_tag: Some("v3".to_string()),
///         comment: Some("# keep me".to_string()),
///         key: "uses".to_string(),
///         provider: pinner::core::CiProvider::GitHub,
///     },
///     new_sha: DependencyRef::from("hashv3".to_string()),
///     new_tag: Some("v3".to_string()),
/// };
///
/// let result = apply_update(&mut content, &res).unwrap();
/// assert!(result.is_some());
/// assert_eq!(content, "uses: actions/checkout@hashv3 # v3 # keep me");
/// ```
pub fn apply_update(
    content: &mut String,
    res: &UpdateResult,
) -> Result<Option<(String, String)>, PinnerError> {
    let line_end = content[res.task.end..]
        .find('\n')
        .map(|pos| res.task.end + pos)
        .unwrap_or(content.len());

    let old_val_with_suffix = &content[res.task.start..line_end];
    let suffix = &content[res.task.end..line_end];

    let mut final_suffix = suffix.trim_start().to_string();
    if let Some(parser_comment) = &res.task.comment {
        let c = parser_comment.trim_start_matches('#').trim();
        if let Some(mat) = COMMENT_REGEX.find(parser_comment) {
            let matched_comment = mat.as_str().trim_start_matches('#').trim();
            if matched_comment == c {
                final_suffix = "".to_string();
            }
        }
    } else if let Some(mat) = COMMENT_REGEX.find(&final_suffix) {
        final_suffix = final_suffix[mat.end()..].trim_start().to_string();
        if final_suffix.starts_with('#') {
            final_suffix = final_suffix[1..].trim_start().to_string();
        }
    }

    let new_comment = if let Some(t) = &res.new_tag {
        let is_sha =
            (t.len() == 40 && t.chars().all(|c| c.is_ascii_hexdigit())) || t.starts_with("sha256:");
        if is_sha {
            "".to_string()
        } else {
            format!(" # {}", t)
        }
    } else {
        "".to_string()
    };

    let extra_suffix = if final_suffix.is_empty() {
        "".to_string()
    } else if final_suffix.starts_with('#') {
        format!(" {}", final_suffix)
    } else {
        format!(" # {}", final_suffix)
    };

    let new_val = if res.task.key == "ref" {
        format!("{}{}{}", res.new_sha, new_comment, extra_suffix)
    } else {
        let separator = if res.task.key == "pipe" { ":" } else { "@" };
        format!(
            "{}{}{}{}{}",
            res.task.action, separator, res.new_sha, new_comment, extra_suffix
        )
    };

    if old_val_with_suffix == new_val {
        return Ok(None);
    }

    let old_val = old_val_with_suffix.to_string();
    content.replace_range(res.task.start..line_end, &new_val);
    Ok(Some((old_val, new_val)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{DependencyName, DependencyRef, UpdateResult, UpdateTask};
    use std::path::PathBuf;

    #[test]
    fn test_apply_update_basic() {
        let mut content = "uses: actions/checkout@v3".to_string();
        let res = UpdateResult {
            action: DependencyName::from("actions/checkout"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v3".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 6,
                end: 25,
                action: DependencyName::from("actions/checkout"),
                current_tag: Some("v3".to_string()),
                comment: None,
                key: "uses".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("hashv3".to_string()),
            new_tag: Some("v3".to_string()),
        };

        let result = apply_update(&mut content, &res).unwrap();
        assert!(result.is_some());
        assert_eq!(content, "uses: actions/checkout@hashv3 # v3");
    }

    #[test]
    fn test_apply_update_with_existing_comment() {
        let mut content = "uses: o/r@v1 # keep me".to_string();
        let res = UpdateResult {
            action: DependencyName::from("o/r"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 6,
                end: 12,
                action: DependencyName::from("o/r"),
                current_tag: Some("v1".to_string()),
                comment: Some("# keep me".to_string()),
                key: "uses".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v2".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v2 # keep me");
    }

    #[test]
    fn test_apply_update_comment_regex_replacement() {
        let mut content = "uses: o/r@v1 # v1".to_string();
        let res = UpdateResult {
            action: DependencyName::from("o/r"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 6,
                end: 12,
                action: DependencyName::from("o/r"),
                current_tag: Some("v1".to_string()),
                comment: Some("# v1".to_string()),
                key: "uses".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v2".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v2");
    }

    #[test]
    fn test_apply_update_no_redundant_sha_comment() {
        let mut content = "image: cimg/base@sha256:oldhash # stable".to_string();
        let res = UpdateResult {
            action: DependencyName::from("cimg/base"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("sha256:oldhash".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 7,
                end: 31,
                action: DependencyName::from("cimg/base"),
                current_tag: Some("sha256:oldhash".to_string()),
                comment: Some("# stable".to_string()),
                key: "image".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("sha256:newhash".to_string()),
            new_tag: Some("sha256:newhash".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        // Since new_tag is a SHA, it shouldn't be added as a comment.
        assert_eq!(content, "image: cimg/base@sha256:newhash # stable");
    }

    #[test]
    fn test_apply_update_gitlab_ref() {
        let mut content = "ref: v1".to_string();
        let res = UpdateResult {
            action: DependencyName::from("proj"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 5,
                end: 7,
                action: DependencyName::from("proj"),
                current_tag: Some("v1".to_string()),
                comment: None,
                key: "ref".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitLab,
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v1".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "ref: hash # v1");
    }
}
