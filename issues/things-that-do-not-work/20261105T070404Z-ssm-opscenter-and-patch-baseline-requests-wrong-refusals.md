Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-association-and-opsitem-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-association-and-opsitem-error-paths ssm-ops-item-events-error-paths ssm-opsitem-read-apis-on-unknown-id ssm-ops-item-lifecycle ssm-ops-item-lifecycle-and-query ssm-opsitem-create-describe-events-delete ssm-patch-baseline-and-compliance-workflow
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|ListOpsItemEvents|2xx->4xx|$.Summaries
Issue-Key: ssm|ListOpsItemEvents|2xx->4xx|status
Issue-Key: ssm|AssociateOpsItemRelatedItem|4xx->4xx|$.Message
Issue-Key: ssm|AssociateOpsItemRelatedItem|4xx->4xx|$.ParameterNames[*]
Issue-Key: ssm|ListOpsItemEvents|2xx->2xx|$.Summaries[*]
Issue-Key: ssm|CreatePatchBaseline|4xx->2xx|$.__type
Issue-Key: ssm|CreatePatchBaseline|4xx->2xx|$.message
Issue-Key: ssm|CreatePatchBaseline|4xx->2xx|status
Issue-Commit: 33039e6d53cbc4640ae4d958d554409d2dbcb976

# SSM OpsCenter and Patch Baseline requests: wrong refusals, missing validation and synthesized events (4 causes)

> _Note: this issue was identified against the code base at commit `33039e6d53cbc4640ae4d958d554409d2dbcb976`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListOpsItemEvents

## Description
Four net-new root causes in service `ssm`, all on the request-admission / response-synthesis boundary of the OpsCenter (OpsItem events, related items) and Patch Manager surfaces: the emulator refuses a request AWS accepts, accepts two requests AWS refuses (one of them with the wrong refusal text and the wrong refused-parameter name), and invents a response element the recording does not contain. 4 CAUSES — each must be closed before submitting.

