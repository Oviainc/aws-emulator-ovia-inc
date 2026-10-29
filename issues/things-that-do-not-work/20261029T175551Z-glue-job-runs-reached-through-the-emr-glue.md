Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.cfn-deploy-glue-job-role-missing-s3-access-negative emr__glue__s3.cfn-deploy-glue-job-s3-script-location-positive emr__glue__s3.cli-glue-job-create-start-monitor-s3-output-workflow emr__glue__s3.eventbridge-glue-job-state-change-triggers-rule-positive emr__glue__s3.eventbridge-rule-missing-target-permission-negative emr__glue__s3.glue-etl-job-reads-s3-csv-source-positive emr__glue__s3.glue-etl-job-writes-parquet-to-s3-positive emr__glue__s3.glue-job-role-cross-account-trust-mismatch-rejected emr__glue__s3.glue-job-start-with-role-missing-glue-trust-rejected emr__glue__s3.iam-role-trust-glue-service-assumed-successfully-positive emr__glue__s3.cfn-deploy-glue-database-table-s3-stack-positive
Issue-Key: emr__glue__s3.cfn-deploy-glue-job-role-missing-s3-access-negative|run#15|AssertionError|assert full_result["JobRunState"] == "SUCCEEDED", (
Issue-Key: emr__glue__s3.cfn-deploy-glue-job-s3-script-location-positive|run#12|AssertionError|assert job_run["JobRunState"] == "SUCCEEDED", (
Issue-Key: emr__glue__s3.cli-glue-job-create-start-monitor-s3-output-workflow|run#15|AssertionError|assert job_run["JobRunState"] == "SUCCEEDED", job_run
Issue-Key: emr__glue__s3.eventbridge-glue-job-state-change-triggers-rule-positive|run#27|AssertionError|assert final_state == "SUCCEEDED", (
Issue-Key: emr__glue__s3.eventbridge-rule-missing-target-permission-negative|run#31|AssertionError|assert final_state == "SUCCEEDED", (
Issue-Key: emr__glue__s3.glue-etl-job-reads-s3-csv-source-positive|_run_job_until_role_propagates#10|AssertionError|raise AssertionError(f"job run {run_id} did not succeed: {error_message}")
Issue-Key: emr__glue__s3.glue-etl-job-writes-parquet-to-s3-positive|run#12|AssertionError|assert job_run["JobRunState"] == "SUCCEEDED", job_run.get("ErrorMessage")
Issue-Key: emr__glue__s3.glue-job-role-cross-account-trust-mismatch-rejected|run#14|AssertionError|assert good_final["JobRunState"] == "SUCCEEDED", f"positive control failed: {good_final}"
Issue-Key: emr__glue__s3.glue-job-start-with-role-missing-glue-trust-rejected|run#11|AssertionError|assert good_final["JobRunState"] == "SUCCEEDED", f"positive control failed: {good_final}"
Issue-Key: emr__glue__s3.iam-role-trust-glue-service-assumed-successfully-positive|run#13|AssertionError|assert final["JobRunState"] == "SUCCEEDED", f"unexpected state: {final}"
Issue-Key: emr__glue__s3|GetTable|EntityNotFoundException
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# Glue job runs reached through the emr__glue__s3 cross-service checks terminate FAILED without executing the script, and CloudFormation-declared Glue databases/tables are absent from the catalog

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
This issue bundles 2 net-new root causes, both in Glue. They are grounded in the cross-service `emr__glue__s3` behavioral checks (the suite's check ids carry the composite group token `emr__glue__s3`, not a single service token; the failing calls are all Glue API calls — StartJobRun/GetJobRun and GetTable — so the service is attributed to `glue`).

Related open issues describe the same family of behavior for OTHER replay keys and do NOT carry these checks: 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md (the suite marks only emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive and emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive as owned by it — those two are excluded here), 20261025T072821Z-glue-job-runs-are-terminated-failed-without-executing.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md, 20261025T022820Z-glue-cfn-declared-resources-and-job-run-execution.md and 20261028T061828Z-glue-cfn-deployed-job-run-never-completes-or-enforces-the-run-role.md. The 11 checks below are still failing and are not carried by any of them.

--- Cause 1 of 2: a started Glue job run is terminated FAILED with an emulator-authored ErrorMessage instead of running the script ---
AWS: Smithy/recorded behavior: StartJobRun accepts a run for an existing job and returns its JobRunId; the run then progresses STARTING -> RUNNING -> SUCCEEDED when the script at the job's ScriptLocation completes successfully, and JobRun.ErrorMessage is absent for a successful run. A run that genuinely fails carries Glue's own error text (an AWS-authored message naming the failure), and ExecutionTime/CompletedOn reflect real elapsed execution. The script's side effects are observable: objects written under the job's --OUTPUT_PATH in S3, bookmark state advanced, and a 'Glue Job State Change' event published to EventBridge on the terminal transition.
Emulator: every run reaches terminal state FAILED within ~0.5s with the emulator-authored ErrorMessage `This emulator records job runs but executes no job script, so the job run ended without running.` (ExecutionTime 0, no S3 output, no log events — one check reports log='<no log events found>'). This also makes every NEGATIVE check unguardable: those checks first require a positive control (a fully-permissioned role) to reach SUCCEEDED before judging the refusal, so a role/trust/permission gate cannot be graded at all.
Why: a client waiting for SUCCEEDED never gets it and sees a failure message no AWS client can interpret (it is not AWS text and names the emulator); downstream S3 outputs, bookmarks, EventBridge 'Glue Job State Change' deliveries and anything gated on job success never happen. Note the message itself is also a divergence: AWS never emits emulator-authored prose in JobRun.ErrorMessage.
Fix: the Glue job-run execution path and its state machine (services/service-com-amazonaws-glue-awsglue — the StartJobRun handler and the job-run state that drives GetJobRun), which currently records the run and immediately settles it FAILED instead of executing the referenced script and settling SUCCEEDED.
Traces: emr__glue__s3.cfn-deploy-glue-job-role-missing-s3-access-negative, emr__glue__s3.cfn-deploy-glue-job-s3-script-location-positive, emr__glue__s3.cli-glue-job-create-start-monitor-s3-output-workflow, emr__glue__s3.eventbridge-glue-job-state-change-triggers-rule-positive, emr__glue__s3.eventbridge-rule-missing-target-permission-negative, emr__glue__s3.glue-etl-job-reads-s3-csv-source-positive, emr__glue__s3.glue-etl-job-writes-parquet-to-s3-positive, emr__glue__s3.glue-job-role-cross-account-trust-mismatch-rejected, emr__glue__s3.glue-job-start-with-role-missing-glue-trust-rejected, emr__glue__s3.iam-role-trust-glue-service-assumed-successfully-positive.

--- Cause 2 of 2: a CloudFormation stack that reached CREATE_COMPLETE leaves no AWS::Glue::Database/AWS::Glue::Table in the catalog ---
AWS: recorded behavior: once a stack declaring AWS::Glue::Database and AWS::Glue::Table reaches CREATE_COMPLETE, the database exists under its declared name and GetTable(DatabaseName=<declared>, Name=<declared>) returns the table. EntityNotFoundException (`Database <name> not found.`) is the correct answer only for a database that was never created.
Emulator: after the stack reports success, GetTable raises EntityNotFoundException — `Database sim2real_xsvc_db_7c33c3c12a3a not found.` (the name is a per-run value; the defect is that the CFN-declared database/table do not exist at all). Fixed text of the refusal is `Database ` + name + ` not found.`, which is correct wording for a missing database — the defect is that the database is missing.
Why: CloudFormation reports success for resources it never created, so every downstream read of a CFN-declared Glue catalog object fails and infrastructure-as-code flows silently diverge from AWS.
Fix: the CloudFormation resource provisioner for the AWS::Glue::* resource types (the CloudFormation crate's resource handlers) and/or the Glue catalog state it should write into — the stack must actually create the database/table it declares before reporting CREATE_COMPLETE.
Traces: emr__glue__s3.cfn-deploy-glue-database-table-s3-stack-positive.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (group emr__glue__s3, 1/46 checks passing):

Cause 1 — identical terminal failure across 10 checks, e.g.
- emr__glue__s3.cfn-deploy-glue-job-s3-script-location-positive: 'expected job run jr_c2e9... to end SUCCEEDED, got FAILED: This emulator records job runs but executes no job script, so the job run ended without running. log=<no log events found>'
- emr__glue__s3.cli-glue-job-create-start-monitor-s3-output-workflow: GetJobRun returned {'JobRunState': 'FAILED', 'ErrorMessage': 'This emulator records job runs but executes no job script, so the job run ended without running.', 'ExecutionTime': 0, 'StartedOn': 17:31:30.348, 'CompletedOn': 17:31:30.848, ...}
- emr__glue__s3.glue-job-start-with-role-missing-glue-trust-rejected and emr__glue__s3.glue-job-role-cross-account-trust-mismatch-rejected: 'positive control failed: {... JobRunState: FAILED, ErrorMessage: This emulator records job runs but executes no job script ...}' — the negative case cannot be graded.
- emr__glue__s3.iam-role-trust-glue-service-assumed-successfully-positive, emr__glue__s3.glue-etl-job-reads-s3-csv-source-positive, emr__glue__s3.glue-etl-job-writes-parquet-to-s3-positive, emr__glue__s3.cfn-deploy-glue-job-role-missing-s3-access-negative: same ErrorMessage.
- emr__glue__s3.eventbridge-glue-job-state-change-triggers-rule-positive and emr__glue__s3.eventbridge-rule-missing-target-permission-negative: 'This emulator records job runs but executes no job script, so the job run ended without running. (state=FAILED)' — so no Glue Job State Change event is ever published.

Cause 2 — emr__glue__s3.cfn-deploy-glue-database-table-s3-stack-positive: botocore.errorfactory.EntityNotFoundException: An error occurred (EntityNotFoundException) when calling the GetTable operation: Database sim2real_xsvc_db_7c33c3c12a3a not found. — raised after the CloudFormation stack declaring that AWS::Glue::Database and its AWS::Glue::Table had completed.

## Steps to Reproduce
Boot the emulator and replay these checks from suite simparity-behavioral-aws-v1:
1. Cause 1 — e.g. emr__glue__s3.iam-role-trust-glue-service-assumed-successfully-positive: create an IAM role trusted by glue.amazonaws.com, upload a script to S3, CreateJob pointing at it, StartJobRun, then poll GetJobRun. Expect SUCCEEDED with no ErrorMessage and the script's S3 side effects; the emulator returns FAILED with 'This emulator records job runs but executes no job script, so the job run ended without running.' Replay the other nine ids listed in trace_ids the same way (the two cfn-deploy-glue-job-* ones go through a CloudFormation-declared job; the two eventbridge-* ones additionally expect a 'Glue Job State Change' event).
2. Cause 2 — emr__glue__s3.cfn-deploy-glue-database-table-s3-stack-positive: CreateStack with a template declaring AWS::Glue::Database + AWS::Glue::Table over an S3 location, wait for CREATE_COMPLETE, then call GetTable(DatabaseName=<declared db>, Name=<declared table>).
