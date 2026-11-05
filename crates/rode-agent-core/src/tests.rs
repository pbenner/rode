//! Provider settings remain independent; constructing a command never starts one.
use super::*;
use serde_json::json;

#[test]
fn legacy_preferences_keep_codex_and_claude_uses_only_its_literal_command() {
    let legacy: Config = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.provider, Provider::Codex);
    let mut config = Config {
        provider: Provider::Claude,
        codex: "/missing-codex".into(),
        acp_program: "/different-acp-agent".into(),
        ..legacy
    };
    assert_eq!(config.argv().unwrap(), [CLAUDE_ACP]);
    config.claude_program = "/tools/Claude adapter".into();
    config.claude_arguments = vec!["literal ; $(not a shell)".into()];
    assert_eq!(
        config.argv().unwrap(),
        ["/tools/Claude adapter", "literal ; $(not a shell)"]
    );
    let restored: Config = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
    assert_eq!(restored, config);
    assert!(restored.provider.is_acp());
}

#[test]
fn claude_and_custom_acp_agents_keep_their_arguments_and_controls_separate() {
    let mut config = Config {
        provider: Provider::Claude,
        acp_program: "other-agent".into(),
        acp_arguments: vec!["--other".into()],
        acp_options: BTreeMap::from([("model".into(), json!("other-model"))]),
        ..Default::default()
    };
    *config.acp_arguments_mut() = vec!["--claude".into()];
    config
        .acp_options_mut()
        .insert("model".into(), json!("claude-model"));
    assert_eq!(config.argv().unwrap(), [CLAUDE_ACP, "--claude"]);
    assert_eq!(config.acp_options()["model"], "claude-model");
    let before = config.clone();
    config
        .acp_options_mut()
        .insert("effort".into(), json!("high"));
    assert!(
        config.same_process(&before),
        "Advertised settings apply to the existing session"
    );
    config.provider = Provider::Acp;
    assert_eq!(config.argv().unwrap(), ["other-agent", "--other"]);
    assert_eq!(config.acp_options()["model"], "other-model");
    assert!(!config.same_process(&before));
}

#[test]
fn claude_never_accepts_codex_file_only_policy_or_invalid_launch_arguments() {
    let mut config = Config {
        provider: Provider::Claude,
        access: Access::FilesOnly,
        ..Default::default()
    };
    assert!(
        config
            .argv()
            .unwrap_err()
            .to_string()
            .contains("requires Codex")
    );
    config.access = Access::Project;
    config.claude_arguments.push("bad\0argument".into());
    assert!(config.argv().is_err());
    config.claude_arguments.clear();
    config
        .claude_options
        .insert("mode".into(), json!({"untrusted":"object"}));
    assert!(config.argv().is_err());
}
