//! Git operation admission shared by initial commands, confirmations and HTTPS retries.
//!
//! Literal arguments determine the operation's effect once, independently of UI
//! labels. The Git owner checks current readiness and dirty buffers immediately
//! before dispatch; a retry retains its operation and must pass the same checks.
use super::{GitJob, State, auth, project};

/// Effect on the editor's saved baseline. Unknown commands conservatively require
/// clean buffers; metadata-only operations remain usable during source editing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    Metadata,
    Baseline,
    Commit,
}
impl Effect {
    fn command(args: &[String]) -> Self {
        let command = args.first().map(String::as_str);
        let action = args.get(1).map(String::as_str);
        match (command, action) {
            (Some("commit"), _) => Self::Commit,
            (Some("stash"), Some("list" | "show"))
            | (Some("worktree"), Some("list" | "prune"))
            | (Some("submodule"), Some("status" | "sync")) => Self::Metadata,
            (
                Some(
                    "rev-parse" | "status" | "diff" | "show" | "log" | "for-each-ref" | "ls-files"
                    | "ls-remote" | "config" | "fetch" | "push" | "init" | "remote" | "branch"
                    | "tag",
                ),
                _,
            ) => Self::Metadata,
            _ => Self::Baseline,
        }
    }
}

#[derive(Clone)]
enum Invocation {
    Command {
        args: Vec<String>,
        input: Option<String>,
    },
    Stash {
        action: project::StashAction,
        reference: String,
        oid: String,
    },
}

/// Captured arguments, completion intent and admission requirements. Stash
/// operations retain both the displayed selector and immutable object identity.
#[derive(Clone)]
pub(super) struct Operation {
    invocation: Invocation,
    job: GitJob,
    effect: Effect,
}
impl Operation {
    pub(super) fn command(args: Vec<String>, mut job: GitJob, input: Option<String>) -> Self {
        let effect = Effect::command(&args);
        if let GitJob::Mutate { clear_message, .. } = &mut job {
            *clear_message = effect == Effect::Commit;
        }
        Self {
            invocation: Invocation::Command { args, input },
            job,
            effect,
        }
    }

    pub(super) fn mutation(args: Vec<String>, label: &str) -> Self {
        Self::command(
            args,
            GitJob::Mutate {
                label: label.into(),
                clear_message: false,
            },
            None,
        )
    }

    pub(super) fn stash(action: project::StashAction, reference: String, oid: String) -> Self {
        Self {
            invocation: Invocation::Stash {
                action,
                reference,
                oid,
            },
            job: GitJob::Mutate {
                label: match action {
                    project::StashAction::Apply => "Apply stash",
                    project::StashAction::Pop => "Pop stash",
                    project::StashAction::Drop => "Drop stash",
                }
                .into(),
                clear_message: false,
            },
            effect: Effect::Baseline,
        }
    }

    pub(super) fn begin_auth(
        &self,
        state: &mut auth::State,
        credentials: &[project::git_auth::Credentials],
    ) {
        match &self.invocation {
            Invocation::Command { args, input } => {
                state.begin(args, &self.job, credentials, input.is_none())
            }
            Invocation::Stash { .. } => state.begin(&[], &self.job, &[], false),
        }
    }

    pub(super) fn into_request(
        self,
        root: String,
        credentials: Vec<project::git_auth::Credentials>,
    ) -> (project::Request, GitJob) {
        let request = match self.invocation {
            Invocation::Command { args, input } if credentials.is_empty() => {
                project::Request::Git { root, args, input }
            }
            Invocation::Command { args, .. } => project::Request::GitAuthenticated {
                root,
                args,
                credentials,
            },
            Invocation::Stash {
                action,
                reference,
                oid,
            } => project::Request::GitStash {
                root,
                action,
                reference,
                oid,
            },
        };
        (request, self.job)
    }
}

/// Rejected admission leaves both the Git busy state and pending request unchanged.
pub(super) enum Admission {
    Busy,
    NotReady,
    Dirty,
}
impl Admission {
    pub(super) fn message(&self) -> &'static str {
        match self {
            Self::Busy => "Wait for the current Git request to finish.",
            Self::NotReady => "Wait for the project host and sandbox to be ready.",
            Self::Dirty => "Save or close modified editor buffers before this Git operation.",
        }
    }
}

impl State {
    /// Inspect current application facts, then acquire the one in-flight operation
    /// slot. Every initial operation and retry enters through this boundary.
    pub(super) fn admit(
        &mut self,
        operation: &Operation,
        ready: bool,
        dirty: bool,
    ) -> Result<(), Admission> {
        if self.git_busy {
            return Err(Admission::Busy);
        }
        if !ready {
            return Err(Admission::NotReady);
        }
        if dirty && operation.effect != Effect::Metadata {
            return Err(Admission::Dirty);
        }
        self.git_busy = true;
        self.committing = operation.effect == Effect::Commit;
        Ok(())
    }
}
