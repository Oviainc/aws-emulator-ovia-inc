Issue-Source: trace-sweep-divergence
Issue-Trace: dataexchange-dataset-catalog-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dataexchange-dataset-catalog-lifecycle dataexchange-event-action-lifecycle dataexchange-job-error-paths
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange|GetDataSet|4xx->4xx|$.Message
Issue-Key: dataexchange|GetEventAction|4xx->4xx|$.Message
Issue-Key: dataexchange|UpdateEventAction|4xx->4xx|$.Message
Issue-Key: dataexchange|DeleteEventAction|4xx->4xx|$.Message
Issue-Key: dataexchange|GetJob|4xx->4xx|$.Message
Issue-Key: dataexchange|CreateJob|4xx->4xx|$.Message
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# DataExchange client-error messages are emulator prose instead of AWS's fixed wording

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataSet

## Description
Every dataexchange client-error body carries the right HTTP status and the right exception, but the Message member is text the emulator composed rather than the wording AWS serves. Six causes across the not-found plane and the CreateJob validation plane.

Causes 1-5 share one rule: ResourceNotFoundException's Message is FIXED text — 'The requested resource was not found.' — with no variable part at all. The identity of the missing resource is carried by the exception's own ResourceId and ResourceType members (Smithy: ResourceNotFoundException { Message required, ResourceId, ResourceType }), not by the sentence. The emulator instead interpolates the resource kind and the requested id into a bespoke sentence of the form \"The <kind> <id> couldn't be found.\" — the id there is a request value, and nothing in that sentence is AWS's text.

Cause 1 of 6: GetDataSet with an unknown DataSetId. AWS: 404 ResourceNotFoundException, Message 'The requested resource was not found.' (Smithy: GetDataSet errors include ResourceNotFoundException @httpError 404). Emulator: 404 but Message \"The data set <DataSetId> couldn't be found.\". Why: a client that matches on the documented message, or logs it, sees text AWS never emits, and the resource identity lands in prose instead of ResourceId/ResourceType. Fix: the shared not-found error constructor for the dataexchange crate (error construction, not the handlers' lookup logic). Traces: dataexchange-dataset-catalog-lifecycle#task#1, #teardown#8.

Cause 2 of 6: GetEventAction with an unknown EventActionId. AWS: 404 ResourceNotFoundException, Message 'The requested resource was not found.'. Emulator: \"The event action <EventActionId> couldn't be found.\". Fix: same error constructor, event-action lookup path. Traces: dataexchange-event-action-lifecycle#task#2.

Cause 3 of 6: UpdateEventAction on an unknown EventActionId. AWS: 404 ResourceNotFoundException with the same fixed Message. Emulator: \"The event action <EventActionId> couldn't be found.\". Fix: same. Traces: dataexchange-event-action-lifecycle#task#4.

Cause 4 of 6: DeleteEventAction on an unknown EventActionId. AWS: 404 ResourceNotFoundException with the same fixed Message. Emulator: \"The event action <EventActionId> couldn't be found.\". Fix: same. Traces: dataexchange-event-action-lifecycle#task#6.

Cause 5 of 6: GetJob with an unknown JobId. AWS: 404 ResourceNotFoundException with the same fixed Message (Smithy: GetJob errors include ResourceNotFoundException @httpError 404). Emulator: \"The job <JobId> couldn't be found.\". Fix: same. Traces: dataexchange-job-error-paths#task#3.

Cause 6 of 6: CreateJob whose Details does not carry the member its Type requires. AWS: 400 ValidationException whose Message is built from the fixed prefix 'The following parameter(s) are not valid: ' followed by one clause per fault, each of the fixed form '\"<member path>\" is required' — the member path being the Details member the requested Type demands (for Type EXPORT_ASSET_TO_SIGNED_URL that is Details.ExportAssetToSignedUrl) — and, when the request also carries Details members that Type does not permit, the fixed clause 'Unsupported request parameter(s) were found.'; clauses are comma-separated (Smithy: CreateJob input Details -> RequestDetails required, Type required; errors include ValidationException @httpError 400). Emulator: 400 ValidationException but Message 'Details must carry exactly the ExportAssetToSignedUrl member for a EXPORT_ASSET_TO_SIGNED_URL job, got ImportAssetFromSignedUrl' — the emulator's own sentence, which names no parameter path in AWS's quoted form and omits the unsupported-parameter clause entirely. Variable parts of the AWS message are the member path(s) derived from the request's Type and Details; everything else is fixed text. Fix: CreateJob's request validation, where the Type/Details cross-check composes its message. Traces: dataexchange-job-error-paths#task#6.

No open issue covers dataexchange; the nearest filed analogues are per-service message-wording issues for other services (e.g. the sagemaker DescribeTrialComponent and ses CustomVerificationEmailTemplateDoesNotExist wording issues), which do not cover this service.

## Evidence
Cause 1 — GetDataSet, expected 404 / actual 404: $.Message value_mismatch (expected 'The requested resource was not found.', got "The data set zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz couldn't be found."). The 32-character id is the request's DataSetId, i.e. a request value interpolated into prose; AWS's Message has no variable part.
Cause 2 — GetEventAction, 404/404: expected 'The requested resource was not found.', got "The event action aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa couldn't be found.".
Cause 3 — UpdateEventAction, 404/404: expected 'The requested resource was not found.', got "The event action bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb couldn't be found.".
Cause 4 — DeleteEventAction, 404/404: expected 'The requested resource was not found.', got "The event action cccccccccccccccccccccccccccccccc couldn't be found.".
Cause 5 — GetJob, 404/404: expected 'The requested resource was not found.', got "The job aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa couldn't be found.".
Cause 6 — CreateJob, 400/400: expected 'The following parameter(s) are not valid: "Details.ExportAssetToSignedUrl" is required, Unsupported request parameter(s) were found.', got 'Details must carry exactly the ExportAssetToSignedUrl member for a EXPORT_ASSET_TO_SIGNED_URL job, got ImportAssetFromSignedUrl'.

Smithy: ResourceNotFoundException { Message: __string required (doc "The resource couldn't be found."), ResourceId: __string ("The unique identifier for the resource that couldn't be found."), ResourceType: ResourceType } @error client @httpError 404. ValidationException { Message required, ExceptionCause } @error client @httpError 400.

The placeholder ids (zzzz…, aaaa…, bbbb…, cccc…) are one run's request values, not part of the required text.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: GET /v1/data-sets/<a syntactically valid but unknown 32-char id> (dataexchange-dataset-catalog-lifecycle#task#1). 404 is returned, but $.Message is the emulator's interpolated sentence instead of 'The requested resource was not found.'.
3. Causes 2-4: GET, PATCH and DELETE /v1/event-actions/<unknown id> (dataexchange-event-action-lifecycle#task#2, #task#4, #task#6) — same substitution with "event action".
4. Cause 5: GET /v1/jobs/<unknown id> (dataexchange-job-error-paths#task#3) — same substitution with "job".
5. Cause 6: POST /v1/jobs with Type=EXPORT_ASSET_TO_SIGNED_URL and Details carrying ImportAssetFromSignedUrl (dataexchange-job-error-paths#task#6). 400 ValidationException is returned with the emulator's own sentence instead of the 'The following parameter(s) are not valid: "Details.ExportAssetToSignedUrl" is required, Unsupported request parameter(s) were found.' form.
6. In each case compare $.Message against AWS's wording for that error class.
