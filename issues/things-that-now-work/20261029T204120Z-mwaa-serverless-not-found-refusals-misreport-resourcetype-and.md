Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-serverless-resource-not-found-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-serverless-resource-not-found-errors
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless|UpdateWorkflow|4xx->4xx|$.ResourceId
Issue-Key: mwaa-serverless|UpdateWorkflow|4xx->4xx|$.ResourceType
Issue-Key: mwaa-serverless|ListTagsForResource|4xx->4xx|$.ResourceType
Issue-Key: mwaa-serverless|TagResource|4xx->4xx|$.ResourceType
Issue-Key: mwaa-serverless|UntagResource|4xx->4xx|$.ResourceType
Issue-Commit: 35c75764fc95ea0d9fe2ac74659979fed222fabe

# MWAA Serverless not-found refusals misreport ResourceType and ResourceId on four operations

> _Note: this issue was identified against the code base at commit `35c75764fc95ea0d9fe2ac74659979fed222fabe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWorkflow

## Description
Replay of the wire corpus (trace set simparity-wire-aws-v1) leaves 4 of 17 steps mismatched, all on service mwaa-serverless, all in fixture mwaa-serverless-resource-not-found-errors, and all the SAME class of fault: the refusal's status (400) and its error shape (ResourceNotFoundException) are right, but the REQUIRED members inside the error document carry values AWS does not use. Two root causes are bundled here; one of them is shared by four operations because they all funnel through one helper. THIS ISSUE HAS 4 CAUSES; all four must be closed before submitting.

Cause 1 of 4: UpdateWorkflow on a WorkflowArn no workflow holds — BOTH ResourceId and ResourceType wrong.
AWS: Smithy (services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json) models UpdateWorkflow with error ResourceNotFoundException; the shape is a structure whose three members are ALL @required — Message (ErrorMessage, length 1..2048), ResourceId (String, doc "The unique identifier of the resource.") and ResourceType (String, doc "The type of the resource."). The model gives the shape @httpError 404 but the recording answers 400 with that same shape, and the emulator already matches that (crate::state::LOOKUP_MISS_STATUS), so the status is NOT at issue. What the recording shows for this refusal is that ResourceId carries the workflow's own identifier — the resource-id segment of the WorkflowArn, i.e. the part after "workflow/", which the WorkflowArn pattern constrains to <name>-<10 alphanumerics> — NOT the whole ARN the request supplied; and that ResourceType is the capitalised resource name "Workflow", not a lowercased token. Both are fixed-vocabulary/derived values: ResourceType is fixed text for the entity class, ResourceId is a value taken from the request's ARN. Recorded behavior only for the choice between ARN and bare id — the model's doc says "unique identifier", and the recording resolves it to the id segment for this operation.
Emulator: answers 400 ResourceNotFoundException with ResourceId set to the full ARN and ResourceType set to "workflow". Replay detail: `$.ResourceId: value_mismatch (expected '<workflow-id>', got 'arn:aws:airflow-serverless:<region>:<account>:workflow/<workflow-id>')` and `$.ResourceType: value_mismatch (expected 'Workflow', got 'workflow')`.
Why: a client that keys its retry/alarm logic off ResourceType compares against AWS's "Workflow" and never matches, so the not-found is classified as an unknown resource class; and a client that feeds ResourceId back into a call or a log line receives a full ARN where AWS hands it a bare workflow identifier, so string-equality against the id it tracks fails and any code that re-composes an ARN from it builds a nested, malformed ARN.
Fix: the shared lookup-miss payload builder in this service's state module — crate::state::unresolved / UnresolvedEntity::resource_type, plus UpdateWorkflow's call site which passes the ARN as the resource id. ResourceType must be the entity's AWS resource name; ResourceId for this operation must be the ARN's resource-id segment.
Traces: mwaa-serverless-resource-not-found-errors#task#3. Code: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state.rs:1882-1890 (UnresolvedEntity::resource_type returns the lowercase token), :1893-1904 (unresolved() copies resource_id through verbatim), :1825-1835 (LOOKUP_MISS_STATUS, the already-correct status override); services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/update_workflow.rs:245-260 (the call site passing &input.workflow_arn as the resource id).

Cause 2 of 4: ListTagsForResource on a ResourceArn no workflow holds — ResourceType wrong.
AWS: Smithy models ListTagsForResource (ResourceArn, TaggableResourceArn, @required, pattern ^arn:aws…:airflow-serverless:<region>:<account>:workflow/<name>-<10>$) with error ResourceNotFoundException, same three @required members. The recorded refusal is 400 ResourceNotFoundException whose ResourceType is "Workflow" — fixed text naming the entity class, with no request-derived part. Here ResourceId IS the ARN the request supplied and the emulator already matches it, so only the type token diverges.
Emulator: ResourceType "workflow". Replay detail: `$.ResourceType: value_mismatch (expected 'Workflow', got 'workflow')`.
Why: same misclassification as cause 1 — a tagging client that branches on the resource class of a not-found never sees the class AWS publishes.
Fix: same component as cause 1, the shared UnresolvedEntity::resource_type vocabulary; this operation's ResourceId needs no change.
Traces: mwaa-serverless-resource-not-found-errors#task#8. Code: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/list_tags_for_resource.rs:200-211; state.rs:1882-1890.

Cause 3 of 4: TagResource on a ResourceArn no workflow holds — ResourceType wrong.
AWS: Smithy models TagResource (ResourceArn @required TaggableResourceArn, Tags @required map length 0..50) with error ResourceNotFoundException; the recorded refusal is 400 ResourceNotFoundException with ResourceType "Workflow" (fixed text for the entity class) and ResourceId the ARN supplied.
Emulator: ResourceType "workflow". Replay detail: `$.ResourceType: value_mismatch (expected 'Workflow', got 'workflow')`.
Why: same as cause 2. It is listed as its own cause because it is its own operation and its own fixture step: a fix verified only on ListTagsForResource must still be re-replayed here, since each tagging handler builds this refusal at its own call site.
Fix: same component as cause 1.
Traces: mwaa-serverless-resource-not-found-errors#task#9. Code: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/tag_resource.rs (its workflow_in lookup-miss arm, built via crate::state::unresolved with UnresolvedEntity::Workflow); state.rs:1882-1890.

Cause 4 of 4: UntagResource on a ResourceArn no workflow holds — ResourceType wrong.
AWS: Smithy models UntagResource (ResourceArn @required TaggableResourceArn, TagKeys @required list length 1..50, @idempotent) with error ResourceNotFoundException; the recorded refusal is 400 ResourceNotFoundException with ResourceType "Workflow" and ResourceId the ARN supplied.
Emulator: ResourceType "workflow". Replay detail: `$.ResourceType: value_mismatch (expected 'Workflow', got 'workflow')`.
Why: same as cause 2, on the untag path.
Fix: same component as cause 1.
Traces: mwaa-serverless-resource-not-found-errors#task#10. Code: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/untag_resource.rs (its workflow_in lookup-miss arm); state.rs:1882-1890.

Caveat for the fixer: crate::state::unresolved is the single funnel for EVERY lookup miss in this service (state.rs:1896-1897 says so), and the other not-found steps in this same fixture — GetWorkflow, DeleteWorkflow, StartWorkflowRun, GetWorkflowRun, which also pass UnresolvedEntity::Workflow or a version/run entity through it — currently PASS. So re-replay the whole fixture (all 17 steps) after changing the shared vocabulary, not just the four steps named above, and keep the workflow-version / workflow-run / task-instance tokens behaving as the passing steps require.

Not covered by any open issue: the nearest mwaa-serverless issues are 20261029T185031Z-mwaa-serverless-six-behavioural-divergences-across-createworkflow-validation.md (CreateWorkflow validation wording and run lifecycle) and 20261029T022948Z-mwaa-serverless-updateworkflow-maintainability--r2.md (duplicate ConflictException construction); neither touches the ResourceNotFoundException member values and neither carries a replay key for these steps. 20261029T190735Z-mwaa-error-refusals-diverge-from-the-recorded-wire.md is a different service (mwaa, not mwaa-serverless). The replay reported owned_by empty and stub_path empty for all four clusters.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true ("/__admin/health answered 200 and the :33875 data listener accepts"), bootstrap_signal false, steps_total 17, steps_passed 13, steps_mismatched 4, steps_blocked 0, steps_errored 0, score_pct 76.5, divergence_count 4. All four divergences: service mwaa-serverless, fixture mwaa-serverless-resource-not-found-errors, verdict mismatch, expected_status 400 / actual_status 400, stub_path empty, owned_by empty, affected_count 1.

1) step task#3, UpdateWorkflow.
   detail: $.ResourceId: value_mismatch (expected 'sim2real-probe-mwaa-serverless-wf-missing-30316a11-abcdefghij', got 'arn:aws:airflow-serverless:us-west-2:000000000000:workflow/sim2real-probe-mwaa-serverless-wf-missing-30316a11-abcdefghij')
           $.ResourceType: value_mismatch (expected 'Workflow', got 'workflow')
   divergence_key: service=mwaa-serverless|UpdateWorkflow|status=400->400|keys=$.ResourceId,$.ResourceType
   (the expected ResourceId is the ARN's resource-id segment; the region/account/name values shown are this run's and are not the requirement — the requirement is "the id segment, not the ARN".)

2) step task#8, ListTagsForResource.
   detail: $.ResourceType: value_mismatch (expected 'Workflow', got 'workflow')
   divergence_key: service=mwaa-serverless|ListTagsForResource|status=400->400|keys=$.ResourceType

3) step task#9, TagResource.
   detail: $.ResourceType: value_mismatch (expected 'Workflow', got 'workflow')
   divergence_key: service=mwaa-serverless|TagResource|status=400->400|keys=$.ResourceType

4) step task#10, UntagResource.
   detail: $.ResourceType: value_mismatch (expected 'Workflow', got 'workflow')
   divergence_key: service=mwaa-serverless|UntagResource|status=400->400|keys=$.ResourceType

Model evidence (smithy_query, model_path services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json):
  shape ResourceNotFoundException -> structure, errorKind client, httpError 404, members {Message: ErrorMessage @required length 1..2048 pattern ^.*$; ResourceId: String @required "The unique identifier of the resource."; ResourceType: String @required "The type of the resource."}
  UpdateWorkflow -> WorkflowArn @required pattern ^arn:aws…:airflow-serverless:([a-z]{2}-[a-z]+-[0-9]{1}):([0-9]{12}):workflow/([a-zA-Z0-9][a-zA-Z0-9.\-_]{0,254}-[a-zA-Z0-9]{10})$; errors include ResourceNotFoundException 404.
  ListTagsForResource / TagResource / UntagResource -> ResourceArn @required TaggableResourceArn, same workflow-ARN pattern; errors include ResourceNotFoundException 404.
So all three members are modelled @required and the recording fixes their values; only their CONTENT diverges, not the status or the shape.

Code evidence — one shared helper produces every one of these payloads, services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state.rs:1882-1904:
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::Workflow => "workflow",
            Self::LatestWorkflowVersion | Self::NamedWorkflowVersion => "workflow-version",
            ...
    pub fn unresolved(entity: UnresolvedEntity, resource_id: &str) -> ResourceNotFoundException {
        ResourceNotFoundException { message: entity.message().to_string(), resource_id: resource_id.to_string(), resource_type: entity.resource_type().to_string() }
and the four call sites all hand it the request's ARN as resource_id: update_workflow.rs:245-260, list_tags_for_resource.rs:200-211, tag_resource.rs and untag_resource.rs (their workflow_in(...).ok_or_else lookup-miss arms). state.rs:1825-1835 shows the status override (LOOKUP_MISS_STATUS = 400) is already correct and must be left alone.

## Steps to Reproduce
Prerequisite for every cause: boot the emulator from this checkout and create no workflow matching the ARN used below, so the lookup misses.

Cause 1: replay fixture mwaa-serverless-resource-not-found-errors, step task#3 — call UpdateWorkflow with a syntactically valid WorkflowArn (matching the modelled workflow-ARN pattern) that names no stored workflow, plus the required DefinitionS3Location and RoleArn. Expect HTTP 400 ResourceNotFoundException whose ResourceType is "Workflow" and whose ResourceId is the ARN's resource-id segment (the part after "workflow/"). Observe ResourceType "workflow" and ResourceId set to the full ARN.

Cause 2: same fixture, step task#8 — call ListTagsForResource with a valid TaggableResourceArn naming no stored workflow. Expect HTTP 400 ResourceNotFoundException with ResourceType "Workflow"; observe "workflow". (ResourceId already matches: the ARN as supplied.)

Cause 3: same fixture, step task#9 — call TagResource with that same unresolvable ResourceArn and any valid Tags map. Expect ResourceType "Workflow"; observe "workflow".

Cause 4: same fixture, step task#10 — call UntagResource with that same unresolvable ResourceArn and at least one TagKey. Expect ResourceType "Workflow"; observe "workflow".

All four: re-run replay_wire_corpus() after the fix. The corpus must reach 17/17 passed (it is 13/17, score 76.5%, before), which also confirms the currently-passing not-found steps of this fixture (GetWorkflow, DeleteWorkflow, StartWorkflowRun, GetWorkflowRun) did not regress when the shared crate::state::unresolved vocabulary changed.
