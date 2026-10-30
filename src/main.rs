//! Desktop entry point and internal helper dispatch.
//!
//! Helper flags return before GUI/storage initialization. Ordinary startup parses
//! local/SSH choices, creates private framework persistence, and hands shared
//! preferences to the field-merging writer before constructing the native window.

use std::path::PathBuf;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--codex") {
        if let Err(error) = rode::bundled::codex(&args[1..]) {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.as_slice() == ["--debug-helper"] {
        if let Err(error) = rode::debugger::bridge::coordinator() {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.as_slice() == ["--vault-helper"] {
        if let Err(error) = rode::vault::coordinator() {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.as_slice() == ["--latex-helper"] {
        if let Err(error) = rode::latex::coordinator() {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--session-service") {
        let result = if args.get(1).is_some_and(|arg| arg == "request") {
            use std::io::Read;
            let mut bytes = vec![];
            std::io::stdin()
                .take(256 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(anyhow::Error::from)
                .and_then(|_| {
                    anyhow::ensure!(bytes.len() <= 256 * 1024, "Session request too large");
                    let request = serde_json::from_slice(&bytes)?;
                    let response = rode_session::request(
                        &request,
                        &rode_session::Launcher {
                            executable: rode_session::current_executable()?,
                            prefix: vec!["--session-service".into()],
                        },
                    )?;
                    serde_json::to_writer(std::io::stdout().lock(), &response)?;
                    Ok(())
                })
        } else {
            rode_session::cli(&args[1..])
        };
        if let Err(error) = result {
            rode_session::report_error(&args[1..], &error);
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.as_slice() == ["--theme-licenses"] {
        for license in two_face::acknowledgement::listing().for_themes() {
            println!("{}\n{}\n", license.rel_path.display(), license.text);
        }
        return Ok(());
    }
    if args.as_slice() == ["--syntax-licenses"] {
        for license in two_face::acknowledgement::listing().for_syntaxes() {
            println!("{}\n{}\n", license.rel_path.display(), license.text);
        }
        return Ok(());
    }
    if args.as_slice() == ["--marp-helper"] {
        if let Err(error) = rode::marp::coordinator() {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.as_slice() == ["--extension-helper"] {
        if let Err(error) = rode::extensions::coordinator() {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.as_slice() == ["--file-helper"] {
        use std::io::Read;
        let result = (|| -> anyhow::Result<()> {
            let mut input = Vec::new();
            std::io::stdin()
                .take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut input)?;
            anyhow::ensure!(
                input.len() <= 64 * 1024 * 1024,
                "File request exceeds its limit"
            );
            let request = serde_json::from_slice(&input)?;
            let response = rode::files::local(&request)?;
            serde_json::to_writer(std::io::stdout().lock(), &response)?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "RODE — native Rust editor\n\nUsage: rode [DIRECTORY] [--theme THEME] [--syntax-theme THEME] [--sandbox]\n       rode --ssh HOST [--remote-path PATH] [--user USER] [--port PORT]\n            [--identity FILE] [--jump HOST] [--ssh-config FILE] [--sandbox]\n\nSSH aliases and settings from ~/.ssh/config are honored.\nRemote file access requires python3; authentication happens in the terminal.\nThemes: phosphor (default), carbon, code-dark, matrix, amber, ice, neon, or custom:FILENAME.\nSyntax themes: ui (follow UI), dracula, catppuccin-mocha, nord, monokai, one-half-dark, or custom:FILENAME.\nEmbedded acknowledgements: rode --syntax-licenses / --theme-licenses."
        );
        println!(
            "\nOptional Codex runtime: rode --codex [ARGUMENTS], e.g. rode --codex login.\nInstall rode-codex or supply an installed Codex executable."
        );
        return Ok(());
    }
    let mut startup = rode::app::Startup::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--sandbox" {
            startup.sandbox = true;
        } else if arg == "--theme" || arg == "--syntax-theme" {
            i += 1;
            let Some(value) = args.get(i) else {
                eprintln!("Missing value for {arg}. Run rode --help.");
                std::process::exit(2);
            };
            if arg == "--syntax-theme" {
                startup.syntax_theme = Some(value.clone());
            } else {
                startup.theme = Some(value.clone());
            }
        } else if matches!(
            arg.as_str(),
            "--ssh"
                | "--remote-path"
                | "--user"
                | "--port"
                | "--identity"
                | "--jump"
                | "--ssh-config"
        ) {
            i += 1;
            let Some(value) = args.get(i) else {
                eprintln!("Missing value for {arg}. Run rode --help.");
                std::process::exit(2);
            };
            let profile = startup.remote.get_or_insert_with(Default::default);
            match arg.as_str() {
                "--ssh" => profile.host.clone_from(value),
                "--remote-path" => profile.root.clone_from(value),
                "--user" => profile.user.clone_from(value),
                "--port" => profile.port.clone_from(value),
                "--identity" => profile.identity.clone_from(value),
                "--jump" => profile.jump.clone_from(value),
                "--ssh-config" => profile.config.clone_from(value),
                _ => unreachable!(),
            }
        } else if arg.starts_with('-') {
            eprintln!("Unknown option: {arg}. Run rode --help.");
            std::process::exit(2);
        } else {
            startup.local = Some(PathBuf::from(arg));
        }
        i += 1;
    }
    let wake = std::sync::Arc::new(std::sync::OnceLock::<eframe::egui::Context>::new());
    let notify = wake.clone();
    let path = eframe::storage_dir("RODE")
        .ok_or_else(|| eframe::Error::AppCreation("Cannot find the preferences directory".into()))?
        .join("app.ron");
    let preferences = rode::persistence::Preferences::open(
        path,
        std::sync::Arc::new(move || {
            if let Some(ctx) = notify.get() {
                ctx.request_repaint();
            }
        }),
    );
    let native_state = rode::persistence::NativeState::new(&preferences)
        .map_err(|e| eframe::Error::AppCreation(e.into()))?;
    let options = eframe::NativeOptions {
        persistence_path: Some(native_state.path.clone()),
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("RODE · Rust / OpenSSH / Development Environment")
            .with_inner_size([1380.0, 900.0])
            .with_min_inner_size([850.0, 560.0]),
        renderer: eframe::Renderer::Glow,
        vsync: true,
        ..Default::default()
    };
    let result = eframe::run_native(
        "RODE",
        options,
        Box::new(move |cc| {
            let _ = wake.set(cc.egui_ctx.clone());
            Ok(Box::new(rode::app::EditorApp::new_native(
                cc,
                startup,
                preferences,
            )))
        }),
    );
    drop(native_state);
    #[cfg(target_os = "linux")]
    if matches!(
        &result,
        Err(eframe::Error::Glutin(_)) | Err(eframe::Error::NoGlutinConfigs(..))
    ) && std::env::var_os("RODE_GL_FALLBACK").is_none()
        && std::path::Path::new("/usr/share/glvnd/egl_vendor.d/50_mesa.json").exists()
    {
        use std::os::unix::process::CommandExt;
        eprintln!("OpenGL startup failed; retrying with Mesa software rendering.");
        if let Ok(executable) = rode_session::current_executable() {
            let error = std::process::Command::new(executable)
                .args(std::env::args_os().skip(1))
                .env("RODE_GL_FALLBACK", "1")
                .env("__GLX_VENDOR_LIBRARY_NAME", "mesa")
                .env(
                    "__EGL_VENDOR_LIBRARY_FILENAMES",
                    "/usr/share/glvnd/egl_vendor.d/50_mesa.json",
                )
                .env("LIBGL_ALWAYS_SOFTWARE", "1")
                .exec();
            eprintln!("Could not restart with software rendering: {error}");
        }
    }
    result
}
