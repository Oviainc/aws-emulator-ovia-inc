Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.update-partnership-nonexistent-capability-not-found
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi.update-partnership-nonexistent-capability-not-found|run#12|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# b2bi UpdatePartnership does not validate referenced capability ids: nonexistent capability answers NotImplemented instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePartnership

## Description
Cause 1 of 1: UpdatePartnership performs no referential validation of the `capabilities` list it is handed, so an update naming a capability id that does not exist is never refused as AWS refuses it.

AWS: Smithy (services/service-com-amazonaws-b2bi-b2bi/smithy-model/com-amazonaws-b2bi-b2bi.json) models UpdatePartnership with input member `capabilities` (target PartnershipCapabilities, a list of CapabilityId) and binds `ResourceNotFoundException` (errorKind client, httpError 404) among its errors. When any id in the submitted `capabilities` list does not resolve to an existing capability in the account/region, AWS refuses the whole call with HTTP 404, error code `ResourceNotFoundException`, and (recorded behavior) the message template `Capability ID <capabilityId> not found.` — the fixed text is `Capability ID ` + ` not found.` and the variable part is the offending capability id taken verbatim from the request's `capabilities` list (in the recording that value was the client-supplied non-existent id). Because the call is refused, the partnership is left untouched: a subsequent GetPartnership still returns the pre-update `capabilities` list, i.e. validation happens before any mutation is applied (no partial write of the valid entries). A later UpdatePartnership naming only existing capability ids succeeds with HTTP 200 and the response (and an independent GetPartnership) echoes the new list.

Emulator: the call is refused with error code `NotImplemented` and message `operation UpdatePartnership is scaffolded but not implemented in this emulator` (HTTP 501), so the check's assertion `code == "ResourceNotFoundException"` fails with `AssertionError: expected ResourceNotFoundException, got 'NotImplemented'`. Nothing downstream of that refusal is exercised: neither the message template, nor the "no mutation on a rejected update" guarantee, nor the positive-control accept path.

Why: a client that adds or swaps capabilities on a partnership cannot distinguish a bad reference from a transport/implementation failure. `NotImplemented`/501 is not in UpdatePartnership's modeled error set, so SDK error handling that branches on `ResourceNotFoundException` (to re-resolve a capability id, or to report "capability X does not exist" to the user) never fires, and a retry-on-5xx client will retry a call that can never succeed. Once the operation is implemented without this check, the worse failure is silent: a partnership would be left referencing a capability id that does not exist, or partially updated.

Fix: the UpdatePartnership handler and its validation — services/service-com-amazonaws-b2bi-b2bi/src/ops/update_partnership.rs. Each id in the request's `capabilities` list must be resolved against capability state BEFORE any mutation; the first unresolvable id must produce the modeled ResourceNotFoundException (404) with the message template above, and the partnership record must be left exactly as it was. NOTE: the open issue `20261029T193110Z-b2bi-updatepartnership-stub.md` ("Implement b2bi UpdatePartnership: unimplemented scaffold stub") asks only that the operation be implemented and carries only the replay key `b2bi.update-partnership-capabilities-list-replace`; it does not describe this referential-validation requirement and does not carry this check's replay key, so implementing the happy path alone will leave this check failing.

Traces: b2bi.update-partnership-nonexistent-capability-not-found.

## Evidence
Check b2bi.update-partnership-nonexistent-capability-not-found (verdict: mismatch).

Expected (AWS): UpdatePartnership(partnershipId=<existing>, capabilities=[<existing cap>, <nonexistent cap id>]) -> HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "Capability ID <the nonexistent id from the request> not found."; a following GetPartnership(partnershipId=<existing>) still returns capabilities == [<existing cap>]; then UpdatePartnership(partnershipId=<existing>, capabilities=[<other existing cap>]) -> HTTP 200 with capabilities == [<other existing cap>], confirmed by an independent GetPartnership.

Actual (emulator): the UpdatePartnership call is refused with HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation UpdatePartnership is scaffolded but not implemented in this emulator".

Replay failure:
  File "check_b2bi_update_partnership_nonexistent_capability_not_found.py", line 43, in run
    assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
  AssertionError: expected ResourceNotFoundException, got 'NotImplemented'

Model evidence: UpdatePartnership errors include ResourceNotFoundException (client, httpError 404); input member `capabilities` targets PartnershipCapabilities. `NotImplemented` is not a modeled error for this operation.

## Steps to Reproduce
1. Boot the b2bi emulator.
2. Create an S3 bucket, then CreateTransformer and UpdateTransformer(status="active").
3. CreateCapability twice against that transformer (capability A and capability B).
4. CreateProfile, then CreatePartnership(profileId=<profile>, name=<name>, email=<email>, capabilities=[A]).
5. Call UpdatePartnership(partnershipId=<partnership>, capabilities=[A, "cap-doesnotexist0000000000"]).
   Expected: HTTP 404 ResourceNotFoundException with message "Capability ID cap-doesnotexist0000000000 not found."
   Observed: HTTP 501 NotImplemented, "operation UpdatePartnership is scaffolded but not implemented in this emulator".
6. Call GetPartnership(partnershipId=<partnership>) and confirm capabilities is still [A] (the rejected update must not mutate state).
7. Call UpdatePartnership(partnershipId=<partnership>, capabilities=[B]); expect HTTP 200 with capabilities == [B], and the same from an independent GetPartnership.
