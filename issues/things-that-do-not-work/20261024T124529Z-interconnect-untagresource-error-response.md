Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# UntagResource answers 404 ResourceNotFoundException for an ARN Interconnect rejects with HTTP 400

_Filed: 2026-10-24T12:45:29Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UntagResource resolves its `arn` by full-string match against the stored connection ARN and answers every miss with `ResourceNotFoundException` / HTTP 404 — both for an in-account, well-formed connection ARN that names no connection and for an ARN whose account id (or region/partition) is not the caller's.

Interconnect answers both of those with HTTP 400, and with two distinct codes, as recorded for the sibling tagging operation that takes the identical `arn` member (`AmazonResourceName`, same `@pattern`, same `ConnectionResource` binding):
* ARN whose account id is NOT the caller's -> `InterconnectValidationException` (400)
* ARN well-formed and in-account but naming no existing Connection -> `InterconnectClientException` (400)

The 400-for-not-found rule is independently recorded for this service's connection-identifier operations (GetConnection/DeleteConnection/UpdateConnection are recorded answering 400 for a pattern-valid identifier that names nothing).

Expected: HTTP 400 with `InterconnectClientException` for an in-account ARN naming nothing and HTTP 400 with `InterconnectValidationException` for a cross-account ARN. Actual: HTTP 404 `ResourceNotFoundException` for both, so a client cannot tell "wrong account ARN" from "no such connection" and status-driven handling diverges on the most common negative path of the operation.

## Evidence

Handler: services/service-com-amazonaws-interconnect-interconnect/src/ops/untag_resource.rs lines 166-181 — `guard.connection_by_arn_mut(&input.arn)` and `else -> ResourceNotFoundException { message: connection_not_retrievable_message(&input.arn) }` (404 via the model's `@httpError`). Resolution rule: src/state.rs `connection_by_arn_mut` + `holds_arn` (field-for-field ARN equality), which makes a foreign-account ARN indistinguishable from an unallocated id.

Model (smithy_query): UntagResource input `arn` -> AmazonResourceName, `tagKeys` required; service common errors include InterconnectClientException (400), InterconnectValidationException (400) and ResourceNotFoundException (404), so all three are available here.

Recorded grounding: open issue 20261022T145736Z-interconnect-error-semantics-... Cause 3 (TagResource: cross-account ARN -> InterconnectValidationException; in-account nonexistent connection -> InterconnectClientException) and open wire issue 20261023T095130Z-... Cause 3 (connection-not-found is HTTP 400, not 404). Neither issue names UntagResource, so this (operation, error-response) pair is unfiled.

Test coverage: tests/all/resource_tags.rs lines 379-428 currently asserts the divergent behaviour (404 + ResourceNotFoundException for "another account", "another region", "another partition" and unallocated ids); those rows must move to 400 with the two distinct codes when this is fixed.

## Steps to Reproduce

1. Boot the emulator, CreateConnection with tags {"env":"test"} on a seeded environment and note the returned `arn`.
2. Call UntagResource with an in-account ARN naming no connection, e.g. arn:aws:interconnect:us-east-1:<emulator account>:connection/mcc-00000000, tagKeys=["env"] -> expect HTTP 400 with code InterconnectClientException; observe HTTP 404 with ResourceNotFoundException.
3. Call UntagResource with the live ARN but the account id replaced by 123456789012 -> expect HTTP 400 with code InterconnectValidationException; observe HTTP 404 with ResourceNotFoundException.
4. Verify the live connection's tags are unchanged by both refusals (they are) and that the two refusals carry different error codes (they currently do not).
