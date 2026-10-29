Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.reject-duplicate-job-name glue.trigger-create-with-nonexistent-job-allowed glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative glue.blueprint-lifecycle-create-run-delete glue.e2e-blueprint-deployment-flow glue.e2e-blueprint-run-deploys-workflow-flow glue.blueprint-run-reaches-terminal-state glue.start-blueprint-run-illegal-state-rejected glue.column-statistics-task-run-poll-until-terminal-within-timeout glue.e2e-column-statistics-task-run-flow glue.glossary-term-lifecycle-create-get-delete glue.glossary-term-update-nonexistent-rejected glue.usage-profile-lifecycle-create-get-delete glue.glossary-term-lifecycle-create-associate-delete glue.e2e-glossary-term-association-workflow glue.session-idle-timeout-transitions-to-timeout-state
Issue-Key: glue|CreateJob|AlreadyExistsException
Issue-Key: glue|CreateTrigger|EntityNotFoundException
Issue-Key: glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative|run#9|AssertionError|assert status in {"ROLLBACK_COMPLETE", "ROLLBACK_FAILED"}, (
Issue-Key: glue.blueprint-lifecycle-create-run-delete|run#28|AssertionError|assert job_names, f"blueprint run did not materialize any Job node in workflow {workflow_name!r}"
Issue-Key: glue.e2e-blueprint-deployment-flow|run#30|AssertionError|assert job_nodes, f"blueprint-generated workflow has no job node: {workflow['Workflow']['Graph']}"
Issue-Key: glue.e2e-blueprint-run-deploys-workflow-flow|run#25|AssertionError|assert job_nodes, f"blueprint-generated workflow has no job node: {workflow['Workflow']['Graph']}"
Issue-Key: glue.blueprint-run-reaches-terminal-state|run#20|AssertionError|assert final_run["State"] == "FAILED", (
Issue-Key: glue.start-blueprint-run-illegal-state-rejected|run#16|AssertionError|assert final_status == "FAILED", f"expected the blueprint to settle on FAILED, got {final_status!r}"
Issue-Key: glue.column-statistics-task-run-poll-until-terminal-within-timeout|run#14|AssertionError|assert final_status == "SUCCEEDED", f"expected column stats task to succeed within 900s, got {final_status}"
Issue-Key: glue.e2e-column-statistics-task-run-flow|run#18|AssertionError|assert final_status == "SUCCEEDED", f"column statistics task run ended in {final_status}"
Issue-Key: glue.glossary-term-lifecycle-create-get-delete|run#23|AssertionError|assert code == "EntityNotFoundException", (
Issue-Key: glue.glossary-term-update-nonexistent-rejected|run#4|AssertionError|assert code == "EntityNotFoundException", code
Issue-Key: glue.usage-profile-lifecycle-create-get-delete|run#18|AssertionError|assert update_code == "EntityNotFoundException", (
Issue-Key: glue.glossary-term-lifecycle-create-associate-delete|run#20|KeyError|assert before["GlossaryTerms"] == []
Issue-Key: glue.e2e-glossary-term-association-workflow|_poll_associate#17|Timeout|_time.sleep(interval_s)
Issue-Key: glue.session-idle-timeout-transitions-to-timeout-state|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Glue: 9 net-new root causes in control-plane semantics — CreateJob is not idempotent, CreateTrigger refuses a late-bound job, CreateCrawler skips the event-mode SQS permission check, blueprint runs deploy nothing, column-stats runs always fail, GetAsset omits GlossaryTerms, UpdateGlossaryTerm/UpdateUsageProfile are stubs, and sessions never expire

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
9 causes; all nine must be closed before submitting.

Related open issues, named for the fixer, none of which carries the replay keys listed here: 20261029T163346Z-glue-createblueprint-registration-for-an-unresolvable-blueprintlocation-settles.md and 20261028T175030Z-glue-blueprint-registration-always-settles-active-a-blueprintlocation.md describe the same defect as Cause 5; 20261025T170315Z-glue-2-net-new-root-causes-column-statistics.md and 20261027T060729Z-glue-a-column-statistics-task-run-auto-started.md describe the same defect as Cause 6; 20261029T103029Z-glue-2-net-new-root-causes-getdashboardurl-s.md (its Cause 2) describes the same defect as Cause 9. The causes are restated here so the checks listed under them are covered.

