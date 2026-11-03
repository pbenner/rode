//! Exercise the shared host services through a separate headless Rust process.
#![allow(dead_code)]
use std::{path::Path, process::Command};
pub fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_rode-sessiond")
}
pub fn command() -> Command {
    let mut command = Command::new(binary());
    command.arg("--file-helper");
    command
}
pub fn prepare(
    root: &str,
    options: &rode::sandbox::Options,
) -> anyhow::Result<rode::sandbox::Plan> {
    rode::sandbox::prepare_with_helper(root, options, Path::new(binary()))
}

pub fn environment_prefix(
    settings: &rode::environment::Settings,
    root: &str,
    python: &str,
) -> anyhow::Result<Vec<String>> {
    rode::sandbox::Plan {
        root: root.into(),
        native_helper: Some(binary().into()),
        environment: settings.clone(),
        ..Default::default()
    }
    .environment_prefix(python)
}

/// Helper paths differ between retained desktop images and immutable headless artifacts.
/// All other grants and launch arguments must still match exactly.
pub fn policy_arguments(plan: &rode::sandbox::Plan) -> Vec<String> {
    plan.prefix
        .iter()
        .map(|argument| {
            if plan.native_helper.as_ref() == Some(argument) {
                "<native-helper>".into()
            } else {
                argument.clone()
            }
        })
        .collect()
}
