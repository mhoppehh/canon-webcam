//! Detecting a **wedged camera** and recovering from it.
//!
//! A rejected property write can leave an M50 on the USB bus, still
//! auto-detected, while every operation returns `PTP Timeout` — that is the
//! observed **wedged camera** of `TEST-RESULTS.md` §4.8. Only two things clear
//! it: a **recovery reset** and a cable reconnect. This module owns the first: a
//! bounded number of `gphoto2 --reset` runs, then a full **PTP session** reopen.
//!
//! # Why a new process is correct here, and only here
//!
//! `docs/adr/0001-single-ptp-session.md` forbids a second `gphoto2` process beside
//! the one session of a run, because the *second* process is what produced the
//! rejected property writes that wedge the camera. A **recovery reset** is not
//! that second process: it destroys the wedged session and returns the camera to
//! the USB bus, and the session it destroys is released before the reset runs.
//! So this is the one place in the program where starting a process is the
//! correct thing to do, and every other camera operation still goes through the
//! one session.
//!
//! # What this module never does
//!
//! It writes no camera property, and it has no way to: the **output setting** is
//! only ever written by [`PtpSession::set_output_setting`], which takes a
//! [`crate::output_setting::OutputSetting`] and therefore cannot carry a value
//! outside the allowlist. Recovery reopens the session and hands it back; the
//! caller re-applies the setting it already had, so recovery cannot re-wedge the
//! camera with a dangerous `output` value.

use std::error;
use std::fmt;
use std::process::Command;

use crate::session::{PtpSession, SessionError};

/// How many **recovery reset** attempts one wedged episode gets before the run
/// gives up and asks the user for a cable reconnect.
///
/// Bounded on purpose: a permanently wedged camera must end the process, never
/// produce a retry loop. Three is more than the one reset the manual test needed
/// (`TEST-RESULTS.md` §4.8), and few enough that a user waits seconds, not
/// minutes, for the answer.
pub const MAX_RECOVERY_RESET_ATTEMPTS: usize = 3;

/// The **recovery reset**: the operation that destroys a wedged PTP session and
/// returns the camera to the USB bus.
///
/// A trait rather than a bare command so that the recovery path is exercised
/// without a camera and without `gphoto2`: a test asserts the command the real
/// implementation builds, and drives the attempt counting through a fake.
pub trait RecoveryReset {
    /// Runs one recovery reset.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryResetError`] if the command could not run or reported
    /// failure.
    fn reset(&mut self) -> Result<(), RecoveryResetError>;

    /// The command this performs, exactly as a user would type it. Used in the
    /// log and in the failure message, so a report names the command that was
    /// tried.
    fn command_line(&self) -> String;
}

/// Why a **recovery reset** did not run, or ran and failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryResetError {
    detail: String,
}

impl RecoveryResetError {
    /// Builds an error from what went wrong.
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    /// What went wrong.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for RecoveryResetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl error::Error for RecoveryResetError {}

/// The recovery reset a real run performs: `gphoto2 --reset`.
///
/// This is the one place where starting a new process is correct; see the module
/// documentation and `docs/adr/0001-single-ptp-session.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRecoveryReset {
    program: String,
    args: Vec<String>,
}

impl CommandRecoveryReset {
    /// The `gphoto2 --reset` a real run performs.
    #[must_use]
    pub fn new() -> Self {
        Self {
            program: "gphoto2".to_owned(),
            args: vec!["--reset".to_owned()],
        }
    }

    /// Runs `program --reset` instead of `gphoto2 --reset`.
    ///
    /// For a `gphoto2` that is not on `PATH`, and for tests that must not touch a
    /// camera: the attempt counting and the exit-status mapping are the same
    /// either way.
    #[must_use]
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Self::new().args,
        }
    }

    /// The program and its arguments, in order.
    #[must_use]
    pub fn invocation(&self) -> (&str, &[String]) {
        (&self.program, &self.args)
    }
}

