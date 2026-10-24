Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__emr-serverless__glue__s3.spark-created-glue-table-read-back-by-athena
Issue-Key: athena__emr-serverless__glue__s3.spark-created-glue-table-read-back-by-athena|run#28|AssertionError|assert final["state"] == "SUCCESS", (
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# EMR Serverless job runs never execute the submitted workload: GetJobRun reaches terminal FAILED instead of SUCCESS

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobRun

## Description
One cause.

Attribution note: the failing check's id leads with the four-service combo token `athena__emr-serverless__glue__s3`, which matches no single bootstrap token. The diverging response in this issue is produced by the emr-serverless API (StartJobRun/GetJobRun), so the issue is filed against `emr-serverless`, which is stood up and answering (it passes 34/34 of its own behavioral checks).

Cause 1 of 1: a spark-submit job run started on an EMR Serverless application always reaches the terminal state FAILED, with stateDetails announcing that the emulator does not run the workload.

AWS: Smithy com.amazonaws.emrserverless#JobRunState enumerates SUBMITTED, PENDING, SCHEDULED, RUNNING, SUCCESS, FAILED, CANCELLING, CANCELLED, QUEUED; com.amazonaws.emrserverless#JobRun documents a job run as "a unit of work, such as a Spark JAR, Hive query, or SparkSQL query, that you submit to an Amazon EMR Serverless application", with required members `state` and `stateDetails`, and `startedAt` documented as "the date and time when the job moved to the RUNNING state". A valid sparkSubmit job run whose entryPoint and entryPointArguments resolve, submitted with an execution role that grants the S3 and Glue actions the script needs, progresses through the non-terminal states and settles on SUCCESS; FAILED is reserved for a run whose driver actually ran and exited non-zero, or that could not be placed (e.g. the documented not-yet-assumable-role symptom the check already retries on). The emulator additionally never produces the side effects of a run that executed — in this scenario, a Glue catalog table registered through the Hive metastore client and the driver's gzipped stderr under the application's configured s3MonitoringConfiguration.logUri. Recorded behavior only for the specific progression of intermediate states.
Emulator: GetJobRun returns state 'FAILED' for every job run, with stateDetails 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.' StartJobRun accepts the run and allocates an id (0000000000000001 in the replay), so the refusal is visible only in the terminal state minutes later. No workload-derived side effect appears.
Why: every client that submits work to EMR Serverless and gates on the outcome breaks. A poller that waits for SUCCESS (as the AWS-documented pattern does) sees a permanent FAILED and takes its error path; an orchestrator (Step Functions .sync, Airflow, a shell driver) marks the step failed and never advances; downstream readers find no catalog entry and no driver log. Worse, FAILED is also the state AWS uses for a genuine job failure, so the emulator makes a negative control indistinguishable from the thing it is supposed to detect: the companion check `spark-saveastable-without-glue-createtable-permission-is-denied` asserts the IAM-denied run is FAILED and that assertion now passes for the wrong reason, so no IAM-denial fidelity can be graded, and its positive control (same job, glue:CreateTable granted, expected SUCCESS) can never pass either.
Fix: the job-run lifecycle in the emr-serverless state/handler layer that drives the state machine behind StartJobRun and GetJobRun — a job run whose driver configuration is accepted must advance through the non-terminal states to SUCCESS rather than being short-circuited to FAILED, and the catalog/log side effects the run is expected to produce must be modelled alongside it rather than left absent.
Traces: athena__emr-serverless__glue__s3.spark-created-glue-table-read-back-by-athena (assertion `final["state"] == "SUCCESS"`). The same gap also blocks the positive control in athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied, which is filed separately for its own distinct glue divergence.

## Evidence
Replay of simparity-behavioral-ga-prod-v1: 34/36 passed, 2 mismatched.

[mismatch] athena__emr-serverless__glue__s3.spark-created-glue-table-read-back-by-athena
check_athena__emr_serverless__glue__s3_spark_created_glue_table_read_back_by_athena.py line 496:
  assert final["state"] == "SUCCESS"
  AssertionError: Spark job run 0000000000000001 ended 'FAILED': 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.'

Expected (AWS): GetJobRun eventually reports jobRun.state == 'SUCCESS' for an accepted sparkSubmit run, having passed through the non-terminal JobRunState values, with the run's side effects present (Glue table written through the metastore client, driver stderr.gz under the configured logUri).
Actual (emulator): GetJobRun reports jobRun.state == 'FAILED' immediately and permanently; stateDetails states no workload is executed. StartJobRun returned 200 and a jobRunId, so nothing in the control-plane response signals this.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-ga-prod-v1 behavioral suite.
2. Run check athena__emr-serverless__glue__s3.spark-created-glue-table-read-back-by-athena, or by hand:
   a. Create an S3 bucket; put a header'd CSV at input/facts.csv and a PySpark script at job/ddl.py that does spark.read.csv(...).write.partitionBy('dt').option('path', warehouse).saveAsTable(db + '.' + tbl) with a Glue database pre-created.
   b. Create an IAM execution role trusted by emr-serverless.amazonaws.com granting the S3 object/bucket actions and the glue:Get*/Create*/Update*/Delete* table and partition actions.
   c. emr-serverless CreateApplication with releaseLabel 'emr-7.9.0', type 'SPARK', and monitoringConfiguration.s3MonitoringConfiguration.logUri pointing into the bucket.
   d. StartJobRun with jobDriver.sparkSubmit.entryPoint = s3://<bucket>/job/ddl.py, the entryPointArguments above, and sparkSubmitParameters setting spark.sql.catalogImplementation=hive plus the AWSGlueDataCatalogHiveClientFactory metastore factory.
   e. Poll GetJobRun until jobRun.state is one of SUCCESS / FAILED / CANCELLED.
3. Observe the terminal state is FAILED with stateDetails 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.'; expected SUCCESS.
4. Confirm the missing side effects: glue GetTable for the table the script registers raises EntityNotFoundException, and no SPARK_DRIVER/stderr.gz object appears under <logUri>applications/<applicationId>/jobs/<jobRunId>/.
