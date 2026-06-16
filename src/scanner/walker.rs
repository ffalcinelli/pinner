use crate::core::UpdateTask;
use crate::error::PinnerError;
use crate::scanner::parser::find_tasks;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tree_sitter::Parser as TSParser;

thread_local! {
    static PARSER: std::cell::RefCell<TSParser> = std::cell::RefCell::new({
        let mut parser = TSParser::new();
        parser.set_language(&tree_sitter_yaml::LANGUAGE.into()).expect("Failed to load YAML grammar");
        parser
    });
}

pub struct Scanner {
    pub ignore_list: Vec<String>,
}

impl Scanner {
    pub fn new(ignore_list: Vec<String>) -> Self {
        Self { ignore_list }
    }

    pub async fn collect_tasks(
        &self,
        paths: &[PathBuf],
    ) -> Result<(Vec<UpdateTask>, HashMap<PathBuf, String>), PinnerError> {
        let mut all_paths = Vec::new();

        for path in paths {
            if !path.exists() {
                return Err(PinnerError::PathNotFound(path.display().to_string()));
            }

            let mut override_builder = OverrideBuilder::new(path);
            for ignore_pattern in &self.ignore_list {
                // If the pattern doesn't start with '!', we treat it as an exclusion
                let pattern = if ignore_pattern.starts_with('!') {
                    ignore_pattern.clone()
                } else {
                    format!("!{}", ignore_pattern)
                };
                override_builder
                    .add(&pattern)
                    .map_err(|e| PinnerError::Parse(format!("Invalid ignore pattern: {}", e)))?;
            }

            let overrides = override_builder
                .build()
                .map_err(|e| PinnerError::Parse(format!("Failed to build overrides: {}", e)))?;

            for entry in WalkBuilder::new(path).overrides(overrides).build() {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
                    all_paths.push(path.to_path_buf());
                }
            }
        }

        let ignore_list = self.ignore_list.clone();

        let results = tokio::task::spawn_blocking(move || {
            use rayon::prelude::*;
            type CollectResult = Result<(Vec<UpdateTask>, (PathBuf, String)), PinnerError>;
            let results: Vec<CollectResult> = all_paths
                .into_par_iter()
                .map(|path| {
                    let content = fs::read_to_string(&path)?;

                    let tree = PARSER
                        .with(|parser| {
                            let mut parser = parser.borrow_mut();
                            parser.reset();
                            parser.parse(&content, None)
                        })
                        .ok_or_else(|| {
                            PinnerError::Parse(format!("Failed to parse {}", path.display()))
                        })?;

                    let tasks =
                        find_tasks(&path, tree.root_node(), content.as_bytes(), &ignore_list)?;
                    Ok((tasks, (path, content)))
                })
                .collect();
            results
        })
        .await
        .unwrap_or_else(|e| vec![Err(PinnerError::Io(std::io::Error::other(e.to_string())))]);

        let mut final_tasks = Vec::new();
        let mut final_file_contents = HashMap::new();

        for res in results {
            let (tasks, (path, content)) = res?;
            final_tasks.extend(tasks);
            final_file_contents.insert(path, content);
        }

        Ok((final_tasks, final_file_contents))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_scanner_collect_tasks() {
        let dir = tempdir().unwrap();
        let wf = dir.path().join("ci.yml");
        fs::write(&wf, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let (tasks, contents) = scanner
            .collect_tasks(&[dir.path().to_path_buf()])
            .await
            .unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].action.0, "actions/checkout");
        assert!(contents.contains_key(&wf));
    }

    #[tokio::test]
    async fn test_scanner_ignore() {
        let dir = tempdir().unwrap();
        let wf = dir.path().join("ci.yml");
        fs::write(&wf, "uses: actions/checkout@v3\nuses: ignore/me@v1").unwrap();

        let scanner = Scanner::new(vec!["ignore/me".to_string()]);
        let (tasks, _) = scanner
            .collect_tasks(&[dir.path().to_path_buf()])
            .await
            .unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].action.0, "actions/checkout");
    }

    #[tokio::test]
    async fn test_scanner_path_not_found() {
        let scanner = Scanner::new(vec![]);
        let res = scanner
            .collect_tasks(&[PathBuf::from("/non/existent/path/999")])
            .await;
        assert!(res.is_err());
    }
}
