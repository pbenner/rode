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

fn wait(mut check: impl FnMut() -> bool) {
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

fn tunnel_entry(name: &str) -> Value {
    json!({
        "host": {"kind": "ssh", "host": "compute", "user": "researcher", "port": "2222", "config": ""},
        "spec": {"name": name, "relay_host": "relay.example.org", "relay_user": "researcher"},
        "relay_jump": "desktop-relay"
    })
}

#[test]
fn saved_tunnels_merge_across_windows_without_losing_renames_fields_or_newer_records() {
    const KEY: &str = ssh::tunnels::STORAGE_KEY;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    let mut entry = tunnel_entry("Compute access");
    entry["future"] = json!({"keep": true});
    entry["spec"]["future_option"] = json!("keep");
    let initial = json!({"compute": entry});
    let mut store = open(&path);
    store.set_string(KEY, initial.to_string());
    drop(store);
    let mut first = open(&path);
    let mut second = open(&path);
    let mut stale_delete = open(&path);
    let mut idle = open(&path);
    let mut next = initial.clone();
    next["compute"]["spec"]["name"] = "Renamed compute".into();
    next["compute"]["spec"]["relay_port"] = json!(8443);
    first.set_string(KEY, next.to_string());
    drop(first);
    let mut next = initial.clone();
    next["compute"]["spec"]["listen_port"] = json!(22444);
    next["compute"]["spec"]["listen_on"] = "all_interfaces".into();
    next["compute"]["spec"]["identity"] = "~/keys/custom-relay".into();
    next["second"] = tunnel_entry("Second relay");
    // A normal typed snapshot drops unknown future fields; the writer retains them.
    let typed = ssh::tunnels::decode(&next.to_string()).unwrap();
    second.set_string(KEY, serde_json::to_string(&typed).unwrap());
    drop(second);
    stale_delete.set_string(KEY, "{}".into());
    drop(stale_delete);
    idle.set_string(
        KEY,
        serde_json::to_string(&ssh::tunnels::decode(&initial.to_string()).unwrap()).unwrap(),
    );
    drop(idle);
    let saved: Value = serde_json::from_str(&values(&path)[KEY]).unwrap();
    assert_eq!(saved["compute"]["spec"]["name"], "Renamed compute");
    assert_eq!(saved["compute"]["spec"]["relay_port"], 8443);
    assert_eq!(saved["compute"]["spec"]["listen_port"], 22444);
    assert_eq!(saved["compute"]["spec"]["listen_on"], "all_interfaces");
    assert_eq!(saved["compute"]["spec"]["identity"], "~/keys/custom-relay");
    assert_eq!(saved["compute"]["future"]["keep"], true);
    assert_eq!(saved["compute"]["spec"]["future_option"], "keep");
    assert!(saved.get("second").is_some());
    let mut deleting = open(&path);
    deleting.set_string(KEY, json!({"second": saved["second"]}).to_string());
    drop(deleting);
    let saved: Value = serde_json::from_str(&values(&path)[KEY]).unwrap();
    assert!(saved.get("compute").is_none());
    assert!(saved.get("second").is_some());
}

#[test]
fn concurrent_tunnel_additions_cannot_publish_duplicate_names_or_exceed_the_limit() {
    const KEY: &str = ssh::tunnels::STORAGE_KEY;
    for limit in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("app.ron");
        let initial: BTreeMap<_, _> = (0..if limit { 127 } else { 0 })
            .map(|i| (format!("setup-{i}"), tunnel_entry(&format!("Setup {i}"))))
            .collect();
        let mut store = open(&path);
        store.set_string(KEY, serde_json::to_string(&initial).unwrap());
        drop(store);
        let mut first = open(&path);
        let mut second = open(&path);
        for (id, name, writer) in [
            ("first", "New tunnel", &mut first),
            (
                "second",
                if limit {
                    "Another tunnel"
                } else {
                    "New tunnel"
                },
                &mut second,
            ),
        ] {
            let mut next = initial.clone();
            next.insert(id.into(), tunnel_entry(name));
            writer.set_string(KEY, serde_json::to_string(&next).unwrap());
        }
        drop(first);
        let preserved = fs::read(&path).unwrap();
        second.flush();
        let mut error = None;
        wait(|| {
            error = second.take_error();
            error.is_some()
        });
        let error = error.unwrap();
        assert!(
            error.contains(if limit {
                "At most 128"
            } else {
                "already saved"
            }),
            "{error}"
        );
        drop(second);
        assert_eq!(fs::read(&path).unwrap(), preserved);
        assert!(open(&path).take_error().is_none());
    }
}

