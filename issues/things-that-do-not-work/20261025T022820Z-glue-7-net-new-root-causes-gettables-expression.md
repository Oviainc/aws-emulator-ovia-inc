Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.glue-tsd-get-tables-expression-star-is-a-wildcard-and-the-rest-is-regex glue.glue-tsd-get-tables-in-a-foreign-catalog-denied glue.gjwr-glue4-job-round-trips-declared-values-and-capacity-defaults glue.gjwr-start-job-run-through-other-region-rejected glue.deploy-glue-database-cfn-behaves glue.deploy-glue-database-via-cloudformation-behaves glue.deploy-crawler-via-cloudformation glue.deploy-job-missing-output-permission-rejected glue.e2e-glue-tsd-database-tables-schema-lifecycle glue__sqs.glue-crawler-missing-sqs-receive-permission-rejected glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds
Issue-Key: glue.glue-tsd-get-tables-expression-star-is-a-wildcard-and-the-rest-is-regex|run#9|AssertionError|assert wrong == {}, f"Expression selected the wrong tables; pattern -> (got, want): {wrong!r}"
Issue-Key: glue.glue-tsd-get-tables-in-a-foreign-catalog-denied|run#12|AssertionError|assert got == want, (
Issue-Key: glue.gjwr-glue4-job-round-trips-declared-values-and-capacity-defaults|run#14|AssertionError|assert filled == ("G.1X", 10, 10.0, {"MaxConcurrentRuns": 1}), (
Issue-Key: glue.gjwr-start-job-run-through-other-region-rejected|_error#6|AssertionError|raise AssertionError("the call under test SUCCEEDED where real AWS refuses it")
Issue-Key: glue|GetDatabase|EntityNotFoundException
Issue-Key: glue|GetCrawler|EntityNotFoundException
Issue-Key: glue|StartJobRun|EntityNotFoundException
Issue-Key: glue.e2e-glue-tsd-database-tables-schema-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: glue__sqs.glue-crawler-missing-sqs-receive-permission-rejected|_create_crawler_expect_error#16|AssertionError|raise AssertionError("expected CreateCrawler to be rejected, but it succeeded")
Issue-Key: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file|run#9|AssertionError|assert run1["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds|run#22|AssertionError|assert warmup["JobRunState"] == "SUCCEEDED", (
Issue-Commit: e9591ea0ffea53e5939624d35a733297d20386ca

# Glue: 7 net-new root causes — GetTables Expression is not a regex and a foreign CatalogId is not denied, Glue 4.0 job capacity defaults are never applied, jobs are not region-scoped, CFN-declared Glue resources are missing under their declared names, CreateCrawler skips the event-queue permission check, and job runs execute no script

