Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-function-policy-permissions
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-function-policy-permissions lambda-function-resource-policy-permissions lambda-permissions-and-resource-policy lambda-resource-policy-statement-and-document lam-err-revision-id-permissions lambda-resource-policy-conditional-update-and-stale-revision lambda-resource-policy-delete-then-recreate lambda-resource-policy-errors lambda-resource-policy-lifecycle lambda-resource-policy-on-alias lambda-resource-policy-precondition-conflict lambda-resource-policy-put-get-delete-lifecycle lambda-resource-policy-revision-mismatch lambda-resource-policy-nonexistent-function lam-err-policy-length-exceeded lam-err-public-policy-access-flag
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|AddPermission|4xx->4xx|$.Type
Issue-Key: lambda|GetPolicy|4xx->4xx|$.Type
Issue-Key: lambda|RemovePermission|4xx->4xx|$.Type
Issue-Key: lambda|GetResourcePolicy|4xx->4xx|$.Type
Issue-Key: lambda|PutResourcePolicy|4xx->4xx|$.Type
Issue-Key: lambda|AddPermission|4xx->4xx|$.message
Issue-Key: lambda|AddPermission|4xx->4xx|$.Message
Issue-Key: lambda|AddPermission|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|AddPermission|4xx->4xx|status
Issue-Key: lambda|GetPolicy|2xx->2xx|$.PublicAccessAllowed
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda permission and policy refusals use the wrong status, limit message and omit Type

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Shared fault: the permission/policy paths pick the wrong modeled exception for an oversized request, reword the policy-size refusal, drop `Type`, and GetPolicy omits a recorded member. Ten net-new clusters.

AWS: Smithy binds to AddPermission both PolicyLengthExceededException (httpError 400, members {Type, message}) and RequestTooLargeException (httpError 413, members {Type, message}); these are different conditions. A request whose serialized body exceeds the per-operation request quota is refused 413 RequestTooLargeException with Message `Request must be smaller than 5120 bytes for the AddPermission operation` — fixed text with the quota and the operation name as variables. A statement that would push the resource policy past its size quota is refused 400 PolicyLengthExceededException with message `The final policy size (<computedSize>) is bigger than the limit (<limit>).` — fixed text; the computed resulting size and the 20480-byte limit are the variables. Recorded behavior additionally shows GetPolicy returning `PublicAccessAllowed` (false here); the on-disk GetPolicyResponse models only Policy and RevisionId, so that member is recorded behavior only. Every client error carries `Type` = `User`.

Emulator: the oversized request is refused 400 PolicyLengthExceededException (wrong status and wrong error class); the policy-size message is the emulator's own `The resource policy of <arn> would be <n> bytes, past the maximum of 20480 bytes.`; `PublicAccessAllowed` is never emitted; `Type` is missing from every error body.

Why: a client retries or splits a request based on 413-vs-400 and on the error code — a 413 surfacing as 400 PolicyLengthExceededException makes an over-large request look like an over-large policy, so the caller shrinks the wrong thing; and consumers of `PublicAccessAllowed` cannot tell whether the policy grants public access.

Fix: AddPermission's validation must separate the request-size check (413 RequestTooLargeException) from the resulting-policy-size check (400 PolicyLengthExceededException) and use AWS's message templates; GetPolicy's serializer must render PublicAccessAllowed; the shared error serializer must emit `Type`.

Cause 1 of 10: AddPermission, 409 — fixture lambda-function-policy-permissions step task#7; `Type` missing.
Cause 2 of 10: GetPolicy, 404 — fixture lambda-function-policy-permissions step task#9; `Type` missing.
Cause 3 of 10: RemovePermission, 404 — fixture lambda-function-resource-policy-permissions step task#8; `Type` missing.
Cause 4 of 10: AddPermission, 412 — fixture lam-err-revision-id-permissions step task#7; `Type` missing.
Cause 5 of 10: RemovePermission, 412 — fixture lam-err-revision-id-permissions step task#10; `Type` missing.
Cause 6 of 10: GetResourcePolicy, 404 — fixture lambda-permissions-and-resource-policy step task#13; `Type` missing.
Cause 7 of 10: PutResourcePolicy, 404 — fixture lambda-resource-policy-nonexistent-function step task#6; `Type` missing.
Cause 8 of 10: AddPermission, 400 — fixture lam-err-policy-length-exceeded step task#6; policy-size message reworded, `Type` missing.
Cause 9 of 10: AddPermission — fixture lam-err-policy-length-exceeded step task#8; expected 413 RequestTooLargeException with the request-size Message, emulator answered 400 PolicyLengthExceededException.
Cause 10 of 10: GetPolicy, 200 — fixture lam-err-policy-length-exceeded step task#7; `PublicAccessAllowed` omitted.

## Evidence
Cluster details: seven clusters `$.Type: missing (expected 'User', got None)` with matching statuses; task#6 `$.message: value_mismatch (expected 'The final policy size (23993) is bigger than the limit (20480).', got 'The resource policy of <arn> would be 23968 bytes, past the maximum of 20480 bytes.')`; task#8 `status: expected 413, got 400` with `$.Message: missing (expected 'Request must be smaller than 5120 bytes for the AddPermission operation')` and `x-amzn-errortype ... got 'PolicyLengthExceededException'`; task#7 `$.PublicAccessAllowed: missing (expected False, got None)`. Smithy: AddPermission binds both PolicyLengthExceededException (400) and RequestTooLargeException (413).

## Steps to Reproduce
Replay the corpus. Fixture lam-err-policy-length-exceeded step task#8: AddPermission with a request body over the 5120-byte operation quota — expect 413 RequestTooLargeException, observe 400 PolicyLengthExceededException. Step task#7: GetPolicy and look for PublicAccessAllowed.
