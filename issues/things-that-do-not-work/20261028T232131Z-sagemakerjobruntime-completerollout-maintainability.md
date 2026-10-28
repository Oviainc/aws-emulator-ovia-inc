Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7f38edfb17ee58eefb2619194500340afad2c5d5

# The rollout pair's trajectory resolution is written out twice, once per handler, where the sampling pair shares one path

_Filed: 2026-10-28T23:21:31Z_
_Operation: CompleteRollout | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7f38edfb17ee58eefb2619194500340afad2c5d5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CompleteRollout` and `UpdateReward` are the two rollout-lifecycle operations of this crate, and both resolve the same thing before they touch the store: the `JobArn` against the shared job directory, then the `(JobArn, TrajectoryId)` pair against this service's own trajectory map, answering the same `ResourceNotFoundException` sentence for either miss. That resolution is written out twice — the same four statements, the same comment, the same `trajectory_not_found_message` fallback, differing only in which operation's `not_found` constructor wraps the sentence — so the two copies can drift on the order of the two misses, on the sentence, or on whether the store lock is taken before or after the job lookup.

The crate already states and implements the opposite discipline for its OTHER operation pair: the sampling pair reaches the store through one shared `crate::state::serve_sampling_turn`, and `state.rs` names that as the reason ("neither can drift from the other on which calls are refused, in which order, or with which sentence"). The job half of the rollout resolution is likewise already factored into one helper (`unresolved_job_trajectory_message`), whose own doc claims the sharing property; only the trajectory-lookup half of the same rule is left copied into both handlers. Folding those four statements into one `state.rs` helper that answers the sentence (each handler keeping only its own error constructor, exactly as the sampling pair keeps only its own error enum) leaves every response and every existing test unchanged.

## Evidence

Copy 1 — `src/ops/complete_rollout.rs:232-250`:
```
        // A trajectory belongs to the job its key names, so the `JobArn` is resolved
        // against the jobs the SageMaker control plane minted before this call touches a
        // stored trajectory: the crate's one job resolution, shared with the sampling
        // path, so the two cannot disagree about whether an ARN denotes a job.
        if let Some(message) = crate::state::unresolved_job_trajectory_message(
            &*self.engine,
            &input.job_arn,
            &input.trajectory_id,
        ) {
            return Err(not_found(message));
        }

        let now = ::std::time::SystemTime::now();
        let mut guard = state.lock();
        let Some(trajectory) = guard.trajectory_mut(&input.job_arn, &input.trajectory_id) else {
            return Err(not_found(crate::state::trajectory_not_found_message(
                &input.trajectory_id,
            )));
        };
```
Copy 2 — `src/ops/update_reward.rs:225-243` is the same text, statement for statement and comment for comment:
```
        // A trajectory belongs to the job its key names, so the `JobArn` is resolved
        // against the jobs the SageMaker control plane minted before this call touches a
        // stored trajectory: the crate's one job resolution, shared with the sampling
        // path, so the two cannot disagree about whether an ARN denotes a job.
        if let Some(message) = crate::state::unresolved_job_trajectory_message(
            &*self.engine,
            &input.job_arn,
            &input.trajectory_id,
        ) {
            return Err(not_found(message));
        }

        let now = ::std::time::SystemTime::now();
        let mut guard = state.lock();
        let Some(trajectory) = guard.trajectory_mut(&input.job_arn, &input.trajectory_id) else {
            return Err(not_found(crate::state::trajectory_not_found_message(
                &input.trajectory_id,
            )));
```
The discipline the crate states for its other pair — `src/state.rs:629-635`:
```
/// The two operations reach the SAME state through this, so one trajectory can
/// mix turns served by either of them and neither can drift from the other on
/// which calls are refused, in which order, or with which sentence. What is
/// left to each of them is what genuinely differs: the response body it renders
/// (`respond`), the `Content-Type` that body carries, the `streamed` flag the
/// captured turn records, and the modelled error enum it reports a
/// [`SamplingRefusal`] through.
```
and the half-shared claim for THIS pair — `src/state.rs:554-555`:
```
/// `CompleteRollout` and `UpdateReward` both take their job resolution from here, so
/// neither can disagree with the sampling path about whether an ARN denotes a job.
```
Test coverage that pins the behaviour a fold must not change: `tests/all/trajectory_lifecycle.rs:923` `an_unknown_trajectory_is_not_found` (404 `ResourceNotFoundException` from both operations for an unknown pair, an unknown job and another Region's ARN), `tests/all/trajectory_lifecycle.rs:861` `an_unresolvable_or_unsealed_trajectory_is_answered_before_the_reward_count`, and `tests/all/rollout_operations_resolve_the_job.rs:472` `the_rollout_operations_serve_the_same_pair_while_the_job_resolves` (both operations before/after job deletion and re-creation).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/complete_rollout.rs` and read lines 232-250.
2. Open `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/ops/update_reward.rs` and read lines 225-243: the same job resolution, the same store lock, the same `trajectory_mut` lookup and the same `trajectory_not_found_message` refusal, differing only in which operation's `not_found` wraps the sentence.
3. Compare with `src/state.rs:625-682`, where the OTHER operation pair (`Sample`, `SampleWithResponseStream`) shares one `serve_sampling_turn` precisely so the two "cannot drift from the other on which calls are refused, in which order, or with which sentence", and with `src/state.rs:545-567`, where only the job half of the rollout resolution is factored out.
4. Edit one copy (for example, move the store lock before the job lookup, or change the sentence) and observe that nothing forces the other copy to follow.