impl Default for CommandRecoveryReset {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryReset for CommandRecoveryReset {
    fn reset(&mut self) -> Result<(), RecoveryResetError> {
        let command_line = self.command_line();
        let output = Command::new(&self.program)
            .args(&self.args)
            .output()
            .map_err(|error| {
                RecoveryResetError::new(format!("could not run `{command_line}`: {error}"))
            })?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(RecoveryResetError::new(if stderr.is_empty() {
            format!("`{command_line}` exited with {}", output.status)
        } else {
            format!("`{command_line}` exited with {}: {stderr}", output.status)
        }))
    }

    fn command_line(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
    }
}

/// Opens the one **PTP session** of a run, and opens a fresh one after a
/// **recovery reset** destroys a wedged session.
///
/// The session cannot be reopened by the type that holds it — that would be a
/// second handle on a camera that has already refused to answer — so recovery
/// gets its sessions from here instead.
pub trait PtpSessionOpener {
    /// Opens a PTP session, or reports why it cannot.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::NoCamera`] for a **disconnected camera** and
    /// [`SessionError::AlreadyOpen`] if a session is still alive.
    fn open_session(&mut self) -> Result<Box<dyn PtpSession + Send>, SessionError>;
}

/// What one **recovery reset** attempt did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResetAttemptOutcome {
    /// The reset command itself did not run or did not succeed.
    ResetFailed {
        /// What the reset tool said.
        detail: String,
    },
    /// The reset succeeded but no PTP session could be opened afterwards.
    ReopenFailed {
        /// What libgphoto2 said.
        detail: String,
    },
    /// The reset succeeded and a fresh PTP session was opened.
    Recovered,
}

impl fmt::Display for ResetAttemptOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResetFailed { detail } => write!(formatter, "the reset command failed: {detail}"),
            Self::ReopenFailed { detail } => {
                write!(
                    formatter,
                    "the reset succeeded but reopening failed: {detail}"
                )
            }
            Self::Recovered => {
                formatter.write_str("the reset succeeded and a fresh PTP session was opened")
            }
        }
    }
}

/// One numbered **recovery reset** attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetAttempt {
    /// The wedged episode this attempt belongs to, counting from one.
    pub episode: usize,
    /// The attempt within its episode, counting from one.
    pub attempt: usize,
    /// What the attempt did.
    pub outcome: ResetAttemptOutcome,
}

impl fmt::Display for ResetAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "recovery reset attempt {} of episode {}: {}",
            self.attempt, self.episode, self.outcome
        )
    }
}

/// Every **recovery reset** a run performed, and what each one did.
///
/// Kept so a user report says how many resets ran and how they ended without a
/// debugger, which is the only way to diagnose a wedged camera the run could not
/// clear.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryLog {
    attempts: Vec<ResetAttempt>,
    episodes: usize,
    recoveries: usize,
}

impl RecoveryLog {
    /// The attempts, in the order they ran.
    #[must_use]
    pub fn attempts(&self) -> &[ResetAttempt] {
        &self.attempts
    }

    /// How many recovery resets ran across the whole run.
    #[must_use]
    pub fn attempts_used(&self) -> usize {
        self.attempts.len()
    }

    /// How many wedged episodes the run had.
    #[must_use]
    pub fn episodes(&self) -> usize {
        self.episodes
    }

    /// How many episodes a recovery reset cleared.
    #[must_use]
    pub fn recoveries(&self) -> usize {
        self.recoveries
    }

    /// Whether the run performed no recovery reset at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.attempts.is_empty()
    }

    /// The attempt that last ran, if any.
    #[must_use]
    pub fn last(&self) -> Option<&ResetAttempt> {
        self.attempts.last()
    }

    fn push(&mut self, attempt: ResetAttempt) {
        self.episodes = self.episodes.max(attempt.episode);
        if attempt.outcome == ResetAttemptOutcome::Recovered {
            self.recoveries += 1;
        }
        self.attempts.push(attempt);
    }
}

impl fmt::Display for RecoveryLog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.attempts.is_empty() {
            return formatter
                .write_str("recovery resets: none (the camera never stopped answering)");
        }
        writeln!(
            formatter,
            "recovery resets: {} attempts, {} wedged episodes, {} recovered",
            self.attempts.len(),
            self.episodes,
            self.recoveries
        )?;
        for attempt in &self.attempts {
            writeln!(formatter, "  {attempt}")?;
        }
        Ok(())
    }
}

