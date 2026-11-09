Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-serverless-workflow-run-two-dependent-tasks-succeed
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-serverless-workflow-run-two-dependent-tasks-succeed
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless|CreateWorkflow|2xx->2xx|$.CreatedAt
Issue-Key: mwaa-serverless|CreateWorkflow|2xx->2xx|$.RevisionId
Issue-Key: mwaa-serverless|GetWorkflowRun|2xx->2xx|$.OverrideParameters
Issue-Key: mwaa-serverless|GetWorkflowRun|2xx->2xx|$.RunDetail.CompletedOn
Issue-Key: mwaa-serverless|GetWorkflowRun|2xx->2xx|$.RunDetail.CreatedAt
Issue-Key: mwaa-serverless|GetWorkflowRun|2xx->2xx|$.RunDetail.ModifiedAt
Issue-Key: mwaa-serverless|GetWorkflowRun|2xx->2xx|$.RunDetail.StartedOn
Issue-Key: mwaa-serverless|GetWorkflowRun|2xx->2xx|$.RunDetail.TaskInstances
Issue-Key: mwaa-serverless|ListTaskInstances|2xx->2xx|$.TaskInstances[*]
Issue-Key: mwaa-serverless|GetWorkflow|2xx->2xx|$.CreatedAt
Issue-Key: mwaa-serverless|GetWorkflow|2xx->2xx|$.LoggingConfiguration
Issue-Key: mwaa-serverless|GetWorkflow|2xx->2xx|$.ModifiedAt
Issue-Key: mwaa-serverless|GetWorkflow|2xx->2xx|$.TriggerMode
Issue-Commit: ab6fe5351849b3ed2ec61e1c169bea3a72511392

# mwaa-serverless responses diverge: epoch timestamps, missing service defaults, un-echoed run parameters and no task instances

> _Note: this issue was identified against the code base at commit `ab6fe5351849b3ed2ec61e1c169bea3a72511392`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow, GetWorkflow, GetWorkflowRun, ListTaskInstances

## Description
FIVE distinct net-new root causes in the `mwaa-serverless` emulator, all grounded in wire-fixture divergences replayed from trace set simparity-wire-aws-v1. Every cause is a read-path defect: the HTTP status is right (200 everywhere) and the body is wrong — a member is serialized in the wrong wire form, or is absent where AWS reports a value. All five must be closed before this issue is submitted.

Model used throughout: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json (sdkId "MWAA Serverless", protocol awsJson1_0).

=== Cause 1 of 5: every modeled timestamp is serialized as an epoch-seconds number instead of an ISO-8601 date-time string ===
AWS: Smithy: every timestamp member of this service targets `TimestampValue`, which carries @timestampFormat("date-time") — the model hoists it onto each member, e.g. CreateWorkflowResponse$CreatedAt {target: TimestampValue, timestampFormat: date-time}, GetWorkflowResponse$CreatedAt/$ModifiedAt/$CodeSnapshottedAt, WorkflowRunDetail$CreatedAt/$StartedOn/$CompletedOn/$ModifiedAt. In awsJson1_0 the protocol default for a timestamp is epoch-seconds, so @timestampFormat("date-time") OVERRIDES it: AWS emits an RFC3339/ISO-8601 JSON *string* (`"2026-11-09T12:34:56.789Z"`) for these members, not a number. The recording confirms it — the comparator shape-classifies the recorded value as a date-time string and the emulator's number does not belong to that class.
Emulator: emits a JSON number of epoch seconds with a fractional part — `$.CreatedAt: 1794246357.4472766` on CreateWorkflow, `$.CreatedAt`/`$.ModifiedAt: 1794246357.4472766` on GetWorkflow, and `$.RunDetail.CreatedAt`/`StartedOn`/`ModifiedAt`/`CompletedOn: 1794246367.4690564`/`1794246368.4690564` on GetWorkflowRun. The handlers build the value correctly (`crate::state::timestamp` returns an `aws_smithy_types::DateTime`, state.rs lines 2420-2423); it is the wire form that is wrong, so the fault is in serialization, not in the handler's value.
Why: a client deserializing with a generated SDK for this model expects a date-time string for these members and fails to parse a number (or silently yields a zero/epoch instant), so every control-plane read of a workflow and every poll of a run breaks at the deserializer — the one thing a client does on EVERY call. Hand-written clients get a timestamp 55 years off if they treat the number as milliseconds.
Fix: the awsJson1_0 output serialization path for this service's timestamps — the generated member/shape schemas must carry the model's @timestampFormat("date-time") through to the serializer (or the shared serializer must stop falling back to the protocol default when the trait is present). One fix covers all four operations; nothing in src/ops/ needs to change.
Traces: mwaa-serverless-workflow-run-two-dependent-tasks-succeed#task#6 (CreateWorkflow), #task#8 (GetWorkflowRun), #task#10 (GetWorkflow).

