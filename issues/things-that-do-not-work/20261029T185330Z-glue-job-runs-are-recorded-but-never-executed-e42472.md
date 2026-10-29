Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.deploy-cfn-glue-job-role-missing-s3-write-permission-negative athena__glue__s3.glue-job-succeeded-event-triggers-eventbridge-rule-positive athena__glue__s3.glue-job-writes-output-to-s3-positive glue.deploy-glue-job-cfn-executes-successfully glue.deploy-glue-job-cfn-insufficient-role-permissions-rejected glue.deploy-glue-job-via-cloudformation-executes-successfully glue.deploy-job-via-cloudformation glue__s3.glue-job-role-without-write-permission-fails-the-run-and-writes-no-output glue__s3.glue-python-shell-job-writes-its-computed-output-to-s3 glue__s3.glue-spark-etl-job-converts-csv-to-parquet-preserving-every-row glue__s3.glue-spark-parquet-partition-layout-and-footer-aggregates-and-a-malformed-row-fails-the-run glue__sqs.cfn-deploy-glue-job-eventbridge-sqs-stack-positive glue__sqs.cfn-glue-job-iam-role-trust-sqs-send-positive glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive glue__sqs.cli-start-glue-job-run-sqs-notification-positive glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive glue__sqs.deploy-eventbridge-rule-missing-target-permission-negative glue__sqs.glue-job-assumes-role-sends-sqs-message-positive glue__sqs.glue-job-role-with-sqs-send-permission-positive glue__sqs.glue-job-role-without-sqs-send-permission-negative glue__sqs.glue-job-state-change-detail-fields-preserved-in-sqs glue__sqs.glue-job-succeeded-eventbridge-to-sqs-positive glue__sqs.iam-role-missing-glue-trust-assume-role-negative glue__sqs.iam-role-trust-glue-amazonaws-com-positive glue__sqs.iam-role-trusts-glue-service-principal-positive glue__sqs.sdk-glue-job-missing-sqs-send-permission-negative glue__sqs.sdk-glue-job-run-writes-batch-messages-to-sqs-positive glue__sqs.sdk-glue-job-sqs-message-attributes-preserved glue__sqs.sdk-glue-job-wrong-queue-url-negative
Issue-Key: athena__glue__s3.deploy-cfn-glue-job-role-missing-s3-write-permission-negative|run#28|AssertionError|assert good_run["JobRunState"] == "SUCCEEDED", good_run.get("ErrorMessage")
Issue-Key: athena__glue__s3.glue-job-succeeded-event-triggers-eventbridge-rule-positive|run#15|AssertionError|assert state == "SUCCEEDED", (
Issue-Key: athena__glue__s3.glue-job-writes-output-to-s3-positive|run#29|AssertionError|assert state == "SUCCEEDED", (
Issue-Key: glue.deploy-glue-job-cfn-executes-successfully|run#24|AssertionError|assert job_run["JobRunState"] == "SUCCEEDED", job_run.get("ErrorMessage")
Issue-Key: glue.deploy-glue-job-cfn-insufficient-role-permissions-rejected|run#26|AssertionError|assert good_run["JobRunState"] == "SUCCEEDED", good_run.get("ErrorMessage")
Issue-Key: glue.deploy-glue-job-via-cloudformation-executes-successfully|run#23|AssertionError|assert final_state == "SUCCEEDED", f"deployed job run ended in {final_state}, not SUCCEEDED"
Issue-Key: glue.deploy-job-via-cloudformation|run#15|AssertionError|assert job_run["JobRunState"] == "SUCCEEDED", job_run.get("ErrorMessage")
Issue-Key: glue__s3.glue-job-role-without-write-permission-fails-the-run-and-writes-no-output|run#19|AssertionError|assert "AccessDenied" in message and "PutObject" in message, (
Issue-Key: glue__s3.glue-python-shell-job-writes-its-computed-output-to-s3|run#16|AssertionError|assert job_run["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__s3.glue-spark-etl-job-converts-csv-to-parquet-preserving-every-row|run#17|AssertionError|assert job_run["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__s3.glue-spark-parquet-partition-layout-and-footer-aggregates-and-a-malformed-row-fails-the-run|run#9|AssertionError|assert good["JobRunState"] == "SUCCEEDED", (
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
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Glue job runs are recorded but never executed: every StartJobRun settles FAILED with an emulator-authored ErrorMessage, so no script side effect, state transition or Job State Change event ever happens (1 cause)

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
1 cause. 29 failing checks across the glue, glue__s3, glue__sqs and athena__glue__s3 suites fail on the same single root cause.

Related open issues, named for the fixer; NONE of them carries the replay keys of the 29 checks listed here, so re-replaying their trace lists will not exercise these checks: 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md, 20261025T072821Z-glue-job-runs-are-terminated-failed-without-executing.md, 20261028T061828Z-glue-cfn-deployed-job-run-never-completes-or-enforces-the-run-role.md, 20261029T175551Z-glue-job-runs-reached-through-the-emr-glue.md, 20261025T000319Z-glue-iam-lakeformation-s3-a-glue-job-run.md. The cause is restated here so these checks are covered; a fixer must make the whole list in trace_ids pass.

