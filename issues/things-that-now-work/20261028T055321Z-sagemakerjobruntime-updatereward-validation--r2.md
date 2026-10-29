Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6b193e67e38a36506ad31c413899507363d5dc88

# UpdateReward never resolves JobArn against the shared job directory, so it records rewards for a job the sampling path refuses

_Filed: 2026-10-28T05:53:21Z_
_Operation: UpdateReward | Class: validation_

> _Note: this issue was identified against the code base at commit `6b193e67e38a36506ad31c413899507363d5dc88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Like `CompleteRollout`, `UpdateReward::handle` resolves only the `(JobArn, TrajectoryId)` pair in this service's own trajectory map (update_reward.rs:220) and never calls the crate's single job-resolution helper `crate::state::job_exists`, which the state module documents as "the ONE job resolution this data plane performs, so no two of its operations can disagree about whether an ARN denotes a job".

Consequence: for an ARN that no longer denotes a job (the control plane deleted it — a state the crate's own tests construct with `delete_job`/`JobDirectory::forget`), a sampling call is refused with nothing written while `UpdateReward` still mutates the stored trajectory: it writes `rewards`, flips `status` to `RewardReceived`, stores the client token and bumps `updated_at` (update_reward.rs:265-268) and answers 200. The defect is that this handler performs no job resolution at all, not which refusal it should pick — the crate already contains both the sampling refusal naming the ARN and this handler's own `ResourceNotFoundException` for an unresolvable resource.

## Evidence

`src/state.rs:523-527` states the invariant:
```
/// This is the ONE job resolution this data plane performs, so no two of its operations
/// can disagree about whether an ARN denotes a job: a job exists only because a create
/// minted one, and this API has no call that creates one.
pub fn job_exists<Eng: Send + Sync + 'static>(engine: &Eng, job_arn: &str) -> bool {
```
Its single caller is the sampling path, `src/state.rs:635-639`:
```
    if !job_exists(engine, request.job_arn) {
        return Err(SamplingRefusal::invalid(job_not_found_message(
            request.job_arn,
        )));
```
`grep -n 'job_exists' services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src` matches only `src/state.rs`; `src/ops/update_reward.rs` contains no call.

This handler resolves the trajectory only — `src/ops/update_reward.rs:220-227`:
```
        let Some(trajectory) = guard.trajectory_mut(&input.job_arn, &input.trajectory_id) else {
            return Err(UpdateRewardErrorModeled::ResourceNotFoundException(
```
and then writes, with no job check — `src/ops/update_reward.rs:265-268`:
```
                trajectory.rewards = Some(input.rewards.clone());
                trajectory.status = crate::state::TrajectoryStatus::RewardReceived;
                trajectory.update_reward_token = input.client_token.clone();
                trajectory.updated_at = now;
```

The deleted-job state is the crate's own test fixture — `tests/all/job_resolution.rs:131-136` (`fn delete_job`, "as the control plane's delete does") and the row at `tests/all/job_resolution.rs:218-222` ("the job was created and then deleted"), exercised against `Sample`/`SampleWithResponseStream` only. `UpdateReward` tests (`tests/all/trajectory_lifecycle.rs:983-1012`, `1085-1098`) always create the job first, so the divergence is untested.

## Steps to Reproduce

1. Register job `arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/run-1` in the shared `JobDirectory`.
2. POST /sample for that ARN with `X-Amzn-SageMaker-Trajectory-Id: traj-1` — 200, one turn captured.
3. POST /complete-rollout with `{"TrajectoryId":"traj-1","Status":"ready"}` — 200, trajectory sealed.
4. Delete the job from the directory (`JobDirectory::forget`).
5. POST /sample for the same ARN — refused (state.rs:635 `job_exists`).
6. POST /update-reward with `{"TrajectoryId":"traj-1","Rewards":[1.0]}` and the same `JobArn` header — answers 200 and persists rewards plus `reward-received` status for an ARN that resolves to no job.
