Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.reject-delete-nonempty-activity-in-use stepfunctions.send-task-failure-kms-access-denied stepfunctions.update-map-run-validation-exception
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.reject-delete-nonempty-activity-in-use|_wait_execution_terminal#24|Timeout|time.sleep(interval_s)
Issue-Key: stepfunctions.send-task-failure-kms-access-denied|_revoke_kms_crypto_access#11|Timeout|time.sleep(3)
Issue-Key: stepfunctions.update-map-run-validation-exception|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: 62a489da4fc45e0eadb1189eb41434873a830e66

# Step Functions activity tasks are never delivered to a worker (GetActivityTask returns no token), and UpdateMapRun rejects a no-field update with NotImplemented instead of ValidationException (2 causes)

> _Note: this issue was identified against the code base at commit `62a489da4fc45e0eadb1189eb41434873a830e66`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetActivityTask

## Description
Two independent net-new root causes in the Step Functions emulator, both grounded in failing behavioral checks that the replay report left unmarked (no ALREADY-FILED / OWNED marker).

Cause 1 of 2 — GetActivityTask never hands out a task token for a RUNNING execution whose Task state names an activity ARN, so every activity-worker scenario stalls.
Failing check ids: sfn.reject-delete-nonempty-activity-in-use, sfn.send-task-failure-kms-access-denied.
AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model) models GetActivityTask with input {activityArn (required, @length 1..256), workerName (@length 1..80)} and output {taskToken (@length 1..2048, @sensitive), input (@length 0..1048576, @sensitive)}; its modeled errors are ActivityDoesNotExist, ActivityWorkerLimitExceeded, InvalidArn and the three Kms* errors — there is no modeled way to report "no task" other than returning an output with taskToken absent. In AWS, once an execution enters a Task state whose Resource is an activity ARN, the task is enqueued on that activity and the next GetActivityTask long-poll (up to ~60 s) returns a non-empty taskToken plus the state input. The downstream behavior these checks grade, once a token exists, is: DeleteActivity is accepted (200, empty body — Smithy DeleteActivityOutput has no members, and its only modeled error is InvalidArn) even while a worker holds an outstanding token; a subsequent DescribeActivity on the deleted ARN is refused with HTTP 400 ActivityDoesNotExist (message example: "Activity Does Not Exist: '<activityArn>'"); the still-held token is refused on SendTaskSuccess with HTTP 400 TaskDoesNotExist (a modeled SendTaskSuccess error) rather than a fabricated success; and the execution still reaches a terminal FAILED status on its own TimeoutSeconds instead of hanging.
Emulator: neither check ever obtains a token. Both poll GetActivityTask in a loop (reject-delete-nonempty-activity-in-use issues a single long-poll; send-task-failure-kms-access-denied loops with a 65 s budget) and both are killed by the harness with "Check timed out after 60.0s" — i.e. GetActivityTask keeps answering with no taskToken for the whole window, so the activity-task queue is never populated by the interpreter entering the Task state. Nothing after the poll is ever exercised.
Why: a client that implements the standard activity-worker pattern (create activity -> state machine Task on the activity ARN -> worker long-polls GetActivityTask -> SendTaskSuccess/SendTaskFailure) never receives work. The worker loop spins forever and the execution never leaves RUNNING, so an entire first-class Step Functions integration model is unusable against the emulator, and no error is surfaced to tell the client why.
Fix: the activity task scheduling path — the interpreter's Task-state handler must enqueue a task against the activity when the Resource is an activity ARN, and GetActivityTask's handler/state must dequeue it and mint a task token; DeleteActivity and SendTaskSuccess must then resolve the outstanding token to ActivityDoesNotExist / TaskDoesNotExist respectively.
Related but NOT covering this: open issue 20261024T190039Z-sfn-step-functions-activities-and-teststate-diverge-task-states.md describes activity Task states not executing and GetActivityTask divergences, and 20261025T013638Z-sfn-sendtaskfailure-stub.md covers SendTaskFailure being an unimplemented stub; the replay report did not mark either of these checks as owned by them, so this cause is filed separately. Note also that send-task-failure-kms-access-denied additionally requires SendTaskFailure on a CUSTOMER_MANAGED_KMS_KEY state machine whose key policy denies kms:Decrypt to be refused with HTTP 400 KmsAccessDeniedException (never a silent fallback to AWS-owned encryption, and never an accepted unencrypted failure) — that assertion is unreachable until Cause 1 is fixed.
Traces: sfn.reject-delete-nonempty-activity-in-use (timeout at the GetActivityTask long-poll), sfn.send-task-failure-kms-access-denied (timeout in _poll_activity_task).