=== Cause 2 of 5: CreateWorkflow's RevisionId is minted in the wrong identifier form ===
AWS: Smithy: CreateWorkflowResponse$RevisionId targets `String` with no length or pattern constraint; its documentation says it is "A unique identifier for this revision of the workflow configuration" used "for optimistic concurrency control in update operations". Recorded behavior only: the recorded RevisionId is shape-asserted, and its shape class is NOT a 32-character lowercase-hex digest — note that in the same fixture the separately-modeled WorkflowVersion (`WorkflowVersion`, @length 32..32, pattern ^[a-z0-9]{32}$, recorded value `f96612fbf07a7b77171cd682fb38021e`) DOES match and passes, so RevisionId is a differently-shaped identifier than the version id. Hypothesis: AWS mints it as a canonical 36-character hyphenated UUID.
Emulator: answers `$.RevisionId: 9895198a5dc6d9e0452733da25d20197` — the first 32 characters of a SHA-256 digest over ("workflow-revision", workflow ARN, version), i.e. the same 32-hex shape it uses for WorkflowVersion, which the comparator rejects as the wrong shape class for this member.
Why: a client that round-trips or stores the revision id against a column/type sized or validated for AWS's identifier form (e.g. a UUID column, or a regex check distinguishing a revision id from a version id) rejects or truncates the emulator's value; code that tells the two identifiers apart by their shape cannot, because the emulator makes them indistinguishable.
Fix: the revision-id minting function in the service state (`mint_revision_id`, state.rs lines 2045-2052) — mint the identifier in AWS's recorded form rather than reusing the 32-hex version-digest form; keep the "changes whenever the version changes" property it already has.
Traces: mwaa-serverless-workflow-run-two-dependent-tasks-succeed#task#6.

=== Cause 3 of 5: GetWorkflow omits the service-applied TriggerMode and LoggingConfiguration defaults ===
AWS: Smithy: GetWorkflowResponse$TriggerMode (GenericString, len 1..255) and GetWorkflowResponse$LoggingConfiguration (LoggingConfiguration, whose LogGroupName is "the name of the CloudWatch log group where workflow execution logs are stored"). Recorded behavior only: for a workflow created from a definition that declares no schedule, AWS reports `TriggerMode` = the fixed token `manual_only` and a `LoggingConfiguration` whose `LogGroupName` is service-generated as the fixed prefix `/aws/mwaa-serverless/` + the workflow's own Name + a trailing `/` (in the recording, `/aws/mwaa-serverless/sim2real-probe-mwaas-wf-9dbe1016-8829e6271f/` — the name segment is the value from the create request, the prefix and trailing slash are fixed text). Both are reported whether or not the caller supplied them, i.e. the service DEFAULTS them at create time: the trigger mode from the snapshotted definition's schedule (no schedule ⇒ manual_only) and the log group from the workflow name.
Emulator: both members are ABSENT from the GetWorkflow body (`$.TriggerMode: missing`, `$.LoggingConfiguration: missing`). The handler echoes only what the caller passed — get_workflow.rs lines 278 and 295 read `snapshot.logging_configuration` / `snapshot.trigger_mode`, and create_workflow.rs lines 405-408 store exactly `input.logging_configuration` / `input.trigger_mode`, so a create that omits them leaves both `None` forever and no default is ever applied.
Why: a client cannot discover where its run output is written — `LoggingConfiguration.LogGroupName` is the only handle on the log group, so log-tailing and alarm wiring built on GetWorkflow have nothing to point at — and a client that branches on TriggerMode to decide whether it must start runs itself sees an absent member instead of `manual_only`. SDK callers that index the modeled member hit a key error rather than reading a defaulted value.
Fix: CreateWorkflow's (and UpdateWorkflow's) snapshot construction in the service state — when the request omits them, derive `trigger_mode` from the snapshotted definition's schedule and synthesize the default `logging_configuration` log-group name from the workflow name, so every reader (GetWorkflow, ListWorkflows, and `crate::state::configured_log_group`, which currently returns None and silently disables run-log export) sees the service default rather than nothing.
Traces: mwaa-serverless-workflow-run-two-dependent-tasks-succeed#task#10.

