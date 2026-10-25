Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__lambda.authorizer-signed-token-either-key-passthrough iot__lambda.authorizer-test-invoke-signature-mismatch-rejected
Issue-Key: iot__lambda|TestInvokeAuthorizer|NotImplemented
Issue-Key: iot__lambda.authorizer-test-invoke-signature-mismatch-rejected|run#22|AssertionError|assert err == expected, (
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# iot__lambda: IoT TestInvokeAuthorizer is an unimplemented scaffold stub, so custom-authorizer verification cannot be exercised (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeAuthorizer

## Description
Cause 1 of 1: iot:TestInvokeAuthorizer answers NotImplemented, so neither the signed-token pass-through nor the unverifiable-signature refusal can be observed.

AWS: Smithy (iot) TestInvokeAuthorizer answers 200 with isAuthenticated, principalId, policyDocuments, refreshAfterInSeconds and disconnectAfterInSeconds produced by the authorizer's Lambda; its modeled refusals are client errors including InvalidRequestException, ResourceNotFoundException, UnauthorizedException, ThrottlingException and InternalFailureException. For a token whose signature cannot be verified against the authorizer's token signing public key, AWS refuses with InvalidRequestException (the recorded message, 'Token signature mismatch for authorizer <name>', is an example).

Emulator: both calls are refused with code 'NotImplemented' and message 'operation TestInvokeAuthorizer is scaffolded but not implemented in this emulator' (recorded message, an example).

Why: NotImplemented is not in the operation's modeled error set, so a client cannot validate a custom authorizer before putting it in the connection path, and cannot distinguish a bad token (its own fault, fixable) from an unavailable API. The signature-verification contract — the whole point of a token-signing authorizer — is unverifiable.

Fix: implement the TestInvokeAuthorizer handler in the iot service: resolve the authorizer, verify the token signature against its configured public keys (refusing InvalidRequestException when it does not match) and invoke the authorizer Lambda for the policy documents.

Traces: iot__lambda.authorizer-signed-token-either-key-passthrough, iot__lambda.authorizer-test-invoke-signature-mismatch-rejected.

## Evidence
Expected (negative): ('InvalidRequestException', 'Token signature mismatch for authorizer <name>') — HTTP 400. Expected (positive): 200 with isAuthenticated/policyDocuments from the authorizer Lambda, for a token signed with either registered key.
Actual: ('NotImplemented', 'operation TestInvokeAuthorizer is scaffolded but not implemented in this emulator') for both.

## Steps to Reproduce
1. CreateAuthorizer with an authorizer Lambda and a tokenSigningPublicKeys map containing two keys.
2. Call TestInvokeAuthorizer with a token signed by either key (positive) and with a token whose signature does not verify (negative).
3. Observe NotImplemented in both cases.
