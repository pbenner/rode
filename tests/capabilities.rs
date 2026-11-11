//! Optional coverage survives libtest output capture; required coverage cannot silently pass.
#[path = "support/capabilities.rs"]
mod capabilities;

#[test]
#[ignore = "subprocess fixture invoked by capability_reporting_is_explicit"]
fn capability_probe() {
    capabilities::tool("fixture-tool", &["RODE_TEST_MISSING_FIXTURE_TOOL"]);
}

#[test]
fn capability_reporting_is_explicit() {
    let directory = tempfile::tempdir().unwrap();
    for required in [false, true] {
        let reports = directory
            .path()
            .join(if required { "required" } else { "optional" });
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "capability_probe", "--ignored"])
            .env_remove("RODE_TEST_MISSING_FIXTURE_TOOL")
            .env("RODE_TEST_SKIP_DIR", &reports)
            .env(
                "RODE_TEST_REQUIRED_CAPABILITIES",
                if required { "fixture-tool" } else { "" },
            )
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            !required,
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        if required {
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .contains("Required capability fixture-tool")
            );
            assert!(!reports.exists());
        } else {
            let paths: Vec<_> = std::fs::read_dir(reports).unwrap().collect();
            assert_eq!(paths.len(), 1);
            let report: serde_json::Value =
                serde_json::from_slice(&std::fs::read(paths[0].as_ref().unwrap().path()).unwrap())
                    .unwrap();
            assert_eq!(report["test"], "capability_probe");
            assert_eq!(report["capability"], "fixture-tool");
        }
    }
}