=== Cause 4 of 5: GetWorkflowRun omits the service-injected OverrideParameters entry ===
AWS: Smithy: GetWorkflowRunResponse$OverrideParameters targets `ObjectMap` and is @sensitive. Recorded behavior only: a run reports an OverrideParameters map containing the service-managed entry whose key is the fixed text `mwaa_serverless_workflow_id` and whose value is the workflow's own Name (in the recording, `sim2real-probe-mwaas-wf-9dbe1016-8829e6271f` — that value comes from the create request, the key is fixed). AWS reports the entry on the run even though the recorded StartWorkflowRun carried no such override, so the service injects it into the run's parameter set.
Emulator: `$.OverrideParameters: missing`. get_workflow_run.rs lines 251-256 emit the member only when `run.override_parameters` is non-empty, and start_workflow_run.rs lines 388-393 populate that map from `input.override_parameters` alone — so a run started without explicit overrides reports no parameters at all.
Why: the workflow-id parameter is how a caller (and the DAG itself) correlates a run back to its workflow out of the run record; a client reading `OverrideParameters["mwaa_serverless_workflow_id"]` gets a missing-member error against the emulator, and tooling that renders a run's effective parameters shows an empty set where AWS shows one.
Fix: the run record's construction in StartWorkflowRun (and in the scheduled-trigger firing path in the service state, so an automated run carries it too) — seed the run's override-parameter map with the service-injected `mwaa_serverless_workflow_id` entry before merging the caller's overrides, so GetWorkflowRun's existing non-empty check reports it.
Traces: mwaa-serverless-workflow-run-two-dependent-tasks-succeed#task#8.

=== Cause 5 of 5: a run of a two-task DAG materializes NO task instances, so RunDetail.TaskInstances is absent and ListTaskInstances is empty ===
AWS: Smithy: WorkflowRunDetail$TaskInstances is "A list of task instance IDs that are part of this workflow run"; ListTaskInstances returns `TaskInstances` (TaskInstanceSummaries) and is @paginated over that member. Recorded behavior: for a DAG declaring two dependent tasks, AWS reports a RunDetail whose `TaskInstances` is a list of length 2, and ListTaskInstances returns two summaries, each carrying `OperatorName` `BashOperator`, the run's own `RunId`, `Status` `SUCCESS`, the executing version's `WorkflowVersion`, the workflow's `WorkflowArn`, plus a `TaskInstanceId` and a `DurationInSeconds` (ids, run id, version and duration are per-run values; the one-instance-per-declared-task fan-out, the operator name taken from the task declaration and the terminal Status are the requirement).
Emulator: GetWorkflowRun's `$.RunDetail.TaskInstances` is missing (the handler emits the member only for a non-empty id list, get_workflow_run.rs lines 271-274) and ListTaskInstances answers 200 with no `TaskInstances` entries at all (`$.TaskInstances[0]`/`[1]`: missing) — for the same run, in the same fixture, that the emulator otherwise reports as settled. The run executes an EMPTY plan, so no instance is ever recorded.
Why: the data plane is hollow — a client cannot observe task fan-out, per-task status, operator or timing, and an SDK caller that indexes the modeled `TaskInstances` member gets a missing key instead of a list. Worse, a run with no instances still reports a terminal, successful run, so a caller believes work it never performed succeeded.
Fix: the definition reader and the execution planner in the service state — the run's plan must contain one task instance per task the snapshotted definition declares (state/workflow_definition.rs's task-key-path predicate and state/workflow_execution.rs's plan construction), and both readers must then report them. NOTE: open behavioral issue 20261029T185031Z-mwaa-serverless-six-behavioural-divergences-across-createworkflow-validation.md (Cause 5) describes the same empty-plan root cause from the behavioral suite and names `at_tasks_path` only recognizing `tasks` at the document root or under `dag`, never under a top-level `<dag_id>` key; the replay reports this wire cluster as NOT owned by any open issue, so it is filed here with its own replay key — fixing that path predicate is expected to close this cause too, and this issue's fixture must be re-replayed to confirm it.
Traces: mwaa-serverless-workflow-run-two-dependent-tasks-succeed#task#8 (GetWorkflowRun RunDetail.TaskInstances), #task#9 (ListTaskInstances).

## Evidence
Trace set simparity-wire-aws-v1, fixture `mwaa-serverless-workflow-run-two-dependent-tasks-succeed`: 34 steps, 29 passed, 4 mismatched, score 85.3%. emulator_up=true (/__admin/health 200). Every diverging step returned the EXPECTED status 200 — only the body diverges. No cluster carried a stub_path or an owned_by.

Cause 1 + Cause 2 — step `task#6`, CreateWorkflow, expected 200 / actual 200:
  $.CreatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794246357.4472766)
  $.RevisionId: value_mismatch (expected '<shaped-not-asserted>', got '9895198a5dc6d9e0452733da25d20197')
  (the epoch number and the digest are per-run values; the divergence is the wire FORM — a number where a @timestampFormat("date-time") string is modeled, and a 32-hex digest where the recorded revision id is a different shape class)

