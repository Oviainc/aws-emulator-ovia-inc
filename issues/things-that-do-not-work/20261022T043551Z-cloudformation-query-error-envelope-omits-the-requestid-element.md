Issue-Source: trace-sweep-divergence
Issue-Trace: cfnerr-max-results-bounds
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfnerr-max-results-bounds cfnrep-drift-modified-property-differences cfnrep-get-template-processed-transform cfnrep-outputs-getatt-sub-export cloudformation-generated-template-workflow cloudformation-stack-create-update-delete-lifecycle cloudformation-stack-recovery-operation-errors cloudformation-template-validation-and-planning-tools cloudformation-stack-lifecycle-basic-crud cloudformation-list-type-registrations-invalid-filter-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ListStackSets|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudformation|ListStackSetOperations|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudformation|DescribeStackResourceDrifts|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudformation|GetTemplate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudformation|DescribeStacks|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudformation|CreateStack|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudformation|UpdateStack|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: cloudformation|GetTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ListTypeRegistrations|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ListTypeRegistrations|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# CloudFormation Query error envelope omits the RequestId element on every error response

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStackSets

## Description
NINE net-new causes, all one shared defect in the CloudFormation awsQuery error serializer: the <ErrorResponse> document it emits contains only <Error>, never the sibling <RequestId> element that AWS always emits. The HTTP status and the error Code are correct in all nine, so this is purely the error envelope. Each cause below is a separate replay cluster (different operation / fixture), named so a fixer closes all nine.

Cause 1 of 9 — ListStackSets (MaxResults out of bounds).
AWS: an awsQuery error response is <ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId>...</RequestId></ErrorResponse>; the RequestId element is always present. Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): protocol awsQuery, every modeled error carries an awsQueryError {code, httpResponseCode}; the RequestId envelope element is protocol-level, so recorded behavior for its presence. Recorded: HTTP 400, Code ValidationError, plus RequestId.
Emulator: HTTP 400 and the Code match, but $.ErrorResponse.RequestId is absent (replay: "$.ErrorResponse.RequestId: missing").
Why: a client cannot obtain the request id for a failed call. SDK error objects expose RequestId from exactly this element, so retry/telemetry/support flows log an empty id and correlation against server-side records is impossible; strict XML consumers that bind the full ErrorResponse shape see a structurally different document from AWS's.
Fix: the CloudFormation awsQuery error serializer (the shared error-envelope writer in services/service-com-amazonaws-cloudformation-cloudformation) must write <RequestId> into every ErrorResponse, from the same request id used in successful responses' ResponseMetadata.
Traces: cfnerr-max-results-bounds#task#2 (also cfnerr-max-results-bounds#task#3).

Cause 2 of 9 — ListStackSetOperations, MaxResults out of bounds. Same envelope omission on a paginated StackSet read. AWS: 400 + Code ValidationError + RequestId. Emulator: 400, Code right, RequestId missing. Why/Fix: as cause 1. Traces: cfnerr-max-results-bounds#task#5.

Cause 3 of 9 — DescribeStackResourceDrifts error. AWS: 400 error response carrying RequestId. Emulator: RequestId missing. Why/Fix: as cause 1. Traces: cfnrep-drift-modified-property-differences#task#10.

Cause 4 of 9 — GetTemplate on a non-existent stack/stage. AWS: 400 error response carrying RequestId. Emulator: RequestId missing. Why/Fix: as cause 1. Traces: cfnrep-get-template-processed-transform#task#6.

Cause 5 of 9 — DescribeStacks on a deleted/absent stack. AWS: 400 ValidationError ("Stack with id <name> does not exist" is an example of the recorded text; the requirement is status 400 + Code ValidationError) carrying RequestId. Emulator: status and Code correct, RequestId missing. This cluster is the widest: it also covers cloudformation-generated-template-workflow#teardown#14, cloudformation-stack-create-update-delete-lifecycle#teardown#20, cloudformation-stack-recovery-operation-errors#task#10 and cloudformation-template-validation-and-planning-tools#task#8. Why/Fix: as cause 1. Traces: cfnrep-outputs-getatt-sub-export#task#5.

Cause 6 of 9 — CreateStack error (duplicate stack / invalid request). AWS: 400 + RequestId. Emulator: RequestId missing. Also affects cloudformation-stack-lifecycle-basic-crud#task#11. Why/Fix: as cause 1. Traces: cloudformation-stack-create-update-delete-lifecycle#task#12.

Cause 7 of 9 — UpdateStack error. AWS: 400 + RequestId. Emulator: RequestId missing. Why/Fix: as cause 1. Traces: cloudformation-stack-create-update-delete-lifecycle#task#13.

