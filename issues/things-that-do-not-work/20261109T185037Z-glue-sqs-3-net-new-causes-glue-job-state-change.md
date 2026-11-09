Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.cfn-deploy-glue-job-eventbridge-sqs-stack-positive glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative glue__sqs.glue-job-state-change-detail-fields-preserved-in-sqs glue__sqs.glue-job-succeeded-eventbridge-to-sqs-positive glue__sqs.iam-role-missing-glue-trust-assume-role-negative glue__sqs.sdk-glue-job-wrong-queue-url-negative
Issue-Key: glue__sqs.cfn-deploy-glue-job-eventbridge-sqs-stack-positive|_wait_for_matching_message#14|Timeout|resp = sqs.receive_message(
Issue-Key: glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive|run#21|AssertionError|assert message is not None, "expected the Glue Job State Change SUCCEEDED event on the queue"
Issue-Key: glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative|run#24|AssertionError|assert good_message is not None, "expected the event on the queue with a correct resource policy"
Issue-Key: glue__sqs.glue-job-state-change-detail-fields-preserved-in-sqs|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: glue__sqs.glue-job-succeeded-eventbridge-to-sqs-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: glue__sqs.iam-role-missing-glue-trust-assume-role-negative|run#16|AssertionError|assert bad_final["JobRunState"] == "FAILED", (
Issue-Key: glue__sqs.sdk-glue-job-wrong-queue-url-negative|run#18|AssertionError|assert error_bodies[0] == expected_report, (
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# 3 net-new causes: Glue Job State Change events never reach EventBridge, an untrusted job role is accepted, and the in-job SDK reports the wrong SQS error code

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Three distinct root causes in the glue__sqs suite. Each is listed with its own failing check ids; all three must be closed.

Cause 1 of 3 — no 'Glue Job State Change' event is ever published, so every rule routing job outcomes to SQS delivers nothing. AWS: a job run that reaches SUCCEEDED (or FAILED) publishes a Glue Job State Change event (source aws.glue) whose detail carries jobName, severity, state, jobRunId and message; a rule on that pattern delivers it to its SQS target, and the detail fields survive the hop verbatim. Emulator: nothing arrives — the checks report 'expected the Glue Job State Change SUCCEEDED event on the queue', 'expected the event on the queue with a correct resource policy', and three more end 'Check timed out after 180.0s' waiting for the same event (including the CloudFormation-deployed variant). Why: every Glue-driven pipeline that reacts to job completion is inert; a client cannot tell a job that never finished from a service that never events, and the negative check (missing target permission) cannot be distinguished from the positive one. Fix: Glue job-run state-change eventing in the Glue job-run state machine (publish to EventBridge on each terminal state). Traces: glue__sqs.cfn-deploy-glue-job-eventbridge-sqs-stack-positive, glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive, glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative, glue__sqs.glue-job-state-change-detail-fields-preserved-in-sqs, glue__sqs.glue-job-succeeded-eventbridge-to-sqs-positive.

Cause 2 of 3 — a job whose Role is not assumable by Glue still runs to SUCCEEDED. AWS: StartJobRun with a role whose trust policy does not allow glue.amazonaws.com to assume it fails the run (the run settles FAILED with the assume-role error); the trust relationship is enforced before the script runs. Emulator: the run reports SUCCEEDED. Why: an authorization defect is invisible in the emulator and fails only in production — exactly the class of mistake a trust-policy check exists to catch. Fix: role-assumption/trust evaluation in the Glue StartJobRun path. Traces: glue__sqs.iam-role-missing-glue-trust-assume-role-negative.

Cause 3 of 3 — the AWS SDK inside a Glue job reports SQS's nonexistent-queue refusal with the wrong code and message. AWS: a send to a queue URL that does not exist is refused with error code 'AWS.SimpleQueueService.NonExistentQueue' and message 'The specified queue does not exist for this wsdl version.' on the Query-protocol path that the in-job SDK uses (Smithy: QueueDoesNotExist carries awsQueryError code AWS.SimpleQueueService.NonExistentQueue, HTTP 400). Emulator: the job observes 'error:QueueDoesNotExist|The specified queue does not exist.' — the modeled shape name instead of the query error code, and the JSON-protocol message. Why: job code that branches on the documented error code never matches, so a recoverable 'queue missing' is handled as an unknown failure. Fix: the SQS error serializer for the Query/WSDL-era protocol used by the in-job SDK (emit the awsQueryError code and that protocol's message). Traces: glue__sqs.sdk-glue-job-wrong-queue-url-negative.

## Evidence
Cause 1: AssertionError: expected the Glue Job State Change SUCCEEDED event on the queue; AssertionError: expected the event on the queue with a correct resource policy; Error: Check timed out after 180.0s (cfn-deploy-glue-job-eventbridge-sqs-stack-positive, glue-job-state-change-detail-fields-preserved-in-sqs, glue-job-succeeded-eventbridge-to-sqs-positive)
Cause 2: AssertionError: expected job run with an untrusted role to fail, got SUCCEEDED
Cause 3: AssertionError: expected the job to observe 'error:AWS.SimpleQueueService.NonExistentQueue|The specified queue does not exist for this wsdl version.', got 'error:QueueDoesNotExist|The specified queue does not exist.'

## Steps to Reproduce
Cause 1: create a Glue job, a rule on {"source":["aws.glue"],"detail-type":["Glue Job State Change"]} with an SQS target, StartJobRun and poll the queue.
Cause 2: create a Glue job whose Role trust policy omits glue.amazonaws.com; StartJobRun and read the terminal JobRunState.
Cause 3: run a Glue job whose script sends to a nonexistent queue URL; read the error code and message the in-job SDK surfaces.
