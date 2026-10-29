Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.cfn-deploy-glue-job-eventbridge-sqs-stack-positive glue__sqs.cfn-glue-job-iam-role-trust-sqs-send-positive glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive glue__sqs.cli-start-glue-job-run-sqs-notification-positive glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative glue__sqs.glue-job-assumes-role-sends-sqs-message-positive glue__sqs.glue-job-role-with-sqs-send-permission-positive glue__sqs.glue-job-role-without-sqs-send-permission-negative glue__sqs.glue-job-state-change-detail-fields-preserved-in-sqs glue__sqs.glue-job-succeeded-eventbridge-to-sqs-positive glue__sqs.iam-role-missing-glue-trust-assume-role-negative glue__sqs.iam-role-trust-glue-amazonaws-com-positive glue__sqs.iam-role-trusts-glue-service-principal-positive glue__sqs.sdk-glue-job-missing-sqs-send-permission-negative glue__sqs.sdk-glue-job-run-writes-batch-messages-to-sqs-positive glue__sqs.sdk-glue-job-sqs-message-attributes-preserved glue__sqs.sdk-glue-job-wrong-queue-url-negative glue__sqs.cfn-deploy-sqs-queue-policy-missing-negative glue__sqs.eventbridge-rule-missing-sqs-resource-policy-negative glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative glue__sqs.glue-crawler-sqs-queue-policy-blocks-send-rejected glue__sqs.eventbridge-rule-missing-sqs-queue-policy-negative glue__sqs.eventbridge-rule-pattern-mismatch-glue-source-negative glue__sqs.glue-job-failed-eventbridge-to-sqs-positive
Issue-Key: glue__sqs.cfn-deploy-glue-job-eventbridge-sqs-stack-positive|run#13|AssertionError|assert final_state == "SUCCEEDED", f"job run ended in {final_state}"
Issue-Key: glue__sqs.cfn-glue-job-iam-role-trust-sqs-send-positive|run#15|AssertionError|assert final_state == "SUCCEEDED", f"job run ended in {final_state}"
Issue-Key: glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive|run#11|AssertionError|assert observed_running_without_message, "never observed the job in RUNNING state"
Issue-Key: glue__sqs.cli-start-glue-job-run-sqs-notification-positive|run#9|AssertionError|assert final_state == "SUCCEEDED", f"job run ended in {final_state}: {error_message}"
Issue-Key: glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive|run#19|AssertionError|assert job_run["JobRunState"] == "SUCCEEDED", job_run
Issue-Key: glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative|run#21|AssertionError|assert job_run["JobRunState"] == "SUCCEEDED", job_run
Issue-Key: glue__sqs.glue-job-assumes-role-sends-sqs-message-positive|run#11|AssertionError|assert final["JobRunState"] == "SUCCEEDED", f"unexpected terminal state: {final}"
Issue-Key: glue__sqs.glue-job-role-with-sqs-send-permission-positive|run#15|AssertionError|assert final["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__sqs.glue-job-role-without-sqs-send-permission-negative|run#17|AssertionError|assert "accessdenied" in error_message or "access denied" in error_message, (
Issue-Key: glue__sqs.glue-job-state-change-detail-fields-preserved-in-sqs|_run_job_until_succeeded#9|AssertionError|raise AssertionError(
Issue-Key: glue__sqs.glue-job-succeeded-eventbridge-to-sqs-positive|_run_job_to_success#9|AssertionError|raise AssertionError(
Issue-Key: glue__sqs.iam-role-missing-glue-trust-assume-role-negative|run#18|AssertionError|assert "assum" in bad_error.lower() or "not authorized" in bad_error.lower() or "sts:assumerole" in bad_error.lower(), (
Issue-Key: glue__sqs.iam-role-trust-glue-amazonaws-com-positive|run#6|AssertionError|assert final["JobRunState"] == "SUCCEEDED", f"unexpected terminal state: {final}"
Issue-Key: glue__sqs.iam-role-trusts-glue-service-principal-positive|run#17|AssertionError|assert final["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__sqs.sdk-glue-job-missing-sqs-send-permission-negative|run#10|AssertionError|assert allowed_final["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__sqs.sdk-glue-job-run-writes-batch-messages-to-sqs-positive|run#8|AssertionError|assert final["JobRunState"] == "SUCCEEDED", final.get("ErrorMessage")
Issue-Key: glue__sqs.sdk-glue-job-sqs-message-attributes-preserved|run#8|AssertionError|assert final["JobRunState"] == "SUCCEEDED", final.get("ErrorMessage")
Issue-Key: glue__sqs.sdk-glue-job-wrong-queue-url-negative|run#12|AssertionError|assert final["JobRunState"] == "SUCCEEDED", final.get("ErrorMessage")
Issue-Key: glue__sqs.cfn-deploy-sqs-queue-policy-missing-negative|_assert_no_message_arrives#12|AssertionError|raise AssertionError(
Issue-Key: glue__sqs.eventbridge-rule-missing-sqs-resource-policy-negative|run#11|AssertionError|assert len(blocked) == 0, (
Issue-Key: glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative|run#9|AssertionError|assert status in {"ROLLBACK_COMPLETE", "ROLLBACK_FAILED"}, (
Issue-Key: glue__sqs.glue-crawler-sqs-queue-policy-blocks-send-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: glue__sqs.eventbridge-rule-missing-sqs-queue-policy-negative|_collect_messages#12|Timeout|resp = sqs.receive_message(
Issue-Key: glue__sqs.eventbridge-rule-pattern-mismatch-glue-source-negative|_collect_messages#14|Timeout|resp = sqs.receive_message(
Issue-Key: glue__sqs.glue-job-failed-eventbridge-to-sqs-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# glue__sqs: 5 net-new causes — job runs never execute the script, EventBridge->SQS ignores the queue policy, a refused CFN resource does not roll back, and S3 destination validation is missing

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Five net-new root causes reached through the glue__sqs group. Related open issues that do NOT carry these replay keys: 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md, 20261027T003436Z-glue-sqs-an-on-demand-trigger-that-starts.md.