Cause 8 of 9 — GetTemplate with an invalid TemplateStage value. AWS: 400 ValidationError for the enum constraint on templateStage, envelope carrying RequestId. Smithy: GetTemplate.TemplateStage -> enum TemplateStage (values Original, Processed). Emulator: status 400 and Code correct, RequestId missing. NOTE: the replay also reports the enum value-set ORDER inside the message text ([Processed, Original] recorded vs [Original, Processed] emitted). That is NOT claimed as a defect here — the model declares the enum in the order the emulator prints, so the rule producing AWS's ordering is not known and the recorded message is only an example. Only the RequestId omission is the cause. Why/Fix: as cause 1. Traces: cfnrep-get-template-processed-transform#task#5.

Cause 9 of 9 — ListTypeRegistrations with an invalid RegistrationStatusFilter. AWS: 400 ValidationError for the enum constraint, envelope carrying RequestId. Smithy: ListTypeRegistrations.RegistrationStatusFilter -> enum RegistrationStatus (values COMPLETE, IN_PROGRESS, FAILED). Emulator: status and Code correct, RequestId missing. As in cause 8, the enum-order difference in the message text is reported by the replay but is explicitly NOT part of this cause (model order matches the emulator's output; AWS's ordering rule is unknown). Why/Fix: as cause 1. Traces: cloudformation-list-type-registrations-invalid-filter-rejected#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200). Nine clusters, identical detail shape.

Cause 1 (representative) — fixture cfnerr-max-results-bounds step task#2, ListStackSets:
  status: expected 400, got 400 (match)
  $.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)

Same single detail line for: cfnerr-max-results-bounds#task#5 (ListStackSetOperations), cfnrep-drift-modified-property-differences#task#10 (DescribeStackResourceDrifts), cfnrep-get-template-processed-transform#task#6 (GetTemplate), cfnrep-outputs-getatt-sub-export#task#5 (DescribeStacks; cluster of 5 steps), cloudformation-stack-create-update-delete-lifecycle#task#12 (CreateStack; cluster of 2), cloudformation-stack-create-update-delete-lifecycle#task#13 (UpdateStack).

Cause 8 — cfnrep-get-template-processed-transform#task#5, GetTemplate:
  $.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)
  $.ErrorResponse.Error.Message: expected "...enum value set: [Processed, Original]", got "...enum value set: [Original, Processed]" — message-text ordering only, not asserted as a defect (Smithy TemplateStage values are [Original, Processed]).

Cause 9 — cloudformation-list-type-registrations-invalid-filter-rejected#task#1, ListTypeRegistrations:
  $.ErrorResponse.RequestId: missing
  $.ErrorResponse.Error.Message: expected "...[IN_PROGRESS, COMPLETE, FAILED]", got "...[COMPLETE, IN_PROGRESS, FAILED]" — again message-text ordering only (Smithy RegistrationStatus values are [COMPLETE, IN_PROGRESS, FAILED]).

Expected (AWS), every cause: 400 with <ErrorResponse><Error>...</Error><RequestId>{uuid}</RequestId></ErrorResponse>.
Actual (emulator): 400 with <ErrorResponse><Error>...</Error></ErrorResponse> — no RequestId sibling.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an awsQuery CloudFormation client at the data listener.
2. Call ListStackSets with MaxResults outside [1,100] (fixture cfnerr-max-results-bounds, step task#2). Observe HTTP 400 with Code ValidationError but no <RequestId> element in the ErrorResponse body; AWS includes one.
3. Repeat with any erroring CloudFormation call to confirm it is envelope-wide: ListStackSetOperations (cfnerr-max-results-bounds#task#5), DescribeStackResourceDrifts (cfnrep-drift-modified-property-differences#task#10), GetTemplate on a missing stack (cfnrep-get-template-processed-transform#task#6), DescribeStacks on a deleted stack (cfnrep-outputs-getatt-sub-export#task#5), CreateStack duplicate (cloudformation-stack-create-update-delete-lifecycle#task#12), UpdateStack error (cloudformation-stack-create-update-delete-lifecycle#task#13), GetTemplate with TemplateStage=Bogus (cfnrep-get-template-processed-transform#task#5), ListTypeRegistrations with RegistrationStatusFilter=NOT_A_REAL_STATUS (cloudformation-list-type-registrations-invalid-filter-rejected#task#1).
4. Re-replay all nine fixture#step bindings listed in Issue-Trace-Ids plus the representative to confirm the fix.
