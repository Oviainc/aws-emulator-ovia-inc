Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6b193e67e38a36506ad31c413899507363d5dc88

# CompleteRollout never resolves JobArn against the shared job directory the sampling path resolves it against

_Filed: 2026-10-28T05:53:21Z_
_Operation: CompleteRollout | Class: validation_

> _Note: this issue was identified against the code base at commit `6b193e67e38a36506ad31c413899507363d5dc88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate holds exactly one job-resolution helper, `crate::state::job_exists`, and its own doc-comment claims it is the single resolution point for the whole data plane so that "no two of its operations can disagree about whether an ARN denotes a job". In fact only the sampling path calls it (`serve_sampling_turn`, state.rs:635). `CompleteRollout::handle` goes straight to the trajectory map keyed by `(JobArn, TrajectoryId)` (complete_rollout.rs:227) and never asks whether the `JobArn` the request carries still denotes a job.

Consequence: the two halves of the crate disagree about the same ARN. After the control plane deletes a job (the crate's own tests model that with `delete_job`, which calls `JobDirectory::forget`), a `Sample` for that ARN is refused and nothing is written, while `CompleteRollout` for the SAME ARN happily mutates the stored trajectory — it sets `status = Complete`, `completion_status`, the client token and `updated_at` (complete_rollout.rs:240-243) — and answers 200 for a job that no longer exists. The defect is that no job resolution happens at all on this path; which modelled refusal it should produce is already settled elsewhere in the crate (the sampling path's refusal naming the ARN, or the sibling `ResourceNotFoundException` these two handlers already use for an unresolvable resource).

## Evidence

The crate states the invariant it then breaks:

`src/state.rs:523-527`:
```
/// This is the ONE job resolution this data plane performs, so no two of its operations
/// can disagree about whether an ARN denotes a job: a job exists only because a create
/// minted one, and this API has no call that creates one. An engine holding no directory
/// at all holds no job either, so nothing resolves.
pub fn job_exists<Eng: Send + Sync + 'static>(engine: &Eng, job_arn: &str) -> bool {
```

The only caller, on the sampling path — `src/state.rs:635-639`:
```
    if !job_exists(engine, request.job_arn) {
        return Err(SamplingRefusal::invalid(job_not_found_message(
            request.job_arn,
        )));
```
`grep -n 'job_exists' services/.../src` matches only `src/state.rs` (lines 307 comment, 527 definition, 613 doc, 635 call) — neither `src/ops/complete_rollout.rs` nor `src/ops/update_reward.rs` calls it.

This handler's only resolution — `src/ops/complete_rollout.rs:227-234`:
```
        let Some(trajectory) = guard.trajectory_mut(&input.job_arn, &input.trajectory_id) else {
            return Err(CompleteRolloutErrorModeled::ResourceNotFoundException(
```
then, with no job check, it writes: `src/ops/complete_rollout.rs:240-243`:
```
                trajectory.status = crate::state::TrajectoryStatus::Complete;
                trajectory.completion_status = Some(requested);
                trajectory.complete_rollout_token = input.client_token.clone();
                trajectory.updated_at = now;
```

A deleted job is a state the crate's tests already construct — `tests/all/job_resolution.rs:129-136`:
```
/// Delete the job `category` / `name` from the shared directory, as the control plane's
/// delete does.
fn delete_job(engine: &engine::InMemoryEngine, category: &str, name: &str) {
```
and `tests/all/job_resolution.rs:218-222` drives the row "the job was created and then deleted" through `Sample` only, asserting a refusal with nothing written. Test coverage for `CompleteRollout` (`tests/all/trajectory_lifecycle.rs:1045`, `tests/all/modelled_rejections.rs:221`) always creates the job first (`create_job`) or omits it only for a trajectory that does not exist either, so no test pins the deleted-job case for this operation.

## Steps to Reproduce

1. Register a job in the shared `JobDirectory` the way the control plane does (`tests/all/job_resolution.rs::create_job`) for `arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/run-1`.
2. POST /sample with `X-Amzn-SageMaker-Job-Arn: <that arn>`, `X-Amzn-SageMaker-Trajectory-Id: traj-1` and a JSON payload — 200, one turn captured.
3. Delete the job from the directory (`JobDirectory::forget`, as `tests/all/job_resolution.rs::delete_job` does).
4. POST /sample again for the same pair — refused, nothing written, because `serve_sampling_turn` calls `job_exists` (state.rs:635).
5. POST /complete-rollout with the same `JobArn` header and `{"TrajectoryId":"traj-1","Status":"ready"}` — answers 200 and seals the stored trajectory, even though the ARN now resolves to no job: the two operations disagree about the same ARN.