Cause 1 of 5: StartJobRun records a run that terminates FAILED without executing the job script, and the run never passes through RUNNING.
AWS: recorded behavior only for execution; the model fixes the surface (glue StartJobRun returns JobRunId; GetJobRun returns JobRunState over the lifecycle STARTING -> RUNNING -> SUCCEEDED, with ErrorMessage set only on a real failure). A successful run executes the script (so its side effects, e.g. SendMessage to the queue named in --QUEUE_URL, are observable) and ends SUCCEEDED with no ErrorMessage; a run whose role lacks the needed permission ends FAILED with an AccessDenied-shaped ErrorMessage naming the denied action.
Emulator: every run jumps straight to JobRunState='FAILED' with the emulator-authored ErrorMessage "This emulator records job runs but executes no job script, so the job run ended without running.", ExecutionTime=0, and RUNNING is never observed. Because of that, the role-permission negative checks also get this sentence instead of the AccessDenied reason they assert.
Why: nothing downstream of a Glue job can be exercised — no script side effects, no SUCCEEDED/FAILED discrimination, no "Glue Job State Change" event with a real state, and no way to tell a permission failure from an engine failure. The ErrorMessage is also emulator prose, so a client that parses failure reasons sees a sentence AWS never emits.
Fix: the glue job-run execution component behind StartJobRun (and the state machine GetJobRun reads) — a run must execute its script, advance through RUNNING, and only set ErrorMessage from a genuine failure.
Traces: glue__sqs.cfn-deploy-glue-job-eventbridge-sqs-stack-positive, glue__sqs.cfn-glue-job-iam-role-trust-sqs-send-positive, glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive, glue__sqs.cli-start-glue-job-run-sqs-notification-positive, glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive, glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative, glue__sqs.glue-job-assumes-role-sends-sqs-message-positive, glue__sqs.glue-job-role-with-sqs-send-permission-positive, glue__sqs.glue-job-role-without-sqs-send-permission-negative, glue__sqs.glue-job-state-change-detail-fields-preserved-in-sqs, glue__sqs.glue-job-succeeded-eventbridge-to-sqs-positive, glue__sqs.iam-role-missing-glue-trust-assume-role-negative, glue__sqs.iam-role-trust-glue-amazonaws-com-positive, glue__sqs.iam-role-trusts-glue-service-principal-positive, glue__sqs.sdk-glue-job-missing-sqs-send-permission-negative, glue__sqs.sdk-glue-job-run-writes-batch-messages-to-sqs-positive, glue__sqs.sdk-glue-job-sqs-message-attributes-preserved, glue__sqs.sdk-glue-job-wrong-queue-url-negative

Cause 2 of 5: an EventBridge rule's SQS target receives the event even when the destination queue has no policy granting events.amazonaws.com sqs:SendMessage.
AWS: recorded behavior only. EventBridge assumes no role for an SQS target; delivery is authorised purely by the queue's resource policy, so a queue with no such grant never receives the event (the attempt is counted as a failed invocation), while a sibling target whose queue does grant it does receive it.
Emulator: the unpolicied queue receives the full event envelope — the queue policy is not consulted at delivery time.
Why: this is an over-permissive delivery: events leak to queues the account never authorised, and every negative test that asserts "no policy, no delivery" passes vacuously, so a real IaC regression that drops the queue policy is undetectable.
Fix: the EventBridge target-delivery path for SQS targets — evaluate the destination queue's resource policy for events.amazonaws.com sqs:SendMessage before enqueuing.
Traces: glue__sqs.cfn-deploy-sqs-queue-policy-missing-negative, glue__sqs.eventbridge-rule-missing-sqs-resource-policy-negative