Cause 1 of 1: StartJobRun never executes the job's Command.ScriptLocation, and the run is forced to a FAILED terminal state carrying an emulator-authored ErrorMessage.
AWS: Smithy: StartJobRun returns JobRunId and models EntityNotFoundException/ConcurrentRunsExceededException/InvalidInputException/ResourceNumberLimitExceededException (all client, HTTP 400); JobRun$JobRunState targets JobRunState whose values include STARTING, RUNNING, SUCCEEDED, FAILED, TIMEOUT, ERROR; JobRun also carries ExecutionTime, CompletedOn and ErrorMessage. A started run is executed by Glue under the job's Role: the script runs, its side effects land (S3 objects written, Glue catalog tables created, SQS messages sent), the run passes through RUNNING and then reaches SUCCEEDED with ErrorMessage absent and ExecutionTime > 0, and Glue publishes a `Glue Job State Change` event to EventBridge. A run whose script is refused by IAM (for example s3:PutObject denied for the run role) reaches FAILED with the script's own refusal text as ErrorMessage, e.g. `ClientError: An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws:sts::<account-id>:assumed-role/<run-role-name>/GlueJobRunnerSession is not authorized to perform: s3:PutObject on resource: <resource arn>` — fixed text from the SDK/Glue runner, with the account id, role name and resource taken from the request and the run role. A run whose role cannot be assumed by glue.amazonaws.com is refused for that reason, not for a generic one.
Emulator: every run is recorded and then immediately settled: JobRunState `FAILED`, ExecutionTime 0, StartedOn == CompletedOn (half a second apart), and ErrorMessage is the constant emulator string `This emulator records job runs but executes no job script, so the job run ended without running.` RUNNING is never observable (glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive: `never observed the job in RUNNING state`). No script side effect is ever produced (no S3 output object, no Parquet conversion, no catalog table, no SQS message), and no `Glue Job State Change` event with state SUCCEEDED is published for a successful run.
Why: the Glue data plane is entirely inert. Positive flows break (a client waiting for SUCCEEDED waits for a state that can never occur, and the data the job was supposed to produce never exists), and negative flows are silently indistinguishable: a run whose role lacks s3:PutObject, a run whose role has no glue.amazonaws.com trust and a perfectly healthy run all end in the same FAILED with the same emulator sentence, so a client cannot tell a permissions defect from success. Downstream integrations keyed on the run outcome (EventBridge rules on Glue Job State Change, SQS notification fan-out, workflow/trigger chaining, bookmark advancement) never fire.
Fix: the job-run execution component of the glue service crate — the StartJobRun handler and the job-run state machine that owns run transitions (services/service-com-amazonaws-glue-awsglue/src/ops/start_job_run.rs plus the job-run state it writes). It must actually execute the job's Command under the job Role (so script side effects and IAM refusals are real), drive JobRunState through RUNNING to SUCCEEDED/FAILED with ExecutionTime/CompletedOn, leave ErrorMessage absent on success, carry the underlying refusal verbatim on an authorization failure, and emit the Glue Job State Change event for each transition. The emulator-authored sentence must not be produced at all.
Traces: all 29 checks in trace_ids; representative replays are glue__s3.glue-python-shell-job-writes-its-computed-output-to-s3, glue__sqs.sdk-glue-job-run-writes-batch-messages-to-sqs-positive and glue__s3.glue-job-role-without-write-permission-fails-the-run-and-writes-no-output.

## Evidence
Positive flow — glue__s3.glue-python-shell-job-writes-its-computed-output-to-s3:
  expected: JobRunState SUCCEEDED, no ErrorMessage, the script's output object present in S3
  actual:   AssertionError: job run ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'

State machine — glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive:
  expected: the run observable in RUNNING before it terminates
  actual:   AssertionError: never observed the job in RUNNING state

Terminal record — glue__sqs.deploy-eventbridge-glue-job-state-rule-to-sqs-positive (GetJobRun):
  {'JobRunState': 'FAILED', 'ExecutionTime': 0, 'StartedOn': 18:27:50.327, 'CompletedOn': 18:27:50.827,
   'ErrorMessage': 'This emulator records job runs but executes no job script, so the job run ended without running.'}
  (the ids, names and timestamps are per-run values, not part of the requirement; the constant
   ErrorMessage sentence and the FAILED/ExecutionTime=0 shape are)

Negative flow misattributed — glue__s3.glue-job-role-without-write-permission-fails-the-run-and-writes-no-output:
  expected: FAILED whose ErrorMessage names AccessDenied on PutObject for
            arn:aws:sts::<account-id>:assumed-role/<run-role>/GlueJobRunnerSession
  actual:   AssertionError: the run failed for the wrong reason; ErrorMessage was 'This emulator records job
            runs but executes no job script, so the job run ended without running.', expected it to name
            AccessDenied on PutObject

Negative flow misattributed — glue__sqs.iam-role-missing-glue-trust-assume-role-negative:
  expected: the failure to name the assume-role rejection
  actual:   ErrorMessage='This emulator records job runs but executes no job script, so the job run ended without running.'

## Steps to Reproduce
1. Create an IAM role trusting glue.amazonaws.com with AWSGlueServiceRole attached and s3:PutObject on a test bucket.
2. Upload a pythonshell script to S3 that writes a known object (e.g. s3://<bucket>/out/output.txt with body 'hello-glue-job\n').
3. glue:CreateJob(Name=J, Role=<that role>, Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':<that script>}, MaxCapacity=0.0625).
4. glue:StartJobRun(JobName=J) and poll glue:GetJobRun(JobName=J, RunId=<id>): the run must be observable in RUNNING and then reach JobRunState SUCCEEDED with ExecutionTime > 0 and no ErrorMessage. Today it is FAILED within ~500ms with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.'
5. s3:GetObject(<bucket>, out/output.txt) must return the script's output. Today the object does not exist.
6. Negative control: repeat with a run role that has NO s3:PutObject. The run must reach FAILED with ErrorMessage 'ClientError: An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws:sts::<account>:assumed-role/<role>/GlueJobRunnerSession is not authorized to perform: s3:PutObject on resource: <arn>' — i.e. a different message from the positive case.
7. Put an EventBridge rule on {'source':['aws.glue'],'detail-type':['Glue Job State Change']} with an SQS target and re-run step 4: a SUCCEEDED event must be delivered.