Cause 1 of 9: CreateJob is not idempotent on the job name — an identical repeat is refused.
AWS: Smithy: CreateJob takes required Name/Role/Command and models both AlreadyExistsException and IdempotentParameterMismatchException (client, HTTP 400). Real Glue treats CreateJob as idempotent on Name: a repeat call carrying the IDENTICAL definition succeeds silently and returns the same Name, while a repeat carrying a DIFFERENT definition is refused with IdempotentParameterMismatchException rather than overwriting the job.
Emulator: the identical repeat is refused with AlreadyExistsException and the message `Job already exists: <job-name>`, so the check cannot even reach its differing-definition assertion.
Why: a client or IaC tool that re-submits an unchanged CreateJob (the normal convergent-create pattern) gets a hard failure where AWS returns success, and the one condition AWS does refuse — a conflicting redefinition — is never distinguishable by its own error type.
Fix: the CreateJob handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_job.rs) must compare the incoming definition with the stored one: equal -> 200 with Name; different -> IdempotentParameterMismatchException.
Traces: glue.reject-duplicate-job-name.

Cause 2 of 9: CreateTrigger validates its action's job at create time; AWS late-binds it.
AWS: Smithy: CreateTrigger takes required Actions (ActionList) and models EntityNotFoundException, but the job reference inside an Action is resolved only when the trigger FIRES. Real Glue accepts a trigger whose Actions[].JobName names no job, stores the dangling reference, and GetTrigger reports Actions[0].JobName unchanged with State `CREATED`.
Emulator: CreateTrigger is refused with EntityNotFoundException / `Job not found.`
Why: the emulator is stricter than AWS, so the normal ordering of a deployment (create triggers, then the jobs they point at) is impossible, and no trigger can ever be created ahead of its job.
Fix: remove the create-time job-existence check from the CreateTrigger handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_trigger.rs); resolve the reference at fire time instead.
Traces: glue.trigger-create-with-nonexistent-job-allowed.

Cause 3 of 9: CreateCrawler accepts a CRAWL_EVENT_MODE crawler whose role cannot consume the event queue.
AWS: recorded behavior (and Smithy: CreateCrawler models InvalidInputException/CrawlerRunningException, takes Targets.S3Targets[].EventQueueArn and RecrawlPolicy.RecrawlBehavior=CRAWL_EVENT_MODE): real Glue validates AT CREATE TIME that the crawler's Role can consume the queue named by EventQueueArn. A role carrying only S3 read permissions is refused, naming the missing action `SQS:ReceiveMessage`; through CloudFormation the AWS::Glue::Crawler resource therefore goes CREATE_FAILED with HandlerErrorCode InvalidRequest and the stack rolls back to ROLLBACK_COMPLETE, leaving no crawler (GetCrawler then answers EntityNotFoundException `Crawler entry with name <name> does not exist`). Granting sqs:ReceiveMessage/sqs:DeleteMessage on the same role/queue pair makes the identical CreateCrawler succeed.
Emulator: the crawler is created unconditionally, so the stack reaches CREATE_COMPLETE and the misconfigured crawler exists.
Why: an overly-permissive create: a deployment that AWS refuses outright appears to succeed, and the missing SQS permission is only discovered (if ever) much later. Any negative test of event-mode wiring passes vacuously.
Fix: the CreateCrawler handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_crawler.rs) must, when RecrawlBehavior is CRAWL_EVENT_MODE and an EventQueueArn is given, evaluate the crawler Role's permissions on that queue and refuse with Glue's InvalidRequest naming SQS:ReceiveMessage.
Traces: glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative (asserts the stack status, the Crawler(CREATE_FAILED) reason text and the absence of the crawler afterwards).

Cause 4 of 9: a blueprint run deploys no workflow entities, and a run that deploys nothing still reports SUCCEEDED.
AWS: Smithy: StartBlueprintRun takes BlueprintName+RoleArn and returns RunId; GetBlueprintRun reports State (BlueprintRunState) and WorkflowName. A successful blueprint run materializes the workflow the blueprint's layout script creates — GetWorkflow(...IncludeGraph=True).Graph carries the Job/Crawler/Trigger Nodes and their Edges. A blueprint whose layout creates zero entities is unrunnable: real Glue ends that run FAILED, not SUCCEEDED.
Emulator: GetBlueprintRun reports SUCCEEDED (with StartedOn/CompletedOn stamped) but the generated workflow's graph is empty — `{'Nodes': [], 'Edges': []}` — and a zero-entity blueprint also reports SUCCEEDED.
Why: the run outcome is decoupled from any effect, so a client that waits for SUCCEEDED and then operates on the deployed workflow finds nothing, and the one case AWS fails is reported as success — a false green in both directions.
Fix: blueprint-run execution and state in the glue crate (services/service-com-amazonaws-glue-awsglue/src/ops/start_blueprint_run.rs and the blueprint-run state it drives): execute the blueprint's layout so the workflow graph is populated, and settle the run FAILED when it produces no entities.
Traces: glue.blueprint-lifecycle-create-run-delete, glue.e2e-blueprint-deployment-flow, glue.e2e-blueprint-run-deploys-workflow-flow, glue.blueprint-run-reaches-terminal-state.

