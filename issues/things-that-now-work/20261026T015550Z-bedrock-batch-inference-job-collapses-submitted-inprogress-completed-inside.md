Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.model-invocation-job-lifecycle
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.model-invocation-job-lifecycle|run#16|AssertionError|assert started_running["status"] == "InProgress", (
Issue-Commit: 1030924aed157d95218fdb79c11a6b06e677df02

# Batch inference job collapses Submitted->InProgress->Completed inside ~1s, so no reader ever observes a running job (1 cause)

> _Note: this issue was identified against the code base at commit `1030924aed157d95218fdb79c11a6b06e677df02`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetModelInvocationJob

## Description
One root cause, in the bedrock control plane's model-invocation-job (batch inference) clock.

Cause 1 of 1: the emulator's batch-inference job never dwells in a running status, and a single settled read can cross two transitions at once, so `InProgress` is unobservable to any client.

AWS: Smithy `com.amazonaws.bedrock#ModelInvocationJobStatus` is an enum of `Submitted | Validating | Scheduled | InProgress | Completed | Stopping | Stopped | Failed | PartiallyCompleted | Expired`, and `GetModelInvocationJob.status` documents the progression ("Submitted - This job has been submitted to a queue for validation", "Validating - This job is being validated for the requirements described in Format and upload your batch inference data", and so on). `CreateModelInvocationJob` takes `inputDataConfig`/`outputDataConfig` and the job then actually invokes the model over every record in the input object; `GetModelInvocationJob` reports `totalRecordCount`/`processedRecordCount`/`successRecordCount` while it does so. Recorded behavior: a job submitted against a batch-capable model is observable in `InProgress` and stays non-terminal while the records are processed (the replayed check polls for up to 600s at 10s intervals precisely because a real submission passes through Submitted/Validating/Scheduled and then runs for minutes once InProgress), i.e. a client polling at a human interval sees a running job before it sees a terminal one.

Emulator: the job is minted `Submitted` with its move to `InProgress` due 500ms later (`MODEL_INVOCATION_JOB_BEGINS_AFTER = Duration::from_millis(500)`) and its move to `Completed` due a further 500ms after that (`MODEL_INVOCATION_JOB_COMPLETE_AFTER = Duration::from_millis(500)`), and `ModelInvocationJobRecord::settle` applies BOTH overdue transitions in the same call: the `in_progress_due_at` branch sets `InProgress` and immediately arms `completed_due_at = due + 500ms`, and the very next `if` in the same function sees that instant already past and overwrites the status with `Completed`. `Validating` and `Scheduled` are never assigned at all. Consequently every `GetModelInvocationJob` taken more than ~1s after the create reports `status = "Completed"` directly out of `Submitted`, with `endTime`/`lastModifiedTime` set to the due instant. The replayed check polled until the status left {Submitted, Validating, Scheduled} and got `Completed` on that first read: "AssertionError: job reached 'Completed' before Stop, expected InProgress".

Why: a client that relies on AWS's job timeline gets it wrong in both directions. Any code that waits for a job to become running before acting on it — monitoring `processedRecordCount` progress, stopping a job that is still running, reacting to a running job in a scheduler — never observes that state here, so those paths are unreachable and a test or operator runbook written against AWS reads the job as instantly finished. It also makes the entire non-terminal half of the modelled status enum dead: no caller can ever be served `Validating`, `Scheduled`, or `InProgress`, and `StopModelInvocationJob` on a running job (its documented Stopping/Stopped path) has no reachable precondition even once that handler is implemented (its stub is filed separately). Reported progress and `endTime` are likewise meaningless: the job claims to have run for 500ms whatever the input size.

Fix: the model-invocation-job clock in this crate's state, plus the initial status the create assigns. `settle` must materialise at most the transition that is actually due and must not let a single read carry a job from pre-start to terminal — arming the next transition's due instant must not make it immediately overdue within the same call — and the running dwell must be long enough that a client polling at a normal interval observes the job while it is running. The pre-start statuses the model documents (`Validating`, `Scheduled`) should be part of that progression rather than skipped.

Traces: failing check `bedrock.model-invocation-job-lifecycle` (GROUP state-machine). Code for the fixer: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/state.rs:1520 and :1526 (the two span constants) and :1618-1631 (`ModelInvocationJobRecord::settle`, where the second `if` overwrites the status the first one just set); services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_model_invocation_job.rs:319-321 (initial `Submitted` + `in_progress_due_at`); services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/get_model_invocation_job.rs:223 (the read that settles).

## Evidence
Expected (AWS / recorded): after CreateModelInvocationJob against a batch-capable model, GetModelInvocationJob reports a non-terminal status (Submitted -> Validating/Scheduled -> InProgress) and the job is observable in `InProgress` while its records are processed; only later does it reach `Completed`.

Actual (emulator): GetModelInvocationJob reports `status = "Completed"` on the first read taken after the ~1s of total dwell, having never been reported as `InProgress`. Replay output for bedrock.model-invocation-job-lifecycle:

  File "check_bedrock_model_invocation_job_lifecycle.py", line 60, in run
    assert started_running["status"] == "InProgress", (
  AssertionError: job reached 'Completed' before Stop, expected InProgress

The check polled `GetModelInvocationJob` every 10s for up to 600s waiting for any status outside {Submitted, Validating, Scheduled}; the first such status served was `Completed`.

Source of the collapse (state.rs): MODEL_INVOCATION_JOB_BEGINS_AFTER = 500ms, MODEL_INVOCATION_JOB_COMPLETE_AFTER = 500ms, and `settle(now)` evaluates `in_progress_due_at` and `completed_due_at` back to back, so the second branch fires on the instant the first branch just armed.

## Steps to Reproduce
1. Create an S3 bucket and a .jsonl input object, and an IAM role Bedrock can assume (as the check does).
2. bedrock:CreateModelInvocationJob with jobName, roleArn, a batch-capable modelId, inputDataConfig.s3InputDataConfig.s3Uri pointing at that object and outputDataConfig.s3OutputDataConfig.s3Uri.
3. Sleep more than 1 second, then call bedrock:GetModelInvocationJob with the returned jobArn.
4. Observe status == "Completed" (with endTime already set). Poll at any interval of 1s or more from the create onward: the job is never reported with status "InProgress" (nor "Validating"/"Scheduled"), so there is no window in which a running job can be observed or acted on.
