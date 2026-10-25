Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.deploy-glue-database-cfn-behaves glue.deploy-glue-database-via-cloudformation-behaves glue.deploy-crawler-via-cloudformation glue.deploy-job-missing-output-permission-rejected glue.e2e-glue-tsd-database-tables-schema-lifecycle glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds
Issue-Key: glue|GetDatabase|EntityNotFoundException
Issue-Key: glue|GetCrawler|EntityNotFoundException
Issue-Key: glue|StartJobRun|EntityNotFoundException
Issue-Key: glue.e2e-glue-tsd-database-tables-schema-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file|run#9|AssertionError|assert run1["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds|run#22|AssertionError|assert warmup["JobRunState"] == "SUCCEEDED", (
Issue-Commit: e9591ea0ffea53e5939624d35a733297d20386ca

# Glue: CloudFormation-declared AWS::Glue resources do not exist under their declared names, and a started job run executes no script

> _Split out of `20261025T022820Z-glue-7-net-new-root-causes-gettables-expression.md`, which bundled seven root causes. Five of them are closed in that report (the `GetTables` `Expression` regular expression, the foreign-`CatalogId` refusal, the Glue job capacity defaults, the Region scoping of job definitions and their runs, and the `CreateCrawler` event-queue permission check). The two below were NOT closed and keep their replay keys here, so a later scan does not read them as working. They were identified against commit `e9591ea0ffea53e5939624d35a733297d20386ca`; the code base may have drifted._

## Operation
StartJobRun

## Description
Cause 1 of 2: CloudFormation-declared AWS::Glue::Database / ::Table / ::Crawler / ::Job resources do not exist in Glue under their declared names after CREATE_COMPLETE, and Ref returns a generated physical id instead of the declared name.
AWS: a stack that reaches CREATE_COMPLETE has created its Glue entities: glue:GetDatabase returns the database with the Description from DatabaseInput and CatalogId = the account, glue:GetCrawler returns the crawler, glue:StartJobRun finds the job, and {'Ref': '<AWS::Glue::Database logical id>'} resolves to the database NAME (so a stack Output carrying that Ref is the catalog name a caller then passes to glue:GetDatabase/GetTable).
Emulator: the stack reports CREATE_COMPLETE, then glue:GetDatabase answers 400 EntityNotFoundException 'Database <declared name> not found.', glue:GetCrawler answers 400 EntityNotFoundException 'Crawler entry with name <declared name> does not exist', glue:StartJobRun answers 400 EntityNotFoundException for the declared job, and the stack Output for Ref of the AWS::Glue::Database resolves to a CFN-generated physical id of the form <stack-name>-<logical-id>-<suffix> instead of the declared database name.
Why: nothing deployed through CloudFormation is usable through the Glue API — the deploy-then-use flow (the normal way these resources are created) breaks at the first Glue call, and the generated physical id means even a caller that reads the name out of the stack Outputs queries a name that does not exist.
Fix: the AWS::Glue::Database / ::Table / ::Crawler / ::Job resource providers in the CloudFormation provisioning engine must create the entities in the Glue service state and use the declared name (DatabaseInput.Name / TableInput.Name / Name) as the resource's physical id that Ref returns. That is a change to `services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs` (one Provisioner variant plus the create/delete/drift calls per type) and to the physical-naming rules in `.../src/state/template.rs`, which read a name only from a TOP-LEVEL property today and have to read the nested `DatabaseInput.Name` / `TableInput.Name`. Two open issues describe the same gap from the CloudFormation side and should be fixed with this one: 20261025T050617Z-cloudformation-glue-iam-s3-a-stack-declaring-aws.md and 20261025T050831Z-cloudformation-glue-iam-s3-cloudformation-never-provisions-aws-glue-job-after-create.md.
Traces: glue.deploy-glue-database-cfn-behaves, glue.deploy-glue-database-via-cloudformation-behaves, glue.deploy-crawler-via-cloudformation, glue.deploy-job-missing-output-permission-rejected, glue.e2e-glue-tsd-database-tables-schema-lifecycle.

Cause 2 of 2: a started job run executes no script, so every run ends FAILED and produces no side effects.
AWS: StartJobRun runs the job's ScriptLocation; the run reaches JobRunState='SUCCEEDED', the script's S3 writes land, and a GlueContext job bookmark advances at job.commit() so a later run over the same prefix sees only new files (and a reset reprocesses everything). A workflow trigger that fires the job observes the same terminal state.
Emulator: the run ends JobRunState='FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' (`JOB_RUN_NOT_EXECUTED_MESSAGE` in services/service-com-amazonaws-glue-awsglue/src/state.rs, materialised by `JobRunEntry::settle_if_due`).
Why: no Glue data-plane behavior can be observed at all — bookmarks never advance, downstream objects are never written, and any orchestration gated on a SUCCEEDED run (workflow triggers, Step Functions sync tasks) can never proceed.
Fix: run the submitted script and drive the run to SUCCEEDED, including bookmark advancement at commit. PLATFORM-BLOCKED as of this report: the script a Glue job run executes is a PySpark/GlueContext program at the job's ScriptLocation, so serving this faithfully needs an in-process Spark + Python runtime, and this platform provides neither that nor the containers or external processes such a runtime would otherwise need. The refusal must NOT be re-worded in the meantime (answering a different error, or synthesising a SUCCEEDED state, a duration or side effects for work nothing ran, is worse than the honest failure), so the run keeps the message above until the runtime exists. The same family is open as 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md and 20261025T072821Z-glue-job-runs-are-terminated-failed-without-executing.md.
Traces: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file, glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the checkout that closed the other five causes:

Cause 1 — glue.deploy-glue-database-cfn-behaves: "botocore.errorfactory.EntityNotFoundException: An error occurred (EntityNotFoundException) when calling the GetDatabase operation: Database sim2real-glue-db-<id> not found." after CREATE_COMPLETE; glue.deploy-glue-database-via-cloudformation-behaves: same on GetDatabase; glue.deploy-crawler-via-cloudformation: "(EntityNotFoundException) when calling the GetCrawler operation: Crawler entry with name sim2real-glue-crawler-<id> does not exist"; glue.deploy-job-missing-output-permission-rejected: "(EntityNotFoundException) when calling the StartJobRun operation: Failed to start job run due to missing metadata."; glue.e2e-glue-tsd-database-tables-schema-lifecycle: "[phase 1/4 deploy] the stack's outputs are ('sim2real-scn-e2etsd-stk-<id>-CatalogDatabase-<suffix>', 'sim2real-scn-e2etsd-data-<id>'), expected ('sim2real_scn_e2etsd_<id>', 'sim2real-scn-e2etsd-data-<id>')" — the template's Output is {'Value': {'Ref': 'CatalogDatabase'}} over an AWS::Glue::Database whose DatabaseInput.Name is the parameter.

Cause 2 — glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file: "AssertionError: run 1 ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'"; glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds: "AssertionError: the standalone producer control ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'"

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running the check ids in trace_ids.
2. Cause 1: CreateStack with AWS::Glue::Database (DatabaseInput.Name from a parameter), AWS::Glue::Table, AWS::Glue::Crawler and AWS::Glue::Job plus an Output of {'Ref': <database logical id>}; after CREATE_COMPLETE call glue:GetDatabase / GetCrawler / StartJobRun on the declared names and read the Output.
3. Cause 2: upload a pythonshell/glueetl script that writes an S3 object, CreateJob over it, StartJobRun, then poll GetJobRun — expect JobRunState='SUCCEEDED' and the object present.