Cause 5 of 9: a blueprint registered against an unresolvable BlueprintLocation settles ACTIVE instead of FAILED.
AWS: Smithy: CreateBlueprint takes BlueprintLocation and Blueprint$Status targets BlueprintStatus (CREATING, ACTIVE, UPDATING, FAILED); a blueprint whose BlueprintLocation does not resolve to a readable archive in S3 reaches terminal FAILED with an ErrorMessage, never ACTIVE, and StartBlueprintRun against it is then refused with IllegalBlueprintStateException.
Emulator: GetBlueprint reports ACTIVE for a location that does not exist, so the illegal-state refusal can never be observed.
Why: registration validity is never asserted, so a broken blueprint looks deployable and the IllegalBlueprintStateException branch is unreachable.
Fix: the CreateBlueprint registration path and blueprint status state (services/service-com-amazonaws-glue-awsglue/src/ops/create_blueprint.rs) must resolve BlueprintLocation and settle FAILED with an ErrorMessage when it cannot be read.
Traces: glue.start-blueprint-run-illegal-state-rejected.

Cause 6 of 9: every column-statistics task run settles FAILED with an emulator-authored ErrorMessage and computes no statistics.
AWS: Smithy: StartColumnStatisticsTaskRun returns ColumnStatisticsTaskRunId; ColumnStatisticsTaskRun$Status targets ColumnStatisticsState (STARTING, RUNNING, SUCCEEDED, FAILED, STOPPED) and the run carries DPUSeconds. A run over a well-formed table with a correctly scoped Role reads the table's underlying data and reaches SUCCEEDED, after which GetColumnStatisticsForTable returns the computed statistics.
Emulator: the run is recorded and immediately settled Status `FAILED`, DPUSeconds 0.0, with the constant emulator ErrorMessage `This emulator records column statistics task runs but reads none of the table's underlying data, so the run ended without generating statistics.`
Why: SUCCEEDED is unreachable, so no caller can ever obtain column statistics, and a genuine failure is indistinguishable from the emulator's blanket one.
Fix: the column-statistics task-run execution component (services/service-com-amazonaws-glue-awsglue/src/ops/start_column_statistics_task_run.rs plus the task-run state) must read the table's data and settle SUCCEEDED, populating the statistics; the emulator sentence must not be produced.
Traces: glue.column-statistics-task-run-poll-until-terminal-within-timeout, glue.e2e-column-statistics-task-run-flow.

Cause 7 of 9: UpdateGlossaryTerm and UpdateUsageProfile are unimplemented scaffold stubs.
AWS: Smithy: UpdateGlossaryTerm and UpdateUsageProfile are modeled operations; each models EntityNotFoundException (client, HTTP 400), so an update naming a deleted or never-created identifier must be refused with EntityNotFoundException, and an update of an existing resource must apply.
Emulator: both answer `NotImplemented` / `operation <Op> is scaffolded but not implemented in this emulator`, which is not a modeled error for either operation.
Why: the update half of both lifecycles is absent, and the not-found branch a client depends on to detect a deleted resource cannot be exercised at all. (Note: the sibling checks glue.glossary-lifecycle-create-get-delete, glue.glossary-lifecycle-create-get-delete-404 and glue.glossary-delete-with-terms-attached-accepted fail the same way on UpdateGlossary, which the already-filed UpdateGlossary stub issue owns; they are intentionally not re-filed here.)
Fix: implement the UpdateGlossaryTerm and UpdateUsageProfile handlers (services/service-com-amazonaws-glue-awsglue/src/ops/update_glossary_term.rs, update_usage_profile.rs) with their EntityNotFoundException paths.
Traces: glue.glossary-term-lifecycle-create-get-delete, glue.glossary-term-update-nonexistent-rejected, glue.usage-profile-lifecycle-create-get-delete.