Cause 1 + Cause 4 + Cause 5 — step `task#8`, GetWorkflowRun, expected 200 / actual 200:
  $.OverrideParameters: missing (expected {'mwaa_serverless_workflow_id': 'sim2real-probe-mwaas-wf-9dbe1016-8829e6271f'}, got None)
  $.RunDetail.CompletedOn: value_mismatch (expected '<shaped-not-asserted>', got 1794246368.4690564)
  $.RunDetail.CreatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794246367.4690564)
  $.RunDetail.ModifiedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794246368.4690564)
  $.RunDetail.StartedOn: value_mismatch (expected '<shaped-not-asserted>', got 1794246367.4690564)
  $.RunDetail.TaskInstances: missing (expected ['<shaped-not-asserted>', '<shaped-not-asserted>'], got None)

Cause 5 — step `task#9`, ListTaskInstances, expected 200 / actual 200:
  $.TaskInstances[0]: missing (expected {'DurationInSeconds': '<shaped-not-asserted>', 'OperatorName': 'BashOperator', 'RunId': 'run-183c4aa96796b13c', 'Status': 'SUCCESS', 'TaskInstanceId': '<shaped-not-asserted>', 'WorkflowArn': 'arn:aws:airflow-serverless:us-west-2:000000000000:workflow/sim2real-probe-mwaas-wf-9dbe1016-8829e6271f', 'WorkflowVersion': 'f96612fbf07a7b77171cd682fb38021e'}, got None)
  $.TaskInstances[1]: missing (same shape, same RunId/WorkflowArn/WorkflowVersion, got None)
  (the RunId, ARN account/region and the 32-hex WorkflowVersion are per-run values; the requirement is TWO summaries for a two-task DAG, each with OperatorName BashOperator and Status SUCCESS)

Cause 1 + Cause 3 — step `task#10`, GetWorkflow, expected 200 / actual 200:
  $.CreatedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794246357.4472766)
  $.ModifiedAt: value_mismatch (expected '<shaped-not-asserted>', got 1794246357.4472766)
  $.LoggingConfiguration: missing (expected {'__type': 'com.amazonaws.mwaaserverless.datatypes#LoggingConfiguration', 'LogGroupName': '/aws/mwaa-serverless/sim2real-probe-mwaas-wf-9dbe1016-8829e6271f/'}, got None)
  $.TriggerMode: missing (expected 'manual_only', got None)

Code evidence: state.rs lines 2420-2423 (`timestamp` -> DateTime, format decided by the serializer), 2045-2052 (`mint_revision_id` = 32 chars of a SHA-256 digest), 327-339 (`configured_log_group` returns None when no LoggingConfiguration was supplied); create_workflow.rs lines 405-408 (snapshot copies `input.logging_configuration` / `input.trigger_mode` verbatim, no default); get_workflow.rs lines 269-295; get_workflow_run.rs lines 251-256 and 271-274; start_workflow_run.rs lines 388-393.

## Steps to Reproduce
Replay the bound wire corpus against a freshly built emulator: `replay_wire_corpus()` on trace set simparity-wire-aws-v1, fixture `mwaa-serverless-workflow-run-two-dependent-tasks-succeed`, steps task#6, task#8, task#9, task#10.

Equivalently, by hand against the emulator's mwaa-serverless endpoint (awsJson1_0, X-Amz-Target dispatch):
1. Create an S3 bucket and PutObject an Airflow YAML DAG that declares TWO dependent BashOperator tasks under a top-level `<dag_id>:` key with `schedule: null`; create an IAM role trusting airflow-serverless.amazonaws.com.
2. CreateWorkflow(Name=N, DefinitionS3Location={Bucket,ObjectKey}, RoleArn=R) with NO LoggingConfiguration and NO TriggerMode. Inspect the raw JSON body: `CreatedAt` must be an ISO-8601 date-time STRING, not a number (Cause 1), and `RevisionId` must be minted in AWS's identifier form, not a 32-char hex digest shaped like WorkflowVersion (Cause 2).
3. StartWorkflowRun(WorkflowArn) with NO OverrideParameters, then poll GetWorkflowRun(WorkflowArn, RunId) to a terminal RunState. In the raw body: `OverrideParameters` must contain `mwaa_serverless_workflow_id` = N (Cause 4); `RunDetail.CreatedAt`/`StartedOn`/`ModifiedAt`/`CompletedOn` must be date-time strings (Cause 1); `RunDetail.TaskInstances` must be a list of length 2 (Cause 5).
4. ListTaskInstances(WorkflowArn, RunId): must return exactly two summaries, each with OperatorName `BashOperator`, Status `SUCCESS`, the run's RunId, the workflow's WorkflowArn, the executing WorkflowVersion, a TaskInstanceId and a DurationInSeconds (Cause 5).
5. GetWorkflow(WorkflowArn): `CreatedAt`/`ModifiedAt` must be date-time strings (Cause 1); `TriggerMode` must be `manual_only` and `LoggingConfiguration.LogGroupName` must be `/aws/mwaa-serverless/<N>/` even though the create request supplied neither (Cause 3).
