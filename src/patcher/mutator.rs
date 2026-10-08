use crate::core::update::VERSION_COMMENT_REGEX as COMMENT_REGEX;
use crate::core::{is_hash_ref, UpdateResult};
use crate::error::PinnerError;

/// Applies an update to the string content of a YAML file.
///
/// This function surgically modifies the source text at the precise byte offsets
/// identified during the scanning phase. It handles:
/// 1. Preservation of existing non-version comments.
/// 2. Appending the old tag as a comment for readability (security best practice).
/// 3. Correct separator usage (`@` for most, `:` for Bitbucket pipes).
///
/// Returns `Ok(Some((old_text, new_text)))` if a change was applied, or `Ok(None)` if
/// the content remains identical.
pub fn apply_update(
    content: &mut String,
    res: &UpdateResult,
) -> Result<Option<(String, String)>, PinnerError> {
    // Determine the end of the line to capture any existing trailing comments.
    // A CRLF terminator is left outside the replaced range so line endings survive.
    let mut line_end = content[res.task.end..]
        .find('\n')
        .map(|pos| res.task.end + pos)
        .unwrap_or(content.len());
    if line_end > res.task.end && content.as_bytes()[line_end - 1] == b'\r' {
        line_end -= 1;
    }

    let suffix = &content[res.task.end..line_end];

    // logic to handle existing comments:
    // If the comment was just the version (e.g., "# v1"), we want to replace it.
    // If it contained more info (e.g., "# v1 # important"), we want to keep the extra info.
    let mut final_suffix = suffix.trim_start().to_string();
    if let Some(parser_comment) = &res.task.comment {
        if let Some(mat) = COMMENT_REGEX.find(parser_comment) {
            // Strip the version part but keep the rest.
            final_suffix = parser_comment[mat.end()..].trim_start().to_string();
        } else {
            final_suffix = parser_comment.clone();
        }
    } else if let Some(mat) = COMMENT_REGEX.find(&final_suffix) {
        final_suffix = final_suffix[mat.end()..].trim_start().to_string();
    }

    // Ensure we don't have a double # at the start if we stripped the first one.
    if final_suffix.starts_with('#') {
        final_suffix = final_suffix[1..].trim_start().to_string();
    }

    // Prepare the new comment showing the symbolic tag (e.g., " # v3").
    let new_comment = if let Some(t) = &res.new_tag {
        if is_hash_ref(t) {
            // Don't add a comment if the tag is already a SHA or digest.
            "".to_string()
        } else {
            format!(" # {}", t)
        }
    } else {
        "".to_string()
    };

    // Reconstruct the trailing part of the line, merging the new version comment with any existing comments.
    let extra_suffix = if final_suffix.is_empty() {
        "".to_string()
    } else if final_suffix.starts_with('#') {
        format!(" {}", final_suffix)
    } else {
        format!(" # {}", final_suffix)
    };

    let new_val = if res.task.key == "ref" {
        format!("{}{}{}", res.new_sha, new_comment, extra_suffix)
    } else if let Some(inline_tag) = &res.task.image_tag {
        // Keep the `name:tag@digest` layout. The tag is already visible inline,
        // so no version comment is added.
        let tag = res
            .new_tag
            .as_deref()
            .filter(|t| !is_hash_ref(t))
            .unwrap_or(inline_tag);
        format!(
            "{}:{}@{}{}",
            res.task.action, tag, res.new_sha, extra_suffix
        )
    } else {
        let separator = if res.task.key == "pipe" { ":" } else { "@" };
        format!(
            "{}{}{}{}{}",
            res.task.action, separator, res.new_sha, new_comment, extra_suffix
        )
    };

    // If the line directly above is a version-only comment (e.g. `# v1`), refresh it
    // as well, keeping any annotation after the version and the original line ending.
    let mut start_range = res.task.start;
    let mut prefix_replacement = String::new();

    let line_start = content[..res.task.start]
        .rfind('\n')
        .map_or(0, |pos| pos + 1);
    if let (true, Some(new_t)) = (line_start > 0, &res.new_tag) {
        let prev_start = content[..line_start - 1]
            .rfind('\n')
            .map_or(0, |pos| pos + 1);
        let prev_line = &content[prev_start..line_start];
        let body = prev_line.trim_end_matches(['\n', '\r']);
        let newline = &prev_line[body.len()..];
        let trimmed = body.trim();

        if let Some(mat) = COMMENT_REGEX.find(trimmed) {
            if !is_hash_ref(new_t) {
                let indent = &body[..body.len() - body.trim_start().len()];
                let rest = trimmed[mat.end()..].trim();
                let rest = if rest.is_empty() {
                    String::new()
                } else {
                    format!(" # {}", rest)
                };

                start_range = prev_start;
                prefix_replacement = format!(
                    "{}# {}{}{}{}",
                    indent,
                    new_t,
                    rest,
                    newline,
                    &content[line_start..res.task.start]
                );
            }
        }
    }

    let full_new_text = format!("{}{}", prefix_replacement, new_val);
    let full_old_text = content[start_range..line_end].to_string();

    if full_old_text == full_new_text {
        return Ok(None);
    }

    // Surgically replace the range in the original content string.
    content.replace_range(start_range..line_end, &full_new_text);
    Ok(Some((full_old_text, full_new_text)))
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
                preceding_comments: None,
                image_tag: None,
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
    fn test_apply_update_preceding_comment() {
        let mut content = "# v1\nuses: actions/checkout@v1".to_string();
        let res = UpdateResult {
            action: DependencyName::from("actions/checkout"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 11,
                end: 30,
                action: DependencyName::from("actions/checkout"),
                current_tag: Some("v1".to_string()),
                comment: None,
                preceding_comments: Some("# v1".to_string()),
                image_tag: None,
                key: "uses".to_string(),
                line: 2,
                column: 7,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("hashv2".to_string()),
            new_tag: Some("v2".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "# v2\nuses: actions/checkout@hashv2 # v2");
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
                preceding_comments: None,
                image_tag: None,
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
                preceding_comments: None,
                image_tag: None,
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
                preceding_comments: None,
                image_tag: None,
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
                preceding_comments: None,
                image_tag: None,
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

    #[test]
    fn test_apply_update_no_newline_at_end() {
        let mut content = "uses: o/r@v1".to_string(); // No newline
        let res = UpdateResult {
            action: "o/r".into(),
            path: "f.yml".into(),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: "f.yml".into(),
                start: 6,
                end: 12,
                action: "o/r".into(),
                current_tag: Some("v1".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v1".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v1");
    }

    #[test]
    fn test_apply_update_complex_comments() {
        let mut content = "uses: o/r@v1  # v1 # keep # me".to_string();
        let res = UpdateResult {
            action: "o/r".into(),
            path: "f.yml".into(),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: "f.yml".into(),
                start: 6,
                end: 12,
                action: "o/r".into(),
                current_tag: Some("v1".to_string()),
                comment: Some("# v1 # keep # me".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v2".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v2 # keep # me");
    }

    #[test]
    fn test_apply_update_docker_digest() {
        let mut content = "image: alpine:latest".to_string();
        let res = UpdateResult {
            action: "alpine".into(),
            path: "f.yml".into(),
            old_tag: Some("latest".to_string()),
            task: UpdateTask {
                path: "f.yml".into(),
                start: 7,
                end: 20,
                action: "alpine".into(),
                current_tag: Some("latest".to_string()),
                key: "image".to_string(),
                ..Default::default()
            },
            new_sha: DependencyRef::from("sha256:digest".to_string()),
            new_tag: Some("latest".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "image: alpine@sha256:digest # latest");
    }

    fn result_for(
        content: &str,
        value: &str,
        comment: Option<&str>,
        new_tag: &str,
    ) -> UpdateResult {
        let start = content.find(value).unwrap();
        let line = content[..start].matches('\n').count() + 1;
        UpdateResult {
            action: "o/r".into(),
            path: "f.yml".into(),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: "f.yml".into(),
                start,
                end: start + value.len(),
                line,
                action: "o/r".into(),
                current_tag: Some("v1".to_string()),
                comment: comment.map(String::from),
                key: "uses".to_string(),
                ..Default::default()
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some(new_tag.to_string()),
        }
    }

    #[test]
    fn test_apply_update_keeps_free_form_trailing_comments() {
        for comment in [
            "# mainly for tests",
            "# 3 retries",
            "# v1 is broken upstream",
        ] {
            let mut content = format!("uses: o/r@v1 {}", comment);
            let res = result_for(&content, "o/r@v1", Some(comment), "v2");
            apply_update(&mut content, &res).unwrap();
            assert_eq!(content, format!("uses: o/r@hash # v2 {}", comment));
        }
    }

    #[test]
    fn test_apply_update_keeps_free_form_trailing_comment_without_parser_comment() {
        let mut content = "uses: o/r@v1 # mainly for tests".to_string();
        let res = result_for(&content, "o/r@v1", None, "v2");
        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v2 # mainly for tests");
    }

    #[test]
    fn test_apply_update_preceding_comment_keeps_annotation() {
        let mut content = "  # v1 # pinned for compat\n  uses: o/r@v1".to_string();
        let res = result_for(&content, "o/r@v1", None, "v2");
        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "  # v2 # pinned for compat\n  uses: o/r@hash # v2");
    }

    #[test]
    fn test_apply_update_preceding_free_form_comment_untouched() {
        for header in [
            "# main build step",
            "# 3 retries below",
            "# v1 important note",
        ] {
            let mut content = format!("{}\nuses: o/r@v1", header);
            let res = result_for(&content, "o/r@v1", None, "v2");
            apply_update(&mut content, &res).unwrap();
            assert_eq!(content, format!("{}\nuses: o/r@hash # v2", header));
        }
    }

    #[test]
    fn test_apply_update_preserves_crlf() {
        let mut content = "# v1\r\nuses: o/r@v1\r\nnext: line\r\n".to_string();
        let res = result_for(&content, "o/r@v1", None, "v2");
        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "# v2\r\nuses: o/r@hash # v2\r\nnext: line\r\n");
    }

    #[test]
    fn test_apply_update_crlf_with_trailing_comment() {
        let mut content = "uses: o/r@v1 # keep\r\n".to_string();
        let res = result_for(&content, "o/r@v1", Some("# keep"), "v2");
        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v2 # keep\r\n");
    }

    #[test]
    fn test_apply_update_keeps_inline_image_tag() {
        let old = format!("sha256:{}", "1".repeat(64));
        let new = format!("sha256:{}", "2".repeat(64));
        let mut content = format!("image: alpine:3.20@{} # keep", old);
        let value = format!("alpine:3.20@{}", old);
        let start = content.find(&value).unwrap();
        let res = UpdateResult {
            action: "alpine".into(),
            path: "f.yml".into(),
            old_tag: Some(old.clone()),
            task: UpdateTask {
                path: "f.yml".into(),
                start,
                end: start + value.len(),
                line: 1,
                action: "alpine".into(),
                current_tag: Some(old.clone()),
                comment: Some("# keep".to_string()),
                image_tag: Some("3.20".to_string()),
                key: "image".to_string(),
                ..Default::default()
            },
            new_sha: DependencyRef::from(new.clone()),
            new_tag: Some("3.20".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, format!("image: alpine:3.20@{} # keep", new));
    }
}
