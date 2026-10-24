Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# SampleWithResponseStream refuses every JobArn outside the two hardcoded fixture ARNs

_Filed: 2026-10-24T05:40:10Z_
_Operation: SampleWithResponseStream | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SampleWithResponseStream` shares `Sample`'s job gate and therefore the same divergence: `if !state.lock().knows_job(&input.job_arn) { return Err(validation(job_not_found_message(..))) }` refuses with HTTP 400 `ValidationException` "Could not find job with jobArn …" unless the ARN is one of the two values `state::seed_job_arns()` seeds (`arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/agentic-rft-rollout-1` / `-2`).

Expected: a streaming sample call made with the ARN of the job the caller is running under returns 200 with a payload-chunk stream and appends a turn to the `(JobArn, TrajectoryId)` trajectory (model doc: "Returns the response as a stream of payload chunks. Each turn is captured for later use"); the only modelled restriction on `JobArn` is `length 1..256` plus the ARN pattern, which allows any partition/region/account.

Actual: only the two fixture ARNs can ever be streamed. The registry can be extended only by the in-process Rust helper `state::register_job`, which no HTTP request reaches, and the SageMaker control plane that would create a job (`CreateTrainingJob`) is still a 501 scaffold — so no reachable sequence of API calls makes a caller's own job known.

## Evidence

Spec: `SampleWithResponseStreamRequest.JobArn` carries only `length 1..256` + `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/…$` (OPERATIONS.md / smithy_query), and the operation doc describes serving a turn for the job named by the request. Handler: `src/ops/sample_with_response_stream.rs` lines ~167-176 (`knows_job` → `validation(job_not_found_message(..))`). State: `src/state.rs::seed_job_arns()` hardcodes the two fixtures and `register_job` is documented as standing in for "the control-plane creation this service models no operation for". Tests: `tests/all/trajectory_lifecycle.rs` calls the in-process `register_job(&engine, &job_arn)` before each streamed case, so no test covers a wire-only client streaming against its own job ARN. Cross-service: `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_training_job.rs` is `//! SCAFFOLDED, NOT IMPLEMENTED:` (501).

## Steps to Reproduce

1. POST /sample-with-response-stream with headers `X-Amzn-SageMaker-Job-Arn: arn:aws:sagemaker:us-west-2:123456789012:job/agentic-rft/run-1`, `X-Amzn-SageMaker-Trajectory-Id: traj-1` and a JSON chat payload body
2. Observe HTTP 400 `ValidationException` `Could not find job with jobArn …` instead of 200 with `Content-Type: text/event-stream` and a captured turn
3. Repeat with the seeded ARN `arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/agentic-rft-rollout-1` — 200, showing the result is decided purely by the hardcoded seed list
4. Verify no wire call can add a job ARN (no modelled job-creation operation here; SageMaker CreateTrainingJob answers 501)
