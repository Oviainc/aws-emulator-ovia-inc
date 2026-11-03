Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ram.create-resource-share-client-token-mismatch-rejected
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram.create-resource-share-client-token-mismatch-rejected|run#9|AssertionError|assert message == (
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# CreateResourceShare's IdempotentParameterMismatchException carries emulator-authored wording instead of AWS's client-token mismatch message

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResourceShare

## Description
One net-new root cause in service `ram`.

Cause 1 of 1: replaying a CreateResourceShare clientToken with different request parameters is refused with the right error class and status but the wrong message text.

AWS: Smithy (services/service-com-amazonaws-ram-amazonresourcesharing/smithy-model/com-amazonaws-ram-amazonresourcesharing.json) models CreateResourceShare -> error IdempotentParameterMismatchException, errorKind client, httpError 400, awsQueryError code IdempotentParameterMismatch; its single member is `message` (String, required), documented as "The operation failed because the client token input parameter matched one that was used with a previous call to the operation, but at least one of the other input parameters is different from the previous call." CreateResourceShareRequest$clientToken's own model doc states the rule: "Passing the same value to a later call to an operation requires that you also pass the same value for all other parameters... If you retry the operation with the same ClientToken, but with different parameters, the retry fails with an IdempotentParameterMismatch error." The recorded AWS response for this refusal carries HTTP 400, Error.Code `IdempotentParameterMismatchException`, and Error.Message exactly "The request has the same client token as a previous request, but the requests are not the same." That message is ENTIRELY fixed text for this error class — it interpolates nothing from the request (no token, no share name, no ARN, no member name), so there are no variable parts to fill in from the request or from emulator state.

Emulator: answers the correct class and status (400 / IdempotentParameterMismatchException) but with its own sentence, "clientToken was already used for a CreateResourceShare request with different parameters". The string is hard-coded at services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/create_resource_share.rs lines 346-348, in the clientToken-fingerprint mismatch branch (the `record.fingerprint != fingerprint` arm). The replay assertion that fails is `assert message == ("The request has the same client token as a previous request, but the requests are not the same.")`, reported as: AssertionError: unexpected message 'clientToken was already used for a CreateResourceShare request with different parameters'. The rest of the check passes, so the code, the 400 status, the identical-replay idempotency path and the single-share invariant are all correct; only the message text diverges.

Why: the error message is part of the operation's contract. A client that matches on RAM's message to distinguish a token-reuse conflict from the other 400s CreateResourceShare models (InvalidClientTokenException, InvalidParameterException, ResourceShareLimitExceededException) never matches, so retry/idempotency logic and log-based alerting keyed on AWS's wording silently fall through to a generic-failure path. Users comparing emulator output against a recording or a real account see a diff on every token-mismatch refusal, and any snapshot test of the error body fails.

Fix: the CreateResourceShare handler's idempotency branch (services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/create_resource_share.rs) — replace the hand-written sentence with AWS's fixed text for IdempotentParameterMismatchException. Because the same client-token mismatch rule is applied by every tokened RAM operation via the shared client-token record in crate::state, the message should come from one shared constant/helper in state.rs rather than being re-spelled per handler, so the sibling operations answer identically.

Traces: ram.create-resource-share-client-token-mismatch-rejected (verdict mismatch). Code citations: services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/create_resource_share.rs:340-353 (the token-record fingerprint comparison and the refusal it builds), :346-348 (the diverging message literal).

No open issue covers this: the only open RAM issue, 20261029T191622Z-ram-3-net-new-response-fidelity-causes-in.md, is about 200-response body fidelity (DisassociateResourceShare$external and the Tag `__type` discriminator in TagResource/UntagResource) and does not touch CreateResourceShare or its refusal wording. No fixed issue claimed this operation, so this is not a reopen. This check was not marked ALREADY FILED or OWNED by the replay report.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the booted emulator (bootstrap_signal false; ram is bootstrapped as service-com-amazonaws-ram-amazonresourcesharing). Total 2/12 passed; this is the single failing ram check that was neither ALREADY FILED nor OWNED.

[mismatch] ram.create-resource-share-client-token-mismatch-rejected
  File "check_ram_create_resource_share_client_token_mismatch_rejected.py", line 59, in run
    assert message == (
  AssertionError: unexpected message 'clientToken was already used for a CreateResourceShare request with different parameters'

Expected (AWS, fixed text for this error class, no interpolated values):
  HTTP 400
  Error.Code    = "IdempotentParameterMismatchException"
  Error.Message = "The request has the same client token as a previous request, but the requests are not the same."

Actual (emulator):
  HTTP 400
  Error.Code    = "IdempotentParameterMismatchException"   (correct — the check's code assertion on line 53 passed)
  Error.Message = "clientToken was already used for a CreateResourceShare request with different parameters"

Source of the divergence:
  services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/create_resource_share.rs:340-353
    if let Some(record) = store.client_token_record(Self::NAME, &token) {
        if record.fingerprint != fingerprint {
            return Err(...IdempotentParameterMismatchException {
                message: "clientToken was already used for a CreateResourceShare \
                          request with different parameters".to_string(), } ...)

Model facts used (smithy_query on services/service-com-amazonaws-ram-amazonresourcesharing/smithy-model/com-amazonaws-ram-amazonresourcesharing.json):
- CreateResourceShare: POST /createresourceshare, success 200; errors include IdempotentParameterMismatchException (client, httpError 400, awsQueryError IdempotentParameterMismatch).
- IdempotentParameterMismatchException: structure, errorKind client, httpError 400, members { message: String required }, doc "The operation failed because the client token input parameter matched one that was used with a previous call to the operation, but at least one of the other input parameters is different from the previous call."
- CreateResourceShareRequest$clientToken doc: retrying with the same ClientToken but different parameters "fails with an IdempotentParameterMismatch error".

Unaffected by this defect (the rest of the check passes): the identical-token replay returns the original resourceShareArn, and GetResourceShares reports exactly one share of that name — so only the refusal's message text is wrong.

## Steps to Reproduce
1. git_clone the emulator repo and boot the emulator from that checkout (RAM is bootstrapped: service-com-amazonaws-ram-amazonresourcesharing).
2. Point a RAM client at the emulator and call CreateResourceShare with a unique name and a fresh clientToken (a UUID); note the returned resourceShareArn.
3. Call CreateResourceShare again with the SAME clientToken but a different `name` (e.g. the first name + "-different") and no tags.
4. Observe the refusal: HTTP 400 with Error.Code "IdempotentParameterMismatchException" (correct) but Error.Message "clientToken was already used for a CreateResourceShare request with different parameters". AWS answers Error.Message "The request has the same client token as a previous request, but the requests are not the same." — fixed text with no request-derived values.
5. Positive control (already passing, keep it passing): repeat the FIRST request verbatim with the same clientToken and the same tags — it must return the original resourceShareArn, and GetResourceShares(name=<name>, resourceOwner="SELF") must report exactly one share.
6. Re-replay behavioral check ram.create-resource-share-client-token-mismatch-rejected after the fix; it must pass before this issue is closed.
