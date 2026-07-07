use crate::error::PinnerError;
use crate::pipeline::Pipeline;
use std::path::PathBuf;

impl Pipeline {
    /// Exports an SBOM for all dependencies in the provided paths.
    pub async fn export_sbom(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let (tasks, _) = self.scanner.collect_tasks(paths).await?;

        #[derive(serde::Serialize)]
        struct Sbom {
            #[serde(rename = "bomFormat")]
            bom_format: String,
            #[serde(rename = "specVersion")]
            spec_version: String,
            components: Vec<Component>,
        }

        #[derive(serde::Serialize)]
        struct Component {
            name: String,
            version: String,
            #[serde(rename = "type")]
            component_type: String,
            purl: String,
        }

        let mut components = Vec::new();
        for task in tasks {
            let name = task.action.to_string();
            let version = task
                .current_tag
                .clone()
                .unwrap_or_else(|| "latest".to_string());
            let (component_type, purl) = if name.contains('/') && !name.contains('.') {
                (
                    "library",
                    format!("pkg:github/{}@{}", name, version.replace('@', "")),
                )
            } else {
                ("container", format!("pkg:oci/{}@{}", name, version))
            };

            components.push(Component {
                name,
                version,
                component_type: component_type.to_string(),
                purl,
            });
        }

        let sbom = Sbom {
            bom_format: "CycloneDX".to_string(),
            spec_version: "1.5".to_string(),
            components,
        };

        println!(
            "{}",
            serde_json::to_string_pretty(&sbom).map_err(|e| PinnerError::Config(e.to_string()))?
        );
        Ok(())
    }
}
