Issue-Source: trace-sweep-divergence
Issue-Trace: rbin-tag-level-rule-lifecycle
Issue-Step: teardown#7
Issue-Severity: LOW
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rbin-tag-level-rule-lifecycle
Issue-Shape-Id: com.amazonaws.rbin#AmazonRecycleBin
Issue-Key: rbin|GetRule|4xx->4xx|$.Reason
Issue-Commit: 76f521ae47bf59ffa39ed55f2f185671b1dad8e5

# GetRule 404 body adds a Reason member the real service omits

> _Note: this issue was identified against the code base at commit `76f521ae47bf59ffa39ed55f2f185671b1dad8e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRule

## Description
Cause 1 of 1: the ResourceNotFoundException body this emulator returns for a rule-addressed read of a rule that does not exist carries a populated `Reason` member, while the recorded service returns the same 404 with no `Reason` at all.

AWS: `GetRule` is `GET /rules/{Identifier}` and models `ResourceNotFoundException` as its 404 (Smithy: GetRule errors = InternalServerException 500, ResourceNotFoundException 404, ValidationException 400; `com.amazonaws.rbin#ResourceNotFoundException` members `Message` -> ErrorMessage and `Reason` -> ResourceNotFoundExceptionReason). `Reason` is an OPTIONAL member — the model does not require it to be serialized, and the recorded response for this condition (reading a rule immediately after it was deleted) carries the 404 with `Reason` ABSENT. So on this path the field is simply not emitted by the service; the recorded behavior is the only statement about it.

Emulator: the emulator answers the same request with HTTP 404 (status matches) but serializes `Reason: "RULE_NOT_FOUND"` into the error body. Replay reports `$.Reason: type_mismatch (expected None, got 'RULE_NOT_FOUND')` for GetRule on step teardown#7, i.e. an extra member in the error payload rather than a status or error-code difference.

Why: a client that branches on whether `ResourceNotFoundException.Reason` is present — the normal way to distinguish an error the service classified from one it did not — takes the classified branch against the emulator and the unclassified branch against AWS. Code generated to treat a present `Reason` as authoritative (logging, retry classification, user-facing mapping) therefore behaves differently between the two, and a test written against the emulator pins a field real Recycle Bin never sends on this path.

Fix: the not-found error constructor shared by the rule-addressed handlers (the helper in the service crate's state module that builds ResourceNotFoundException, consumed by the GetRule handler and its siblings) should leave `Reason` unset for the rule-does-not-exist path so the serialized body matches the recorded shape. Note this reverses a previous speculative, non-trace-grounded change: the resolved issue 20261024T092642Z-rbin-getrule-error-response.md ("ResourceNotFoundException never carries its modelled Reason (RULE_NOT_FOUND)") argued from the model alone that the member should be populated, and the earlier resolved issue 20261022T174824Z-rbin-error-responses-validationexception-body-omits-message-and.md had previously recorded the opposite. Neither is open and neither carries this replay key, so this divergence is unowned. The regression test that pins the populated value (the retention-rule fidelity test asserting Reason is present) must be updated with the helper, and only the rule-does-not-exist path is in scope here — the other reason enums this service populates (ConflictException.Reason, ValidationException.Reason) are not affected.

Traces: rbin-tag-level-rule-lifecycle#teardown#7 (GetRule after the rule is deleted; expected 404 without Reason, got 404 with Reason=RULE_NOT_FOUND).

## Evidence
Replay divergence (cluster key `service=rbin|GetRule|status=404->404|keys=$.Reason`, verdict=mismatch, affected_count=1):
  fixture_id: rbin-tag-level-rule-lifecycle
  step_key:   teardown#7
  operation:  GetRule
  expected_status: 404   actual_status: 404
  detail: `$.Reason: type_mismatch (expected None, got 'RULE_NOT_FOUND')`

Expected (recorded service): HTTP 404, ResourceNotFoundException, body with Message only — `Reason` absent.
Actual (emulator):           HTTP 404, ResourceNotFoundException, body additionally carrying `Reason: "RULE_NOT_FOUND"`.

Model evidence: smithy_query query=operation operation=GetRule -> http GET /rules/{Identifier}, errors include ResourceNotFoundException (httpError 404). smithy_query query=shape shape=ResourceNotFoundException -> members {Message: ErrorMessage, Reason: ResourceNotFoundExceptionReason}; `Reason` is not required, so the model does not mandate emitting it and the recording shows it is not emitted on this path.

Status and error code agree; the divergence is confined to the presence of the optional `Reason` member in the error body.

## Steps to Reproduce
1. Create a tag-level retention rule (POST /rules) and note its Identifier.
2. Delete it (DELETE /rules/{Identifier}).
3. Read it again: GET /rules/{Identifier}.
4. Observe HTTP 404 with a ResourceNotFoundException body that includes "Reason":"RULE_NOT_FOUND"; the recorded service returns the 404 with no "Reason" member.
5. Equivalent: GET /rules/0000000000x for any syntactically valid identifier no rule holds.
6. Replay fixture rbin-tag-level-rule-lifecycle, step teardown#7, to reproduce the reported mismatch on $.Reason.
