Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9e4273988d175f89e8a017aa0737ee7107d62a89

# Bootstrap the AgenticRFTRuntimeService (SageMaker job-runtime) emulator

_Filed: 2026-10-21T23:47:46Z_
_Operation: service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice | Class: stub_

> _Note: this issue was identified against the code base at commit `9e4273988d175f89e8a017aa0737ee7107d62a89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 4 modeled operations is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state. Implement a first, small, stateful happy-path slice covering the whole trajectory lifecycle of the service's primary resource (a trajectory within a job).

Happy path to implement, in order:

1. **Sample** — `POST /sample`. Inputs: `JobArn` (header `X-Amzn-SageMaker-Job-Arn`, required, must match the job ARN pattern), `TrajectoryId` (header `X-Amzn-SageMaker-Trajectory-Id`, required), `Body` (blob, `@httpPayload`, required). Output: `Body` (blob, `@httpPayload`, required) plus `ContentType` (header `Content-Type`). First Sample for a (JobArn, TrajectoryId) pair CREATES the trajectory in state with status `in-progress` and appends the request body as a turn; each later Sample appends another turn. The returned body must be produced by the emulator's model stub and RECORDED as part of the trajectory (so the turn count is observable), not simply echoed back without being stored.
2. **SampleWithResponseStream** — `POST /sample-with-response-stream`. Same inputs/outputs as Sample; same state effect (append a turn to the same trajectory). For the first slice it may deliver the response as a single payload, but it must go through the same state path so a trajectory can mix Sample and SampleWithResponseStream turns.
3. **CompleteRollout** — `POST /complete-rollout`. Inputs: `JobArn` (header, required), `TrajectoryId` (body, required), `Status` (`CompletionStatus`: `failed` | `ready`), `ClientToken` (`@idempotencyToken`). Empty output (200, no members). Must SEAL the trajectory: record the terminal completion status, reject further Sample turns on that trajectory, and make the trajectory eligible for reward submission. `@idempotent`: a repeat call with the same `ClientToken` and the same `Status` succeeds again; a second call that tries to change the sealed status must return `ConflictException` (409).
4. **UpdateReward** — `POST /update-reward`. Inputs: `JobArn` (header, required), `TrajectoryId` (body, required), `Rewards` (list<double>, required), `ClientToken` (`@idempotencyToken`). Empty output (200). Must store the rewards on the sealed trajectory and transition it to `reward-received`. Calling UpdateReward on a trajectory that has not been completed, or a second time with different rewards, must return `ConflictException` (409).

Unknown `JobArn`/`TrajectoryId` on CompleteRollout or UpdateReward must return `ResourceNotFoundException` (404). Missing required headers/payload, a `JobArn` that violates the modeled `^arn:aws[a-z\-]*:sagemaker:...:job/...$` pattern, or a `TrajectoryId` outside 1..256 characters must return `ValidationException` (400).

NEVER STUBS: each of the four handlers must compute its response from the shared engine state reached via `crate::state::service_state(&*self.engine)`. No canned 200s, no echoed bodies without persistence, no accepting a reward for a trajectory that was never created.

## State Model

In `src/state.rs`, on the service state struct:

- `trajectories: HashMap<(String /*JobArn*/, String /*TrajectoryId*/), Trajectory>`
- `struct Trajectory { job_arn: String, trajectory_id: String, status: TrajectoryStatus, turns: Vec<Turn>, completion_status: Option<CompletionStatus /* failed | ready */>, rewards: Option<Vec<f64>>, complete_rollout_token: Option<String>, update_reward_token: Option<String>, created_at: SystemTime, updated_at: SystemTime }`
- `enum TrajectoryStatus { InProgress, Complete, RewardReceived }`
- `struct Turn { request_body: Vec<u8>, request_content_type: Option<String>, response_body: Vec<u8>, response_content_type: Option<String>, streamed: bool, index: usize, at: SystemTime }`
- Idempotency: `ClientToken` values recorded per trajectory (the `*_token` fields above) so a replayed `@idempotent` call returns 200 instead of 409, while a conflicting re-submission returns `ConflictException`.

Done = a client script can Sample twice against a fresh (JobArn, TrajectoryId), CompleteRollout it with `Status=ready`, UpdateReward with a reward list, and observe the correct 200/409/404/400 outcomes driven by persisted trajectory state across requests.

## Evidence

`services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/OPERATIONS.md` lists all 4 operations with the routes, required members and error sets quoted above (restJson1, sigv4 signing name `sagemaker`, endpoint prefix `job-runtime.sagemaker`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'` matches all four handlers — `src/ops/sample.rs`, `src/ops/sample_with_response_stream.rs`, `src/ops/complete_rollout.rs`, `src/ops/update_reward.rs` — each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). Lifecycle ordering is grounded in the model docs: CompleteRollout "Marks a rollout as complete ... the trajectory is sealed and eligible for reward submission via the UpdateReward operation"; UpdateReward "Updates the reward values for a trajectory and transitions it to reward-received status". Both declare `ConflictException` and carry `@idempotent` + `@idempotencyToken`, which is what grounds the 409-on-conflicting-resubmission requirement. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST /sample with headers `X-Amzn-SageMaker-Job-Arn: arn:aws:sagemaker:us-east-1:123456789012:job/rft-job/traj-run` and `X-Amzn-SageMaker-Trajectory-Id: traj-1` and a JSON payload body — expect 200 with a payload body and Content-Type header (currently 501).
2. POST /sample again for the same job/trajectory with a second payload — expect 200; the trajectory now holds 2 turns in state.
3. POST /sample-with-response-stream for the same job/trajectory — expect 200 and a third recorded turn.
4. POST /complete-rollout with `{"TrajectoryId":"traj-1","Status":"ready","ClientToken":"tok-1"}` and the job ARN header — expect 200 and the trajectory sealed.
5. Replay the identical /complete-rollout call with `ClientToken: tok-1` — expect 200 (idempotent); then POST /complete-rollout with `Status: failed` — expect 409 ConflictException.
6. POST /sample again for traj-1 after sealing — expect a 409/validation refusal, not a silently appended turn.
7. POST /update-reward with `{"TrajectoryId":"traj-1","Rewards":[0.5,1.0],"ClientToken":"tok-2"}` — expect 200 and the trajectory in `reward-received` with the rewards persisted; a second call with different rewards must return 409.
8. POST /update-reward or /complete-rollout for `TrajectoryId: never-sampled` — expect 404 ResourceNotFoundException.
9. POST /sample with a malformed `X-Amzn-SageMaker-Job-Arn` (e.g. `not-an-arn`) or with no payload body — expect 400 ValidationException.
