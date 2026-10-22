Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: verifiedpermissions.delete-policy-store-deletion-protection-blocks-delete-rejected
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions.delete-policy-store-deletion-protection-blocks-delete-rejected|run#7|AssertionError|assert err["Error"]["Message"] == (
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Verified Permissions DeletePolicyStore deletion-protection refusal interpolates the policy-store id into the InvalidStateException message instead of AWS's fixed, identifier-free text

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePolicyStore

## Description
Cause 1 of 1: when `DeletePolicyStore` is refused because the store was created with `deletionProtection=ENABLED`, the emulator builds the `InvalidStateException` message from the request by splicing in the policy-store id. AWS's message for this refusal is a fixed string that contains no request-specific value. The HTTP status and the error code are correct (the check's code assertion on `InvalidStateException` passed, and the store did survive the rejected delete); the divergence is confined to the message rule.

AWS: Smithy com.amazonaws.verifiedpermissions#InvalidStateException — client error, httpError 406, single required `message` member; shape doc: "The policy store can't be deleted because deletion protection is enabled. To delete this policy store, disable deletion protection." The model documents no interpolation of the policy-store id, and `DeletePolicyStoreInput` carries only `policyStoreId`. Recorded behavior: the refusal message is a fixed sentence pair, identifier-free — recorded example text "The policy store cannot be deleted because deletion protection is enabled. Deletion protection must be disabled first." The requirement asserted here is the RULE, not that exact wording: the message is constant for this refusal and must not embed the store id (or any other request/generated value). Status 406 and code `InvalidStateException` are the exact requirements.

Emulator: returns `InvalidStateException` with a message templated from the request, e.g. "policy store PSJPfe8fgQnwl8FAu3TKk6j5 can't be deleted because deletion protection is enabled. To delete this policy store, disable deletion protection." — the generated store id is spliced in and the sentence is re-ordered so it no longer matches the fixed form.

Why: a client (or test, or log-scrubbing/alerting rule) that matches AWS's constant deletion-protection message never matches, and the message now leaks a resource identifier where AWS emits none, so message text is no longer a stable, id-free constant across calls.

Fix: the DeletePolicyStore handler's deletion-protection refusal path in the verifiedpermissions crate (services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/delete_policy_store.rs) must emit the constant InvalidStateException message rather than formatting the `policyStoreId` into it.

Traces: verifiedpermissions.delete-policy-store-deletion-protection-blocks-delete-rejected.

## Evidence
Expected (recorded AWS behavior; Smithy InvalidStateException doc confirms a fixed deletion-protection sentence): HTTP 406, Error.Code = "InvalidStateException", Error.Message a constant, identifier-free string — recorded example: "The policy store cannot be deleted because deletion protection is enabled. Deletion protection must be disabled first."

Actual (emulator): HTTP/code correct, message = "policy store PSJPfe8fgQnwl8FAu3TKk6j5 can't be deleted because deletion protection is enabled. To delete this policy store, disable deletion protection." — the generated policy-store id is interpolated.

Suite output: [mismatch] verifiedpermissions.delete-policy-store-deletion-protection-blocks-delete-rejected — AssertionError: unexpected message="policy store PSJPfe8fgQnwl8FAu3TKk6j5 can't be deleted because deletion protection is enabled. To delete this policy store, disable deletion protection." (assertion at check line 40, after the Error.Code == "InvalidStateException" assertion passed).

## Steps to Reproduce
1. Boot the emulator and point a botocore verifiedpermissions client at it.
2. store_id = client.create_policy_store(validationSettings={"mode": "OFF"}, deletionProtection="ENABLED")["policyStoreId"]
3. client.delete_policy_store(policyStoreId=store_id) -> ClientError.
4. Inspect exc.response: Error.Code is InvalidStateException (correct), but Error.Message contains the store id from step 2 instead of AWS's constant deletion-protection message.
5. Re-replay verifiedpermissions.delete-policy-store-deletion-protection-blocks-delete-rejected.