> _Note: this issue was identified against the code base at commit `e9591ea0ffea53e5939624d35a733297d20386ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
This issue bundles 7 net-new root causes in which the emulator answers 2xx (or the wrong 4xx) for Glue requests whose semantics real AWS implements. Each cause must be closed before this issue is done.

Cause 1 of 7: GetTables ignores/does not implement the Expression filter as a regular expression.
AWS: Smithy GetTablesRequest.Expression (FilterString, length 0..2048) is documented as 'A regular expression pattern. If present, only those tables whose names match the pattern are returned.' Recorded: Expression='ev_.*' selects the tables ev_one and ev_two, and Expression='ev?' selects the table named ev (a bare '*' is additionally accepted as a match-everything wildcard).
Emulator: every one of those patterns selects NOTHING — got [] where ev_one/ev_two were owed for 'ev_.*' and [] where ev was owed for 'ev?'.
Why: a caller filtering a large catalog server-side silently receives an empty catalog rather than its matching tables — the failure mode is 'the database looks empty', which a client cannot distinguish from a real empty database.
Fix: the Expression handling in services/service-com-amazonaws-glue-awsglue/src/ops/get_tables.rs — match table names against the value as a regular expression (with '*' treated as the wildcard the recording shows), not as a literal or a glob.
Traces: glue.glue-tsd-get-tables-expression-star-is-a-wildcard-and-the-rest-is-regex.

Cause 2 of 7: a GetTables against another account's CatalogId is answered as not-found instead of denied.
AWS: Smithy GetTables takes CatalogId and models both EntityNotFoundException and (via the catalog resource-policy path) an access-denied refusal. Recorded: naming a CatalogId belonging to a different account, with no resource-based policy granting the caller, is refused with 400 AccessDeniedException whose message is built from the request (principal ARN, action glue:GetTables, the foreign catalog ARN) — the authorization decision happens before any lookup of the database.
Emulator: 400 EntityNotFoundException ('Database <name> not found.'), i.e. the foreign CatalogId is ignored and the lookup is performed in the caller's own catalog.
Why: cross-account catalog access is silently reinterpreted as local access. A caller probing whether it may read another account's catalog is told the database does not exist; worse, had the same-named database existed locally the call would have SUCCEEDED and returned the wrong account's tables.
Fix: resolve CatalogId in the Glue catalog lookup path and evaluate the catalog resource policy before the entity lookup, refusing with AccessDeniedException (400) when the caller has no grant on a foreign catalog.
Traces: glue.glue-tsd-get-tables-in-a-foreign-catalog-denied.

Cause 3 of 7: CreateJob does not apply the Glue 4.0 worker/capacity defaults, so GetJob reports an under-specified job.
AWS: Smithy CreateJob takes WorkerType (enum), NumberOfWorkers and MaxCapacity (all optional) with no modeled default; recorded behavior for a Glue 4.0 glueetl job created with none of them set is that the service fills them in and GetJob reads back WorkerType='G.1X', NumberOfWorkers=10 and MaxCapacity=10.0, alongside ExecutionProperty {'MaxConcurrentRuns': 1}.
Emulator: the same job reads back (None, None, None) for (WorkerType, NumberOfWorkers, MaxCapacity); only MaxConcurrentRuns=1 is present.
Why: the job representation is incomplete — a client that reads a job back to size a run, to re-create it from the read-back definition, or to display its capacity gets nulls where AWS always provides concrete values, and a round-trip re-create produces a different job.
Fix: the defaulting step of create_job.rs (and the symmetric path in update_job.rs) must materialize the version-dependent worker/capacity defaults into the stored job so GetJob/BatchGetJobs report them.
Traces: glue.gjwr-glue4-job-round-trips-declared-values-and-capacity-defaults.

Cause 4 of 7: Glue jobs are not scoped to the region they were created in, so a cross-region StartJobRun succeeds.
AWS: recorded behavior — a job created in region A does not exist in region B: StartJobRun in B is refused with 400 EntityNotFoundException (recorded example message 'Failed to start job run due to missing metadata.'), GetJobRuns in B is refused with 400 EntityNotFoundException, no run is recorded anywhere, and the job remains served in A with an empty run history. (Smithy: StartJobRun models EntityNotFoundException httpError 400.)
Emulator: StartJobRun through the other region's endpoint SUCCEEDS — the call under test returned 2xx and started a run for a job that region does not own.
Why: region isolation is absent for Glue state, which makes every region-scoping test pass incorrectly and lets a misconfigured client start billable runs against a region where the job does not exist; it also means two regions share one namespace, so a same-named job in two regions collides.
Fix: key the Glue job/run state by region in the service state and make the job-lookup path of start_job_run.rs / get_job_runs.rs refuse a job from another region with EntityNotFoundException.
Traces: glue.gjwr-start-job-run-through-other-region-rejected.

Cause 5 of 7: CloudFormation-declared AWS::Glue::Database / ::Table / ::Crawler / ::Job resources do not exist in Glue under their declared names after CREATE_COMPLETE, and Ref returns a generated physical id instead of the declared name.
AWS: a stack that reaches CREATE_COMPLETE has created its Glue entities: glue:GetDatabase returns the database with the Description from DatabaseInput and CatalogId = the account, glue:GetCrawler returns the crawler, glue:StartJobRun finds the job, and {'Ref': '<AWS::Glue::Database logical id>'} resolves to the database NAME (so a stack Output carrying that Ref is the catalog name a caller then passes to glue:GetDatabase/GetTable).
Emulator: the stack reports CREATE_COMPLETE, then glue:GetDatabase answers 400 EntityNotFoundException 'Database <declared name> not found.', glue:GetCrawler answers 400 EntityNotFoundException 'Crawler entry with name <declared name> does not exist', glue:StartJobRun answers 400 EntityNotFoundException 'Job <declared name> not found', and the stack Output for Ref of the AWS::Glue::Database resolves to a CFN-generated physical id of the form <stack-name>-<logical-id>-<suffix> instead of the declared database name.
Why: nothing deployed through CloudFormation is usable through the Glue API — the deploy-then-use flow (the normal way these resources are created) breaks at the first Glue call, and the generated physical id means even a caller that reads the name out of the stack Outputs queries a name that does not exist.
Fix: the AWS::Glue::Database / ::Table / ::Crawler / ::Job resource providers in the CloudFormation provisioning engine must create the entities in the Glue service state and use the declared name (DatabaseInput.Name / TableInput.Name / Name) as the resource's physical id that Ref returns. Related open issues name neighbouring symptoms but carry none of these replay keys: 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md (Cause 8, AWS::Glue::Database/Table, athena__cloudformation__glue check ids) and 20261022T131025Z-glue-createjob-is-a-501-stub-gettable-ignores.md.
Traces: glue.deploy-glue-database-cfn-behaves, glue.deploy-glue-database-via-cloudformation-behaves, glue.deploy-crawler-via-cloudformation, glue.deploy-job-missing-output-permission-rejected, glue.e2e-glue-tsd-database-tables-schema-lifecycle.

Cause 6 of 7: CreateCrawler does not validate the crawler role's SQS access to the S3 target's EventQueueArn.
AWS: Smithy CreateCrawler (Name, Role, Targets required) models InvalidInputException (400). Recorded: for an S3Target that carries an EventQueueArn with RecrawlPolicy RecrawlBehavior=CRAWL_EVENT_MODE, Glue checks at CREATE time that the supplied Role can receive from that queue; a role with no SQS grant on it is refused with 400 InvalidInputException and NO crawler is created, while an identically-shaped request whose role does hold the grant is created.
Emulator: CreateCrawler SUCCEEDS for the role with no SQS grant, and the crawler exists.
Why: the negative path of event-mode crawler provisioning inverts — a client is told its wiring is valid, and the failure moves from create time to crawl time (or, in an emulator that never crawls, to nowhere at all). This is a distinct check from the role TRUST validation already filed in 20261022T192452Z-glue-createcrawler-skips-role-validation-and-deleteschemaversions-raises.md (Cause 1), which that issue's traces exercise with an un-assumable role; here the role is assumable but lacks sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the EventQueueArn.
Fix: in services/service-com-amazonaws-glue-awsglue/src/ops/create_crawler.rs, when an S3Target declares EventQueueArn, evaluate the Role's policy against that queue in the IAM/SQS state before persisting, and refuse with InvalidInputException.
Traces: glue__sqs.glue-crawler-missing-sqs-receive-permission-rejected.

Cause 7 of 7: a started job run executes no script, so every run ends FAILED and produces no side effects.
AWS: StartJobRun runs the job's ScriptLocation; the run reaches JobRunState='SUCCEEDED', the script's S3 writes land, and a GlueContext job bookmark advances at job.commit() so a later run over the same prefix sees only new files (and a reset reprocesses everything). A workflow trigger that fires the job observes the same terminal state.
Emulator: the run ends JobRunState='FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.'
Why: no Glue data-plane behavior can be observed at all — bookmarks never advance, downstream objects are never written, and any orchestration gated on a SUCCEEDED run (workflow triggers, Step Functions sync tasks) can never proceed. The open issue 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md describes this family for other check ids and does not carry the two replay keys below.
Fix: job-run execution in the Glue job-run state machine — run the submitted script (or a faithful emulation of its effects) and drive the run to SUCCEEDED, including bookmark advancement at commit.
Traces: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file, glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout (20/223 checks passed).

Cause 1 — glue.glue-tsd-get-tables-expression-star-is-a-wildcard-and-the-rest-is-regex: "AssertionError: Expression selected the wrong tables; pattern -> (got, want): {'ev_.*': ([], ['ev_one', 'ev_two']), 'ev?': ([], ['ev'])}".
Cause 2 — glue.glue-tsd-get-tables-in-a-foreign-catalog-denied: "AssertionError: GetTables against a foreign CatalogId returned ('EntityNotFoundException', 'Database <db> not found.'), expected ('AccessDeniedException', 'User: arn:aws:iam::<acct>:user/... is not authorized to perform: glue:GetTables on resource: arn:aws:glue:<region>:<other-acct>:catalog because no resource-based policy allows the glue:GetTables action')" — the requirement is the 400 AccessDeniedException code; the message text is the recorded example of the request-derived template.
Cause 3 — glue.gjwr-glue4-job-round-trips-declared-values-and-capacity-defaults: "AssertionError: defaults for an omitted worker config came back as (None, None, None, {'MaxConcurrentRuns': 1}), expected ('G.1X', 10, 10.0, {'MaxConcurrentRuns': 1})".
Cause 4 — glue.gjwr-start-job-run-through-other-region-rejected: "AssertionError: the call under test SUCCEEDED where real AWS refuses it" at `got = _error(lambda: g_other.start_job_run(JobName=name))`; AWS answers EntityNotFoundException for both StartJobRun and GetJobRuns in the other region.
Cause 5 — glue.deploy-glue-database-cfn-behaves: "botocore.errorfactory.EntityNotFoundException: An error occurred (EntityNotFoundException) when calling the GetDatabase operation: Database sim2real-glue-db-<id> not found." after CREATE_COMPLETE; glue.deploy-glue-database-via-cloudformation-behaves: same on GetDatabase; glue.deploy-crawler-via-cloudformation: "(EntityNotFoundException) when calling the GetCrawler operation: Crawler entry with name sim2real-glue-crawler-<id> does not exist"; glue.deploy-job-missing-output-permission-rejected: "(EntityNotFoundException) when calling the StartJobRun operation: Job sim2real-glue-jobgood-<id> not found"; glue.e2e-glue-tsd-database-tables-schema-lifecycle: "[phase 1/4 deploy] the stack's outputs are ('sim2real-scn-e2etsd-stk-<id>-CatalogDatabase-<suffix>', 'sim2real-scn-e2etsd-data-<id>'), expected ('sim2real_scn_e2etsd_<id>', 'sim2real-scn-e2etsd-data-<id>')" — the template's Output is {'Value': {'Ref': 'CatalogDatabase'}} over an AWS::Glue::Database whose DatabaseInput.Name is the parameter.
Cause 6 — glue__sqs.glue-crawler-missing-sqs-receive-permission-rejected: "AssertionError: expected CreateCrawler to be rejected, but it succeeded" for CreateCrawler(Role=<role without an SQS grant>, Targets={'S3Targets': [{'Path': 's3://<bucket>/bad/', 'EventQueueArn': <queue arn>}]}, RecrawlPolicy={'RecrawlBehavior': 'CRAWL_EVENT_MODE'}); expected 400 InvalidInputException.
Cause 7 — glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file: "AssertionError: run 1 ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'"; glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds: "AssertionError: the standalone producer control ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'"

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running the check ids in trace_ids.
2. Cause 1: create tables ev, ev_one, ev_two in a database, then glue:GetTables(DatabaseName=..., Expression='ev_.*') — expect ev_one and ev_two; and Expression='ev?' — expect ev.
3. Cause 2: glue:GetTables(CatalogId=<another account id>, DatabaseName=<any>) — expect 400 AccessDeniedException naming glue:GetTables on the foreign catalog ARN, not EntityNotFoundException.
4. Cause 3: glue:CreateJob with GlueVersion='4.0', Command {'Name': 'glueetl'} and no WorkerType/NumberOfWorkers/MaxCapacity, then glue:GetJob — expect WorkerType='G.1X', NumberOfWorkers=10, MaxCapacity=10.0.
5. Cause 4: CreateJob in region A, then StartJobRun and GetJobRuns through region B's endpoint — expect 400 EntityNotFoundException from both, no run anywhere, and the job still served in A with an empty run history.
6. Cause 5: CreateStack with AWS::Glue::Database (DatabaseInput.Name from a parameter), AWS::Glue::Table, AWS::Glue::Crawler and AWS::Glue::Job plus an Output of {'Ref': <database logical id>}; after CREATE_COMPLETE call glue:GetDatabase / GetCrawler / StartJobRun on the declared names and read the Output.
7. Cause 6: create a queue and a Glue-assumable role with NO sqs permissions on it, then glue:CreateCrawler with an S3Target carrying that EventQueueArn and RecrawlBehavior=CRAWL_EVENT_MODE — expect 400 InvalidInputException and no crawler; repeat with a role that holds the SQS grant — expect success.
8. Cause 7: upload a pythonshell/glueetl script that writes an S3 object, CreateJob over it, StartJobRun, then poll GetJobRun — expect JobRunState='SUCCEEDED' and the object present.
