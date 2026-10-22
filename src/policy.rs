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
    #[serde(default, deserialize_with = "present")]
    compression: Option<Compression>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Condition {
    #[serde(default, deserialize_with = "present")]
    tool: Option<Pattern>,
    #[serde(default, deserialize_with = "present")]
    command: Option<Vec<Pattern>>,
    #[serde(default)]
    exact: bool,
    #[serde(default, deserialize_with = "present")]
    input: Option<serde_json::Map<String, serde_json::Value>>,
}

// Missing conditions are allowed; explicit nulls must not silently broaden a rule.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Pattern {
    Literal(String),
    Glob(Glob),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Glob {
    glob: String,
}

impl Pattern {
    fn text(&self) -> &str {
        match self {
            Self::Literal(value) => value,
            Self::Glob(value) => &value.glob,
        }
    }

    fn matches(&self, value: &str) -> bool {
        match self {
            Self::Literal(literal) => literal == value,
            Self::Glob(pattern) => glob_matches(&pattern.glob, value),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if let Self::Glob(pattern) = self {
            validate_glob(&pattern.glob)?;
        }
        Ok(())
    }
}

impl Condition {
    fn validate(&self) -> Result<(), String> {
        if self.exact && self.command.is_none() {
            return Err("exact requires command".into());
        }
        if let Some(command) = &self.command {
            if command.is_empty() || command[0].text().is_empty() {
                return Err("command requires a program".into());
            }
            for pattern in command {
                pattern.validate()?;
            }
        }
        if let Some(tool) = &self.tool {
            tool.validate()?;
        }
        if let Some(input) = &self.input {
            validate_input(&serde_json::Value::Object(input.clone()))?;
        }
        Ok(())
    }

    fn matches(&self, context: Option<&CallContext>, tool: &Tool) -> bool {
        if self
            .tool
            .as_ref()
            .is_some_and(|pattern| !pattern.matches(&tool.name))
        {
            return false;
        }
        if let Some(input) = &self.input
            && !input_matches(&serde_json::Value::Object(input.clone()), &tool.input)
        {
            return false;
        }
        if let Some(patterns) = &self.command {
            let Some(context) = context else {
                return false;
            };
            let Some(argv) = context.command.as_ref() else {
                return false;
            };
            if argv.len() < patterns.len() || (self.exact && argv.len() != patterns.len()) {
                return false;
            }
            if !program_matches(&patterns[0], &argv[0], &context.cwd) {
                return false;
            }
            if !patterns[1..]
                .iter()
                .zip(&argv[1..])
                .all(|(pattern, value)| pattern.matches(value))
            {
                return false;
            }
        }
        true
    }
}

fn validate_glob(pattern: &str) -> Result<(), String> {
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.next().is_none() {
            return Err("glob ends with an escape".into());
        }
    }
    Ok(())
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    // A rolling row avoids recursive/exponential backtracking for user patterns.
    let chars: Vec<_> = value.chars().collect();
    let mut row = vec![false; chars.len() + 1];
    row[0] = true;
    let mut pattern = pattern.chars();
    while let Some(token) = pattern.next() {
        if token == '*' {
            for index in 1..row.len() {
                row[index] |= row[index - 1];
            }
        } else {
            let literal = if token == '\\' {
                pattern.next()
            } else {
                Some(token)
            };
            let wildcard = token == '?';
            for index in (1..row.len()).rev() {
                row[index] = row[index - 1] && (wildcard || literal == Some(chars[index - 1]));
            }
            row[0] = false;
        }
    }
    row[chars.len()]
}

fn validate_input(value: &serde_json::Value) -> Result<(), String> {
    if let Some(object) = value.as_object() {
        if object.contains_key("glob") {
            let pattern: Glob =
                serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
            validate_glob(&pattern.glob)?;
        } else {
            for value in object.values() {
                validate_input(value)?;
            }
        }
    }
    Ok(())
}

fn input_matches(expected: &serde_json::Value, actual: &serde_json::Value) -> bool {
    if let Some(object) = expected.as_object() {
        if let Some(pattern) = object.get("glob").and_then(serde_json::Value::as_str) {
            return actual
                .as_str()
                .is_some_and(|value| glob_matches(pattern, value));
        }
        return actual.as_object().is_some_and(|actual| {
            object.iter().all(|(key, value)| {
                actual
                    .get(key)
                    .is_some_and(|actual| input_matches(value, actual))
            })
        });
    }
    expected == actual
}

fn program_matches(pattern: &Pattern, program: &str, cwd: &Path) -> bool {
    let has_directory =
        |value: &str| value.contains('/') || (cfg!(windows) && value.contains('\\'));
    let (expected, actual) = if has_directory(pattern.text()) {
        let expected = match pattern {
            Pattern::Literal(value) => program_path(value, cwd),
            Pattern::Glob(value) => {
                let cwd = cwd.to_string_lossy();
                let cwd = if cfg!(windows) {
                    cwd.replace('\\', "/")
                } else {
                    cwd.into_owned()
                };
                // Normalize escaped native separators without changing
                // escapes that make wildcard characters literal.
                let glob = if cfg!(windows) {
                    value.glob.replace("\\\\", "/")
                } else {
                    value.glob.clone()
                };
                let path = if Path::new(&glob).is_absolute() {
                    glob
                } else {
                    format!("{cwd}/{glob}")
                };
                // Resolve dot components without consuming glob escape characters.
                let mut components = Vec::new();
                for component in path.split('/') {
                    match component {
                        "." => {}
                        ".." if components.last().is_some_and(|part: &&str| {
                            !part.is_empty() && !part.ends_with(':')
                        }) =>
                        {
                            components.pop();
                        }
                        ".." => {}
                        _ => components.push(component),
                    }
                }
                components.join("/")
            }
        };
        (expected, program_path(program, cwd))
    } else {
        (
            pattern.text().to_owned(),
            Path::new(program)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        )
    };
    let expected = native_program(expected, matches!(pattern, Pattern::Glob(_)));
    let actual = native_program(actual, false);
    match pattern {
        Pattern::Literal(_) => expected == actual,
        Pattern::Glob(_) => glob_matches(&expected, &actual),
    }
}

fn program_path(program: &str, cwd: &Path) -> String {
    let path = cwd.join(program);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component),
        }
    }
    normalized.to_string_lossy().into_owned()
}

fn native_program(value: String, glob: bool) -> String {
    #[cfg(windows)]
    {
        let value = if glob {
            value
        } else {
            value.replace('\\', "/")
        }
        .to_lowercase();
        for extension in [".exe", ".com", ".cmd", ".bat"] {
            if let Some(value) = value.strip_suffix(extension) {
                return value.to_owned();
            }
        }
        value
    }
    #[cfg(not(windows))]
    {
        let _ = glob;
        value
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Compression {
    One(String),
    Many(Vec<Method>),
}

impl Rule {
    fn selection(&self) -> Result<Selection, String> {
        self.condition.validate()?;
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
    let global_identity = global.canonicalize().ok();
    let project = context.and_then(|context| {
        context.cwd.ancestors().find_map(|directory| {
            let path = directory.join(".outrim/config.json");
            let is_global = path == global
                || global_identity
                    .as_ref()
                    .is_some_and(|global| path.canonicalize().ok().as_ref() == Some(global));
            (!is_global && path.try_exists().unwrap_or(true)).then_some(path)
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
                    rule.condition.matches(context, tool).then_some(selection)
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
