Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-error-invalid-filter-and-pagination-constraints
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-error-invalid-filter-and-pagination-constraints
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListObjectives|4xx->4xx|$.Message
Issue-Key: controlcatalog|ListCommonControls|4xx->4xx|$.Message
Issue-Commit: 75d52ea0c257a701244c018d7c3f550d51e1d46f

# Invalid-filter ValidationException Message omits AWS's downstream-exception envelope on filtered list operations

> _Note: this issue was identified against the code base at commit `75d52ea0c257a701244c018d7c3f550d51e1d46f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectives

## Description
Two net-new wire causes in the controlcatalog emulator, one root fault: the single shared invalid-filter refusal sentence is emitted bare, without the downstream-exception envelope AWS appends to that Message, on every list operation that validates a single-resource filter. HTTP status (400) and error code (ValidationException) are correct in both causes; only the Message text diverges.

Related open issue, which does NOT cover these: 20261102T095923Z-controlcatalog-listcommoncontrols-the-invalid-filter-validationexception-message-omits-aws.md records the same fault from the BEHAVIORAL suite, but it carries only behavioral check ids (Issue-Trace-Ids) and no wire replay key, so neither of the fixture steps below is bound to it and neither would be re-replayed by a fixer closing it. The other open controlcatalog issues are different faults: ...-listcommoncontrols-returns-an-empty-commoncontrols-list-... and ...-listdomains-and-listobjectives-return-empty-catalog-taxonomy-lists.md are about empty result sets, not refusal wording.

Cause 1 of 2: ListObjectives refusal of an invalid ObjectiveFilter returns the bare sentence (fixture controlcatalog-error-invalid-filter-and-pagination-constraints, step task#1).
  AWS: ListObjectives takes ObjectiveFilter and declares ValidationException (Smithy: com.amazonaws.controlcatalog#ListObjectives errors AccessDeniedException/403, InternalServerException/500, ThrottlingException/429, ValidationException errorKind client @httpError 400; http POST /objectives). The model does not fix the message text, so recorded behavior only for the wording: on the wire the Message is the fixed sentence "The provided filter is not valid. Ensure that each part of the filter is valid, and try again." followed by a single space and the envelope of the downstream exception ControlCatalog surfaces, whose fixed text is the literal "(Service: " plus the "Status Code: " and "Request ID: " labels, and whose variable parts are the downstream service name, the numeric status code (400 for this class) and the request id of the request being refused. Those variable parts must come from the live request/deployment, never from a recording. (The recording's trailing " (SDK Attempt Count: 1)" parenthetical is client-side SDK decoration and the recorded request id is per-run; neither is a requirement.)
  Emulator: returns 400 / ValidationException with Message exactly "The provided filter is not valid. Ensure that each part of the filter is valid, and try again." — the string ends at the period and the envelope is absent entirely.
  Why: a client, test or log pipeline that reads the ValidationException Message for this class and keys on the envelope — to extract the request id for correlation or a support case, or to match the full documented message — finds nothing to parse against the emulator, so a correlation path that works against AWS is silently dead.
  Fix: the shared invalid-filter refusal wording in this service's single-resource-filter validation, and the point where each list handler lifts it into its modeled ValidationException. Because the envelope carries the current request's id it cannot be a compile-time constant and must be assembled where the error is built.
  Traces: controlcatalog-error-invalid-filter-and-pagination-constraints#task#1.

Cause 2 of 2: ListCommonControls refusal of an invalid CommonControlFilter returns the same bare sentence (fixture controlcatalog-error-invalid-filter-and-pagination-constraints, step task#2).
  AWS: ListCommonControls takes CommonControlFilter and declares the same error set (Smithy: com.amazonaws.controlcatalog#ListCommonControls errors AccessDeniedException/403, InternalServerException/500, ThrottlingException/429, ValidationException errorKind client @httpError 400; http POST /common-controls). Recorded behavior only for the wording, identical to cause 1: fixed sentence, then the "(Service: " / "Status Code: " / "Request ID: " envelope whose values are per-request.
  Emulator: 400 / ValidationException with the bare sentence, envelope absent.
  Why: same as cause 1 — the envelope AWS always appends to this Message is never present, so request-id extraction and full-message matching fail.
  Fix: same component as cause 1 — both operations funnel through the one shared invalid-filter sentence, so one fix must satisfy both, and both fixture steps must be re-replayed.
  Traces: controlcatalog-error-invalid-filter-and-pagination-constraints#task#2.

## Evidence
Replay of simparity-wire-aws-v1, fixture controlcatalog-error-invalid-filter-and-pagination-constraints:

step task#1 (ListObjectives) — expected status 400, actual 400 (match). $.Message: value_mismatch — expected "The provided filter is not valid. Ensure that each part of the filter is valid, and try again. (Service: ControlCatalogOntology, Status Code: 400, Request ID: <per-run id>) (SDK Attempt Count: 1)", got "The provided filter is not valid. Ensure that each part of the filter is valid, and try again.".

step task#2 (ListCommonControls) — expected status 400, actual 400 (match). $.Message: value_mismatch — expected "The provided filter is not valid. Ensure that each part of the filter is valid, and try again. (Service: ControlCatalogOntology, Status Code: 400, Request ID: <per-run id>) (SDK Attempt Count: 1)", got "The provided filter is not valid. Ensure that each part of the filter is valid, and try again.".

Which parts are the requirement: the fixed sentence "The provided filter is not valid. Ensure that each part of the filter is valid, and try again.", the single space after it, and the envelope's fixed text "(Service: ", "Status Code: ", "Request ID: " with their separators. Variable, taken from the live request/deployment and NOT to be copied from these recordings: the downstream service name, the status code value, and the request id. The trailing "(SDK Attempt Count: 1)" is client-side SDK rendering present in the recording and is not part of the wire requirement. The emulator supplies none of the envelope at all, which is the defect — not merely a differing per-run value.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture controlcatalog-error-invalid-filter-and-pagination-constraints, step task#1: POST /objectives (ListObjectives) with an invalid ObjectiveFilter (more than one resource ARN in the filter). Observe HTTP 400 / ValidationException whose Message ends at "... and try again." with no " (Service: ..." envelope.
3. Same fixture, step task#2: POST /common-controls (ListCommonControls) with an invalid CommonControlFilter (more than one objective ARN). Observe the identical truncated Message.
4. Both steps are reported as mismatch clusters on $.Message only; status and error code already match.