Cause 2 of 2 — UpdateMapRun called with only mapRunArn is refused with an unmodeled NotImplemented error code instead of ValidationException.
Failing check id: sfn.update-map-run-validation-exception.
AWS: Smithy models UpdateMapRun input as {mapRunArn (required, LongArn, @length 1..2000), maxConcurrency (default null, @range min 0), toleratedFailurePercentage (default null, @range 0..100), toleratedFailureCount (default null, @range min 0)} with modeled errors InvalidArn (400), ResourceNotFound (404) and ValidationException (400). When none of the three threshold members is supplied the update is a no-op shape and AWS refuses it with HTTP 400, error code ValidationException; the recorded message is "Either the max concurrency, the tolerated failure count, or the tolerated failure percentage must be specified" — that text is built from the operation's own "at least one of" rule, so the requirement is that rule (the exact wording is an example).
Emulator: the call is refused with error code "NotImplemented", which is not in UpdateMapRun's modeled error set at all, so the SDK surfaces an unmodeled code and the assertion `code == "ValidationException"` fails with `AssertionError: expected ValidationException, got NotImplemented`.
Why: a client cannot distinguish "my UpdateMapRun request was malformed" from "the service is broken". ValidationException is a modeled, retry-never client error; NotImplemented is unmodeled, so SDK error-shape matching and any `except sfn.exceptions.ValidationException` handler never fires, and concurrency/tolerance tuning of a running Distributed Map cannot be validated.
Fix: UpdateMapRun's handler/validation (services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/update_map_run.rs) — reject the request with ValidationException when none of maxConcurrency / toleratedFailureCount / toleratedFailurePercentage is present, before any not-implemented path is taken. The replay report also reports UpdateMapRun as an unimplemented scaffold stub blocking sfn.map-run-tolerated-failure-enforced and says an issue naming that operation was filed for this run; this check was NOT marked as covered by it, and the specific divergence here is the wrong error CODE on a validation refusal, so it is recorded here.
Traces: sfn.update-map-run-validation-exception.

## Evidence
sfn.reject-delete-nonempty-activity-in-use — verdict mismatch. Expected: GetActivityTask(activityArn=<activity>, workerName="sim2real-worker") returns a non-empty taskToken for the RUNNING execution; then DeleteActivity -> 200; DescribeActivity -> 400 ActivityDoesNotExist; SendTaskSuccess(held token) -> 400 TaskDoesNotExist; DescribeExecution -> status FAILED. Actual: "Error: Check timed out after 60.0s" — the long-poll never yielded a taskToken, so the assertion "worker never received a task token for the running execution" was never reached and nothing downstream was exercised.

sfn.send-task-failure-kms-access-denied — verdict mismatch. Expected: _poll_activity_task obtains a taskToken within 65 s, then SendTaskFailure after the CMK's crypto permissions are revoked -> 400 KmsAccessDeniedException. Actual: "Error: Check timed out after 60.0s" inside the GetActivityTask poll loop; SendTaskFailure was never reached.

sfn.update-map-run-validation-exception — verdict mismatch. Expected: UpdateMapRun(mapRunArn=<running map run>) with no threshold fields -> HTTP 400, error code ValidationException (message example "Either the max concurrency, the tolerated failure count, or the tolerated failure percentage must be specified"). Actual (from the replay traceback): File "check_sfn_update_map_run_validation_exception.py", line 228, in run / assert code == "ValidationException" / AssertionError: expected ValidationException, got NotImplemented.

## Steps to Reproduce
Cause 1:
1. iam:CreateRole with a trust policy for states.amazonaws.com.
2. sfn:CreateActivity(name=<act>) -> activityArn.
3. sfn:CreateStateMachine with definition {"StartAt":"WaitForWorker","States":{"WaitForWorker":{"Type":"Task","Resource":"<activityArn>","TimeoutSeconds":90,"End":true}}} and the role above.
4. sfn:StartExecution(stateMachineArn=<sm>, input={"k":"v"}).
5. sfn:GetActivityTask(activityArn=<activityArn>, workerName="worker"). Expect a non-empty taskToken; the emulator returns no token for the whole long-poll window.
6. (unreachable today) sfn:DeleteActivity -> 200; sfn:DescribeActivity -> 400 ActivityDoesNotExist; sfn:SendTaskSuccess(held token) -> 400 TaskDoesNotExist; sfn:DescribeExecution -> FAILED.
Replay: sfn.reject-delete-nonempty-activity-in-use and sfn.send-task-failure-kms-access-denied.

Cause 2:
1. Create an execution-role allowing states:StartExecution/DescribeExecution/StopExecution on *.
2. sfn:CreateStateMachine with a Distributed Map state whose ItemProcessor waits ~10 s per item at MaxConcurrency 1.
3. sfn:StartExecution with {"items":[1,2]} and obtain the RUNNING mapRunArn.
4. sfn:UpdateMapRun(mapRunArn=<mapRunArn>) with no maxConcurrency / toleratedFailureCount / toleratedFailurePercentage.
5. Observe the error code: expected ValidationException (HTTP 400); the emulator answers NotImplemented.
Replay: sfn.update-map-run-validation-exception.
