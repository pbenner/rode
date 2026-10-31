//! Project-scoped runtime paths and literal environment values shared by tasks and shells.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Common runtime executables exposed by the Environment panel.
pub const TOOLS: &[(&str, &str)] = &[
    ("node", "Node.js"),
    ("npm", "npm"),
    ("cargo", "Rust / Cargo"),
    ("rustc", "Rust compiler"),
    ("go", "Go"),
    ("java", "Java"),
    ("javac", "Java compiler"),
    ("dotnet", ".NET"),
];

/// Settings belong to a host/project, never to the editor process environment.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Executable names mapped to literal host paths or alternative command names.
    pub runtimes: BTreeMap<String, String>,
    /// Literal values; shell expansion is never performed on these strings.
    pub variables: BTreeMap<String, String>,
}

/// Portable shell environment variable name.
pub fn variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Settings {
    /// Validate bounds before executing a command or saving preferences.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.runtimes.len() <= 32 && self.variables.len() <= 64,
            "Too many project environment settings."
        );
        for (name, value) in &self.variables {
            ensure!(
                variable_name(name) && name.len() <= 128,
                "Invalid environment variable name: {name}"
            );
            ensure!(
                value.len() <= 4096 && !value.contains(['\0', '\n', '\r']),
                "Environment values must be single lines of at most 4096 bytes."
            );
        }
        for (name, value) in &self.runtimes {
            ensure!(
                !name.is_empty()
                    && name.len() <= 64
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)),
                "Invalid runtime name."
            );
            ensure!(
                value.len() <= 4096 && !value.chars().any(char::is_control),
                "Invalid runtime executable."
            );
            ensure!(
                !value.contains('/') || value.starts_with('/'),
                "Use an absolute host path for a runtime executable."
            );
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 64 * 1024,
            "Project environment exceeds 64 KiB."
        );
        Ok(())
    }

    /// Resolve an executable without treating it as a shell command.
    pub fn program(&self, name: &str) -> String {
        self.runtimes
            .get(name)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| name.into())
    }

    fn directories(&self) -> Vec<String> {
        let mut dirs = vec![];
        for value in self.runtimes.values() {
            let path = std::path::Path::new(value);
            if path.is_absolute()
                && let Some(parent) = path.parent()
            {
                let dir = parent.to_string_lossy().into_owned();
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }
        dirs
    }

    /// Apply to a captured host environment before starting a new process.
    pub fn apply(&self, env: &mut BTreeMap<String, String>) -> Result<()> {
        self.validate()?;
        env.extend(self.variables.clone());
        if !self.variables.contains_key("RUSTC")
            && let Some(compiler) = self
                .runtimes
                .get("rustc")
                .filter(|value| !value.trim().is_empty())
        {
            env.insert("RUSTC".into(), compiler.clone());
        }
        let dirs = self.directories();
        if !dirs.is_empty() {
            let path = env.get("PATH").cloned().unwrap_or_default();
            env.insert("PATH".into(), format!("{}:{path}", dirs.join(":")));
        }
        Ok(())
    }

    /// Literal exports for a new login shell after its startup files, or a task subshell.
    pub fn exports(&self) -> Result<String> {
        self.validate()?;
        let mut commands = self
            .variables
            .iter()
            .map(|(name, value)| format!("export {name}={};", crate::ssh::quote(value)))
            .collect::<Vec<_>>();
        if !self.variables.contains_key("RUSTC")
            && let Some(compiler) = self
                .runtimes
                .get("rustc")
                .filter(|value| !value.trim().is_empty())
        {
            commands.push(format!("export RUSTC={};", crate::ssh::quote(compiler)));
        }
        let dirs = self.directories();
        if !dirs.is_empty() {
            commands.push(format!(
                "export PATH={}:\"$PATH\";",
                crate::ssh::quote(&dirs.join(":"))
            ));
        }
        Ok(commands.join(" "))
    }

    /// Parse the editable NAME=value representation, preserving literal values.
    pub fn parse_variables(text: &str) -> Result<BTreeMap<String, String>> {
        let mut settings = Self::default();
        for line in text
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        {
            let (name, value) = line
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("Use NAME=value on each line."))?;
            let name = name.trim().to_owned();
            ensure!(
                !settings.variables.contains_key(&name),
                "Duplicate environment variable: {name}"
            );
            settings.variables.insert(name, value.into());
        }
        settings.validate()?;
        Ok(settings.variables)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn values_are_literal_and_runtime_paths_are_shared_by_tasks_and_shells() {
        let mut settings = Settings {
            variables: Settings::parse_variables("LABEL=hello 'world' $(touch unwanted)\nEMPTY=\n")
                .unwrap(),
            ..Default::default()
        };
        settings
            .runtimes
            .insert("node".into(), "/opt/node/bin/node".into());
        let mut env = BTreeMap::from([("PATH".into(), "/usr/bin".into())]);
        settings.apply(&mut env).unwrap();
        assert_eq!(env["PATH"], "/opt/node/bin:/usr/bin");
        assert_eq!(settings.program("node"), "/opt/node/bin/node");
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "{} printf '%s' \"$LABEL\"",
                settings.exports().unwrap()
            ))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "hello 'world' $(touch unwanted)"
        );
        assert!(Settings::parse_variables("BAD-NAME=value").is_err());
        assert!(Settings::parse_variables("KEY=one\nKEY=two").is_err());
    }
}
