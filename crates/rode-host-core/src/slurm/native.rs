//! Native scheduler boundary. Compute launches require a verified shared helper copy;
//! an unavailable or mismatched compute helper never falls back to the login host.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap, os::unix::process::CommandExt, path::Path, process::Command,
    time::Duration,
};

#[derive(Serialize, Deserialize)]
struct Spec {
    target: super::Target,
    root: String,
    prefix: Vec<String>,
    python: String,
    debugger: bool,
    helper: String,
    digest: String,
    architecture: String,
}

/// Capture a native compute invocation. The scheduler host hashes the helper at launch,
/// then the compute node verifies its shared copy before executing any helper code.
pub fn argv(
    target: &super::Target,
    plan: &crate::sandbox::Plan,
    python: &str,
    debugger: bool,
) -> Result<Vec<String>> {
    let mut compute_plan = compute_plan(plan)?;
    let helper = match &compute_plan.native_helper {
        Some(helper) => helper.clone(),
        None => local_helper()?.to_string_lossy().into_owned(),
    };
    compute_plan.native_helper = Some(helper.clone());
    let spec = Spec {
        target: target.clone(),
        root: plan.root.clone(),
        prefix: compute_plan.execution_prefix(python)?,
        python: python.into(),
        debugger,
        helper: helper.clone(),
        digest: String::new(),
        architecture: String::new(),
    };
    let encoded = serde_json::to_string(&spec)?;
    parse(std::slice::from_ref(&encoded))?;
    Ok(vec![helper, "--slurm-step".into(), encoded])
}

/// Replace a desktop-only retained image with the shared headless helper before
/// constructing a compute-node policy. Remote plans already name their verified
/// immutable helper generation. Keep the original desktop plan unchanged.
pub(super) fn compute_plan(plan: &crate::sandbox::Plan) -> Result<crate::sandbox::Plan> {
    let mut computed = plan.clone();
    if plan.helper.is_some() {
        let helper = local_helper()?.to_string_lossy().into_owned();
        if let Some(previous) = &plan.native_helper {
            for argument in &mut computed.prefix {
                if argument == previous {
                    argument.clone_from(&helper);
                }
            }
        }
        computed.native_helper = Some(helper);
    }
    Ok(computed)
}

fn local_helper() -> Result<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("RODE_SESSION_HELPER") {
        return std::path::PathBuf::from(path)
            .canonicalize()
            .context("Cannot open RODE_SESSION_HELPER for Slurm");
    }
    let executable = std::env::current_exe()?;
    let bin = executable
        .parent()
        .context("Missing executable directory")?;
    let bin = if bin.file_name().is_some_and(|name| name == "deps") {
        bin.parent().unwrap_or(bin)
    } else {
        bin
    };
    [bin.join("rode-sessiond-static"), bin.join("rode-sessiond")]
        .into_iter().find(|path| path.is_file())
        .context("Local Slurm runtimes need the headless rode-sessiond helper beside the editor or RODE_SESSION_HELPER; the desktop binary is not deployed to compute nodes")?
        .canonicalize().context("Cannot resolve Slurm helper")
}
fn parse(arguments: &[String]) -> Result<Spec> {
    ensure!(
        arguments.len() == 1 && arguments[0].len() <= 256 * 1024,
        "Invalid native Slurm launch metadata"
    );
    let spec: Spec = serde_json::from_str(&arguments[0])?;
    ensure!(
        !spec.target.id.is_empty()
            && spec.target.id.len() <= 20
            && spec.target.id.bytes().all(|b| b.is_ascii_digit()),
        "Only ordinary Slurm allocations can run project runtimes"
    );
    for program in [&spec.target.queue, &spec.target.runner] {
        ensure!(
            !program.trim().is_empty()
                && !program.starts_with('-')
                && program.len() <= 1024
                && !program.chars().any(char::is_control),
            "Invalid squeue/srun executable"
        );
    }
    ensure!(
        Path::new(&spec.helper).is_absolute() && spec.prefix.len() <= 256,
        "Invalid compute helper or policy"
    );
    Ok(spec)
}
fn scheduler_environment() -> BTreeMap<String, String> {
    let mut env = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .filter(|(key, _)| {
            ![
                "SALLOC_",
                "SRUN_",
                "SQUEUE_",
                "SCANCEL_",
                "SLURM_JOB",
                "SLURM_STEP",
                "SLURM_NTASKS",
                "SLURM_MEM_PER",
                "SLURM_GPUS",
                "SLURM_TRES",
            ]
            .iter()
            .any(|prefix| key.starts_with(prefix))
                && ![
                    "SLURM_CLUSTERS",
                    "SLURM_NNODES",
                    "SLURM_CPUS_PER_TASK",
                    "CUDA_VISIBLE_DEVICES",
                    "ROCR_VISIBLE_DEVICES",
                    "GPU_DEVICE_ORDINAL",
                ]
                .contains(&key.as_str())
        })
        .collect::<BTreeMap<_, _>>();
    env.insert("LC_ALL".into(), "C".into());
    env
}

