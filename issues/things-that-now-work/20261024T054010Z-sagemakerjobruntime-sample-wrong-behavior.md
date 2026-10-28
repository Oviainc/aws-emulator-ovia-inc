Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# Sample only serves two hardcoded fixture job ARNs; any client-chosen JobArn is refused 400 with no way to register a job

_Filed: 2026-10-24T05:40:10Z_
_Operation: Sample | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Sample` gates every call on an in-memory job registry that is seeded with exactly two fixture ARNs and can only be extended by an in-process Rust call. `ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState::default()` fills `jobs` from `state::seed_job_arns()`, which returns `arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/agentic-rft-rollout-1` and `…-rollout-2` (built from `engine::emulator_arn`, i.e. account `000000000000`, region `us-east-1` only). The handler then refuses anything else: `if !state.lock().knows_job(&input.job_arn) { return Err(validation(job_not_found_message(..))) }` → HTTP 400 `ValidationException` "Could not find job with jobArn …".

Expected: an SDK/HTTP client that calls `Sample` with the ARN of the job it is running under gets 200 and a captured turn — the real runtime serves rollouts for whatever job exists in the caller's account/region, and the modelled `JobArn` constraint is only the ARN pattern `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/…$` (any account id, any region, any partition).

Actual: every well-formed JobArn other than the two fixtures is refused 400, and no reachable path can make a job known over the wire: `state::register_job` is a Rust API used only by the crate's own tests (`tests/all/trajectory_lifecycle.rs`, `tests/all/modelled_rejections.rs` both define a local `register_job(&engine, arn)` helper), and the SageMaker control-plane emulator cannot supply one either — `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_training_job.rs` is still a `//! SCAFFOLDED, NOT IMPLEMENTED:` 501 stub. The result is that the service's primary data path is usable only with two magic fixture ARNs that no real client would send (notably the repro ARN in this service's own bootstrap issue, `arn:aws:sagemaker:us-east-1:123456789012:job/rft-job/traj-run`, now answers 400 instead of the 200 that issue specifies). Either the registry should admit any job ARN the model's pattern allows on first use (creating the job/trajectory lazily, the way the trajectory itself is created on first turn) or it must be fed by a reachable registration path.

## Evidence

Spec: `SampleRequest.JobArn` (smithy_query shape SampleRequest) is constrained only by `length 1..256` + the ARN pattern, with doc "The job ARN that identifies which model session to route the inference request to"; `TrajectoryId` doc says "Each turn (prompt and response) is captured for later use" — nothing restricts the job to a server-side fixture list. Handler: `src/ops/sample.rs` lines ~170-174 (`knows_job` → `validation(job_not_found_message(..))`). State: `src/state.rs` `seed_job_arns()` (two `engine::emulator_arn("sagemaker", "us-east-1", "job/AgentRFT/…")` entries), `Default::default()` seeding `jobs`, and `register_job` documented as "Stands in for the control-plane creation this service models no operation for". Tests: `tests/all/trajectory_lifecycle.rs` reaches into the engine with `engine.state::<ServiceState>().lock().register_job(job_arn)` before every sampling case, i.e. no test exercises (and nothing asserts) a pure-HTTP client being able to sample its own job; `tests/all/modelled_rejections.rs::a_sampling_call_for_an_unknown_job_is_refused_as_invalid` only pins the negative case. Cross-service: `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_training_job.rs` is a 501 scaffold, so there is no control-plane source for job ARNs.

## Steps to Reproduce

1. Start the emulator and POST /sample with headers `X-Amzn-SageMaker-Job-Arn: arn:aws:sagemaker:us-east-1:123456789012:job/rft-job/traj-run`, `X-Amzn-SageMaker-Trajectory-Id: traj-1`, `Content-Type: application/json` and body `{"model":"rft-policy","messages":[{"role":"user","content":"hi"}]}`
2. Observe HTTP 400 `ValidationException` with message `Could not find job with jobArn arn:aws:sagemaker:us-east-1:123456789012:job/rft-job/traj-run` instead of 200 with an inference payload and a captured turn
3. Repeat the same request with `X-Amzn-SageMaker-Job-Arn: arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/agentic-rft-rollout-1` (one of the two seeded fixtures) — now it answers 200, showing the outcome depends solely on the hardcoded seed
4. Confirm no wire path can register a job: no operation in the model creates one, and SageMaker `CreateTrainingJob` answers 501
