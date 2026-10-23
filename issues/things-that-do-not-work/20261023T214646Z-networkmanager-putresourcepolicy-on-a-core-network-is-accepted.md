Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmanager.resource-policy-put-get-delete-lifecycle
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager.resource-policy-put-get-delete-lifecycle|run#11|AssertionError|raise AssertionError(
Issue-Commit: 2e01089bdacbc50b0d00d102e60dd087026d905e

# networkmanager PutResourcePolicy on a core network is accepted instead of refused with ValidationException (400)

> _Note: this issue was identified against the code base at commit `2e01089bdacbc50b0d00d102e60dd087026d905e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
One root cause.

Cause 1 of 1: PutResourcePolicy against a core network ARN is accepted by the emulator where the recorded AWS behavior is a deterministic refusal.

AWS: recorded behavior only. The recording shows PutResourcePolicy against a core-network ARN refused for every well-formed policy document (same-account or cross-account principal, wildcard or specific action, with or without a LIVE core-network policy already executed) with HTTP 400 and error code `ValidationException` (reason FieldValidationFailed; recorded message example: "Invalid policy."). The Smithy model supports this classification: PutResourcePolicy (POST /resource-policy/{ResourceArn}, success code 200) models `ValidationException` -> httpError 400 among its errors, and `PolicyDocument` is a required SynthesizedJsonResourcePolicyDocument. The model itself says nothing about which resource types may carry a policy, so the refusal is grounded in the recording. Hypothesis: core networks are not a policy-attachable resource for this API, so the service rejects the document outright. Note the originating check hedges the refusal as observed "in this account" — the status (400) and error code (ValidationException) are the requirement; the recorded message "Invalid policy." is only an example.

Emulator: the call SUCCEEDS and returns a 200 PutResourcePolicyResponse, so no refusal is produced at all. The replayed check reached its else-branch and raised "PutResourcePolicy on a core network unexpectedly SUCCEEDED". Because the request short-circuits there, the rest of the recorded contract exercised by the same check (GetResourcePolicy reporting no policy after the refused Put, and DeleteResourcePolicy on an absent policy behaving as a no-op) is unverified behind this acceptance.

Why: a client relying on AWS's behavior gets a false positive: code written against the emulator believes a resource policy was attached to a core network and will later read it back or rely on cross-account access that real AWS never granted. Negative-path tests of policy validation pass against the emulator and fail in production, and the emulator is over-permissive on a rejection axis — the strongest class of fidelity gap, since it hides an error the real service always raises.

Fix: the PutResourcePolicy handler and its input validation for this service — the request must be classified and refused (400 / ValidationException) for a resource ARN that cannot carry a resource policy, rather than being written to state and answered 200. The handler file lives under the networkmanager service crate's ops module (services/service-com-amazonaws-networkmanager-networkmanager/src/ops/); the sibling GetResourcePolicy/DeleteResourcePolicy paths must keep reporting no attached policy once the Put is refused.

Traces: networkmanager.resource-policy-put-get-delete-lifecycle (mismatch).

## Evidence
Expected (recorded AWS): PutResourcePolicy(ResourceArn=<core-network ARN>, PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"networkmanager:GetCoreNetwork"}]}) -> HTTP 400, Error.Code = "ValidationException" (recorded message example: "Invalid policy."), and GetResourcePolicy afterwards still reports no PolicyDocument.

Actual (emulator): the same call returns HTTP 200 with a PutResourcePolicyResponse and raises no error. Replay assertion from the suite: `AssertionError: PutResourcePolicy on a core network unexpectedly SUCCEEDED in this account -- the deterministic refusal this check pins is no longer reproduced; re-audit before trusting this result`.

Smithy (services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json): PutResourcePolicy http {POST /resource-policy/{ResourceArn}, code 200}; errors include ValidationException (httpError 400), ConflictException (409), AccessDeniedException (403); PolicyDocument required, mediaType application/json.

## Steps to Reproduce
1. CreateGlobalNetwork and poll DescribeGlobalNetworks until State == AVAILABLE.
2. CreateCoreNetwork(GlobalNetworkId=<gn>) and poll GetCoreNetwork until State == AVAILABLE; keep CoreNetworkArn.
3. GetResourcePolicy(ResourceArn=<CoreNetworkArn>) -> confirm no PolicyDocument (this passes today).
4. PutResourcePolicy(ResourceArn=<CoreNetworkArn>, PolicyDocument='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"networkmanager:GetCoreNetwork"}]}').
5. Observe HTTP 200 success; recorded AWS answers HTTP 400 ValidationException.
6. Re-replay check networkmanager.resource-policy-put-get-delete-lifecycle to confirm the fix (it also asserts GetResourcePolicy still reports no policy after the refusal and that DeleteResourcePolicy on an absent policy is a no-op).