Cause 3 of 5: a stack whose AWS::Glue::Crawler resource is refused still reports CREATE_COMPLETE instead of rolling back.
AWS: recorded behavior only. When a resource provider refuses a create, CloudFormation marks that resource CREATE_FAILED and the stack rolls back to ROLLBACK_COMPLETE.
Emulator: the stack ends CREATE_COMPLETE even though the Crawler resource was refused.
Why: a deployment that must fail closed reports success, so a pipeline promotes a stack whose resources do not exist.
Fix: the CloudFormation resource-provisioning loop — a provider refusal must fail the resource and trigger the stack's rollback.
Traces: glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative

Cause 4 of 5: S3 PutBucketNotificationConfiguration accepts an SQS destination whose queue policy grants SendMessage to the wrong principal.
AWS: S3 validates destination permissions at configuration time and refuses with HTTP 400, Error.Code `InvalidArgument`, message fixed text "Unable to validate the following destination configurations" (the variable part is the destination ARN from the request).
Emulator: the call answers HTTP 200 with an empty body and persists the wiring.
Why: a notification path that can never deliver is accepted at configuration time, which is exactly what the AWS-side validation prevents; the misconfiguration surfaces later only as permanently lost events.
Fix: the S3 PutBucketNotificationConfiguration handler's destination-validation step.
Traces: glue__sqs.glue-crawler-sqs-queue-policy-blocks-send-rejected

Cause 5 of 5: three further glue->EventBridge->SQS checks never settle and are killed at the 60s harness budget, because no "Glue Job State Change" event with a real terminal state is ever published (a consequence that must be re-verified separately from cause 1, since these checks assert the absence/presence window rather than the run state).
AWS: recorded behavior only; a job run's terminal transition publishes a `Glue Job State Change` event on the default bus, which a matching rule forwards to its SQS target and a non-matching rule does not.
Emulator: the checks observe neither the positive delivery nor a bounded absence window, and time out.
Why: event-driven orchestration off Glue job completion cannot be built or tested at all.
Fix: the glue job-run state-transition publisher, plus the EventBridge delivery path named in cause 2.
Traces: glue__sqs.eventbridge-rule-missing-sqs-queue-policy-negative, glue__sqs.eventbridge-rule-pattern-mismatch-glue-source-negative, glue__sqs.glue-job-failed-eventbridge-to-sqs-positive

## Evidence
Cause 1 — e.g. `AssertionError: job run ended in FAILED: This emulator records job runs but executes no job script, so the job run ended without running.`; `AssertionError: never observed the job in RUNNING state`; `AssertionError: expected an AccessDenied failure reason, got: 'This emulator records job runs but executes no job script, so the job run ended without running.'`; GetJobRun payload shows JobRunState='FAILED', ExecutionTime=0, CompletedOn 0.5s after StartedOn.
Cause 2 — `AssertionError: expected no message on unpolicied queue, got 1: ['{"version":"0",...,"detail-type":"Glue Job State Change","source":"sim2real.glue.job",...}']`; `AssertionError: event was delivered to SQS despite the queue having no policy granting events.amazonaws.com sqs:SendMessage`.
Cause 3 — `AssertionError: expected the stack to roll back once the Crawler resource was refused, got CREATE_COMPLETE`.
Cause 4 — `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... 'HTTPStatusCode': 200, 'content-type': 'application/xml', 'content-length': '0' ...}` for s3.put_bucket_notification_configuration; the check asserts Error.Code 'InvalidArgument' and message 'Unable to validate the following destination configurations'.
Cause 5 — `Error: Check timed out after 60.0s`.

## Steps to Reproduce
Cause 1: CreateJob with a script in S3, StartJobRun, then poll GetJobRun — the run never reports RUNNING and lands JobRunState='FAILED' with the emulator-authored ErrorMessage; no script side effect (e.g. SendMessage to --QUEUE_URL) occurs.
Cause 2: PutRule on source 'aws.glue' with two SQS targets, one queue with a policy granting events.amazonaws.com sqs:SendMessage and one with no policy at all; PutEvents a matching event — both queues receive it.
Cause 3: CreateStack with an AWS::Glue::Crawler whose configuration the provider refuses — the stack ends CREATE_COMPLETE.
Cause 4: SetQueueAttributes with a Policy granting sqs:SendMessage to glue.amazonaws.com (not s3.amazonaws.com), then PutBucketNotificationConfiguration with that QueueArn — the call returns 200.
Cause 5: wire a rule from 'Glue Job State Change' to a queue, StartJobRun and poll the queue — nothing arrives within the window.