#[test]
fn sandbox_presets_merge_across_windows_and_preserve_newer_records() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    let mut initial = open(&path);
    initial.set_string(
        "sandbox_presets",
        json!({"Site": {
            "enabled": true, "backend": "landlock", "future": {"keep": true},
            "overrides": {"~/.codex": "read_only"}
        }})
        .to_string(),
    );
    drop(initial);
    let mut first = open(&path);
    let mut second = open(&path);
    let mut deleting = open(&path);
    let mut idle = open(&path);
    // Each window saves its typed snapshot, which does not know future fields.
    first.set_string("sandbox_presets", json!({
        "Site": {"enabled": true, "backend": "landlock", "read_only": ["~/.config/site-shell"], "overrides": {}},
        "Local": {"read_write": ["~/.cache/site-tools"]}
    }).to_string());
    drop(first);
    second.set_string("sandbox_presets", json!({
        "Site": {"enabled": true, "backend": "landlock", "share_network": false, "overrides": {"~/.codex": "read_only"}},
        "Remote": {"backend": "landlock"}
    }).to_string());
    drop(second);
    // Deleting an old version must not delete a record another window edited.
    deleting.set_string("sandbox_presets", "{}".into());
    drop(deleting);
    idle.set_string(
        "sandbox_presets",
        json!({"Site": {
            "enabled": true, "backend": "landlock", "overrides": {"~/.codex": "read_only"}
        }})
        .to_string(),
    );
    drop(idle);
    let presets: Value = serde_json::from_str(&values(&path)["sandbox_presets"]).unwrap();
    assert_eq!(
        presets["Site"]["read_only"],
        json!(["~/.config/site-shell"])
    );
    assert_eq!(presets["Site"]["share_network"], false);
    assert_eq!(presets["Site"]["future"]["keep"], true);
    assert!(presets["Site"]["overrides"].get("~/.codex").is_none());
    assert!(presets.get("Local").is_some());
    assert!(presets.get("Remote").is_some());

    let mut deleting = open(&path);
    deleting.set_string(
        "sandbox_presets",
        json!({
            "Local": presets["Local"], "Remote": presets["Remote"]
        })
        .to_string(),
    );
    drop(deleting);
    let presets: Value = serde_json::from_str(&values(&path)["sandbox_presets"]).unwrap();
    assert!(
        presets.get("Site").is_none(),
        "Explicit deletion of the current record must persist"
    );
}

#[test]
fn sandbox_presets_with_only_default_options_survive_restart() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    let mut store = open(&path);
    let policy = rode::sandbox::Options::default();
    store.set_string(
        "sandbox_presets",
        json!({"Default rules": policy}).to_string(),
    );
    drop(store);
    let stored: Value = serde_json::from_str(&values(&path)["sandbox_presets"]).unwrap();
    assert_eq!(
        stored["Default rules"],
        serde_json::to_value(policy).unwrap()
    );
}

#[test]
fn concurrent_sandbox_preset_additions_cannot_publish_an_unreadable_library() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    let initial: BTreeMap<_, _> = (0..63)
        .map(|i| (format!("Preset {i}"), rode::sandbox::Options::default()))
        .collect();
    let mut store = open(&path);
    store.set_string("sandbox_presets", serde_json::to_string(&initial).unwrap());
    drop(store);
    let mut first = open(&path);
    let mut second = open(&path);
    for (name, writer) in [
        ("First addition", &mut first),
        ("Second addition", &mut second),
    ] {
        let mut presets = initial.clone();
        presets.insert(name.into(), rode::sandbox::Options::default());
        writer.set_string("sandbox_presets", serde_json::to_string(&presets).unwrap());
    }
    drop(first);
    let preserved = fs::read(&path).unwrap();
    second.flush();
    let mut error = None;
    wait(|| {
        error = second.take_error();
        error.is_some()
    });
    assert!(error.unwrap().contains("At most 64 sandbox presets"));
    drop(second);
    assert_eq!(fs::read(&path).unwrap(), preserved);
    let reopened = open(&path);
    assert!(reopened.take_error().is_none());
    let presets: BTreeMap<String, rode::sandbox::Options> =
        serde_json::from_str(&reopened.get_string("sandbox_presets").unwrap()).unwrap();
    assert_eq!(presets.len(), 64);
    assert!(presets.contains_key("First addition"));
    assert!(!presets.contains_key("Second addition"));
}

