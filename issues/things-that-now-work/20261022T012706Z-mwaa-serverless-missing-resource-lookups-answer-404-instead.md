Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-serverless-resource-not-found-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-serverless-resource-not-found-errors
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless|GetWorkflow|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|GetWorkflow|4xx->4xx|status
Issue-Key: mwaa-serverless|DeleteWorkflow|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|DeleteWorkflow|4xx->4xx|status
Issue-Key: mwaa-serverless|StartWorkflowRun|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|StartWorkflowRun|4xx->4xx|status
Issue-Key: mwaa-serverless|GetWorkflowRun|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|GetWorkflowRun|4xx->4xx|status
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# MWAA Serverless missing-resource lookups answer 404 instead of the recorded 400

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkflow

## Description
Four causes, all the same fault: every MWAA Serverless operation that is handed an ARN for a workflow/run that does not exist is answered by the emulator with HTTP 404 and one generic message, where the recorded service answers HTTP 400 with an entity-specific message. Smithy (com.amazonaws.mwaaserverless, awsJson1_0): ResourceNotFoundException carries @httpError 404, so the emulator is serving that modelled shape; the recordings show the live service does NOT use it on these paths and instead answers 400 (the status of ValidationException, the service-common error, @httpError 400). Hypothesis: for a syntactically well-formed but non-existent workflow ARN the live service fails the request in request validation (400) rather than in resource lookup. The status code is the requirement here and it is wrong in all four causes; the expected message texts are recorded examples, but note that the live messages distinguish WHICH entity was absent (workflow vs. latest version vs. workflow run) while the emulator emits one workflow-level string everywhere.

Cause 1 of 4: GetWorkflow on a non-existent workflow ARN. AWS: HTTP 400, body Message (recorded example 'Resource not found'). Emulator: HTTP 404, Message 'No workflow found for arn:aws:airflow-serverless:...:workflow/...'. Fixture mwaa-serverless-resource-not-found-errors, step task#1.

Cause 2 of 4: DeleteWorkflow on a non-existent workflow ARN. AWS: HTTP 400, Message (recorded example 'Resource not found'). Emulator: HTTP 404 with the generic 'No workflow found for <arn>' message. Fixture mwaa-serverless-resource-not-found-errors, step task#2.

Cause 3 of 4: StartWorkflowRun on a non-existent workflow ARN. AWS: HTTP 400, Message (recorded example 'Unable to find latest version' — the live service reports the missing workflow VERSION for this operation). Emulator: HTTP 404 with the generic 'No workflow found for <arn>' message. Fixture mwaa-serverless-resource-not-found-errors, step task#4.

Cause 4 of 4: GetWorkflowRun for a run under a non-existent workflow. AWS: HTTP 400, Message (recorded example 'Workflow run not found' — the live service reports the missing RUN). Emulator: HTTP 404 with the generic 'No workflow found for <arn>' message. Fixture mwaa-serverless-resource-not-found-errors, step task#5.

Why: an awsJson1_0 client distinguishes these by HTTP status and error shape. Against AWS a missing workflow surfaces as a 400-class validation failure; against the emulator it surfaces as a 404 ResourceNotFoundException. Retry and error-classification logic keyed on the status, and any code that branches on ValidationException vs ResourceNotFoundException, takes the wrong branch, so negative-path tests that pass against the emulator fail against AWS (and vice versa). The single generic message also hides which entity is missing, so a client cannot tell 'workflow absent' from 'no published version' or 'run absent'.

Fix: the not-found path of these four operation handlers and the service's error mapping — the lookup miss must be mapped to the 400-status validation error the recordings show, not to ResourceNotFoundException's 404, and each handler must report the entity it actually failed to resolve (workflow, latest version, workflow run) instead of a shared workflow-level string.

Traces: mwaa-serverless-resource-not-found-errors#task#1 (GetWorkflow), #task#2 (DeleteWorkflow), #task#4 (StartWorkflowRun), #task#5 (GetWorkflowRun).

## Evidence
Replay of fixture mwaa-serverless-resource-not-found-errors (4 divergence clusters, verdict=mismatch):
- task#1 GetWorkflow: status expected 400, got 404; $.Message expected 'Resource not found', got 'No workflow found for arn:aws:airflow-serverless:us-west-2:000000000000:workflow/sim2real-probe-mwaa-serverless-wf-missing-...'
- task#2 DeleteWorkflow: status expected 400, got 404; $.Message expected 'Resource not found', got 'No workflow found for arn:aws:airflow-serverless:...'
- task#4 StartWorkflowRun: status expected 400, got 404; $.Message expected 'Unable to find latest version', got 'No workflow found for arn:aws:airflow-serverless:...'
- task#5 GetWorkflowRun: status expected 400, got 404; $.Message expected 'Workflow run not found', got 'No workflow found for arn:aws:airflow-serverless:...'
Model reference: ResourceNotFoundException @httpError 404 (required Message, ResourceId, ResourceType); ValidationException @httpError 400 (required Message, Reason). Emulator reachable (health 200), bootstrap_signal=false, state_reset_failures=0, so these are genuine response divergences.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay wire fixture mwaa-serverless-resource-not-found-errors from trace set simparity-wire-aws-v1.
3. Steps task#1 (GetWorkflow), task#2 (DeleteWorkflow), task#4 (StartWorkflowRun) and task#5 (GetWorkflowRun) each target an ARN for a workflow that was never created.
4. Observe HTTP 404 with a generic 'No workflow found for <arn>' Message on all four, where the recording has HTTP 400 with an entity-specific Message.
