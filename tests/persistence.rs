//! Real cooperating writers, field-level preference/profile merging, malformed-file
//! preservation and private GUI state.

use eframe::Storage;
use rode::{
    persistence::{self, Preferences},
    ssh::{self, Profile},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

fn wait(check: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(
            Instant::now() < until,
            "Timed out waiting for child process"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn values(path: &Path) -> BTreeMap<String, String> {
    ron::de::from_bytes(&fs::read(path).unwrap()).unwrap()
}
fn open(path: &Path) -> Preferences {
    Preferences::open(path.into(), Arc::new(|| {}))
}
fn child(root: &Path, role: &str, scenario: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "preference_child", "--nocapture"])
        .env("RODE_PREF_TEST_ROOT", root)
        .env("RODE_PREF_TEST_ROLE", role)
        .env("RODE_PREF_TEST_SCENARIO", scenario)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
fn finish(child: Child) {
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[ignore = "child entry point, invoked by process-level tests"]
fn preference_child() {
    let root = PathBuf::from(std::env::var("RODE_PREF_TEST_ROOT").unwrap());
    let role = std::env::var("RODE_PREF_TEST_ROLE").unwrap();
    let scenario = std::env::var("RODE_PREF_TEST_SCENARIO").unwrap();
    if scenario == "holder" {
        persistence::transaction(&root.join("app.ron"), |_| {
            fs::write(root.join("ready-holder"), "")?;
            wait(|| root.join("release").exists());
            Ok(())
        })
        .unwrap();
        return;
    }
    let mut store = open(&root.join("app.ron"));
    let mut project: Value =
        serde_json::from_str(&store.get_string("project_preferences").unwrap()).unwrap();
    let mut agent: Value =
        serde_json::from_str(&store.get_string("agent_preferences").unwrap()).unwrap();
    fs::write(root.join(format!("ready-{role}")), "").unwrap();
    wait(|| root.join("go").exists());
    if scenario == "profiles" {
        let before = Profile {
            name: "shared".into(),
            host: "original".into(),
            ..Default::default()
        };
        let mut profile = before.clone();
        if role == "first" {
            profile.user = "alice".into();
        } else {
            profile.port = "2222".into();
        }
        ssh::save_profile_at(&root.join("connections.json"), &profile, Some(&before)).unwrap();
        ssh::save_profile_at(
            &root.join("connections.json"),
            &Profile {
                name: role.clone(),
                host: role,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    } else if scenario == "extensions" {
        let mut prefs: Value =
            serde_json::from_str(&store.get_string("extension_preferences").unwrap()).unwrap();
        // Keep only known fields as a typed manager would, preserving future keys on disk.
        prefs["test.adapter"]
            .as_object_mut()
            .unwrap()
            .remove("future");
        if role == "first" {
            prefs["rode.git"]["enabled"] = false.into();
            prefs["test.adapter"]["settings"]["one"] = "first".into();
        } else {
            prefs["rode.monitor"]["enabled"] = false.into();
            prefs["test.adapter"]["settings"]["two"] = "second".into();
        }
        store.set_string("extension_preferences", prefs.to_string());
    } else {
        // Simulate typed settings from an older window/version: omitted future
        // fields must not be deleted when a known field is edited.
        project["same"].as_object_mut().unwrap().remove("future");
        agent["same"].as_object_mut().unwrap().remove("future");
        if role == "first" {
            store.set_string("color_theme", "carbon".into());
            project["same"]["python"] = "/venv/bin/python".into();
            project["same"]["latex"]["program"] = "custom-tex".into();
            project["same"]["debugger"]["config"]["adapter"] = "custom-dap".into();
            agent["same"]["model"] = "model-a".into();
        } else {
            store.set_string("font_size", "19".into());
            project["same"]["ruff"] = "/tools/ruff".into();
            project["same"]["latex"]["arguments"] = json!(["{file}", "{outdir}"]);
            project["same"]["debugger"]["watches"] = json!(["answer"]);
            project["other"] = json!({"python":"/other/venv/bin/python"});
            agent["same"]["local_url"] = "http://localhost:11434/v1".into();
            agent["other"] = json!({"model":"model-b"});
        }
        store.set_string("project_preferences", project.to_string());
        store.set_string("agent_preferences", agent.to_string());
        store.flush();
    }
    drop(store);
}

fn seed(root: &Path) {
    let values=BTreeMap::from([
        ("color_theme","phosphor".to_string()),("font_size","14".into()),
        ("project_preferences",json!({"same":{"python":"auto","ruff":"uvx","future":null}}).to_string()),
        ("agent_preferences",json!({"same":{"model":"","local_url":"http://127.0.0.1:11434/v1","future":{"setting":true}}}).to_string()),
    ]);
    fs::write(root.join("app.ron"), ron::ser::to_string(&values).unwrap()).unwrap();
}

#[test]
fn processes_merge_independent_fields_projects_and_manual_changes() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let first = child(root.path(), "first", "preferences");
    let second = child(root.path(), "second", "preferences");
    wait(|| root.path().join("ready-first").exists() && root.path().join("ready-second").exists());
    let mut manual = values(&root.path().join("app.ron"));
    manual.insert("manual".into(), "preserve me".into());
    fs::write(
        root.path().join("app.ron"),
        ron::ser::to_string(&manual).unwrap(),
    )
    .unwrap();
    fs::write(root.path().join("go"), "").unwrap();
    finish(first);
    finish(second);
    let values = values(&root.path().join("app.ron"));
    assert_eq!(values["color_theme"], "carbon");
    assert_eq!(values["font_size"], "19");
    assert_eq!(values["manual"], "preserve me");
    let project: Value = serde_json::from_str(&values["project_preferences"]).unwrap();
    assert_eq!(project["same"]["python"], "/venv/bin/python");
    assert_eq!(project["same"]["ruff"], "/tools/ruff");
    assert_eq!(project["same"]["latex"]["program"], "custom-tex");
    assert_eq!(
        project["same"]["latex"]["arguments"],
        json!(["{file}", "{outdir}"])
    );
    assert_eq!(
        project["same"]["debugger"]["config"]["adapter"],
        "custom-dap"
    );
    assert_eq!(project["same"]["debugger"]["watches"], json!(["answer"]));
    assert!(project["same"].get("future").is_some());
    assert!(project.get("other").is_some());
    let agent: Value = serde_json::from_str(&values["agent_preferences"]).unwrap();
    assert_eq!(agent["same"]["model"], "model-a");
    assert_eq!(agent["same"]["local_url"], "http://localhost:11434/v1");
    assert_eq!(agent["same"]["future"]["setting"], true);
    assert!(agent.get("other").is_some());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(root.path().join("app.ron"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn connection_profile_edits_merge_without_losing_other_profiles_or_unknown_fields() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    fs::write(
        root.path().join("connections.json"),
        json!([{"name":"shared","host":"original","future":42}]).to_string(),
    )
    .unwrap();
    let first = child(root.path(), "first", "profiles");
    let second = child(root.path(), "second", "profiles");
    wait(|| root.path().join("ready-first").exists() && root.path().join("ready-second").exists());
    fs::write(root.path().join("go"), "").unwrap();
    finish(first);
    finish(second);
    let profiles: Value =
        serde_json::from_slice(&fs::read(root.path().join("connections.json")).unwrap()).unwrap();
    assert_eq!(profiles.as_array().unwrap().len(), 3);
    let shared = profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "shared")
        .unwrap();
    assert_eq!(shared["user"], "alice");
    assert_eq!(shared["port"], "2222");
    assert_eq!(shared["future"], 42);
    let merged = ssh::save_profile_at(
        &root.path().join("connections.json"),
        &Profile {
            name: "shared".into(),
            host: "original".into(),
            jump: "gateway".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let shared = merged.iter().find(|p| p.name == "shared").unwrap();
    assert_eq!(shared.user, "alice");
    assert_eq!(shared.port, "2222");
    assert_eq!(shared.jump, "gateway");
    fs::write(root.path().join("connections.json"), "broken").unwrap();
    assert!(
        ssh::save_profile_at(
            &root.path().join("connections.json"),
            &Profile {
                host: "new".into(),
                ..Default::default()
            },
            None
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.path().join("connections.json")).unwrap(),
        "broken"
    );
}

#[test]
fn killed_process_releases_the_lock_and_private_snapshots_do_not_touch_shared_state() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let mut holder = child(root.path(), "holder", "holder");
    wait(|| root.path().join("ready-holder").exists());
    holder.kill().unwrap();
    holder.wait().unwrap();
    let mut store = open(&root.path().join("app.ron"));
    let first = persistence::NativeState::new(&store).unwrap();
    let second = persistence::NativeState::new(&store).unwrap();
    assert_ne!(first.path, second.path);
    fs::write(&first.path, "private GUI snapshot").unwrap();
    drop(first);
    assert!(second.path.exists());
    store.set_string("color_theme", "carbon".into());
    drop(store);
    assert_eq!(
        values(&root.path().join("app.ron"))["color_theme"],
        "carbon"
    );
}

#[test]
fn malformed_preferences_are_preserved_and_failed_edits_can_be_retried() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    fs::write(&path, "broken").unwrap();
    let mut store = open(&path);
    assert!(store.take_error().is_some());
    store.set_string("color_theme", "carbon".into());
    store.flush();
    wait(|| store.take_error().is_some());
    assert_eq!(fs::read_to_string(&path).unwrap(), "broken");
    seed(root.path());
    store.set_string("font_size", "19".into());
    drop(store);
    let values = values(&path);
    assert_eq!(values["color_theme"], "carbon");
    assert_eq!(values["font_size"], "19");
}

#[test]
fn unchanged_windows_and_symlinked_settings_do_not_overwrite_new_values() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let path = root.path().join("app.ron");
    let stale = open(&path);
    let mut explicit = open(&path);
    let mut first = open(&path);
    first.set_string("color_theme", "carbon".into());
    drop(first);
    drop(stale);
    assert_eq!(values(&path)["color_theme"], "carbon");
    explicit.set_string("color_theme", "phosphor".into());
    drop(explicit);
    assert_eq!(
        values(&path)["color_theme"],
        "phosphor",
        "An explicit change back to the original value must win"
    );
    std::os::unix::fs::symlink(&path, root.path().join("linked.ron")).unwrap();
    let mut store = open(&root.path().join("linked.ron"));
    store.set_string("font_size", "20".into());
    drop(store);
    assert!(
        fs::symlink_metadata(root.path().join("linked.ron"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(values(&path)["font_size"], "20");
}

#[test]
fn eviction_does_not_remove_another_processes_updated_profile() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let path = root.path().join("app.ron");
    let mut evicting = open(&path);
    let mut editing = open(&path);
    editing.set_string(
        "agent_preferences",
        json!({"same":{"model":"new-model","local_url":"http://127.0.0.1:11434/v1"}}).to_string(),
    );
    drop(editing);
    evicting.set_string("agent_preferences", "{}".into());
    drop(evicting);
    let agents: Value = serde_json::from_str(&values(&path)["agent_preferences"]).unwrap();
    assert_eq!(agents["same"]["model"], "new-model");
}

#[test]
fn implicit_defaults_from_an_old_window_do_not_reset_newer_settings() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    fs::write(
        &path,
        ron::ser::to_string(&BTreeMap::from([(
            "project_preferences",
            json!({"same":{"python":"auto"}}).to_string(),
        )]))
        .unwrap(),
    )
    .unwrap();
    let mut stale = open(&path);
    let mut newer = open(&path);
    newer.set_string(
        "project_preferences",
        json!({"same":{"python":"auto","ruff":"/new/ruff"},"new":{"ruff":"/new/ruff"}}).to_string(),
    );
    drop(newer);
    // Forms serialize defaults that did not exist in the startup file. These
    // are not user edits, including defaults for a newly discovered project.
    stale.set_string("project_preferences",json!({"same":{"python":"/my/venv","ruff":"","sandbox":rode::sandbox::Options::default()},"new":{"python":"auto","ruff":"","sandbox":rode::sandbox::Options::default()}}).to_string());
    drop(stale);
    let projects: Value = serde_json::from_str(&values(&path)["project_preferences"]).unwrap();
    assert_eq!(projects["same"]["python"], "/my/venv");
    assert_eq!(projects["same"]["ruff"], "/new/ruff");
    assert_eq!(projects["new"]["ruff"], "/new/ruff");
}

#[test]
fn queued_changes_and_eviction_keep_their_order_and_override_removals_win() {
    use std::os::fd::AsRawFd;
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let path = root.path().join("app.ron");
    let locked = fs::File::create(root.path().join("app.ron.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(locked.as_raw_fd(), libc::LOCK_EX) }, 0);
    let mut store = open(&path);
    for model in ["first", "second"] {
        store.set_string(
            "agent_preferences",
            json!({"same":{"model":model}}).to_string(),
        );
        store.flush();
    }
    store.set_string("agent_preferences", "{}".into());
    store.flush();
    drop(locked);
    drop(store);
    let agents: Value = serde_json::from_str(&values(&path)["agent_preferences"]).unwrap();
    assert!(agents.get("same").is_none());

    let mut initial = open(&path);
    initial.set_string(
        "sandbox_defaults",
        json!({"overrides":{"~/.codex":"read_only"}}).to_string(),
    );
    drop(initial);
    let mut restoring = open(&path);
    let mut other = open(&path);
    other.set_string(
        "sandbox_defaults",
        json!({"overrides":{"~/.codex":"hidden"}}).to_string(),
    );
    drop(other);
    restoring.set_string("sandbox_defaults", json!({"overrides":{}}).to_string());
    drop(restoring);
    let options: Value = serde_json::from_str(&values(&path)["sandbox_defaults"]).unwrap();
    assert!(options["overrides"].get("~/.codex").is_none());
}

#[test]
fn syntax_and_interface_edits_merge_and_explicit_syntax_selection_wins() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    let mut seed = open(&path);
    seed.set_string("syntax_theme", "ui".into());
    drop(seed);
    let mut syntax = open(&path);
    let mut interface = open(&path);
    let stale = open(&path);
    let mut explicit = open(&path);
    syntax.set_string("syntax_theme", "dracula".into());
    interface.set_string("color_theme", "carbon".into());
    drop(syntax);
    drop(interface);
    drop(stale);
    let result = values(&path);
    assert_eq!(result["syntax_theme"], "dracula");
    assert_eq!(result["color_theme"], "carbon");
    explicit.set_string("syntax_theme", "ui".into());
    drop(explicit);
    let result = values(&path);
    assert_eq!(result["syntax_theme"], "ui");
    assert_eq!(result["color_theme"], "carbon");
}

#[test]
fn concurrent_extension_toggles_and_namespaced_settings_merge() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let mut initial = values(&root.path().join("app.ron"));
    initial.insert("extension_preferences".into(),json!({"rode.git":{"enabled":true},"rode.monitor":{"enabled":true},"test.adapter":{"enabled":true,"settings":{"one":"old","two":"old"},"future":42}}).to_string());
    fs::write(
        root.path().join("app.ron"),
        ron::ser::to_string(&initial).unwrap(),
    )
    .unwrap();
    let first = child(root.path(), "first", "extensions");
    let second = child(root.path(), "second", "extensions");
    wait(|| root.path().join("ready-first").exists() && root.path().join("ready-second").exists());
    fs::write(root.path().join("go"), "").unwrap();
    finish(first);
    finish(second);
    let result: Value =
        serde_json::from_str(&values(&root.path().join("app.ron"))["extension_preferences"])
            .unwrap();
    assert_eq!(result["rode.git"]["enabled"], false);
    assert_eq!(result["rode.monitor"]["enabled"], false);
    assert_eq!(result["test.adapter"]["settings"]["one"], "first");
    assert_eq!(result["test.adapter"]["settings"]["two"], "second");
    assert_eq!(result["test.adapter"]["future"], 42);
}