#[test]
fn terminal_color_edits_merge_and_explicit_stale_reset_wins() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    let initial = BTreeMap::from([
        ("terminal_color.carbon.foreground", "theme"),
        ("color_theme", "carbon"),
    ]);
    fs::write(&path, ron::ser::to_string(&initial).unwrap()).unwrap();
    let mut foreground = open(&path);
    let mut background = open(&path);
    let idle = open(&path);
    let mut stale_reset = open(&path);
    foreground.set_string("terminal_color.carbon.foreground", "#eeeeee".into());
    background.set_string("terminal_color.carbon.background", "#001122".into());
    background.set_string("terminal_color.carbon.ansi.4", "#99aaff".into());
    background.set_string("terminal_color.ice.minimum_contrast", "7".into());
    drop(foreground);
    drop(background);
    drop(idle);
    assert_eq!(values(&path)["terminal_color.carbon.foreground"], "#eeeeee");
    stale_reset.set_string("terminal_color.carbon.foreground", "theme".into());
    drop(stale_reset);
    let saved = values(&path);
    assert_eq!(saved["terminal_color.carbon.foreground"], "theme");
    assert_eq!(saved["terminal_color.carbon.background"], "#001122");
    assert_eq!(saved["terminal_color.carbon.ansi.4"], "#99aaff");
    assert_eq!(saved["terminal_color.ice.minimum_contrast"], "7");
    assert_eq!(saved["color_theme"], "carbon");
}

