Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative
Issue-Key: glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive|_poll_for_matching_message#20|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative|_poll_for_matching_message#23|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 3ddd7d1a9606c20c808c737ad5d54c6c77061be0

# glue__sqs: a CloudFormation-deployed EventBridge rule never receives a "Glue Job State Change" SUCCEEDED event for a completed Glue job run, so its SQS target gets nothing

> _Note: this issue was identified against the code base at commit `3ddd7d1a9606c20c808c737ad5d54c6c77061be0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One (1) net-new root cause, grounded in two failing checks. Cause 1 of 1 must be closed before this issue is submitted. Neither check was marked ALREADY FILED or OWNED by the replay report, so no open issue carries their current replay keys; both now fail as a check-level TIMEOUT because the event they wait for never arrives.

=== Cause 1 of 1: a Glue job run started against a CFN-deployed AWS::Glue::Job publishes no "Glue Job State Change" event, so the CFN-deployed AWS::Events::Rule never delivers anything to its SQS target ===
AWS: recorded behavior only for the event (EventBridge does not model event payloads in Smithy; Glue models StartJobRun -> JobRunId and GetJobRun -> JobRun{JobRunState}). Recorded behavior: the run reaches JobRunState == "SUCCEEDED", and Glue publishes onto the default event bus an event with source == "aws.glue", detail-type == "Glue Job State Change" and a detail object carrying jobName (the started job), state ("SUCCEEDED") and jobRunId (the id StartJobRun returned). A rule whose EventPattern matches {source: [aws.glue], detail-type: [Glue Job State Change], detail: {jobName: [<job>], state: [SUCCEEDED]}} and whose SQS target queue carries a policy allowing events.amazonaws.com sqs:SendMessage (conditioned on aws:SourceArn == the rule ARN) receives that event as a message body; a sibling queue with no such resource policy receives nothing and reports ApproximateNumberOfMessages 0.
Emulator: the stack deploys and StartJobRun returns a JobRunId, but the run never reaches SUCCEEDED and/or no Glue Job State Change event is ever published or delivered. The poll for a matching message on the permissioned queue never converges, so both checks end as "Check timed out after 60.0s" before reaching their assertions; the negative half (the unpermissioned queue must stay empty) is never exercised and would be vacuous if it were.
Why: event-driven Glue orchestration is entirely dead — a client whose pipeline reacts to job completion through EventBridge (to SQS, Lambda or Step Functions) never receives the notification, so nothing downstream of a job ever runs; and because no event flows to either queue, the queue-policy permission boundary that should distinguish a permitted target from an unpermitted one is unobservable, letting a broken authorization check look correct.
Related but NOT covering: 20261109T185037Z-glue-sqs-3-net-new-causes-glue-job-state-change.md and 20261102T094631Z-glue-sqs-a-started-job-run-never-passes.md describe this area; the report left these two checks unowned, so neither open issue carries their current replay keys.
Fix: the Glue job-run execution path (a started run must advance to a terminal JobRunState) and the event publication hooked to that transition — Glue must put a "Glue Job State Change" event with source aws.glue and detail{jobName, state, jobRunId} on the default bus — plus the EventBridge rule-matching and SQS target delivery that carries it to the queue, honouring the queue's resource policy for events.amazonaws.com.
Traces: glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive, glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emulator up, bootstrap_signal false; 338/645 passed; glue__sqs 28/52). Both checks are listed by the report under the service heading ### glue__sqs and neither is marked ALREADY FILED or OWNED.

- [mismatch] glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive — "Error: Check timed out after 60.0s". The check deploys a stack with AWS::SQS::Queue + AWS::SQS::QueuePolicy (Allow events.amazonaws.com sqs:SendMessage, Condition ArnEquals aws:SourceArn = the rule ARN) + AWS::Events::Rule (EventPattern {"source":["aws.glue"],"detail-type":["Glue Job State Change"],"detail":{"jobName":[<job>],"state":["SUCCEEDED"]}}, Targets=[the queue]) + AWS::S3::Bucket + AWS::IAM::Role + AWS::Glue::Job, puts the job script, calls glue.start_job_run(JobName=<job>) and waits `_wait_for_job_run(..., timeout_s=600)` for `assert job_run["JobRunState"] == "SUCCEEDED"`, then `_poll_for_matching_message(..., timeout_s=120)` for the body matching source/detail-type/detail.jobName/detail.state. The check exhausts its budget inside those waits; expected is JobRunState SUCCEEDED followed by a message whose detail.jobName == <job>, detail.state == "SUCCEEDED" and detail.jobRunId == the returned run id.
- [mismatch] glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative — "Error: Check timed out after 60.0s". Same wiring with two queues (one with the events.amazonaws.com policy, one without). The positive control `assert good_message is not None, "expected the event on the queue with a correct resource policy"` is never satisfied, so the assertions that the unpermissioned queue receives nothing and reports ApproximateNumberOfMessages 0 are never reached.

Queue, bucket, role, job and rule names, the account id and the job-run id are per-run values, not part of the requirement.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay simparity-behavioral-aws-v1 and re-run both checks: glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive and glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative. Both must pass before submitting.
3. Deploy a CloudFormation stack containing: AWS::SQS::Queue <good>; AWS::SQS::QueuePolicy on <good> allowing Principal {Service: events.amazonaws.com} sqs:SendMessage with Condition ArnEquals aws:SourceArn = arn:aws:events:<region>:<account>:rule/<rule>; AWS::SQS::Queue <bad> with NO policy; AWS::Events::Rule <rule> with EventPattern {"source":["aws.glue"],"detail-type":["Glue Job State Change"],"detail":{"jobName":[<job>],"state":["SUCCEEDED"]}} targeting both queues; AWS::S3::Bucket for the script; AWS::IAM::Role trusted by glue.amazonaws.com with read on that bucket; AWS::Glue::Job <job> (pythonshell) whose ScriptLocation is s3://<bucket>/script.py.
4. Put s3://<bucket>/script.py, then call glue.start_job_run(JobName=<job>) and poll glue.get_job_run: JobRunState must reach 'SUCCEEDED'.
5. Receive from <good>: a message body must arrive with source 'aws.glue', detail-type 'Glue Job State Change', detail.jobName == <job>, detail.state == 'SUCCEEDED' and detail.jobRunId == the JobRunId from step 4. The emulator delivers nothing.
6. Receive from <bad> over the same window: nothing must arrive and sqs.get_queue_attributes(QueueUrl=<bad>, AttributeNames=['ApproximateNumberOfMessages']) must report 0.
