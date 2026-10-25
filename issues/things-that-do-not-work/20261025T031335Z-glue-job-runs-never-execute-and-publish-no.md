Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.glue-job-authored-catalog-table-is-read-by-athena-and-a-partition-denied-role-fails athena__glue__s3.glue-job-failed-event-missing-target-permission-negative glue.reject-concurrent-run-over-limit glue.reject-get-nonexistent-job-run
Issue-Key: athena__glue__s3.glue-job-authored-catalog-table-is-read-by-athena-and-a-partition-denied-role-fails|run#15|AssertionError|assert good["JobRunState"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.glue-job-failed-event-missing-target-permission-negative|_wait_for_matching_message#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Key: glue.reject-concurrent-run-over-limit|run#10|AssertionError|assert message == "Concurrent runs exceeded.", message
Issue-Key: glue.reject-get-nonexistent-job-run|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 176bb95e3c08e52684921328b2da3d677bd84f34

# Glue job runs never execute and publish no Glue Job State Change event, and two job-run refusal messages are rewritten with request-derived text (4 causes)

> _Note: this issue was identified against the code base at commit `176bb95e3c08e52684921328b2da3d677bd84f34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Four root causes, grounded in four failing behavioral checks of suite simparity-behavioral-ga-prod-v1. All four are Glue's; they are bundled because they share the service and all concern the job-run surface.

Cause 1 of 4 — a started job run executes no script, so a catalog-writing run ends FAILED instead of SUCCEEDED.
AWS: Smithy com.amazonaws.glue#StartJobRun returns JobRunId (errors ConcurrentRunsExceededException, EntityNotFoundException, InvalidInputException, ResourceNumberLimitExceededException, OperationTimeoutException, InternalServiceException); GetJobRun returns JobRun, whose JobRunState walks to SUCCEEDED when the script completes. Recorded behavior: a Glue 4.0 Spark run that writes a Glue catalog table and its S3 output reaches JobRunState == 'SUCCEEDED' and those side effects exist afterwards.
Emulator: the run settles FAILED with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' — the run record is created and terminated without any execution.
Why: no ETL behavior is observable. Every downstream assertion (a catalog table authored by the job, its partitions, a reader role being denied on them) is unreachable, and a client cannot distinguish a genuinely failing script from a run that was never executed.
Fix: the glue job-run execution/state machine (services/service-com-amazonaws-glue-awsglue — job-run state and the StartJobRun/GetJobRun handlers). Same root cause as the open issue 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, which does not carry this check's replay key; it is recorded here so this check id is re-replayed with that fix.
Traces: athena__glue__s3.glue-job-authored-catalog-table-is-read-by-athena-and-a-partition-denied-role-fails.

Cause 2 of 4 — a terminal job run publishes no 'Glue Job State Change' event, so a matching EventBridge rule's permitted SQS target never receives it.
AWS: recorded behavior only (Glue's Smithy model carries no eventing shapes). A job run that reaches FAILED publishes an event on the default event bus with source 'aws.glue', detail-type 'Glue Job State Change' and a detail carrying jobName, jobRunId and state == 'FAILED'; a rule whose pattern matches it delivers one message to every target whose SQS queue policy grants events.amazonaws.com sqs:SendMessage, and silently drops delivery to a queue with no such policy (no error back to PutTargets).
Emulator: nothing is ever delivered. After PutRule/PutTargets with two SQS targets (one permitted, one not) and a job run that does reach a terminal state, the poll of the PERMITTED queue (the positive control, 180 s budget) never observes a message and the check is killed at the suite's 60 s wall clock, before the negative assertion on the unpermitted queue is even reached. Hypothesis: Glue publishes no job-state-change event to the default bus at all; the related open issues 20261025T002357Z-events-eventbridge-target-delivery-never-completes-for-sns-step.md and 20261025T002357Z-events-s3-sqs-s3-produces-no-aws-s3-events-on-either.md record the delivery/emission gap for other sources but neither names Glue or carries this check's replay key.
Why: event-driven ETL is unbuildable — alarms, retries and downstream triggers keyed on a FAILED run never fire. The negative control is also ungradeable: an unpermitted queue staying empty is indistinguishable from every queue staying empty, so an emulator that never delivers looks 'correct' on the permission boundary.
Fix: publish the Glue Job State Change event from the glue job-run state transitions to the default EventBridge bus (glue job-run state), and the EventBridge SQS target delivery path that consumes it.
Traces: athena__glue__s3.glue-job-failed-event-missing-target-permission-negative.

Cause 3 of 4 — StartJobRun's concurrency refusal carries emulator-written, request-derived text instead of AWS's constant message.
AWS: Smithy com.amazonaws.glue#ConcurrentRunsExceededException is a modeled StartJobRun error, client kind, HTTP 400 ('Too many jobs are being run concurrently'); its Message member is 'A message describing the problem.' Recorded behavior: the message is the constant 'Concurrent runs exceeded.' — it names no job and no counts.
Emulator: the code and status are right (ConcurrentRunsExceededException/400) but the message is rebuilt from the request: 'Job <jobName> already has 1 of 1 concurrent runs in flight'.
Why: the message rule here is a fixed constant, and clients, SDK wrappers and CFN providers string-match on it; a request-derived message also leaks the job name and the in-flight count that AWS deliberately withholds.
Fix: the refusal in the glue StartJobRun handler (services/service-com-amazonaws-glue-awsglue/src/ops/start_job_run.rs) — emit AWS's constant message for the concurrency refusal.
Traces: glue.reject-concurrent-run-over-limit.

Cause 4 of 4 — GetJobRun's not-found refusal echoes the request instead of AWS's constant message.
AWS: Smithy com.amazonaws.glue#GetJobRun models EntityNotFoundException (client, HTTP 400, 'A specified entity does not exist'). Recorded behavior: the message is the constant 'Job run not found for the given job name and runId.' — it echoes neither the JobName nor the RunId.
Emulator: code and status are right (EntityNotFoundException/400) but the message is 'Job run <runId> not found for job <jobName>'.
Why: same as cause 3 — the constant is the contract clients match on, and echoing the identifiers back changes a request-independent message into a request-derived one.
Fix: the not-found refusal in the glue GetJobRun handler (services/service-com-amazonaws-glue-awsglue/src/ops/get_job_run.rs).
Traces: glue.reject-get-nonexistent-job-run.

Service-attribution note: causes 3 and 4 come from checks whose id token is exactly 'glue'. Causes 1 and 2 come from checks whose leading token is the composite 'athena__glue__s3' (SERVICES = ['athena','glue','s3']); both failed on Glue's own job-run behavior before any Athena call, so they are filed under 'glue' (bootstrapped). The Athena legs of those checks are blocked separately by Athena not being bootstrapped and are not part of this issue.

## Evidence
Cause 1 — expected: glue.get_job_run(JobName=j, RunId=r)['JobRun']['JobRunState'] == 'SUCCEEDED' for the catalog-writing run. Actual: AssertionError: the catalog-writing run ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'

Cause 2 — expected: after the run reaches FAILED, sqs.receive_message on the PERMITTED queue yields one message whose body['detail']['jobName'] == <job> and body['detail']['state'] == 'FAILED', and the unpermitted queue stays empty for a further 45 s. Actual: 'Check timed out after 60.0s' — the permitted-queue poll (timeout_s=180) never saw a message, so the negative assertion was never reached.

Cause 3 — expected: (code, message) == ('ConcurrentRunsExceededException', 'Concurrent runs exceeded.') from a second StartJobRun while the first run is non-terminal. Actual: code is correct, message is 'Job sim2real-behav-job-<id> already has 1 of 1 concurrent runs in flight' (AssertionError on the message assertion).

Cause 4 — expected: (code, message) == ('EntityNotFoundException', 'Job run not found for the given job name and runId.') from GetJobRun with a RunId that was never created. Actual: code is correct, message is 'Job run jr_<real-run-id>-does-not-exist not found for job sim2real-behav-job-<id>' (AssertionError: expected message 'Job run not found for the given job name and runId.').

## Steps to Reproduce
Setup for all causes: create an IAM role trusting glue.amazonaws.com with AWSGlueServiceRole attached.

Cause 1: create a Glue 4.0 Spark job whose script writes a table into a Glue database and S3, StartJobRun, then poll GetJobRun -> observe JobRunState 'FAILED' with the 'executes no job script' ErrorMessage; expect 'SUCCEEDED' and the catalog table/S3 output to exist. Re-replay athena__glue__s3.glue-job-authored-catalog-table-is-read-by-athena-and-a-partition-denied-role-fails.

Cause 2: create a pythonshell job whose script is 'import sys\nsys.exit(1)'; events.put_rule(EventPattern={'source':['aws.glue'],'detail-type':['Glue Job State Change'],'detail':{'jobName':[job],'state':['FAILED']}}, State='ENABLED'); create two SQS queues, grant events.amazonaws.com sqs:SendMessage on only one, events.put_targets with both; StartJobRun and wait for the run to be terminal; poll the PERMITTED queue -> observe nothing delivered; expect exactly one matching message there and none on the other queue. Re-replay athena__glue__s3.glue-job-failed-event-missing-target-permission-negative.

Cause 3: create a pythonshell job (default MaxConcurrentRuns=1), StartJobRun, confirm GetJobRun is non-terminal, then StartJobRun again -> compare Error.Message to 'Concurrent runs exceeded.'. Re-replay glue.reject-concurrent-run-over-limit.

Cause 4: create a pythonshell job, StartJobRun to get a real RunId, then glue.get_job_run(JobName=job, RunId=<realRunId> + '-does-not-exist') -> compare Error.Message to 'Job run not found for the given job name and runId.'. Re-replay glue.reject-get-nonexistent-job-run.