#[test]
fn explicit_scale_reset_overrides_another_window_without_reverting_its_font() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    let initial = BTreeMap::from([("ui_scale", "1"), ("font_size", "14")]);
    fs::write(&path, ron::ser::to_string(&initial).unwrap()).unwrap();
    let mut first = open(&path);
    let mut second = open(&path);
    second.set_string("ui_scale", "0.75".into());
    second.set_string("font_size", "19".into());
    second.flush();
    wait(|| values(&path)["ui_scale"] == "0.75");
    // This window still displays 100%. Reset is an explicit change, despite
    // matching its local baseline, and must survive the scalar no-change guard.
    first.set_string("ui_scale", "1".into());
    first.flush();
    wait(|| values(&path)["ui_scale"] == "1");
    assert_eq!(values(&path)["font_size"], "19");
    drop(second);
    assert_eq!(values(&path)["ui_scale"], "1");
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
    if scenario == "profile-list" {
        let path = root.join("connections.json");
        if role == "first" {
            let before = Profile {
                name: "b".into(),
                host: "b.example".into(),
                ..Default::default()
            };
            let mut edited = before.clone();
            edited.user = "new-login".into();
            ssh::save_profile_at(&path, &edited, Some(&before)).unwrap();
            ssh::save_profile_at(
                &path,
                &Profile {
                    name: "new".into(),
                    host: "new.example".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        } else {
            ssh::edit_profiles_at(
                &path,
                &ssh::ProfileListEdit::Move {
                    label: "c".into(),
                    relative_to: "a".into(),
                    after: false,
                },
            )
            .unwrap();
            ssh::edit_profiles_at(
                &path,
                &ssh::ProfileListEdit::Remove(Box::new(Profile {
                    name: "d".into(),
                    host: "d.example".into(),
                    ..Default::default()
                })),
            )
            .unwrap();
        }
    } else if scenario == "profiles" {
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
            agent["same"]["personality"] = "jackson_lamb".into();
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
    assert_eq!(agent["same"]["personality"], "jackson_lamb");
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
fn saved_profile_management_merges_with_other_process_edits_and_additions() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path());
    let path = root.path().join("connections.json");
    let profiles: Vec<Value> = ["a", "b", "c", "d"]
        .iter()
        .map(|name| {
            json!({
                "name": name, "host": format!("{name}.example"), "future": {"keep": true}
            })
        })
        .collect();
    fs::write(&path, serde_json::to_vec(&profiles).unwrap()).unwrap();
    let first = child(root.path(), "first", "profile-list");
    let second = child(root.path(), "second", "profile-list");
    wait(|| root.path().join("ready-first").exists() && root.path().join("ready-second").exists());
    fs::write(root.path().join("go"), "").unwrap();
    finish(first);
    finish(second);
    let result: Vec<Value> = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        result
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["c", "a", "b", "new"]
    );
    assert_eq!(result[2]["user"], "new-login");
    for item in &result[..3] {
        assert_eq!(item["future"]["keep"], true);
    }
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
fn resetting_agent_controls_preserves_other_windows_model_and_permission_edits() {
    for (options, other) in [
        ("acp_options", "claude_options"),
        ("claude_options", "acp_options"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("app.ron");
        let initial = json!({"same":{
            "acp_options":{"mode":"ask"},
            "claude_options":{"mode":"default"},
            "model":"old"
        }});
        fs::write(
            &path,
            ron::ser::to_string(&BTreeMap::from([(
                "agent_preferences",
                initial.to_string(),
            )]))
            .unwrap(),
        )
        .unwrap();
        let mut resetting = open(&path);
        let mut changing = open(&path);
        let mut next = initial.clone();
        next["same"]["model"] = "new-model".into();
        next["same"]["approval"] = "never".into();
        next["same"][options]["model"] = "remote-model".into();
        next["same"][other]["model"] = "other-provider-model".into();
        changing.set_string("agent_preferences", next.to_string());
        drop(changing);
        let mut reset = initial;
        reset["same"][options] = json!({});
        resetting.set_string("agent_preferences", reset.to_string());
        drop(resetting);
        let merged: Value = serde_json::from_str(&values(&path)["agent_preferences"]).unwrap();
        assert_eq!(merged["same"]["model"], "new-model");
        assert_eq!(merged["same"]["approval"], "never");
        assert_eq!(
            merged["same"][options],
            json!({"model":"remote-model"}),
            "{options}"
        );
        assert_eq!(merged["same"][other]["model"], "other-provider-model");
        assert!(merged["same"][other].get("mode").is_some());
    }
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
    let locked = rode_host_core::locking::open_private(
        &root.path().join("app.ron.lock"),
        rode_host_core::locking::PrivateFile::Lock,
    )
    .unwrap();
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

fn roundtrip(key: &str, initial: Value, next: Value) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.ron");
    let data = BTreeMap::from([(key, initial.to_string())]);
    fs::write(&path, ron::ser::to_string(&data).unwrap()).unwrap();
    let mut prefs = Preferences::open(path.clone(), Arc::new(|| {}));
    prefs.set_string(key, next.to_string());
    drop(prefs);
    let saved: BTreeMap<String, String> = ron::de::from_bytes(&fs::read(path).unwrap()).unwrap();
    serde_json::from_str(&saved[key]).unwrap()
}

#[test]
fn resetting_the_last_agent_control_survives_restart() {
    for options in ["acp_options", "claude_options"] {
        let before = json!({"local":{
            "acp_options":{"mode":"ask"},
            "claude_options":{"mode":"default"},
            "future_agent_field":{"retained":true}
        }});
        let mut after = before.clone();
        after["local"][options] = json!({});
        assert_eq!(roundtrip("agent_preferences", before, after.clone()), after);
    }
}

#[test]
fn environment_removal_survives_restart() {
    let before = json!({"local": {"environment": {"variables": {"REVIEW": "stale"}, "runtimes": {"node": "/old/bin/node"}}}});
    let after = json!({"local": {"environment": {"variables": {}, "runtimes": {}}}});
    let saved = roundtrip("project_preferences", before, after);
    assert_eq!(
        saved["local"]["environment"],
        json!({"variables": {}, "runtimes": {}})
    );
}

#[test]
fn last_breakpoint_removal_survives_restart() {
    let before =
        json!({"local": {"debugger": {"breakpoints": {"/project/test.py": [{"line": 3}]}}}});
    let after = json!({"local": {"debugger": {"breakpoints": {}}}});
    let saved = roundtrip("project_preferences", before, after);
    assert_eq!(saved["local"]["debugger"]["breakpoints"], json!({}));
}

#[test]
fn nested_extension_key_removal_survives_restart() {
    let before =
        json!({"review.example": {"settings": {"options": {"removed": 1, "retained": 2}}}});
    let after = json!({"review.example": {"settings": {"options": {"retained": 2}}}});
    let saved = roundtrip("extension_preferences", before, after);
    assert_eq!(
        saved["review.example"]["settings"]["options"],
        json!({"retained": 2})
    );
}

#[test]
fn empty_object_setting_survives_restart() {
    let before = json!({"review.example": {"settings": {"options": "old"}}});
    let after = json!({"review.example": {"settings": {"options": {}}}});
    let saved = roundtrip("extension_preferences", before, after);
    assert_eq!(saved["review.example"]["settings"]["options"], json!({}));
}

#[test]
fn map_deletions_preserve_other_windows_additions_and_unknown_typed_fields() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ron");
    let initial = json!({"same": {
        "future_project_field": {"retained": true},
        "environment": {"future_environment_field": true, "variables": {"OLD":"old"}, "runtimes": {"node":"/old/node"}},
        "debugger": {"breakpoints": {"/old.py":[{"line":1}]}, "config":{"extra":{"old":true}}}
    }});
    fs::write(
        &path,
        ron::ser::to_string(&BTreeMap::from([(
            "project_preferences",
            initial.to_string(),
        )]))
        .unwrap(),
    )
    .unwrap();
    let mut deleting = open(&path);
    let mut adding = open(&path);
    let mut addition = initial;
    addition["same"]["environment"]["variables"]["NEW"] = json!("new");
    addition["same"]["environment"]["runtimes"]["go"] = json!("/new/go");
    addition["same"]["debugger"]["breakpoints"]["/new.py"] = json!([{"line":2}]);
    addition["same"]["debugger"]["config"]["extra"]["new"] = json!(true);
    adding.set_string("project_preferences", addition.to_string());
    drop(adding);
    deleting.set_string(
        "project_preferences",
        json!({"same": {
            "environment": {"variables": {}, "runtimes": {}},
            "debugger": {"breakpoints": {}, "config":{"extra":{}}}
        }})
        .to_string(),
    );
    drop(deleting);
    let saved: Value = serde_json::from_str(&values(&path)["project_preferences"]).unwrap();
    assert_eq!(
        saved["same"]["environment"]["variables"],
        json!({"NEW":"new"})
    );
    assert_eq!(
        saved["same"]["environment"]["runtimes"],
        json!({"go":"/new/go"})
    );
    assert_eq!(
        saved["same"]["debugger"]["breakpoints"],
        json!({"/new.py":[{"line":2}]})
    );
    assert_eq!(
        saved["same"]["debugger"]["config"]["extra"],
        json!({"new":true})
    );
    assert_eq!(saved["same"]["future_project_field"]["retained"], true);
    assert_eq!(
        saved["same"]["environment"]["future_environment_field"],
        true
    );
}

#[test]
fn newly_added_empty_objects_and_nested_json_deletions_are_real_edits() {
    let initial =
        json!({"review.example":{"settings":{"options":{"old":true,"kept":2},"scalar":"old"}}});
    let next = json!({"review.example":{"settings":{"options":{"kept":2},"scalar":{},"added":{}}}});
    let saved = roundtrip("extension_preferences", initial, next.clone());
    assert_eq!(saved, next);
    for before in [
        Value::Null,
        json!("old"),
        json!([]),
        json!({"removed":true}),
    ] {
        let mut actual = before.clone();
        persistence::apply(&mut actual, &persistence::changes(&before, &json!({})));
        assert_eq!(actual, json!({}));
    }
}