Cause 8 of 9: GetAsset omits GlossaryTerms, and a catalog table is never resolvable as an asset.
AWS: Smithy: GetAsset output carries GlossaryTerms (GlossaryTermIdList) alongside Id/AssetTypeId/Forms, and AssociateGlossaryTerms attaches terms to an asset. Recorded behavior: GetAsset for an asset with no terms attached returns GlossaryTerms as an empty list (the member is present), and after AssociateGlossaryTerms it lists the associated term ids; a catalog table is indexed as a business-context asset, so AssociateGlossaryTerms against the table's ARN succeeds (possibly after a short indexing delay) rather than failing forever.
Emulator: GetAsset's response has no GlossaryTerms member at all for an asset created with PutAsset (the client raises KeyError: 'GlossaryTerms'), and AssociateGlossaryTerms against a catalog table's ARN keeps answering EntityNotFoundException until the caller gives up. Hypothesis: the asset store is populated only by PutAsset and carries no glossary-term field, so catalog tables are never projected into it.
Why: a client cannot read an asset's glossary associations (the member a caller indexes is absent instead of empty), and the whole table-to-glossary association workflow is unreachable.
Fix: the asset representation and the asset lookup used by GetAsset/AssociateGlossaryTerms (services/service-com-amazonaws-glue-awsglue/src/ops/get_asset.rs, associate_glossary_terms.rs plus the asset state) — always serialize GlossaryTerms, and resolve a catalog table ARN as an asset.
Traces: glue.glossary-term-lifecycle-create-associate-delete (KeyError on GetAsset), glue.e2e-glossary-term-association-workflow (AssociateGlossaryTerms against a table ARN 404s until the budget is exhausted).

Cause 9 of 9: an idle interactive Session never honours IdleTimeout — it never leaves READY for TIMEOUT.
AWS: Smithy: CreateSession accepts IdleTimeout (Timeout, @range min 1, minutes) and Session$Status targets SessionStatus whose values include TIMEOUT; Session carries CompletedOn and ExecutionTime. After a session becomes READY and no statement is run for IdleTimeout minutes, real Glue expires it server-side and GetSession reports Status TIMEOUT. The transition must also not fire early (not before roughly half the declared window).
Emulator: GetSession keeps reporting a non-terminal status for a session created with IdleTimeout=1 and left completely idle; none of TIMEOUT/FAILED/STOPPED is ever observed and the poll exhausts its budget. Hypothesis: there is no idle-expiry reaper, so TIMEOUT is a modeled status no code path can produce.
Why: a client relying on Glue to reclaim idle sessions sees a session that stays READY forever, any workflow waiting on TIMEOUT hangs, and CompletedOn/ExecutionTime are never populated for an expired session.
Fix: session lifecycle state in the glue crate — a time-driven expiry that moves a READY session with no statement activity for IdleTimeout minutes to TIMEOUT and stamps CompletedOn/ExecutionTime, observable through GetSession without a client call.
Traces: glue.session-idle-timeout-transitions-to-timeout-state.

## Evidence
Cause 1 — glue.reject-duplicate-job-name: the identical repeat CreateJob raised botocore.errorfactory.AlreadyExistsException: An error occurred (AlreadyExistsException) when calling the CreateJob operation: Job already exists: <job-name>. Expected: 200 for the identical repeat, IdempotentParameterMismatchException only for a differing definition.
Cause 2 — glue.trigger-create-with-nonexistent-job-allowed: CreateTrigger(Type='ON_DEMAND', Actions=[{'JobName': <no such job>}]) -> EntityNotFoundException / 'Job not found.' Expected: 200, then GetTrigger showing that JobName and State 'CREATED'.
Cause 3 — glue__sqs.deploy-sqs-queue-policy-missing-glue-principal-negative: expected stack status ROLLBACK_COMPLETE with reasons containing 'Crawler(CREATE_FAILED)', 'SQS:ReceiveMessage' and 'HandlerErrorCode: InvalidRequest'; actual CREATE_COMPLETE.
Cause 4 — glue.e2e-blueprint-deployment-flow: AssertionError: blueprint-generated workflow has no job node: {'Nodes': [], 'Edges': []}
        glue.blueprint-run-reaches-terminal-state: expected FAILED for a zero-entity workflow, got State 'SUCCEEDED' (GetBlueprintRun with StartedOn/CompletedOn set).
        glue.blueprint-lifecycle-create-run-delete: 'blueprint run did not materialize any Job node in workflow <name>'.
Cause 5 — glue.start-blueprint-run-illegal-state-rejected: expected the blueprint to settle FAILED, got 'ACTIVE'.
Cause 6 — glue.column-statistics-task-run-poll-until-terminal-within-timeout: expected SUCCEEDED, got FAILED.
        glue.e2e-column-statistics-task-run-flow: 'column statistics task run ended in FAILED'; the run record carries Status FAILED, DPUSeconds 0.0 and ErrorMessage "This emulator records column statistics task runs but reads none of the table's underlying data, so the run ended without generating statistics."
