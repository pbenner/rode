//! Explorer mutation contracts shared by in-process and native-helper dispatch.
use super::*;
use rode::files::Mutation;
use rode::sandbox::PlanExt as _;

fn mutate(
    run: fn(&Request) -> anyhow::Result<files::Response>,
    operation: Mutation,
) -> anyhow::Result<files::Response> {
    run(&Request::Mutate {
        mutation: operation,
    })
}

#[test]
fn explorer_operations_preserve_contents_and_refuse_collisions_on_both_dispatchers() {
    for run in [files::local, headless] {
        let base = TempDir::new().unwrap();
        let path = |name: &str| base.path().join(name).to_string_lossy().into_owned();
        let source = path("folder λ with spaces");
        mutate(
            run,
            Mutation::CreateDirectory {
                path: source.clone(),
            },
        )
        .unwrap();
        assert!(
            mutate(
                run,
                Mutation::CreateDirectory {
                    path: source.clone()
                }
            )
            .is_err()
        );
        let file = format!("{source}/literal 'quoted' $(not-a-command).txt");
        run(&Request::Write {
            path: file.clone(),
            text: "saved contents 🦀".into(),
            expected: None,
        })
        .unwrap();
        let duplicate = path("duplicate");
        mutate(
            run,
            Mutation::Copy {
                path: source.clone(),
                destination: duplicate.clone(),
            },
        )
        .unwrap();
        let name = std::path::Path::new(&file).file_name().unwrap();
        assert_eq!(
            fs::read_to_string(std::path::Path::new(&duplicate).join(name)).unwrap(),
            "saved contents 🦀"
        );
        assert!(
            mutate(
                run,
                Mutation::Copy {
                    path: source.clone(),
                    destination: duplicate.clone()
                }
            )
            .is_err()
        );
        assert!(
            mutate(
                run,
                Mutation::Rename {
                    path: source.clone(),
                    destination: duplicate.clone()
                }
            )
            .is_err()
        );
        assert!(std::path::Path::new(&source).is_dir());
        let renamed = path("renamed");
        let response = mutate(
            run,
            Mutation::Rename {
                path: duplicate,
                destination: renamed.clone(),
            },
        )
        .unwrap();
        assert_eq!(response.path, renamed);
        mutate(
            run,
            Mutation::Remove {
                path: renamed.clone(),
            },
        )
        .unwrap();
        assert!(!std::path::Path::new(&renamed).exists());
        assert!(std::path::Path::new(&file).is_file());
        assert!(
            mutate(
                run,
                Mutation::Copy {
                    path: source.clone(),
                    destination: format!("{source}/inside")
                }
            )
            .is_err()
        );
        assert_eq!(
            fs::read_dir(base.path()).unwrap().count(),
            1,
            "No failed-copy staging directories remain"
        );
    }
}

