Issue-Source: trace-sweep-divergence
Issue-Trace: nova-act-error-paths-not-found
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: nova-act-error-paths-not-found nova-act-workflow-definition-lifecycle
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act|GetWorkflowRun|4xx->4xx|$.message
Issue-Key: nova-act|GetWorkflowRun|4xx->4xx|$.resourceId
Issue-Key: nova-act|CreateSession|4xx->4xx|$.message
Issue-Key: nova-act|CreateSession|4xx->4xx|$.resourceId
Issue-Key: nova-act|GetWorkflowDefinition|4xx->4xx|$.message
Issue-Key: nova-act|GetWorkflowDefinition|4xx->4xx|$.resourceId
Issue-Key: nova-act|DeleteWorkflowDefinition|4xx->4xx|$.message
Issue-Key: nova-act|DeleteWorkflowDefinition|4xx->4xx|$.resourceId
Issue-Commit: 0f903c65ef17614b948ed7843da2bc7f944d5d1a

# nova-act ResourceNotFoundException returns bare name instead of resource ARN

> _Note: this issue was identified against the code base at commit `0f903c65ef17614b948ed7843da2bc7f944d5d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkflowDefinition

## Description
Every nova-act operation that resolves a workflow definition by name populates the ResourceNotFoundException body with the raw path identifier rather than the resource's ARN. The status (404) and x-amzn-errortype are correct on all four paths; the defect is in how the exception's modeled members are filled in, and it is a single shared fault in the ResourceNotFoundException construction helper used by every workflow-definition lookup. FOUR causes must be closed.

Cause 1 of 4: GetWorkflowRun on a missing workflow definition.
AWS: Smithy: GetWorkflowRun errors include ResourceNotFoundException (errorKind client, httpError 404) whose required members are message, resourceId and resourceType, with resourceId documented as 'The identifier of the resource that wasn't found'. The model does not state the identifier's form, so recorded behavior only: the recording shows resourceId carrying the fully qualified workflow-definition ARN (arn:aws:nova-act:<region>:<account>:workflow-definition/<name>) and message the generic 'Workflow definition not found'.
Emulator: returns 404 with resourceId set to the bare definition name taken from the URI path, and a message built by interpolating the name ('workflow-definition <name> was not found').
Why: a client that reads resourceId to key retries, logging or cross-resource lookups receives a value that is not an ARN, so ARN parsing fails and the error cannot be correlated to the resource it names; the identifier is also not account/region qualified as it is on AWS.
Fix: the ResourceNotFoundException construction path shared by the workflow-definition lookup handlers (and/or the error serializer that fills resourceId) must render the resource ARN from the configured partition, region and account plus the definition name. The differing message text is an example only, not a requirement.
Traces: nova-act-error-paths-not-found#task#3.

Cause 2 of 4: CreateSession against a workflow definition that does not exist.
AWS: Smithy: CreateSession (PUT /workflow-definitions/{workflowDefinitionName}/workflow-runs/{workflowRunId}/sessions) models ResourceNotFoundException, httpError 404, required resourceId. Recorded behavior only: resourceId is the workflow-definition ARN.
Emulator: 404 with resourceId = bare definition name.
Why: same as cause 1, on the session-creation path.
Fix: same construction helper, reached from the CreateSession handler's parent-resource resolution.
Traces: nova-act-error-paths-not-found#task#4.

Cause 3 of 4: GetWorkflowDefinition on a missing definition.
AWS: Smithy: GetWorkflowDefinition errors = [ResourceNotFoundException, ValidationException]; httpError 404; required resourceId. Recorded behavior only: resourceId is the workflow-definition ARN.
Emulator: 404 with resourceId = bare definition name.
Why: same as cause 1. This cluster also covers the teardown path of the lifecycle fixture, so a client that deletes then re-reads a definition sees the same non-ARN identifier.
Fix: same construction helper, reached from the GetWorkflowDefinition handler.
Traces: nova-act-error-paths-not-found#task#6 (cluster also contains nova-act-workflow-definition-lifecycle#teardown#5).

Cause 4 of 4: DeleteWorkflowDefinition on a missing definition.
AWS: Smithy: DeleteWorkflowDefinition (DELETE /workflow-definitions/{workflowDefinitionName}, success code 202) models ResourceNotFoundException, httpError 404, required resourceId. Recorded behavior only: resourceId is the workflow-definition ARN.
Emulator: 404 with resourceId = bare definition name.
Why: same as cause 1, on the delete path; repeated deletes are a normal client idiom and the resulting error is unparseable as an ARN.
Fix: same construction helper, reached from the DeleteWorkflowDefinition handler.
Traces: nova-act-error-paths-not-found#task#7 (cluster also contains nova-act-workflow-definition-lifecycle#teardown#6).

## Evidence
Replay of trace set simparity-wire-aws-v1, four mismatch clusters, all status 404 expected and 404 actual:

1) nova-act-error-paths-not-found#task#3 GetWorkflowRun — $.resourceId: expected 'arn:aws:nova-act:us-east-1:000000000000:workflow-definition/sim2real-probe-missingwf-0b5dcf1a', got 'sim2real-probe-missingwf-0b5dcf1a'; $.message: expected 'Workflow definition not found', got 'workflow-definition sim2real-probe-missingwf-0b5dcf1a was not found' (message cited as an example).
2) nova-act-error-paths-not-found#task#4 CreateSession — same two field diffs.
3) nova-act-error-paths-not-found#task#6 GetWorkflowDefinition — same two field diffs; affected_count 2 (also nova-act-workflow-definition-lifecycle#teardown#5).
4) nova-act-error-paths-not-found#task#7 DeleteWorkflowDefinition — same two field diffs; affected_count 2 (also nova-act-workflow-definition-lifecycle#teardown#6).

Expected (AWS, per recording): resourceId = ARN of the workflow definition. Actual (emulator): resourceId = unqualified definition name. resourceType ('workflow-definition') and the 404 status match on all four.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture nova-act-error-paths-not-found, steps task#3, task#4, task#6, task#7: each calls a nova-act operation (GetWorkflowRun, CreateSession, GetWorkflowDefinition, DeleteWorkflowDefinition) naming a workflow definition that was never created.
3. Inspect the 404 JSON body: $.resourceId is the bare definition name, not arn:aws:nova-act:<region>:<account>:workflow-definition/<name>.
4. Re-replay the same fixture/steps plus nova-act-workflow-definition-lifecycle#teardown#5 and #teardown#6 to confirm the fix.