/// Revalidate the allocation on the scheduler host and replace this process with srun.
pub fn step(arguments: &[String]) -> Result<()> {
    let mut spec = parse(arguments)?;
    let root = crate::files::expand_home(&spec.root).canonicalize()?;
    let environment = scheduler_environment();
    let mut query = Command::new(crate::command::resolve_program(&root, &spec.target.queue));
    query
        .args([
            "--noheader",
            "--array",
            "--local",
            "--user",
            &unsafe { libc::geteuid() }.to_string(),
            "--jobs",
            &spec.target.id,
            "--Format",
            "JobID:0|,State:0|,Name:0",
        ])
        .current_dir(&root)
        .env_clear()
        .envs(&environment);
    let output = crate::project::run_controlled_limited(
        query,
        vec![],
        Duration::from_secs(15),
        &crate::command::CANCELLED,
        false,
        8192,
        Duration::ZERO,
    )?
    .checked()?;
    let rows = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.splitn(3, '|').map(str::trim).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    ensure!(
        rows == [vec![
            spec.target.id.as_str(),
            "RUNNING",
            spec.target.name.as_str()
        ]],
        "Selected allocation is no longer your running job; refresh Slurm targets"
    );
    let bytes = crate::persistence::read(Path::new(&spec.helper), 24 * 1024 * 1024)?
        .context("Missing native compute helper")?;
    spec.digest = crate::files::digest(&bytes);
    spec.architecture = std::env::consts::ARCH.into();
    let quote = crate::command::quote;
    // No user shell startup or downloaded executable is involved. The same private,
    // immutable cache path must be shared with the compute node and pass verification.
    let bootstrap = format!(
        "set -eu; test \"${{SLURM_JOB_ID:-}}\" = {job} || {{ echo 'Slurm allocation identity mismatch' >&2; exit 1; }}; test \"$(uname -m)\" = {arch} || {{ echo 'Native compute helper architecture mismatch' >&2; exit 1; }}; test -f {helper} && test ! -L {helper} || {{ echo 'Native compute helper is not on a shared filesystem; configure a shared helper path' >&2; exit 1; }}; _rode_owner=$(stat -c %u -- {helper}); test \"$_rode_owner\" = \"$(id -u)\" || test \"$_rode_owner\" = 0 || {{ echo 'Native compute helper has an unsafe owner' >&2; exit 1; }}; _rode_mode=$(stat -c %a -- {helper}); test $((0$_rode_mode & 022)) -eq 0 || {{ echo 'Native compute helper must not be writable by group or others' >&2; exit 1; }}; test \"$(sha256sum < {helper} | cut -d ' ' -f 1)\" = {digest} || {{ echo 'Native compute helper failed verification' >&2; exit 1; }}; exec {helper} --compute-step {spec}",
        job = quote(&spec.target.id),
        arch = quote(&spec.architecture),
        helper = quote(&spec.helper),
        digest = quote(&spec.digest),
        spec = quote(&serde_json::to_string(&spec)?),
    );
    Err(
        Command::new(crate::command::resolve_program(&root, &spec.target.runner))
            .args([
                format!("--jobid={}", spec.target.id),
                "--overlap".into(),
                "--export=ALL".into(),
                "--nodes=1".into(),
                "--ntasks=1".into(),
                "/bin/sh".into(),
                "-c".into(),
                bootstrap,
            ])
            .current_dir(root)
            .env_clear()
            .envs(environment)
            .exec(),
    )
    .context("Cannot start native Slurm step")
}

/// The verified compute helper reapplies the captured policy before runtime resolution.
pub fn compute(arguments: &[String], inside: bool) -> Result<()> {
    let spec = parse(arguments)?;
    ensure!(
        std::env::var("SLURM_JOB_ID").as_deref() == Ok(spec.target.id.as_str()),
        "Slurm did not supply the selected allocation; refusing a host runtime"
    );
    let root = crate::files::expand_home(&spec.root).canonicalize()?;
    if !inside {
        let argv = spec
            .prefix
            .into_iter()
            .chain([
                spec.helper,
                "--compute-runtime".into(),
                arguments[0].clone(),
            ])
            .collect::<Vec<_>>();
        return Err(crate::command::from_argv(&argv)?.current_dir(root).exec())
            .context("Cannot apply compute sandbox");
    }
    if spec.debugger {
        return crate::debugger::bridge::coordinator();
    }
    let python = crate::project::python_path(&root, &spec.python);
    Err(Command::new(&python)
        .args(["-u", "-c", include_str!("../notebook_kernel.py")])
        .current_dir(root)
        .envs(crate::project::environment_variables(
            &python.to_string_lossy(),
        ))
        .exec())
    .context("Cannot start compute notebook interpreter")
}
