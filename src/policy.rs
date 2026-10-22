//! Per-call configuration discovery and ordered policy selection.
use crate::protocol::Tool;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallContext {
    pub cwd: PathBuf,
    #[serde(default)]
    pub command: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default)]
pub enum Selection {
    #[default]
    Builtin,
    Off,
    Methods(Vec<Method>),
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    LogCompaction,
    SearchHitGroups,
    PathPrefixGroups,
    JsonCompact,
    JsonToon,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    #[serde(rename = "match")]
    condition: Condition,
    compression: Option<Compression>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Condition {
    tool: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Compression {
    One(String),
    Many(Vec<Method>),
}

impl Rule {
    fn selection(&self) -> Result<Selection, String> {
        match &self.compression {
            None => Ok(Selection::Builtin),
            Some(Compression::One(value)) if value == "off" => Ok(Selection::Off),
            Some(Compression::One(value)) => {
                serde_json::from_value(serde_json::Value::String(value.clone()))
                    .map(|method| Selection::Methods(vec![method]))
                    .map_err(|error| error.to_string())
            }
            Some(Compression::Many(methods)) if !methods.is_empty() => {
                Ok(Selection::Methods(methods.clone()))
            }
            _ => Err("compression methods must not be empty".into()),
        }
    }
}

pub fn resolve(home: &Path, context: Option<&CallContext>, tool: &Tool) -> Selection {
    let global = home.join(".outrim/config.json");
    let project = context.and_then(|context| {
        context.cwd.ancestors().find_map(|directory| {
            let path = directory.join(".outrim/config.json");
            (path != global && path.try_exists().unwrap_or(true)).then_some(path)
        })
    });
    for path in project.iter().chain(std::iter::once(&global)) {
        let result = (|| -> Result<Option<Selection>, String> {
            let text = match fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.to_string()),
            };
            let config: Config = serde_json::from_str(&text).map_err(|error| error.to_string())?;
            // Validate the entire file before allowing any rule to apply.
            let selections = config
                .rules
                .iter()
                .map(Rule::selection)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(config
                .rules
                .iter()
                .zip(selections)
                .find_map(|(rule, selection)| {
                    rule.condition
                        .tool
                        .as_ref()
                        .is_none_or(|name| name == &tool.name)
                        .then_some(selection)
                }))
        })();
        match result {
            Ok(Some(selection)) => return selection,
            Ok(None) => {}
            Err(error) => eprintln!("outrim: invalid configuration {}: {error}", path.display()),
        }
    }
    Selection::Builtin
}
