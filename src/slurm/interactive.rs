//! Allocations belong to native sessions. Interactive sessions end with their
//! srun shell; notebook reservations instead hold salloc until explicitly ended. The job
//! name is tied to a random session ID so reattachment and renaming retain the
//! association without saving another allocation database.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Last explicitly requested resources and installed launch programs, per project.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Allocation {
    /// Installed allocator or site wrapper, not a shell command line.
    pub allocator: String,
    /// Installed step launcher or site wrapper.
    pub runner: String,
    /// Site partition; empty uses the scheduler default.
    pub partition: String,
    /// Billing account; empty uses the scheduler default.
    pub account: String,
    /// Optional quality of service.
    pub qos: String,
    /// Optional reservation.
    pub reservation: String,
    /// Total GPU count; zero requests a CPU-only allocation.
    pub gpus: u32,
    /// Generic resources per node, e.g. gpu:2 or gpu:a100:2; empty requests no GRES.
    pub gres: String,
    /// Number of allocated nodes; the interactive shell runs on the first node.
    pub nodes: u32,
    /// CPUs for the interactive task.
    pub cpus: u32,
    /// Optional memory per node with Slurm units, e.g. 32G.
    pub memory: String,
    /// Scheduler time limit, e.g. 02:00:00.
    pub time: String,
    /// Optional node features, e.g. a100&fast.
    pub constraint: String,
}
impl Default for Allocation {
    fn default() -> Self {
        Self {
            allocator: "salloc".into(),
            runner: "srun".into(),
            partition: String::new(),
            account: String::new(),
            qos: String::new(),
            reservation: String::new(),
            gpus: 1,
            gres: String::new(),
            nodes: 1,
            cpus: 1,
            memory: "80".into(),
            time: "2".into(),
            constraint: String::new(),
        }
    }
}
/// A validated, literal launch command shared by the local and SSH adapters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Launch {
    /// Unique scheduler job name retained in the native session metadata.
    pub job_name: String,
    /// POSIX launcher including explicit scheduler and compute-shell arguments.
    pub argv: Vec<String>,
}
impl Allocation {
    /// Validate bounded form values before creating a terminal or saving settings.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=4096).contains(&self.nodes),
            "Nodes must be between 1 and 4096"
        );
        ensure!(
            (1..=65536).contains(&self.cpus),
            "CPUs must be between 1 and 65536"
        );
        ensure!(self.gpus <= 65536, "At most 65536 GPUs may be requested");
        for value in [&self.allocator, &self.runner] {
            ensure!(
                !value.trim().is_empty() && !value.trim().starts_with('-'),
                "Choose an installed salloc/srun executable or wrapper"
            );
        }
        for value in [
            &self.allocator,
            &self.runner,
            &self.partition,
            &self.account,
            &self.qos,
            &self.reservation,
            &self.gres,
            &self.memory,
            &self.time,
            &self.constraint,
        ] {
            ensure!(
                value.len() <= 1024 && !value.chars().any(char::is_control),
                "Slurm fields must be at most 1024 bytes and contain no control characters"
            );
        }
        time_limit(&self.time)?;
        memory_limit(&self.memory)?;
        Ok(())
    }
    /// Build one allocation and one interactive task. Reapply the prepared policy
    /// on the compute node: Slurm's remote execution does not inherit the login
    /// host's mount namespace. Missing compute-node sandboxing fails visibly.
    pub fn launch(&self, plan: &crate::sandbox::Plan, id: &str) -> Result<Launch> {
        self.launch_inner(plan, id, false)
    }
    /// Hold a reservation in a native session independently of interactive shells.
    /// Its view is a progress log; cancellation or ending the session releases it.
    pub fn reserve(&self, plan: &crate::sandbox::Plan, id: &str) -> Result<Launch> {
        self.launch_inner(plan, id, true)
    }
    fn launch_inner(&self, plan: &crate::sandbox::Plan, id: &str, reserve: bool) -> Result<Launch> {
        self.validate()?;
        ensure!(
            id.len() == 34
                && id.starts_with("s-")
                && id[2..].bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid allocation terminal ID"
        );
        let job_name = format!("rode-{id}");
        let mut options = vec![
            format!("--job-name={job_name}"),
            format!("--nodes={}", self.nodes),
            "--ntasks-per-node=1".into(),
            format!("--cpus-per-task={}", self.cpus),
        ];
        if self.gres.trim().is_empty() && self.gpus > 0 {
            options.push(format!("--gpus={}", self.gpus));
        }
        let memory = memory_limit(&self.memory)?;
        let time = time_limit(&self.time)?;
        for (key, value) in [
            ("partition", &self.partition),
            ("account", &self.account),
            ("qos", &self.qos),
            ("reservation", &self.reservation),
            ("gres", &self.gres),
            ("mem", &memory),
            ("time", &time),
            ("constraint", &self.constraint),
        ] {
            if !value.trim().is_empty() {
                options.push(format!("--{key}={}", value.trim()));
            }
        }
        let quote = crate::ssh::quote;
        let program = |value: &str| match value.trim().strip_prefix("~/") {
            Some(tail) => format!("\"$HOME\"/{}", quote(tail)),
            None => quote(value.trim()),
        };
        let mut step = vec![
            "--export=ALL".into(),
            "--nodes=1".into(),
            "--ntasks=1".into(),
            "--pty".into(),
        ];
        if plan.active {
            step.extend(plan.prefix.clone());
        }
        step.extend([
            "/bin/sh".into(), "-c".into(),
            "if [ -z \"${SLURM_JOB_ID:-}\" ]; then printf '%s\\n' 'RODE: Slurm did not provide SLURM_JOB_ID; refusing an unallocated shell.' >&2; exit 1; fi; exec \"${SHELL:-/bin/sh}\" -l".into(),
        ]);
        // Clear inherited job identity/GPU visibility. Keep site configuration/auth such
        // as SLURM_CONF and SLURM_JWT. The new values come from salloc/srun,
        // never from an editor-side guess or a separate squeue response.
        let command = if reserve {
            ["/bin/sh", "-c", "printf 'Notebook allocation %s is ready. Select it in the notebook Execution target.\\nClosing this persistent view keeps the reservation. Release it in Slurm controls when finished.\\n' \"$SLURM_JOB_ID\"; trap 'exit 0' HUP INT TERM; while :; do sleep 3600 & wait $!; done"]
                .iter().map(|v| quote(v)).collect::<Vec<_>>().join(" ")
        } else {
            format!(
                "{} {}",
                program(&self.runner),
                step.iter().map(|v| quote(v)).collect::<Vec<_>>().join(" ")
            )
        };
        let script = format!(
            "unset SLURM_JOB_ID SLURM_JOBID SLURM_STEP_ID SLURM_STEPID SLURM_CLUSTERS SLURM_STEP_GPUS SLURM_JOB_GPUS SLURM_GPUS_ON_NODE CUDA_VISIBLE_DEVICES ROCR_VISIBLE_DEVICES GPU_DEVICE_ORDINAL; exec {} {} {}",
            program(&self.allocator),
            options
                .iter()
                .map(|v| quote(v))
                .collect::<Vec<_>>()
                .join(" "),
            command,
        );
        Ok(Launch {
            job_name,
            argv: vec!["/bin/sh".into(), "-c".into(), script],
        })
    }
}

