Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d034a3e938ef16cd07dd30892ca1ec04abab6413

# Trajectories are keyed by the raw JobArn string while job identity ignores partition/Region/account, so one job gets several trajectory namespaces and CompleteRollout's seal is bypassable

_Filed: 2026-10-29T01:01:04Z_
_Operation: Sample | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d034a3e938ef16cd07dd30892ca1ec04abab6413`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate resolves a `JobArn` through the shared job directory, which deliberately treats only the ARN's `<category>/<name>` resource path as the job's identity: two ARNs that differ in partition, Region or account resolve to the SAME job record, and the crate's own test prose states that rule ("an emulator serves one account in one Region, so a job it created resolves however a caller addresses it"). The trajectory store, however, keys a trajectory by the RAW `JobArn` string the request happened to carry (`crate::state::append_turn`). The two notions of job identity disagree inside one request path, with three observable consequences for one and the same job:

1. `Sample`/`SampleWithResponseStream` calls for one rollout that address the job through two equivalent ARNs (e.g. the minted `us-east-1` ARN and the same job addressed in `eu-west-1`) are split into two separate trajectories, each restarting at turn index 0 — so `TrajectoryId` ("for grouping turns into a single rollout") no longer groups the rollout's turns.
2. The seal `CompleteRollout` writes is bypassable: after `CompleteRollout` seals (job ARN spelling A), a `Sample` for the same job and the same `TrajectoryId` under spelling B is answered 200 and a turn is captured, although the model says "no further turns will be appended to the trajectory". The handler's refusal path (`AppendRefusal::Sealed`) is simply never consulted, because the lookup key differs.
3. `CompleteRollout`/`UpdateReward` answer 404 `ResourceNotFoundException` ("Trajectory <id> not found") for a trajectory of an existing, resolvable job, purely because the caller spelled the job's ARN with another Region/account than the sampling calls did.

Note on existing coverage: `tests/all/trajectory_lifecycle.rs` pins the current keying, but its stated reason is "the same trajectory id under a different job is a different trajectory", and the row it applies that reason to is NOT a different job — `job/AgentRFT/run-a` addressed in `us-east-1` and in `eu-west-1` register (and resolve) as the one directory record `AgentRFT/run-a`. So the behaviour above is pinned by a reason that does not cover it, and it contradicts the model sentence quoted below.

## Evidence

Raw-ARN trajectory key — services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/state.rs:314-317:
```
        let key = (
            request.job_arn.to_string(),
            request.trajectory_id.to_string(),
        );
```
(and `pub type TrajectoryKey = (String, String);`, state.rs:222)

Job identity, as the directory this crate resolves against defines it — services/common/src/agentic_rft_jobs.rs:58-64: "The `<category>/<name>` pair this record is keyed by: the resource path of the job's ARN, which is the only part of an ARN that identifies a job rather than the partition, Region or account it was addressed through." and :112-114: "Two ARNs that differ only in partition, Region or account collapse onto one record, which is correct for an emulator serving a single account and Region." — reached from this crate at state.rs:531-534 (`job_exists` -> `directory.lock().get_by_arn(job_arn)`).

This crate's own statement that a trajectory belongs to the resolved job — src/state.rs:28-30: "`CompleteRollout` and `UpdateReward` because a trajectory lives inside the job its key names, so the pair names no trajectory of it either".

This crate's own test prose on spelling-independent resolution — tests/all/job_resolution.rs:184-185: "// one Region, so a job it created resolves however a caller addresses it." (the row "addressed in another region" is served 200).

Smithy contract that the bypass breaks — `smithy_query(query="doc",shape="CompleteRollout")`: "Marks a rollout as complete, indicating that no further turns will be appended to the trajectory. After calling this operation, the trajectory is sealed and eligible for reward submission via the UpdateReward operation." The sealed refusal that is meant to enforce it — src/state.rs:321-327:
```
        if let Some(existing) = self.trajectories.get(&key) {
            if existing.status != TrajectoryStatus::InProgress {
                return Err(AppendRefusal::Sealed {
```

Existing coverage whose stated reason does not cover the same-job case — tests/all/trajectory_lifecycle.rs:363-371:
```
/// RULE: a trajectory is keyed by the `(JobArn, TrajectoryId)` PAIR, so the same
/// trajectory id under a different job is a different trajectory.
...
    let first = job_arn("aws", "us-east-1", "123456789012", "AgentRFT", "run-a");
    let third = job_arn("aws", "eu-west-1", "123456789012", "AgentRFT", "run-a");
```
Both rows are registered with `create_job_at_arn`, which takes "its category and name out of the ARN's own resource path" (tests/all/harness.rs:214-224), i.e. both are the single job `AgentRFT/run-a` — not "a different job" — yet the test asserts three distinct trajectories for the shared id.

## Steps to Reproduce

1. Mount the service and register one job with the control-plane directory: category `AgentRFT`, name `run-a` (ARN `arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/run-a`).
2. POST /sample with `X-Amzn-SageMaker-Job-Arn: arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/run-a`, `X-Amzn-SageMaker-Trajectory-Id: traj-1` and an OpenAI-style JSON payload — 200, one turn captured.
3. POST /complete-rollout with the same ARN, `{"TrajectoryId":"traj-1","Status":"ready","ClientToken":"tok"}` — 200, the trajectory is sealed.
4. POST /sample again with `traj-1` but the SAME job addressed as `arn:aws:sagemaker:eu-west-1:000000000000:job/AgentRFT/run-a` (which `JobDirectory::get_by_arn` resolves to the same job record): observed 200 with a new turn captured on a second, in-progress trajectory; expected the sealed refusal, since the model says no further turns are appended to the trajectory after CompleteRollout.
5. POST /update-reward with the `eu-west-1` spelling, `{"TrajectoryId":"traj-1","Rewards":[1.0]}`: it is answered against the second trajectory (and conversely, calling /complete-rollout or /update-reward with a spelling other than the sampling one yields 404 `ResourceNotFoundException` "Trajectory traj-1 not found" for a job that resolves).
