Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# ListTagsForResource answers 404 ResourceNotFoundException for an ARN Interconnect rejects with HTTP 400

_Filed: 2026-10-24T12:45:29Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An `arn` that is a well-formed Interconnect connection ARN but names no connection is answered by this emulator with `ResourceNotFoundException` and HTTP 404, and so is an ARN whose account id (or region/partition) is not the caller's — the handler resolves the resource by full-string ARN match and funnels every miss into one 404.

Interconnect does not classify these as 404s. The recorded behaviour of the sibling tagging operation on the identical `arn` member (`AmazonResourceName`, `@length 59..150`, `@pattern ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$`, bound to the same `ConnectionResource`) splits the two negative classes and makes both HTTP 400:
* ARN whose account id is NOT the caller's -> `InterconnectValidationException` (400)
* ARN well-formed and in-account but naming no existing Connection -> `InterconnectClientException` (400)

The same 400-for-not-found rule is independently recorded for the connection-identifier operations of this service (GetConnection/DeleteConnection/UpdateConnection are recorded answering 400, not 404, for a pattern-valid identifier that names nothing).

Expected: an in-account connection ARN naming nothing -> HTTP 400 `InterconnectClientException`; a cross-account ARN -> HTTP 400 `InterconnectValidationException` (two DIFFERENT codes, so a client can tell "wrong account" from "no such connection"). Actual: HTTP 404 `ResourceNotFoundException` for both.

## Evidence

Handler: services/service-com-amazonaws-interconnect-interconnect/src/ops/list_tags_for_resource.rs lines 178-193 — the single `found` lookup via `store.connection_by_arn(&input.arn)` and `else -> ResourceNotFoundException { message: connection_not_retrievable_message(&input.arn) }` (serialized 404 by the model's `@httpError`). Resolution rule: src/state.rs `connection_by_arn` + `holds_arn` (full field-for-field ARN match), so a cross-account/other-region/other-partition ARN is indistinguishable from an unallocated id.

Model (smithy_query): ListTagsForResource input `arn` -> AmazonResourceName (the same shape TagResource takes); service common errors include InterconnectClientException (400), InterconnectValidationException (400), ResourceNotFoundException (404) — all three are available to this operation.

Recorded grounding: the open issue 20261022T145736Z-interconnect-error-semantics-... Cause 3 records TagResource answering InterconnectValidationException for a cross-account ARN and InterconnectClientException for an in-account ARN naming no connection; the open wire issue 20261023T095130Z-... Cause 3 records HTTP 400 (not 404) for connection-not-found on this service. Neither issue names ListTagsForResource, so this (operation, error-response) pair is unfiled.

Test coverage: tests/all/list_tags_for_resource.rs lines 341-392 currently ASSERTS the divergent behaviour (status 404 + ResourceNotFoundException for "another account", "another region", "another partition" and unallocated ids), so those rows must move to 400 with the two distinct codes when this is fixed.

## Steps to Reproduce

1. Boot the emulator and create a connection: CreateConnection on a seeded environment; note the returned connection `arn`.
2. Call ListTagsForResource with an in-account, well-formed ARN naming no connection, e.g. arn:aws:interconnect:us-east-1:<emulator account>:connection/mcc-00000000 -> expect HTTP 400 with code InterconnectClientException; observe HTTP 404 with ResourceNotFoundException.
3. Call ListTagsForResource with the live connection's ARN but the account id replaced by 123456789012 -> expect HTTP 400 with code InterconnectValidationException; observe HTTP 404 with ResourceNotFoundException.
4. Confirm the two negative classes are reported with DIFFERENT error codes (validation vs client), which they currently are not.