Cause 7 — glue.glossary-term-lifecycle-create-get-delete: expected EntityNotFoundException from UpdateGlossaryTerm on a deleted id, got 'NotImplemented'
        glue.glossary-term-update-nonexistent-rejected: 'NotImplemented'
        glue.usage-profile-lifecycle-create-get-delete: expected UpdateUsageProfile on a deleted profile to raise EntityNotFoundException, got 'NotImplemented'
Cause 8 — glue.glossary-term-lifecycle-create-associate-delete: KeyError: 'GlossaryTerms' on get_asset(Identifier=<asset created by PutAsset>) where AWS returns an empty list
        glue.e2e-glossary-term-association-workflow: Check timed out after 60.0s while AssociateGlossaryTerms(AssetIdentifier=<table ARN>) kept answering EntityNotFoundException
Cause 9 — glue.session-idle-timeout-transitions-to-timeout-state: Check timed out after 60.0s polling GetSession for a terminal status after CreateSession(IdleTimeout=1) with no statement run (the exhausted budget is the observation; the missing reaper is labelled Hypothesis).

## Steps to Reproduce
Cause 1: CreateJob(Name=J, Role=R, Command=C, MaxCapacity=0.0625, Timeout=10); repeat the identical call -> must succeed (200, Name=J); repeat with Timeout=20 -> must be IdempotentParameterMismatchException.
Cause 2: CreateTrigger(Name=T, Type='ON_DEMAND', Actions=[{'JobName':'no-such-job'}]) -> 200; GetTrigger(Name=T).Trigger.Actions[0].JobName == 'no-such-job' and State == 'CREATED'.
Cause 3: deploy a stack with AWS::SQS::Queue, AWS::S3::Bucket, an IAM role whose policy grants only S3 reads, AWS::Glue::Database and AWS::Glue::Crawler with Targets.S3Targets=[{Path:s3://<bucket>/data/, EventQueueArn:<queue arn>}] and RecrawlPolicy.RecrawlBehavior=CRAWL_EVENT_MODE -> the Crawler resource must be CREATE_FAILED (HandlerErrorCode InvalidRequest naming SQS:ReceiveMessage) and the stack must reach ROLLBACK_COMPLETE; GetCrawler(Name=<crawler>) must then be EntityNotFoundException 'Crawler entry with name <crawler> does not exist'. Granting sqs:ReceiveMessage/sqs:DeleteMessage on the same role/queue must make the identical CreateCrawler succeed.
Cause 4: CreateBlueprint with a layout that creates a job+trigger workflow; StartBlueprintRun; poll GetBlueprintRun until terminal -> SUCCEEDED, and GetWorkflow(Name=<WorkflowName>, IncludeGraph=True).Workflow.Graph.Nodes must contain the JOB node. Repeat with a blueprint whose layout creates nothing -> the run must end FAILED.
Cause 5: CreateBlueprint(Name=B, BlueprintLocation='s3://<bucket>/does-not-exist.zip') -> poll GetBlueprint: Status must reach FAILED with an ErrorMessage; StartBlueprintRun(BlueprintName=B) must then be IllegalBlueprintStateException.
Cause 6: create a database+table over real S3 data, then StartColumnStatisticsTaskRun(DatabaseName=D, TableName=T, Role=R, ColumnNameList=['id']) -> poll GetColumnStatisticsTaskRun until terminal: Status must be SUCCEEDED with no ErrorMessage, and GetColumnStatisticsForTable must return the computed statistics.
Cause 7: CreateGlossaryTerm then DeleteGlossaryTerm, then UpdateGlossaryTerm(Identifier=<deleted id>) -> EntityNotFoundException; CreateUsageProfile then DeleteUsageProfile, then UpdateUsageProfile(Name=<deleted>) -> EntityNotFoundException.
Cause 8: PutFormType + PutAssetType + PutAsset(Identifier=A), then GetAsset(Identifier=A) -> the response must contain GlossaryTerms == []; AssociateGlossaryTerms(AssetIdentifier=A, GlossaryTermIdentifiers=[term]) then GetAsset(A).GlossaryTerms must contain the term. Repeat with AssetIdentifier=<a catalog table ARN> -> must succeed instead of failing EntityNotFoundException forever.
Cause 9: CreateSession(Id=S, Role=R, Command={'Name':'glueetl','PythonVersion':'3'}, GlueVersion='4.0', WorkerType='G.1X', NumberOfWorkers=2, IdleTimeout=1); wait for READY; run no statement; poll GetSession -> Status must become TIMEOUT, and must not be TIMEOUT sooner than ~30s after READY.
