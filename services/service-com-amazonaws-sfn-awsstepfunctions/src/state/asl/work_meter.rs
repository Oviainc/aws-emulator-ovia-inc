// SPDX-License-Identifier: CC0-1.0

//! The WORK one interpretation may do, counted in units of the work itself.
//!
//! One concern, split out of [`super`]: the other ceilings there bound each FACTOR of
//! what a definition can ask for (its transitions, the bytes one state may carry, the
//! paths and rules one visit may evaluate), and this bounds their PRODUCT, so a
//! definition that stays inside every factor cannot ask for unbounded work by
//! multiplying them.
//!
//! ## Why it is work and not a clock
//!
//! What an execution ENDS AS is the definition's own business: a state's outcome, the
//! `TimeoutSeconds` the definition declares, or a `StopExecution`. A ceiling measured
//! on the wall clock would add one more — and one whose verdict depends on how fast
//! and how loaded the host is, so the same definition over the same input would end
//! `SUCCEEDED` on an idle machine and `FAILED` on a busy one. A ceiling on the work
//! itself is decided by the definition and the input alone, which is what makes a
//! run's outcome reproducible and leaves the clock to the definition.
//!
//! One unit is roughly one byte looked at: what a document the walk measures costs,
//! what matching a `StringMatches` pattern against a value costs, what evaluating a
//! path costs. ONE cost is not countable that way, and is the single exception: a
//! caller's own expression running inside the evaluation engine is caller CODE, not a
//! step of this interpreter, so what it costs is the span it ran for, charged at
//! [`WORK_PER_EVALUATED_MILLISECOND`] — the same clock the engine's own per-expression
//! ceiling already holds one expression to, and the reason a definition whose
//! expressions spend seconds inside the engine is bounded at all. Everything the
//! interpreter itself does is counted and never timed, so for a definition written in
//! the other query language the outcome of a run is decided by the definition and the
//! input alone.
//!
//! ## How it is reached
//!
//! The allowance is AMBIENT to the thread the walk runs on ([`WorkAllowance`]), rather
//! than threaded through every evaluator the language reaches: one walk runs to its end
//! on one thread, and the primitives that spend the work — measuring a document,
//! matching a glob, evaluating a path or an expression — are plain functions the walk
//! calls through several layers. A thread with no allowance installed is UNMETERED, so
//! the same primitives serve the create-time validation of a definition — which runs
//! no execution — without an allowance being invented for it. Every WALK installs one:
//! an execution's own, and the single-state trial `TestState` runs.

use std::cell::Cell;

use super::*;

/// The units of interpretation work ONE run of a definition may do.
///
/// Emulator quota, NOT an AWS one, and the companion of [`MAX_STATE_TRANSITIONS`]: it
/// is the ceiling on the product the other ceilings bound factor by factor. It is wide
/// enough for the heaviest run those factors allow — every transition carrying a
/// document the size of a state's maximum through a template that rewrites it — and
/// what it stops is a definition that multiplies them to ask for work no execution
/// does.
pub const MAX_INTERPRET_WORK: u64 = 4_000_000_000;

/// What one millisecond of EVALUATING a caller's own expression costs.
///
/// An expression runs inside the evaluation engine, where the work it does is the
/// caller's code rather than this interpreter's steps, so the span it took is what it
/// is charged for. The rate is set so that one run's whole allowance buys about two
/// seconds of expression evaluation — far more than a definition's expressions take,
/// and a bound on a definition written to spend the engine's own per-expression
/// ceiling over and over.
pub const WORK_PER_EVALUATED_MILLISECOND: u64 = 2_000_000;

thread_local! {
    /// What is LEFT of the work the interpretation running on THIS thread may do.
    ///
    /// [`u64::MAX`] — unmetered — on a thread no interpretation installed an allowance
    /// on: the primitives below are also what validates a definition at create time,
    /// which runs no execution. Every walk installs an allowance of its own, the
    /// single-state trial included.
    static LEFT: Cell<u64> = const { Cell::new(u64::MAX) };
}

/// The allowance one interpretation runs under, installed on the thread it runs on for
/// as long as the walk lasts.
///
/// The allowance in force when it was installed is restored when it is dropped — on the
/// error paths and while unwinding as much as at the end of a walk — so a blocking
/// thread that carries one walk after another gives each its own allowance, and a
/// nested interpretation cannot leave the outer one metered by what it spent.
#[derive(Debug)]
pub struct WorkAllowance {
    /// What was left of the allowance this one interrupted.
    restored: u64,
}

impl WorkAllowance {
    /// Install `units` of work for the interpretation about to run on this thread.
    pub fn of(units: u64) -> Self {
        Self {
            restored: LEFT.with(|left| left.replace(units)),
        }
    }
}

impl Drop for WorkAllowance {
    fn drop(&mut self) {
        LEFT.with(|left| left.set(self.restored));
    }
}

/// Charge `units` of work to the interpretation running on this thread, and answer
/// whether its allowance covered them.
///
/// A charge past the allowance spends what was left of it, so every later charge
/// answers `false` too: a run that has asked for more work than it may do does not get
/// another chance at a cheaper step.
pub fn spend_work(units: u64) -> bool {
    LEFT.with(|left| match left.get().checked_sub(units) {
        Some(remaining) => {
            left.set(remaining);
            true
        }
        None => {
            left.set(0);
            false
        }
    })
}

/// Whether the interpretation running on this thread has spent its allowance.
pub fn work_spent() -> bool {
    LEFT.with(Cell::get) == 0
}

/// What a run that asked for more work than [`MAX_INTERPRET_WORK`] fails with: the
/// quota it went past, stated the way the step quota beside it is.
pub fn out_of_work() -> Failure {
    runtime(format!(
        "the execution asked for more than the {MAX_INTERPRET_WORK} units of interpretation work \
         this emulator does for one definition, so it was stopped"
    ))
}