/// Why a wedged camera stayed wedged through every bounded attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryFailure {
    attempts: usize,
    budget: usize,
    command_line: String,
    detail: String,
}

impl RecoveryFailure {
    /// How many recovery resets this episode attempted.
    #[must_use]
    pub fn attempts(&self) -> usize {
        self.attempts
    }

    /// How many recovery resets one run may perform in total.
    #[must_use]
    pub fn budget(&self) -> usize {
        self.budget
    }

    /// The command every attempt ran.
    #[must_use]
    pub fn command_line(&self) -> &str {
        &self.command_line
    }

    /// The last thing the reset tool or libgphoto2 said.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for RecoveryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the camera is still wedged: {} recovery resets in this episode and {} per run are all \
             it is allowed (`{}`); last attempt: {}",
            self.attempts, self.budget, self.command_line, self.detail
        )
    }
}

impl error::Error for RecoveryFailure {}

/// The bounded **recovery reset** sequence, and the log of what it did.
///
/// One `Recovery` serves a whole run, and its budget is a budget for the whole
/// run rather than for one episode. That is deliberate: a per-episode allowance
/// is not a bound at all if the camera can be reopened into a wedged state, which
/// is exactly what a permanently wedged camera does. A run that wedges twice with
/// a budget of three therefore has one reset left, and both episodes are in
/// [`Recovery::log`].
pub struct Recovery {
    reset: Box<dyn RecoveryReset>,
    budget: usize,
    log: RecoveryLog,
}

impl Recovery {
    /// A recovery that may run [`MAX_RECOVERY_RESET_ATTEMPTS`] resets in one run.
    #[must_use]
    pub fn new(reset: Box<dyn RecoveryReset>) -> Self {
        Self {
            reset,
            budget: MAX_RECOVERY_RESET_ATTEMPTS,
            log: RecoveryLog::default(),
        }
    }

    /// Replaces the run's budget. It is never zero: a recovery that may not try is
    /// a recovery that cannot recover.
    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        assert!(max_attempts >= 1, "a recovery must be allowed an attempt");
        self.budget = max_attempts;
        self
    }

    /// How many resets one run may perform in total.
    #[must_use]
    pub fn max_attempts(&self) -> usize {
        self.budget
    }

    /// Every attempt this run performed, in order.
    #[must_use]
    pub fn log(&self) -> &RecoveryLog {
        &self.log
    }

    /// Runs a bounded number of **recovery resets**, then a full **PTP session**
    /// reopen, and returns the reopened session.
    ///
    /// The caller must have released the wedged session first: a recovery reset
    /// destroys that session, it does not sit beside it.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryFailure`] once the run's budget is used up, which is the
    /// point at which only a cable reconnect is left.
    pub fn recover(
        &mut self,
        opener: &mut dyn PtpSessionOpener,
    ) -> Result<Box<dyn PtpSession + Send>, RecoveryFailure> {
        let episode = self.log.episodes + 1;
        let mut attempted = 0;
        let mut detail = format!(
            "the run's {} recovery resets were already used",
            self.budget
        );

        while self.log.attempts_used() < self.budget {
            attempted += 1;
            let outcome = match self.reset.reset() {
                Err(error) => {
                    detail = error.detail().to_owned();
                    ResetAttemptOutcome::ResetFailed {
                        detail: detail.clone(),
                    }
                }
                Ok(()) => match opener.open_session() {
                    Ok(session) => {
                        self.log.push(ResetAttempt {
                            episode,
                            attempt: attempted,
                            outcome: ResetAttemptOutcome::Recovered,
                        });
                        return Ok(session);
                    }
                    Err(error) => {
                        detail = error.to_string();
                        ResetAttemptOutcome::ReopenFailed {
                            detail: detail.clone(),
                        }
                    }
                },
            };

            self.log.push(ResetAttempt {
                episode,
                attempt: attempted,
                outcome,
            });
        }

        Err(RecoveryFailure {
            attempts: attempted,
            budget: self.budget,
            command_line: self.reset.command_line(),
            detail,
        })
    }
}

impl fmt::Debug for Recovery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Recovery")
            .field("budget", &self.budget)
            .field("log", &self.log)
            .finish_non_exhaustive()
    }
}