/// The form follows the user's interactive helper: bare numbers mean hours,
/// H:MM adds minutes. Full Slurm day/hour/minute/second notation is also accepted.
fn time_limit(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    let (days, time) = match value.split_once('-') {
        Some((days, time)) => (days.parse::<u32>()?, time),
        None => (0, value),
    };
    let fields: Vec<u32> = time
        .split(':')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| anyhow::anyhow!("Use hours, H:MM, HH:MM:SS or D-HH:MM:SS for time"))?;
    ensure!(
        (1..=3).contains(&fields.len()),
        "Use hours, H:MM, HH:MM:SS or D-HH:MM:SS for time"
    );
    let hours = fields[0];
    let minutes = fields.get(1).copied().unwrap_or(0);
    let seconds = fields.get(2).copied().unwrap_or(0);
    ensure!(
        minutes < 60 && seconds < 60 && (days > 0 || hours > 0 || minutes > 0 || seconds > 0),
        "Time must be positive; minutes and seconds must be 0–59"
    );
    Ok(format!(
        "{}{hours}:{minutes:02}:{seconds:02}",
        if days == 0 {
            String::new()
        } else {
            format!("{days}-")
        }
    ))
}

/// Bare memory values mean GB, as in the user's helper. Explicit Slurm unit
/// suffixes remain available and an empty field leaves the site default intact.
fn memory_limit(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.bytes().all(|b| b.is_ascii_digit()) {
        ensure!(value.parse::<u64>()? > 0, "Memory must be positive");
        return Ok(format!("{value}G"));
    }
    Ok(value.into())
}