#[test]
#[cfg(unix)]
fn explorer_symlinks_are_copied_renamed_and_removed_without_touching_targets() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for run in [files::local, headless] {
        let base = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("keep"), "untouched").unwrap();
        let source = base.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("script"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(source.join("script"), fs::Permissions::from_mode(0o750)).unwrap();
        symlink(outside.path(), source.join("link")).unwrap();
        symlink("missing-target", source.join("broken")).unwrap();
        let copied = base.path().join("copied");
        mutate(
            run,
            Mutation::Copy {
                path: source.to_string_lossy().into(),
                destination: copied.to_string_lossy().into(),
            },
        )
        .unwrap();
        assert_eq!(fs::read_link(copied.join("link")).unwrap(), outside.path());
        assert_eq!(
            fs::read_link(copied.join("broken")).unwrap(),
            std::path::Path::new("missing-target")
        );
        assert_eq!(
            fs::metadata(copied.join("script"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
        let renamed = copied.join("renamed-link");
        mutate(
            run,
            Mutation::Rename {
                path: copied.join("link").to_string_lossy().into(),
                destination: renamed.to_string_lossy().into(),
            },
        )
        .unwrap();
        mutate(
            run,
            Mutation::Remove {
                path: renamed.to_string_lossy().into(),
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(outside.path().join("keep")).unwrap(),
            "untouched"
        );
        assert!(
            mutate(
                run,
                Mutation::Rename {
                    path: copied.join("script").to_string_lossy().into(),
                    destination: copied.join("broken").to_string_lossy().into()
                }
            )
            .is_err()
        );
        assert!(
            fs::symlink_metadata(copied.join("broken"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        mutate(
            run,
            Mutation::Remove {
                path: copied.to_string_lossy().into(),
            },
        )
        .unwrap();
        assert!(outside.path().join("keep").is_file());
    }
}

#[test]
fn explorer_copy_traversal_limits_leave_the_destination_unpublished_and_remove_staging() {
    for run in [files::local, headless] {
        let base = TempDir::new().unwrap();
        let source = base.path().join("source");
        fs::create_dir(&source).unwrap();
        let mut nested = source.clone();
        for _ in 0..66 {
            nested = nested.join("nested");
            fs::create_dir(&nested).unwrap();
        }
        let destination = base.path().join("destination");
        let error = mutate(
            run,
            Mutation::Copy {
                path: source.to_string_lossy().into(),
                destination: destination.to_string_lossy().into(),
            },
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("64-level"));
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(base.path()).unwrap().count(), 1);
    }
}

#[test]
fn concurrent_explorer_moves_never_replace_the_winning_destination() {
    for run in [files::local, headless] {
        let base = TempDir::new().unwrap();
        let first = base.path().join("first");
        let second = base.path().join("second");
        let destination = base.path().join("destination");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let jobs: Vec<_> = [&first, &second]
                .into_iter()
                .map(|source| {
                    let destination = &destination;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        mutate(
                            run,
                            Mutation::Rename {
                                path: source.to_string_lossy().into(),
                                destination: destination.to_string_lossy().into(),
                            },
                        )
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let winner = fs::read_to_string(&destination).unwrap();
        assert!(
            (winner == "first" && !first.exists() && second.exists())
                || (winner == "second" && first.exists() && !second.exists())
        );
    }
}

#[test]
fn explorer_mutations_obey_project_and_read_only_sandbox_grants() {
    for backend in [
        rode::sandbox::Backend::Bwrap,
        rode::sandbox::Backend::Landlock,
    ] {
        let base = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let secret = outside.path().join("keep");
        fs::write(&secret, "private").unwrap();
        let options = rode::sandbox::Options {
            enabled: true,
            backend,
            ..Default::default()
        };
        let plan = native_peer::prepare(base.path().to_str().unwrap(), &options).unwrap();
        if !plan.active {
            if backend == rode::sandbox::Backend::Bwrap {
                assert_ne!(
                    std::env::var("RODE_TEST_REQUIRE_SANDBOX").as_deref(),
                    Ok("1"),
                    "{}",
                    plan.reason
                );
            }
            if backend == rode::sandbox::Backend::Landlock {
                assert_ne!(
                    std::env::var("RODE_TEST_REQUIRE_LANDLOCK").as_deref(),
                    Ok("1"),
                    "{}",
                    plan.reason
                );
            }
            eprintln!("Explorer sandbox unavailable: {}", plan.reason);
            continue;
        }
        let request = |mutation| Request::Mutate { mutation };
        let folder = base.path().join("created");
        plan.local_request_with_helper(
            &request(Mutation::CreateDirectory {
                path: folder.to_string_lossy().into(),
            }),
            std::path::Path::new(native_peer::binary()),
        )
        .unwrap();
        assert!(folder.is_dir());
        assert!(
            plan.local_request_with_helper(
                &request(Mutation::Remove {
                    path: secret.to_string_lossy().into()
                }),
                std::path::Path::new(native_peer::binary())
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&secret).unwrap(), "private");
        let readonly = native_peer::prepare(
            base.path().to_str().unwrap(),
            &rode::sandbox::Options {
                project_read_only: true,
                ..options
            },
        )
        .unwrap();
        assert!(readonly.active, "{}", readonly.reason);
        assert!(
            readonly
                .local_request_with_helper(
                    &request(Mutation::Remove {
                        path: folder.to_string_lossy().into()
                    }),
                    std::path::Path::new(native_peer::binary())
                )
                .is_err()
        );
        assert!(
            readonly
                .local_request_with_helper(
                    &request(Mutation::CreateDirectory {
                        path: base.path().join("denied").to_string_lossy().into()
                    }),
                    std::path::Path::new(native_peer::binary())
                )
                .is_err()
        );
        assert!(folder.is_dir());
        assert!(!base.path().join("denied").exists());
    }
}

#[test]
fn explorer_streams_file_copies_larger_than_the_previous_256_mib_cap() {
    use std::io::{Read, Seek, SeekFrom, Write};
    for run in [files::local, headless] {
        let base = TempDir::new().unwrap();
        let source = base.path().join("large.bin");
        let destination = base.path().join("copy.bin");
        let length = 256 * 1024 * 1024 + 1;
        let mut file = fs::File::create(&source).unwrap();
        file.set_len(length).unwrap();
        file.seek(SeekFrom::End(-1)).unwrap();
        file.write_all(&[0x71]).unwrap();
        drop(file);
        mutate(
            run,
            Mutation::Copy {
                path: source.to_string_lossy().into(),
                destination: destination.to_string_lossy().into(),
            },
        )
        .unwrap();
        let mut copied = fs::File::open(&destination).unwrap();
        assert_eq!(copied.metadata().unwrap().len(), length);
        copied.seek(SeekFrom::End(-1)).unwrap();
        let mut tail = [0];
        copied.read_exact(&mut tail).unwrap();
        assert_eq!(tail, [0x71]);
        assert_eq!(fs::read_dir(base.path()).unwrap().count(), 2);
    }
}