Cause 1 of 4 — ListOpsItemEvents filtered on an OpsItemId that does not exist is refused 400 instead of answering 200 with an empty event list (fixture ssm-association-and-opsitem-error-paths, step task#5).
AWS: Smithy: ListOpsItemEvents takes Filters -> OpsItemEventFilters (OpsItemEventFilter.Key -> OpsItemEventFilterKey, whose only value is `OpsItemId`, Operator `Equal`, Values a list of OpsItem IDs) and returns Summaries -> OpsItemEventSummaries plus NextToken; it is @paginated on items=Summaries. OpsItemNotFoundException IS bound to the operation, but the recorded response for a filter value naming an OpsItem that was never created (or was deleted) is HTTP 200 with `Summaries` present and EMPTY: a filter that selects nothing is an empty page, not a missing resource. The requirement is the status (200) and the presence of an empty `Summaries` list; no id or timestamp from the recording is part of it.
Emulator: answers HTTP 400 and the body carries no `Summaries` member at all (expected status 200 with Summaries [], got status 400) — it resolves the filter value as a resource lookup and raises the not-found class instead of filtering.
Why: a client that lists the event history of an OpsItem it cannot be sure still exists — a dashboard, a cleanup sweep, or any paginator that walks Summaries — gets an exception where AWS hands back an empty page, so the paginator aborts and the caller must special-case an error AWS never sends.
Fix: the ListOpsItemEvents handler's filter evaluation (an OpsItemId filter value that matches no stored OpsItem must yield an empty Summaries page, not an OpsItem-not-found refusal).
Traces: ssm-association-and-opsitem-error-paths#task#5, ssm-ops-item-events-error-paths#task#2, ssm-opsitem-read-apis-on-unknown-id#task#2.

Cause 2 of 4 — AssociateOpsItemRelatedItem's invalid-resource-type refusal carries emulator-authored text and names the wrong parameter (fixture ssm-ops-item-lifecycle, step task#9).
AWS: Smithy: AssociateOpsItemRelatedItem requires OpsItemId (@pattern ^(oi)-[0-9a-f]{12}$), AssociationType, ResourceType (OpsItemRelatedItemAssociationResourceType, a plain string — the permitted set is NOT modelled as an enum) and ResourceUri, and binds OpsItemInvalidParameterException (client error, HTTP 400, members Message and ParameterNames -> OpsItemParameterNamesList). The recorded refusal for an unsupported ResourceType is status 400, error OpsItemInvalidParameterException, Message `The resourceType entered is not valid. (Service: AmazonSSM; Status Code: 400; Error Code: OpsItemInvalidParameterException; Request ID: <request id>; Proxy: null)` and ParameterNames `['AssociatedResourceType']`. Fixed text: `The resourceType entered is not valid.` followed by the wrapped suffix `(Service: AmazonSSM; Status Code: 400; Error Code: OpsItemInvalidParameterException; Request ID: ` ... `; Proxy: null)`; the only variable part is the request id, taken from the serving request (the recording's id is per-run and is not a requirement). Note that AWS does NOT echo the offending value into the message and names the refused member `AssociatedResourceType`, not the wire member name.
Emulator: status 400 and the error class are right, but Message is `ResourceType <value> isn't one of the resource types OpsCenter supports: AWS::SSMIncidents::IncidentRecord, AWS::SSM::Document` (authored prose that interpolates the request's ResourceType and enumerates the emulator's own allow-list, with no wrapped Service/Status Code/Error Code/Request ID suffix) and ParameterNames[0] is `ResourceType` instead of `AssociatedResourceType`.
Why: a client that matches AWS's sentence, or that reads ParameterNames to point the user at the field to correct, sees text AWS never emits and is told to fix a parameter name that is not the one AWS reports; log/alert rules keyed on the wrapped `Error Code: ...; Request ID: ...` suffix the SDKs surface never fire.
Fix: the AssociateOpsItemRelatedItem handler's ResourceType validation error construction (AWS's fixed sentence plus the shared wrapped-service-message suffix, and ParameterNames = ['AssociatedResourceType']).
Traces: ssm-ops-item-lifecycle#task#9.

Cause 3 of 4 — ListOpsItemEvents synthesizes an `OpsItem Create` event the recorded response does not contain (fixture ssm-ops-item-lifecycle-and-query, step task#4).
AWS: Smithy: ListOpsItemEvents output Summaries -> OpsItemEventSummaries. Recorded behavior only: listing the events of a just-created OpsItem (filter Key=OpsItemId, Operator=Equal) returns status 200 with an EMPTY Summaries list — the OpsItem event history is not populated at the moment the create returns, so no entry exists to serve yet. The requirement is that the emulator does not report event entries it has not actually recorded; nothing in the recording asks for a create event at this point.
Emulator: returns status 200 but with one extra entry in Summaries, a fabricated create event (DetailType `OpsItem Create`, Source `aws.ssm`, Detail `{"Version":"1","Status":"Open"}`, CreatedBy.Arn naming the emulator's own principal `...:user/emulator`, plus a generated EventId and CreatedTime) — expected no element at $.Summaries[0], got that object. Hypothesis: the CreateOpsItem path writes a synthetic event record into the event store unconditionally, and ListOpsItemEvents serves it.
Why: a client that counts or diffs the event history (asserting a newly created item has no user-visible events yet, or correlating event ids it was handed) reads an event that never happened and is attributed to a principal that is not the caller, so event-driven OpsCenter consumers fire on a phantom event.
Fix: the ListOpsItemEvents handler / OpsItem event state — do not emit a synthesized creation event; serve only events the surface actually records.
Traces: ssm-ops-item-lifecycle-and-query#task#4, ssm-opsitem-create-describe-events-delete#task#2.

Cause 4 of 4 — CreatePatchBaseline accepts a baseline with neither ApprovedPatches nor ApprovalRules, which AWS refuses (fixture ssm-patch-baseline-and-compliance-workflow, step task#13).
AWS: Smithy models CreatePatchBaseline with Name required and both ApprovedPatches (PatchIdList, @length 0..50) and ApprovalRules (PatchRuleGroup) OPTIONAL individually, and binds only IdempotentParameterMismatch / ResourceLimitExceededException / InternalServerError — so the cross-member rule and its error class are recorded behavior only: a request that supplies neither an approved-patch list nor an approval rule group is refused with status 400, __type `ValidationException` and the fixed message `Patch Baseline attribute: Approved Patches and Approval Rule, at least one must be non empty.` (no variable parts). At least one of the two must be non-empty.
Emulator: returns HTTP 200 and mints a BaselineId — no __type and no message (expected 400 / ValidationException / the sentence above, got 200) — so the cross-member requirement is not validated at all.
Why: a client learns that a patch baseline which can never approve a patch was created successfully; it is then registered to patch groups and used by compliance scans that silently approve nothing, and the validation error the caller must surface to its user never arrives.
Fix: CreatePatchBaseline's input validation (reject before any state is written when both ApprovedPatches and ApprovalRules are absent or empty).
Traces: ssm-patch-baseline-and-compliance-workflow#task#13.

No open issue owns any of these four clusters — the replay reported owned_by empty and stub_path empty for all four. The nearest open issues cover other parts of the same crate and do NOT carry these replay keys: 20261104T073124Z-ssm-opscenter-mutations-are-never-applied-to-stored.md (UpdateOpsItem/DeleteOpsItem state and the GetOpsItem not-found message), 20261104T121102Z-ssm-10-net-new-root-causes-across-getdocument.md (behavioral: GetDocument representation, resource-policy/service-setting/GetOpsItem messages, CreateCloudConnector), 20261104T121102Z-ssm-describeopsitems-stub.md and 20261104T121102Z-ssm-updateopsitem-stub.md (unimplemented operations), and 20261103T173026Z-ssm-enum-validationexception-reports-stale-value-sets-and.md (filter-key enum text).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false, 815 steps, 77 divergence clusters — all service `ssm`; these 4 are the only clusters with stub_path empty AND owned_by empty). Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json.

Cause 1 — ssm-association-and-opsitem-error-paths#task#5, ListOpsItemEvents, verdict mismatch, status 200->400: `status: expected 200, got 400` and `$.Summaries: missing (expected [], got None)`. Cluster also affects ssm-ops-item-events-error-paths#task#2 and ssm-opsitem-read-apis-on-unknown-id#task#2 (affected_count 3). Model: ListOpsItemEvents output Summaries -> OpsItemEventSummaries, @paginated items=Summaries; OpsItemEventFilterKey enum values = [OpsItemId].

Cause 2 — ssm-ops-item-lifecycle#task#9, AssociateOpsItemRelatedItem, verdict mismatch, status 400->400: `$.Message: value_mismatch` — expected 'The resourceType entered is not valid. (Service: AmazonSSM; Status Code: 400; Error Code: OpsItemInvalidParameterException; Request ID: <request id>; Proxy: null)', got "ResourceType /aws/cloudwatch/alarm isn't one of the resource types OpsCenter supports: AWS::SSMIncidents::IncidentRecord, AWS::SSM::Document"; and `$.ParameterNames[0]: value_mismatch` — expected 'AssociatedResourceType', got 'ResourceType'. Model: OpsItemInvalidParameterException {ParameterNames: OpsItemParameterNamesList, Message: String}, httpError 400; ResourceType target OpsItemRelatedItemAssociationResourceType is an unconstrained string.

Cause 3 — ssm-ops-item-lifecycle-and-query#task#4, ListOpsItemEvents, verdict mismatch, status 200->200: `$.Summaries[0]: extra (expected None, got {'OpsItemId': <id>, 'EventId': <uuid>, 'Source': 'aws.ssm', 'DetailType': 'OpsItem Create', 'Detail': '{"Version":"1","Status":"Open"}', 'CreatedBy': {'Arn': 'arn:aws:iam::<account>:user/emulator'}, 'CreatedTime': <epoch>})`. Cluster also affects ssm-opsitem-create-describe-events-delete#task#2 (affected_count 2). The ids/timestamps above are the per-run values of one replay and are not themselves the requirement; the divergence is the presence of the entry.

Cause 4 — ssm-patch-baseline-and-compliance-workflow#task#13, CreatePatchBaseline, verdict mismatch, status 400->200: `status: expected 400, got 200`, `$.__type: missing (expected 'ValidationException', got None)`, `$.message: missing (expected 'Patch Baseline attribute: Approved Patches and Approval Rule, at least one must be non empty.', got None)`. Model: CreatePatchBaseline input Name required, ApprovedPatches/ApprovalRules optional, errors = IdempotentParameterMismatch, InternalServerError, ResourceLimitExceededException (no ValidationException bound — the class and sentence are recorded behavior).

## Steps to Reproduce
1. git clone the repository and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Cause 1: replay fixture ssm-association-and-opsitem-error-paths and inspect step task#5 — ListOpsItemEvents with Filters=[{Key:'OpsItemId', Operator:'Equal', Values:['oi-<12 hex of an OpsItem that does not exist>']}] must answer 200 with Summaries []; the emulator answers 400 with no Summaries member. Same for ssm-ops-item-events-error-paths#task#2 and ssm-opsitem-read-apis-on-unknown-id#task#2.
4. Cause 2: replay ssm-ops-item-lifecycle step task#9 — CreateOpsItem, then AssociateOpsItemRelatedItem on that OpsItemId with an unsupported ResourceType (e.g. '/aws/cloudwatch/alarm'); compare Message and ParameterNames against AWS's 'The resourceType entered is not valid. (Service: AmazonSSM; ... Proxy: null)' / ['AssociatedResourceType'].
5. Cause 3: replay ssm-ops-item-lifecycle-and-query step task#4 (or ssm-opsitem-create-describe-events-delete step task#2) — CreateOpsItem then ListOpsItemEvents filtered on the new OpsItemId; the emulator returns a fabricated 'OpsItem Create' summary where the recording has an empty Summaries list.
6. Cause 4: replay ssm-patch-baseline-and-compliance-workflow step task#13 — CreatePatchBaseline with a valid Name and OperatingSystem but neither ApprovedPatches nor ApprovalRules; the emulator returns 200 with a BaselineId instead of 400 ValidationException 'Patch Baseline attribute: Approved Patches and Approval Rule, at least one must be non empty.'
