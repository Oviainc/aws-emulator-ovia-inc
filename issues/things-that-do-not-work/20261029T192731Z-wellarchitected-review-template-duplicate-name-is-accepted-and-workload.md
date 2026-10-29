Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-review-template-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-review-template-lifecycle wellarchitected-resource-notfound-envelopes wellarchitected-workload-lifecycle
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|CreateReviewTemplate|4xx->2xx|$.Message
Issue-Key: wellarchitected|CreateReviewTemplate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|CreateReviewTemplate|4xx->2xx|status
Issue-Key: wellarchitected|GetReviewTemplate|4xx->4xx|$.Message
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.FTRSupported
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Review template duplicate name is accepted and workload/template envelopes diverge

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReviewTemplate

## Description
This bundle has 3 causes on the review-template and workload read/create envelopes.

Cause 1 of 3: a duplicate review-template name is accepted instead of refused with Conflict.
AWS: CreateReviewTemplate (POST /reviewTemplates, Smithy errors include ConflictException httpError 409) refuses a TemplateName already used in the account with 409 ConflictException{Message, ResourceId, ResourceType}; the message is the fixed text '[Conflict] A review template with name <templateName> already exists.' — fixed apart from <templateName>, taken verbatim from the request.
Emulator: returns 200 and creates a second template, with no Message and no x-amzn-errortype header.
Why: template names stop being unique in the emulator, so a client that relies on create-or-conflict to make creation idempotent silently produces duplicates, and the ARN it keeps may not be the one later lookups by name resolve to.
Fix: the CreateReviewTemplate handler's duplicate-name check against the review-template store, raising ConflictException.
Traces: wellarchitected-review-template-lifecycle#task#4.

Cause 2 of 3: GetReviewTemplate's ResourceNotFound message does not use AWS's wording for the resource noun.
AWS: GetReviewTemplate (GET /reviewTemplates/{TemplateArn}, errors ResourceNotFoundException httpError 404) answers an unknown template ARN with the fixed text '[ResourceNotFound] No Review Template with ARN <templateArn> was found in account <accountId>.' — capitalised 'Review Template' — with <templateArn> from the request and <accountId> the resolved account.
Emulator: returns 404 with the same sentence but lower-cased 'review template'.
Why: the error message is a requirement of the error class; a client matching AWS's text (or a fixture comparing it) diverges on a case difference the emulator introduces, and it is the only difference in this response, so nothing else masks it.
Fix: the ResourceNotFoundException message for the review-template resource in the GetReviewTemplate handler.
Traces: wellarchitected-resource-notfound-envelopes#task#5 (cluster also covers wellarchitected-review-template-lifecycle#task#10).

Cause 3 of 3: GetWorkload omits Workload$FTRSupported.
AWS: GetWorkload (GET /workloads/{WorkloadId}) returns GetWorkloadOutput$Workload including the FTRSupported flag, emitted as false for a workload that is not FTR-supported rather than omitted. Recorded behavior only: the on-disk Smithy model's Workload shape predates this member (it lists WorkloadId..JiraConfiguration with no FTRSupported), and the recording shows AWS emitting FTRSupported: false.
Emulator: returns 200 with Workload.FTRSupported absent from the body.
Why: a client reading the flag to decide whether to offer an FTR (Foundational Technical Review) workflow dereferences a missing member and cannot distinguish 'not supported' from 'unknown'.
Fix: the Workload serializer in the GetWorkload path (emit FTRSupported, defaulting to false).
Traces: wellarchitected-workload-lifecycle#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1, 3 mismatch clusters, all service=wellarchitected.

Cause 1 — wellarchitected-review-template-lifecycle#task#4, CreateReviewTemplate, expected 409, got 200: $.Message expected '[Conflict] A review template with name sim2real-probe-tpl-5c84e17b already exists.', got None; $headers.x-amzn-errortype expected (shaped), got None. Fixed text: '[Conflict] A review template with name ' + ' already exists.'; variable: the TemplateName from the request.

Cause 2 — wellarchitected-resource-notfound-envelopes#task#5, GetReviewTemplate, 404/404: $.Message expected '[ResourceNotFound] No Review Template with ARN arn:aws:wellarchitected:us-west-2:000000000000:review-template/00000000000000000000000000000000 was found in account 000000000000.', got '[ResourceNotFound] No review template with ARN arn:aws:wellarchitected:us-west-2:000000000000:review-template/00000000000000000000000000000000 was found in account 000000000000.' — the only difference is the casing of the fixed noun 'Review Template'. Cluster also covers wellarchitected-review-template-lifecycle#task#10.

Cause 3 — wellarchitected-workload-lifecycle#task#2, GetWorkload, 200/200: $.Workload.FTRSupported expected False, got absent.

Smithy: CreateReviewTemplate POST /reviewTemplates -> {TemplateArn}, errors AccessDenied(403)/Conflict(409)/InternalServer(500)/ResourceNotFound(404)/ServiceQuotaExceeded(402)/Throttling(429)/Validation(400); ConflictException{Message, ResourceId, ResourceType} httpError 409; GetReviewTemplate GET /reviewTemplates/{TemplateArn}, errors ResourceNotFoundException(404); GetWorkload GET /workloads/{WorkloadId} -> GetWorkloadOutput$Workload (model's Workload shape has no FTRSupported member — recorded behavior only).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. POST /reviewTemplates with a TemplateName, then POST again with the same TemplateName (and a fresh ClientRequestToken) — the emulator answers 200 instead of 409 ConflictException (cause 1).
4. GET /reviewTemplates/arn:aws:wellarchitected:<region>:<account>:review-template/00000000000000000000000000000000 and compare $.Message's resource noun casing (cause 2).
5. POST /workloads, then GET /workloads/{WorkloadId} and look for Workload.FTRSupported in the body (cause 3).
