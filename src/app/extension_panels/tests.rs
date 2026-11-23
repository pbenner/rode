use super::*;

fn fixture(code: &str) -> (EditorApp, egui::Context) {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    let manifest:crate::extensions::Manifest=serde_json::from_value(serde_json::json!({"api":1,"id":"test.panel","name":"Test panel","version":"1","description":"A desktop provider","panel":true,"tool":{"program":"python3","args":["-u","-c",code]}})).unwrap();
    manifest.validate().unwrap();
    app.extensions
        .registry
        .entries
        .push(crate::extensions::Entry {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            description: manifest.description.clone(),
            activation: "Explicit desktop panel",
            feature: None,
            manifest: Some(manifest),
        });
    (app, ctx)
}
fn ready(app: &mut EditorApp, ctx: &egui::Context) {
    let until = Instant::now() + Duration::from_secs(10);
    while app
        .extension_panels
        .instances
        .get("test.panel")
        .unwrap()
        .busy
        .is_some()
    {
        assert!(
            Instant::now() < until,
            "{}",
            app.extension_panels
                .instances
                .get("test.panel")
                .unwrap()
                .error
        );
        app.poll_extension_panels(ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        app.extension_panels
            .instances
            .get("test.panel")
            .unwrap()
            .error
            .is_empty(),
        "{}",
        app.extension_panels
            .instances
            .get("test.panel")
            .unwrap()
            .error
    );
}
#[test]
fn panels_are_opt_in_desktop_owned_and_preserve_edits_during_background_replies() {
    let code = "import json,sys,time\nfor line in sys.stdin:\n r=json.loads(line)\n time.sleep(0.03)\n print(json.dumps({'api':1,'id':r['id'],'view':{'title':r['action'],'fields':[{'key':'draft','label':'Draft','value':'server value'}],'refresh_seconds':30}}),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    assert!(!app.extensions.registry.enabled("test.panel"));
    assert!(!app.extension_panels.instances.contains_key("test.panel"));
    app.open_extension_panel("test.panel", &ctx);
    assert!(!app.extension_panels.instances.contains_key("test.panel"));
    app.set_extension("test.panel", true);
    app.workspace
        .set_fixture(Some(crate::app::tests::remote_workspace()));
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    assert_eq!(app.presentation.area, areas::Area::Extension);
    assert_eq!(
        app.extension_panels
            .instances
            .get("test.panel")
            .unwrap()
            .values["draft"],
        "server value"
    );
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .values
        .insert("draft".into(), "before request".into());
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .send("refresh")
        .unwrap();
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .values
        .insert("draft".into(), "typed while waiting".into());
    ready(&mut app, &ctx);
    assert_eq!(
        app.extension_panels
            .instances
            .get("test.panel")
            .unwrap()
            .values["draft"],
        "typed while waiting"
    );
    app.presentation.enter_area(areas::Area::Welcome);
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .due = Some(Instant::now());
    app.poll_extension_panels(&ctx);
    ready(&mut app, &ctx);
    assert_eq!(
        app.extension_panels
            .instances
            .get("test.panel")
            .unwrap()
            .view
            .title,
        "tick"
    );
    assert_eq!(
        app.extension_panels
            .instances
            .get("test.panel")
            .unwrap()
            .values["draft"],
        "typed while waiting"
    );
    app.set_extension("test.panel", false);
    assert!(!app.extension_panels.instances.contains_key("test.panel"));
    assert!(!app.extensions.registry.enabled("test.panel"));
}
#[test]
fn invalid_protocol_stops_only_the_provider() {
    let (mut app, ctx) = fixture(
        "import sys,json\nfor line in sys.stdin: print(json.dumps({'api':1,'id':999,'view':{'title':'bad'}}),flush=True)\n",
    );
    app.set_extension("test.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    let until = Instant::now() + Duration::from_secs(10);
    while app
        .extension_panels
        .instances
        .get("test.panel")
        .unwrap()
        .session
        .is_some()
    {
        assert!(Instant::now() < until);
        app.poll_extension_panels(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        app.extension_panels
            .instances
            .get("test.panel")
            .unwrap()
            .error
            .contains("identity")
    );
    assert!(app.workspace.is_none());
}
#[test]
fn panel_schema_rejects_ambiguous_controls_and_invalid_tables() {
    let mut view:View=serde_json::from_value(serde_json::json!({"title":"Test","actions":[{"id":"save","label":"Save","enabled":true},{"id":"save","label":"Other"}]})).unwrap();
    assert!(view.validate().is_err());
    view.actions.pop();
    assert!(view.validate().is_ok());
    view.tables.push(protocol::Table {
        key: String::new(),
        title: "Bad".into(),
        columns: vec!["One".into()],
        rows: vec![vec!["one".into(), "two".into()]],
    });
    assert!(view.validate().is_err());
    view.tables.clear();
    view.refresh_seconds = Some(1);
    assert!(view.validate().is_err());
    view.refresh_seconds = Some(30);
    assert!(view.validate().is_ok());
}

#[test]
fn selecting_another_provider_preserves_background_work_and_disabling_is_scoped() {
    let code = "import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n print(json.dumps({'api':r['api'],'id':r['id'],'view':{'title':r['action'],'refresh_seconds':30}}),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    let mut other = app.extensions.registry.entries.last().unwrap().clone();
    other.id = "other.panel".into();
    other.manifest.as_mut().unwrap().id = other.id.clone();
    app.extensions.registry.entries.push(other);
    app.set_extension("test.panel", true);
    app.set_extension("other.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    app.open_extension_panel("other.panel", &ctx);
    assert!(
        app.extension_panels.instances["test.panel"]
            .session
            .is_some()
    );
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .due = Some(Instant::now());
    app.poll_extension_panels(&ctx);
    ready(&mut app, &ctx);
    assert_eq!(
        app.extension_panels.instances["test.panel"].view.title,
        "tick"
    );
    app.set_extension("test.panel", false);
    assert!(
        app.extension_panels.instances["other.panel"]
            .session
            .is_some()
    );
    assert_eq!(app.extension_panels.selected, "other.panel");
}
#[test]
fn provider_settings_merge_at_leaf_level_and_keep_host_edits() {
    let before = serde_json::json!({"symbol":"AAPL","watchlist":{"AAPL":true,"SIE.DE":false}});
    let after = serde_json::json!({"symbol":"MSFT","watchlist":{"AAPL":true,"SIE.DE":true}});
    let mut current = serde_json::json!({"symbol":"7203.T","watchlist":{"AAPL":false,"SIE.DE":false},"unrelated":true});
    merge_settings(&before, &mut current, &after);
    assert_eq!(
        current,
        serde_json::json!({"symbol":"7203.T","watchlist":{"AAPL":false,"SIE.DE":true},"unrelated":true})
    );
}

#[test]
fn registered_panels_keep_separate_native_window_and_focus_owners() {
    let code = "import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n print(json.dumps({'api':r['api'],'id':r['id'],'view':{'title':'Test'}}),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    let mut second = app.extensions.registry.entries.last().unwrap().clone();
    second.id = "other.panel".into();
    second.manifest.as_mut().unwrap().id = second.id.clone();
    app.extensions.registry.entries.push(second);
    app.set_extension("test.panel", true);
    app.set_extension("other.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    let first = app.extension_panels.tab("test.panel").unwrap();
    app.detach_tab(first, &ctx);
    app.open_extension_panel("other.panel", &ctx);
    let other = app.extension_panels.tab("other.panel").unwrap();
    app.detach_tab(other, &ctx);
    assert_ne!(app.tool_viewport(first), app.tool_viewport(other));
    assert_eq!(app.presentation.area, areas::Area::Projects);
    app.open_extension_panel("test.panel", &ctx);
    assert_eq!(
        app.presentation.area,
        areas::Area::Projects,
        "Raising a window must not replace the root selection"
    );
    assert_eq!(app.extension_panels.selected, "other.panel");
    app.reattach_tool_tab(first, &ctx);
    assert_eq!(app.extension_panels.selected, "test.panel");
    assert!(!app.tab_detached(first));
    assert!(app.tab_detached(other));
    app.set_extension("other.panel", false);
    assert!(!app.tab_detached(other));
    assert!(
        app.extension_panels.instances["test.panel"]
            .session
            .is_some()
    );
}

#[test]
fn newly_created_settings_objects_merge_independent_leaves() {
    let before = serde_json::json!({});
    let mut current = serde_json::json!({"watchlist":{"AAPL":true}});
    merge_settings(
        &before,
        &mut current,
        &serde_json::json!({"watchlist":{"SIE.DE":true}}),
    );
    assert_eq!(
        current,
        serde_json::json!({"watchlist":{"AAPL":true,"SIE.DE":true}})
    );
}

#[test]
fn provider_settings_cannot_exceed_the_shared_preference_file_budget() {
    let (mut app, _ctx) = fixture("");
    app.extensions
        .registry
        .preferences
        .entry("other.panel".into())
        .or_default()
        .settings
        .insert("large".into(), "x".repeat(160 * 1024).into());
    let state = Instance {
        id: "test.panel".into(),
        ..Default::default()
    };
    let updated = BTreeMap::from([("large".into(), "y".repeat(120 * 1024).into())]);
    assert!(update_preferences(&mut app.extensions.registry, &state, updated).is_err());
    assert!(
        !app.extensions
            .registry
            .preferences
            .contains_key("test.panel")
    );
    assert!(!app.extensions.registry.pending);
}

#[test]
fn panel_enablement_keeps_unrelated_project_tool_work_alive() {
    let (mut app, _ctx) = fixture("");
    let cancel = app.extensions.cancel.clone();
    let revision = app.extensions.registry.revision;
    app.set_extension("test.panel", true);
    app.set_extension("test.panel", false);
    assert!(Arc::ptr_eq(&cancel, &app.extensions.cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert_eq!(app.extensions.registry.revision, revision);
}

#[test]
fn restart_recovers_the_draft_without_submitting_it_during_open() {
    let code = "import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n assert r['action'] != 'open' or not r['values'], 'Reopening must not submit a draft'\n print(json.dumps({'api':r['api'],'id':r['id'],'view':{'title':'Test','fields':[{'key':'draft','label':'Draft','value':'provider default'}]}}),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    app.set_extension("test.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    let state = app
        .extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap();
    state
        .values
        .insert("draft".into(), "Unsent local input".into());
    let tab = app.extension_panels.tab("test.panel").unwrap();
    app.detach_tab(tab, &ctx);
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .stop();
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    assert_eq!(
        app.extension_panels.instances["test.panel"].values["draft"],
        "Unsent local input"
    );
    assert_eq!(app.extension_panels.tab("test.panel"), Some(tab));
    assert!(app.tab_detached(tab));
    let state = app
        .extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap();
    state.send("refresh").unwrap();
    ready(&mut app, &ctx);
    assert_eq!(
        app.extension_panels.instances["test.panel"].values["draft"], "provider default",
        "An explicit action may accept the provider's updated value"
    );
}

#[test]
fn settings_deletions_distinguish_absence_null_and_concurrent_group_edits() {
    for (before, mut current, after, expected) in [
        (
            serde_json::json!({"clear":null,"other":1}),
            serde_json::json!({"clear":null,"other":2}),
            serde_json::json!({"other":1}),
            serde_json::json!({"other":2}),
        ),
        (
            serde_json::json!({"other":1}),
            serde_json::json!({"other":2}),
            serde_json::json!({"added":null,"other":1}),
            serde_json::json!({"added":null,"other":2}),
        ),
        (
            serde_json::json!({"group":{"old":1}}),
            serde_json::json!({"group":{"old":1,"host":2}}),
            serde_json::json!({}),
            serde_json::json!({"group":{"host":2}}),
        ),
        (
            serde_json::json!({"group":{"old":1}}),
            serde_json::json!({"group":{"old":3}}),
            serde_json::json!({}),
            serde_json::json!({"group":{"old":3}}),
        ),
    ] {
        merge_settings(&before, &mut current, &after);
        assert_eq!(current, expected);
    }
}

#[test]
fn editing_panel_settings_keeps_drafts_and_unrelated_project_work() {
    let code = "import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n print(json.dumps({'api':r['api'],'id':r['id'],'view':{'title':str(r['settings'].get('configured',False)),'fields':[{'key':'draft','label':'Draft','value':'default'}]}}),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    app.extensions
        .registry
        .entries
        .last_mut()
        .unwrap()
        .manifest
        .as_mut()
        .unwrap()
        .panel_version = 2;
    app.set_extension("test.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .values
        .insert("draft".into(), "Unsubmitted".into());
    let cancel = app.extensions.cancel.clone();
    let revision = app.extensions.registry.revision;
    app.save_extension_settings(
        "test.panel",
        BTreeMap::from([("configured".into(), true.into())]),
    )
    .unwrap();
    app.poll_extension_panels(&ctx);
    ready(&mut app, &ctx);
    assert_eq!(
        app.extension_panels.instances["test.panel"].view.title,
        "True"
    );
    assert_eq!(
        app.extension_panels.instances["test.panel"].values["draft"],
        "Unsubmitted"
    );
    assert_eq!(app.extensions.registry.revision, revision);
    assert!(Arc::ptr_eq(&cancel, &app.extensions.cancel));
    assert!(!cancel.load(Ordering::Acquire));
    let before = serde_json::to_value(&app.extensions.registry.preferences).unwrap();
    assert!(
        app.save_extension_settings(
            "test.panel",
            BTreeMap::from([("too-large".into(), "x".repeat(193 * 1024).into())])
        )
        .is_err()
    );
    assert_eq!(
        serde_json::to_value(&app.extensions.registry.preferences).unwrap(),
        before
    );
}

#[test]
fn provider_settings_updates_invalidate_only_declared_project_contributions() {
    let code = "import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n result={'api':r['api'],'id':r['id'],'view':{'title':'Test'}}\n if r['action']=='refresh': result['settings']={'configured':True}\n print(json.dumps(result),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    let entry = app.extensions.registry.entries.last_mut().unwrap();
    let mut raw = serde_json::to_value(entry.manifest.take().unwrap()).unwrap();
    let panel_tool = raw.as_object_mut().unwrap().remove("tool").unwrap();
    raw.as_object_mut().unwrap().remove("panel");
    raw["contributions"] = serde_json::json!([
        {"kind":"panel","host":"desktop","protocol":2,"tool":panel_tool},
        {"kind":"project-tool","host":"project","tool":{"program":"unused-project-tool","commands":[{"id":"test.inspect","title":"Inspect"}]}}
    ]);
    let manifest: crate::extensions::Manifest = serde_json::from_value(raw).unwrap();
    manifest.validate().unwrap();
    entry.manifest = Some(manifest);
    app.set_extension("test.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    let cancel = app.extensions.cancel.clone();
    let revision = app.extensions.registry.revision;
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .send("refresh")
        .unwrap();
    ready(&mut app, &ctx);
    assert!(
        cancel.load(Ordering::Acquire),
        "A combined extension's old project work must not apply under new settings"
    );
    assert!(app.extensions.registry.revision > revision);
    assert!(
        app.extension_panels.instances["test.panel"]
            .session
            .is_some()
    );
}

#[test]
fn failed_send_preserves_sequence_snapshots_and_pending_configuration() {
    let mut state = Instance {
        version: 2,
        sequence: 17,
        settings_dirty: true,
        sent: BTreeMap::from([("old".into(), "snapshot".into())]),
        ..Default::default()
    };
    assert!(state.send("settings").is_err());
    assert_eq!(state.sequence, 17);
    assert!(state.settings_dirty);
    assert_eq!(state.sent["old"], "snapshot");
    assert!(state.busy.is_none());
}

#[test]
fn confirmations_reject_changed_input_settings_and_disabled_actions() {
    let code = "import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n print(json.dumps({'api':r['api'],'id':r['id'],'view':{'title':r['action'],'fields':[{'key':'draft','label':'Draft','value':'default'}],'actions':[{'id':'commit','label':'Commit','enabled':True,'confirm':'Confirm these inputs?'}],'refresh_seconds':30}}),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    app.extensions
        .registry
        .entries
        .last_mut()
        .unwrap()
        .manifest
        .as_mut()
        .unwrap()
        .panel_version = 2;
    app.set_extension("test.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    let state = app
        .extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap();
    let sequence = state.sequence;
    state.activate("commit").unwrap();
    state.due = Some(Instant::now());
    assert!(
        !state.schedule(&ctx),
        "A refresh ran behind a pending confirmation"
    );
    assert!(state.next_deadline().is_none());
    state.values.insert("draft".into(), "changed input".into());
    assert!(state.confirm_action().is_err());
    assert_eq!(state.sequence, sequence);
    state.activate("commit").unwrap();
    state.view.actions[0].enabled = false;
    assert!(state.confirm_action().is_err());
    assert_eq!(state.sequence, sequence);
    state.view.actions[0].enabled = true;
    state.activate("commit").unwrap();
    state.sync_settings(Some(&BTreeMap::from([("changed".into(), true.into())])));
    assert!(state.confirm_action().is_err());
    assert_eq!(state.sequence, sequence);
    assert!(state.settings_dirty);
    state.sync_settings(None);
    state.schedule(&ctx);
    ready(&mut app, &ctx);
    let state = app
        .extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap();
    state.activate("commit").unwrap();
    state.confirm_action().unwrap();
    ready(&mut app, &ctx);
    assert_eq!(
        app.extension_panels.instances["test.panel"].view.title,
        "commit"
    );
}

#[test]
fn configuration_precedes_refresh_and_merges_against_the_completed_request() {
    let code = "import json,sys,time\nfor line in sys.stdin:\n r=json.loads(line)\n result={'api':r['api'],'id':r['id'],'view':{'title':r['action'],'status':json.dumps(r.get('settings',{}),sort_keys=True),'refresh_seconds':30}}\n if r['action']=='refresh':\n  time.sleep(0.05)\n  result['settings']=dict(r['settings'],provider=True)\n print(json.dumps(result),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    app.extensions
        .registry
        .entries
        .last_mut()
        .unwrap()
        .manifest
        .as_mut()
        .unwrap()
        .panel_version = 2;
    app.set_extension("test.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .send("refresh")
        .unwrap();
    app.save_extension_settings("test.panel", BTreeMap::from([("host".into(), true.into())]))
        .unwrap();
    app.extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap()
        .due = Some(Instant::now());
    app.poll_extension_panels(&ctx);
    ready(&mut app, &ctx);
    let state = &app.extension_panels.instances["test.panel"];
    assert_eq!(state.view.title, "settings");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&state.view.status).unwrap(),
        serde_json::json!({"host":true,"provider":true})
    );
    let due = state.next_deadline().unwrap();
    assert!(
        due.saturating_duration_since(Instant::now()) > Duration::from_secs(25),
        "Hidden panel is polling instead of sleeping to its deadline"
    );
}

#[test]
fn readonly_refresh_ignores_oversized_unsent_drafts_and_v1_host_settings() {
    let code = "import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n print(json.dumps({'api':r['api'],'id':r['id'],'view':{'title':r['action'],'fields':[{'key':'draft','label':'Draft','value':'default'}]}}),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    app.set_extension("test.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    app.save_extension_settings(
        "test.panel",
        BTreeMap::from([("future".into(), true.into())]),
    )
    .unwrap();
    app.poll_extension_panels(&ctx);
    let state = app
        .extension_panels
        .instances
        .get_mut("test.panel")
        .unwrap();
    assert!(
        state.ready(),
        "V1 provider was blocked by settings it cannot consume"
    );
    state.values.insert("draft".into(), "x".repeat(950 * 1024));
    state.send("tick").unwrap();
    ready(&mut app, &ctx);
    let state = &app.extension_panels.instances["test.panel"];
    assert_eq!(state.view.title, "tick");
    assert_eq!(state.values["draft"].len(), 950 * 1024);
}

#[test]
fn reload_and_project_metadata_changes_preserve_an_unchanged_panel_process() {
    let code = "import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n print(json.dumps({'api':r['api'],'id':r['id'],'view':{'title':'unchanged'}}),flush=True)\n";
    let (mut app, ctx) = fixture(code);
    app.set_extension("test.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    let directory = tempfile::tempdir().unwrap();
    let entry = app.extensions.registry.entries.last_mut().unwrap();
    entry.manifest.as_mut().unwrap().name = "New display name".into();
    entry.manifest.as_mut().unwrap().icon = "chart".into();
    std::fs::write(
        directory.path().join("panel.json"),
        serde_json::to_vec(entry.manifest.as_ref().unwrap()).unwrap(),
    )
    .unwrap();
    let cancel = app.extensions.cancel.clone();
    let revision = app.extensions.registry.revision;
    let sequence = app.extension_panels.instances["test.panel"].sequence;
    let tab = app.extension_panels.tab("test.panel");
    app.reload_extensions_from(directory.path());
    app.poll_extension_panels(&ctx);
    assert!(Arc::ptr_eq(&cancel, &app.extensions.cancel));
    assert!(!cancel.load(Ordering::Acquire));
    assert_eq!(app.extensions.registry.revision, revision);
    assert_eq!(
        app.extension_panels.instances["test.panel"].sequence,
        sequence
    );
    assert_eq!(app.extension_panels.tab("test.panel"), tab);
    assert!(
        app.extension_panels.instances["test.panel"]
            .session
            .is_some()
    );
}

#[test]
fn protocol_three_providers_publish_ordered_views_and_update_settings() {
    let code = r#"import json,sys
for line in sys.stdin:
 r=json.loads(line)
 assert r['api']==3 and 'settings' in r
 print(json.dumps({'api':3,'id':r['id'],'settings':{'region':'EU'},'view':{'title':'Service','fields':[{'key':'region','label':'Region','value':'EU'}],'content':[{'kind':'fields','keys':['region']}]}}),flush=True)
"#;
    let (mut app, ctx) = fixture(code);
    app.extensions
        .registry
        .entries
        .last_mut()
        .unwrap()
        .manifest
        .as_mut()
        .unwrap()
        .panel_version = 3;
    app.set_extension("test.panel", true);
    app.open_extension_panel("test.panel", &ctx);
    ready(&mut app, &ctx);
    let state = &app.extension_panels.instances["test.panel"];
    assert_eq!(state.version, 3);
    state.view.validate_version(3).unwrap();
    assert_eq!(
        app.extensions.registry.preferences["test.panel"].settings["region"],
        "EU"
    );
}
