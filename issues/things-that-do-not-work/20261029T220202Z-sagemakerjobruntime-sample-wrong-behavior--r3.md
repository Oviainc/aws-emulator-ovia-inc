Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 066b7cda09b446bd4bd9be1c3ecfcbe16940769e

# The served completion envelope reports `model` from the request's ARN spelling, so turns of one trajectory disagree about the job that served them

_Filed: 2026-10-29T22:02:02Z_
_Operation: Sample | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `066b7cda09b446bd4bd9be1c3ecfcbe16940769e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::completion_envelope` builds the inference response body both sampling operations serve (and that every captured turn retains). Inside ONE envelope two per-job values are derived two incompatible ways:

* `"id"` goes through `completion_id`, which normalises the ARN to the JOB's identity via `trajectory_key`, explicitly "so the same turn always reports the same id however its call addressed the job";
* `"model"` is the raw `job_arn` the request header carried, un-normalised.

The crate states everywhere else that the partition, Region and account an ARN was addressed through identify no job, and that per-job state must therefore be keyed by the identity rather than by the request's own string — which is why one trajectory groups turns served through ANY spelling of its job's ARN (`trajectory_key`, pinned by `tests/all/job_identity_keys_the_trajectory.rs`). The `model` field breaks that rule: the SAME trajectory of the SAME job reports a different `model` for each turn depending on which spelling of the job ARN that turn's call used, and the differing strings are persisted in `Turn::response_body`. Two turns of one rollout thus claim to have been served by two different models, while their ids correctly claim one trajectory of one job.

Every other place in `state.rs` that touches a `JobArn` either normalises it (`trajectory_key`, `Trajectory::new`, `completion_id`) or states a reason for using the raw request value (`job_not_found_message`: "the ARN the caller asked to be routed to is what the sentence names, so the refusal says which value could not be resolved"). `"model": job_arn` does neither — the function's own doc only lists `model` among the OpenAI fields it carries and says nothing about which spelling it should be.

## Evidence

Both sides are in this crate.

The raw spelling, inside the envelope:
- `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/state.rs:893-896`:
```
        "id": completion_id(job_arn, trajectory_id, index),
        "object": if streaming { "chat.completion.chunk" } else { "chat.completion" },
        "created": created_secs,
        "model": job_arn,
```

The rule the same function's other field obeys:
- `src/state.rs:838-845`: "Derived from the trajectory's identity — the JOB the call resolves to and the trajectory id, the pair the turn is stored under ([`trajectory_key`]) … so the same turn always reports the same id however its call addressed the job" / `let (job, trajectory_id) = trajectory_key(job_arn, trajectory_id);`
- `src/state.rs:233-241` (`trajectory_key`): "A trajectory belongs to the JOB a call resolves to, and this crate resolves a `JobArn` against the shared directory, which identifies a job by the `<category>/<name>` resource path of its ARN alone …: the partition, the Region and the account an ARN was addressed through identify no job, so they cannot distinguish one job's trajectories either. Keying by the identity rather than by the request's own string is what keeps the turns of one rollout in ONE trajectory however its calls address the job"
- `README.md:119-122`: "Which partition, Region and account an ARN was addressed through identifies no job, so a job the control plane created resolves however a caller addresses it, and a trajectory is keyed by the job that resolution identifies together with the request's `TrajectoryId` (`state::trajectory_key`)."
- a raw-ARN use that DOES state its reason, for contrast — `src/state.rs:566-572`: "Built from the request's own `JobArn`: the ARN the caller asked to be routed to is what the sentence names, so the refusal says which value could not be resolved."

The served body is the stored one, so the divergence is persisted:
- `src/state.rs:381-395`: `let served = ServedTurn { body: response_body.clone(), … }` then `trajectory.turns.push(Turn { … response_body, … })`.
- `src/ops/sample.rs:199-210` and `src/ops/sample_with_response_stream.rs:199-210` call `sample_response_body(&job_arn, …)` / `sample_stream_response_body(&job_arn, …)` with `let job_arn = input.job_arn.clone();` (the request header value).

Test coverage that drives exactly this scenario but only reads `id`:
- `tests/all/job_identity_keys_the_trajectory.rs:285-301` creates the job under two ARN spellings and samples `"shared-id"` through `first` then `later`; `:343-356` asserts only the completion `id` (`served_completion_id` at `:249-267` reads `document.get("id")`). No row reads `model`, so the spelling-dependent field is unpinned.

## Steps to Reproduce

1. Register one AgenticRFT job in the shared directory, e.g. `arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/rollout-1` (as the SageMaker control plane's create does).
2. POST `/sample` with `X-Amzn-SageMaker-Job-Arn: arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/rollout-1`, `X-Amzn-SageMaker-Trajectory-Id: shared-id` and an OpenAI-compatible body; note the response JSON's `id` and `model`.
3. POST `/sample` again for the SAME job addressed through another Region/account the modelled `@pattern` admits, e.g. `arn:aws:sagemaker:eu-west-1:123456789012:job/AgentRFT/rollout-1`, with the same `shared-id`.
4. Observe the second turn lands in the SAME trajectory (one trajectory, turn indices 0 and 1 — the behaviour `tests/all/job_identity_keys_the_trajectory.rs` pins) and its `id` is the id of turn 1 of that one trajectory, but its `model` is `arn:aws:sagemaker:eu-west-1:123456789012:job/AgentRFT/rollout-1` while turn 0's `model` is `arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/rollout-1`.
5. Read `trajectory.turns[*].response_body` from the store: the two retained bodies of one rollout disagree about the model/job that served them.
