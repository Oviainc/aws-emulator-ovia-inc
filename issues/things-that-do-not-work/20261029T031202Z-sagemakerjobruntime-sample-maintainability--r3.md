Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7a484987dc823baaa4ff3cdf8b9628f66a8b0eeb

# The crate's README states the opposite job-resolution rule from the one state.rs implements for the sampling path

_Filed: 2026-10-29T03:12:02Z_
_Operation: Sample | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7a484987dc823baaa4ff3cdf8b9628f66a8b0eeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The job-resolution rule of this data plane is written down TWICE inside this crate, and the two copies now contradict each other.

`src/state.rs` implements — and documents — resolution of the request's `JobArn` against the SHARED job directory the SageMaker control plane writes (`crate::state::job_exists` -> `JobDirectory::get_by_arn`), and the one sampling path refuses a well-formed ARN that resolves to no created job with `job_not_found_message` before any turn is captured.

`README.md`'s "Job resolution" section still describes the behaviour the crate had BEFORE that: it asserts that "the runtime keeps no registry of jobs to resolve it against", that every ARN the modelled constraints admit "names the job the call is served for", and that the only things "a sampling call IS refused on" are a sealed trajectory and the four store bounds — omitting job resolution entirely and denying that a registry exists.

So one copy of the rule describes a registry lookup that exists and is exercised by the suite, and the other denies it. The README copy is covered by no test, which is why it could drift; a reader (or a fixer) following it would conclude the `job_exists` pre-flight in `serve_sampling_turn` is wrong and remove it, reversing behaviour that is pinned by `tests/all/job_resolution.rs`.

The defect is the contradictory second statement of one rule, not any response: correcting the README leaves every response and every existing test byte-for-byte unchanged.

This is distinct from the two already-fixed `Sample` maintainability issues (duplicated sampling pre-flight; duplicated test harness), which were about duplicated CODE rather than a drifted statement of the rule.

## Evidence

Copy 1 — the rule as the code implements and documents it, `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/state.rs:19-24`:
```
//! The job itself is NOT a resource of this store. This service is the data plane
//! of a job the SageMaker control plane created: its model binds no operation that
//! creates, lists or deletes a job, so the job a call names is resolved against the
//! SHARED job directory ([`::service_common::agentic_rft_jobs::JobDirectory`]) that
//! the control plane writes when it mints one
```
the lookup itself, `src/state.rs:528-537`:
```
/// Whether `job_arn` names a job the SageMaker control plane created.
...
pub fn job_exists<Eng: Send + Sync + 'static>(engine: &Eng, job_arn: &str) -> bool {
    job_directory(engine)
        .is_some_and(|directory| directory.lock().get_by_arn(job_arn).is_some())
}
```
and the sampling refusal that uses it, `src/state.rs:711-715`:
```
    if !job_exists(engine, request.job_arn) {
        return Err(SamplingRefusal::invalid(job_not_found_message(
            request.job_arn,
        )));
    }
```

Copy 2 — the same rule as the crate's README states it, `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/README.md:109-124`:
```
## Job resolution
...
execution was handed, and the runtime keeps no registry of jobs to resolve it
against: every value the member's modelled constraints admit (`@length` 1..256 and
the ARN `@pattern`, which allow any partition, region, account and job name) names
the job the call is served for, and a trajectory is keyed by the
`(JobArn, TrajectoryId)` pair.
...
What a sampling call IS refused on is state this service owns: a trajectory
`CompleteRollout` has already sealed, and the emulator's own store bounds
(`state::MAX_TURNS_PER_TRAJECTORY`, `MAX_TRAJECTORIES`, `MAX_TURN_BODY_BYTES`,
`MAX_RETAINED_BODY_BYTES`).
```
Compare `src/state.rs:33-36`, which lists job resolution first and only THEN this service's own state: "Beside that, a sampling call is refused on state this service owns: a trajectory `CompleteRollout` has sealed, and the emulator bounds below."

Test coverage of the implemented copy (and none of the README copy) — `tests/all/job_resolution.rs:73-75`:
```
/// RULE (the refused half): a well-formed `JobArn` that resolves to no created job is
/// refused `400 ValidationException` naming the ARN the request carried, on both sampling
/// operations, and nothing is written.
```
asserted at `tests/all/job_resolution.rs:131-139` (`(400, "ValidationException")`) and at `:167-172` on the whole sentence `format!("Could not find job with jobArn {arn}")`.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/README.md:109-124` ("Job resolution"): it states the runtime keeps no job registry and that a sampling call is refused only on a sealed trajectory and the four store bounds.
2. Read `src/state.rs:528-537` and `:711-715`: the sampling path resolves `JobArn` through `job_exists` -> `JobDirectory::get_by_arn` and refuses an ARN that resolves to no created job.
3. Run `cargo test -p service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice job_resolution`: `a_sampling_call_for_a_job_that_was_never_created_is_refused` passes, i.e. the behaviour the README denies is the pinned behaviour.
4. Conclude the two statements of one rule disagree; align the README with `src/state.rs` (no handler change), and re-run the suite: every existing test must still pass unchanged.
