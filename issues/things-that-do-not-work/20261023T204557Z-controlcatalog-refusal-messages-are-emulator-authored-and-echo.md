Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-error-getcontrol-nonexistent-control
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-error-getcontrol-nonexistent-control controlcatalog-error-invalid-filter-and-pagination-constraints
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|GetControl|4xx->4xx|$.Message
Issue-Key: controlcatalog|ListObjectives|4xx->4xx|$.Message
Issue-Key: controlcatalog|ListCommonControls|4xx->4xx|$.Message
Issue-Commit: 9b28682be3d06b0e1ca8f56d8f0bc182f6004def

# controlcatalog refusal messages are emulator-authored and echo request detail AWS keeps out of the message

> _Note: this issue was identified against the code base at commit `9b28682be3d06b0e1ca8f56d8f0bc182f6004def`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetControl

## Description
Three refusal paths return the correct HTTP status and error shape but a locally composed message. In each case the recorded AWS message is a FIXED, request-independent string — the same text appears for different offending members and different ARNs — so the rule that produces it is observable: ControlCatalog does not echo the caller's input in the message. The emulator instead interpolates the rejected ARN / the offending filter member. All three causes must be closed. The resolved issue 20261021T234107Z-controlcatalog-validation-error-messages-inject-a-rejected-value-rendering describes this family but is CLOSED, so it does not cover these replays.

Cause 1 of 3: GetControl on an unknown control ARN.
  AWS: ResourceNotFoundException (Smithy: GetControl's only modelled error, @httpError 404, doc 'The requested resource does not exist.'), status 404, with the constant message 'AWS Control Tower cannot find a control with the ControlArn value that you provided.' — the message does not contain the submitted ARN (recorded behavior; the rule is that the text is request-independent).
  Emulator: status 404 (correct) with Message 'the Control Catalog holds no control with the ARN arn:aws:controlcatalog:::control/nonexistentctrlid00000000' — lowercase, emulator-authored, and echoing the caller's ARN.
  Why: clients and test fixtures that assert on the documented not-found text, and log scrubbers that assume AWS does not echo caller input into ControlCatalog messages, both break.
  Fix: the GetControl handler's not-found refusal (the message template, not the status or shape).
  Traces: controlcatalog-error-getcontrol-nonexistent-control#task#1.

Cause 2 of 3: ListObjectives with two ARNs in ObjectiveFilter.Domains.
  AWS: ValidationException (service-common error, @httpError 400, doc 'The request has invalid or missing parameters.'), status 400, message 'The provided filter is not valid. Ensure that each part of the filter is valid, and try again.' The Smithy doc for ObjectiveFilter.Domains states 'You can use this parameter to specify one domain ARN at a time. Passing multiple ARNs in the ObjectiveFilter isn't supported', and the recorded refusal does NOT name the member or the count. (The '(Service: ControlCatalogOntology, Status Code: 400, Request ID: ...)' suffix in the recording is SDK/environment decoration and is not a requirement.)
  Emulator: status 400 (correct) with Message 'objectiveFilter.domains accepts one domain ARN at a time; 2 were supplied' — a member-and-count rendering AWS never emits.
  Why: the emulator teaches clients a message format AWS does not use; assertions on the generic text fail.
  Fix: the ListObjectives filter validation's refusal message.
  Traces: controlcatalog-error-invalid-filter-and-pagination-constraints#task#1.

Cause 3 of 3: ListCommonControls with two ARNs in CommonControlFilter.Objectives.
  AWS: the same ValidationException with the same constant text 'The provided filter is not valid. Ensure that each part of the filter is valid, and try again.' — identical to cause 2 despite a different member, which is the evidence that the text is fixed rather than request-built. Smithy doc for CommonControlFilter.Objectives: 'You can use this parameter to specify one objective ARN at a time.'
  Emulator: status 400 (correct) with Message 'commonControlFilter.objectives accepts one objective ARN at a time; 2 were supplied'.
  Why: as cause 2.
  Fix: the ListCommonControls filter validation's refusal message.
  Traces: controlcatalog-error-invalid-filter-and-pagination-constraints#task#2.

## Evidence
Cause 1 (controlcatalog-error-getcontrol-nonexistent-control#task#1, expected 404 / actual 404): "$.Message: value_mismatch (expected 'AWS Control Tower cannot find a control with the ControlArn value that you provided.', got 'the Control Catalog holds no control with the ARN arn:aws:controlcatalog:::control/nonexistentctrlid00000000')".
Cause 2 (controlcatalog-error-invalid-filter-and-pagination-constraints#task#1, expected 400 / actual 400): "$.Message: value_mismatch (expected 'The provided filter is not valid. Ensure that each part of the filter is valid, and try again. (Service: ControlCatalogOntology, Status Code: 400, Request ID: ...)', got 'objectiveFilter.domains accepts one domain ARN at a time; 2 were supplied')".
Cause 3 (same fixture, #task#2): expected the identical generic filter text, got 'commonControlFilter.objectives accepts one objective ARN at a time; 2 were supplied'.
Status and error classification match in all three; only the message body diverges.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. POST /get-control with ControlArn arn:aws:controlcatalog:::control/nonexistentctrlid00000000 (fixture controlcatalog-error-getcontrol-nonexistent-control, step task#1) -> 404 with an emulator-authored message echoing the ARN.
3. POST /objectives with ObjectiveFilter.Domains holding two domain ARNs (fixture controlcatalog-error-invalid-filter-and-pagination-constraints, step task#1) -> 400 with a member/count-specific message.
4. POST /common-controls with CommonControlFilter.Objectives holding two objective ARNs (same fixture, step task#2) -> 400 with a member/count-specific message.
