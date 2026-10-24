Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fms.get-policy-nonexistent-not-found fms.delete-policy-malformed-id-rejected
Issue-Shape-Id: com.amazonaws.fms#AWSFMS_20180101
Issue-Key: fms.get-policy-nonexistent-not-found|run#3|AssertionError|assert non_uuid_code == "ResourceNotFoundException", (
Issue-Key: fms.delete-policy-malformed-id-rejected|run#3|AssertionError|assert non_uuid_code == "ResourceNotFoundException", (
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# GetPolicy and DeletePolicy refuse with AccessDeniedException: the Firewall Manager administrator gate is applied to operations AWS answers ungated

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
REOPENS 20261024T000557Z-fms-putpolicy-error-response.md: its fix left the call refused.

That issue asked for the Firewall Manager administrator gate on the policy operations; the fix added `crate::state::require_administrator` to the GetPolicy and DeletePolicy handlers, and both operations now refuse every request made without an administrator association instead of answering the policy-lookup outcome AWS answers. This issue is ONE root cause (the gate's operation coverage) observed at two operations.

Cause 1 of 1 - GetPolicy and DeletePolicy are refused by the administrator gate before the policy id is resolved (failing checks: fms.get-policy-nonexistent-not-found, fms.delete-policy-malformed-id-rejected).

AWS: both operations take `PolicyId` (Smithy: com.amazonaws.fms#PolicyId, @length min 36 max 36, pattern ^[a-z0-9A-Z-]{36}$) and treat it as an opaque lookup key - a value that satisfies the length/pattern constraint but is not a real UUID is not further validated server-side. An id that no stored policy carries answers HTTP 400 with error code `ResourceNotFoundException` (Smithy: com.amazonaws.fms#ResourceNotFoundException, httpError 400, doc "The specified resource was not found."; recorded message example: "The referenced item does not exist."), and that is the same answer for a well-formed-but-missing UUID and for a well-formed-length non-UUID. Neither operation models `AccessDeniedException` at all: GetPolicy's modeled errors are InternalErrorException, InvalidOperationException, InvalidTypeException, ResourceNotFoundException; DeletePolicy's are InternalErrorException, InvalidInputException, InvalidOperationException, LimitExceededException, ResourceNotFoundException; the service's only common error is InternalErrorException (service_info common_errors). Recorded behavior: the recordings reach these operations in an account with no Firewall Manager administrator configuration of its own and still receive the not-found refusal, so neither operation is gated on the caller's Firewall Manager administrator status.

Emulator: `src/ops/get_policy.rs` handle calls `crate::state::require_administrator(&store, &request_region(...))?` BEFORE `store.policies.get(...)` (see the handler's own comment "The authorization gate, BEFORE the identifier is looked up"), and `src/ops/delete_policy.rs` does the same. With no administrator associated in the account/Region, both answer error code `AccessDeniedException` - an error code outside the modeled error set of either operation - for every PolicyId, existing or not.

Why: a client cannot use policy reads or deletes at all unless it first associates an administrator, and when it does exercise the not-found path it gets a code the SDK's modeled error union for these operations does not contain, so `except ResourceNotFoundException` handling never fires and the client cannot distinguish "no such policy" (expected, retryable with another id) from an authorization failure (not retryable). Negative-path test suites that assert the documented ResourceNotFoundException break.

Fix: the administrator gate's operation coverage - remove the `require_administrator` call from these two handlers so the answer is decided by the policy store (not-found for an unknown id), and keep the gate only on the operations AWS actually gates. The gate helper itself and the operations that legitimately use it are unchanged.

Traces: fms.get-policy-nonexistent-not-found (GetPolicy with PolicyId="x"*36 and with a random UUID: expected ResourceNotFoundException, got AccessDeniedException); fms.delete-policy-malformed-id-rejected (DeletePolicy with the same two ids, plus a GetPolicy cross-read on the same missing id: expected ResourceNotFoundException, got AccessDeniedException). Code citations: src/ops/get_policy.rs lines 162-173; src/ops/delete_policy.rs handle (same gate-then-lookup shape); gate helper `require_administrator` in src/state.rs.

## Evidence
Check fms.get-policy-nonexistent-not-found: AssertionError: expected ResourceNotFoundException for a well-formed-length non-UUID id, got AccessDeniedException (fms.get_policy(PolicyId="x"*36)).
Check fms.delete-policy-malformed-id-rejected: AssertionError: expected ResourceNotFoundException for a well-formed-length non-UUID id, got AccessDeniedException (fms.delete_policy(PolicyId="x"*36)).
Expected per AWS/model: HTTP 400, error code ResourceNotFoundException (message example "The referenced item does not exist.") for any PolicyId no policy carries, with no dependence on the caller's Firewall Manager administrator status.
Actual: HTTP 400, error code AccessDeniedException, which is not a modeled error of GetPolicy or DeletePolicy (model errors listed above; service common_errors = [InternalErrorException]).

## Steps to Reproduce
1. Boot the emulator and do NOT call AssociateAdminAccount / PutAdminAccount.
2. Call fms GetPolicy {"PolicyId":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"} (36 chars). Observed: 400 AccessDeniedException. Expected: 400 ResourceNotFoundException.
3. Call fms GetPolicy with a freshly generated UUID. Observed: 400 AccessDeniedException. Expected: 400 ResourceNotFoundException.
4. Call fms DeletePolicy with each of those same two ids. Observed: 400 AccessDeniedException. Expected: 400 ResourceNotFoundException.
