Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.update-key-group-already-exists-rejected cloudfront.update-key-group-duplicate-name-fails cloudfront.update-key-group-duplicate-name-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.update-key-group-already-exists-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cloudfront.update-key-group-duplicate-name-fails|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cloudfront.update-key-group-duplicate-name-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: c6cd310496053acfb8677287c8f8c63a1ffba3db

# cloudfront: UpdateKeyGroup's KeyGroupAlreadyExists refusal carries the error shape's model documentation instead of AWS's runtime message

> _Note: this issue was identified against the code base at commit `c6cd310496053acfb8677287c8f8c63a1ffba3db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateKeyGroup

## Description
REOPENS 20261029T175823Z-cloudfront-updatekeygroup-stub.md: its fix left the call refused. UpdateKeyGroup now reaches its duplicate-name path and refuses the rename, but the refusal it emits does not carry AWS's message, so three checks that exercise that refusal still fail.

This issue contains ONE root cause. It must be closed before submitting.

Cause 1 of 1: renaming a key group (UpdateKeyGroup) to a Name already used by another key group is refused with the right code and status but the wrong message text — the message is the Smithy error shape's documentation string, not the service's runtime message.
AWS: Smithy: UpdateKeyGroup -> PUT /2020-05-31/key-group/{Id}, success code 200, output KeyGroup (httpPayload) + ETag header, IfMatch bound to the If-Match header; modeled errors InvalidArgument(400), InvalidIfMatchVersion(400), KeyGroupAlreadyExists(409), NoSuchResource(404), PreconditionFailed(412), TooManyPublicKeysInKeyGroup(400). A KeyGroupConfig.Name that already belongs to another key group in the account is refused with `KeyGroupAlreadyExists` (HTTP 409) and the message "Another key group with the same name already exists within the aws account." — that whole string is fixed text for this error class; it has no variable part (it names neither the group nor the name). The same text is what CreateKeyGroup's duplicate-name refusal carries (recorded behavior for both entry points into the key-group name-uniqueness rule).
Emulator: the status (409) and the code (KeyGroupAlreadyExists) are correct, but the message is "A key group with this name already exists. You must provide a unique name. To modify an existing key group, use UpdateKeyGroup." That is verbatim the documentation trait of the model's KeyGroupAlreadyExists shape (Smithy doc: "<p>A key group with this name already exists. You must provide a unique name. To modify an existing key group, use <code>UpdateKeyGroup</code>.</p>", with the markup stripped), i.e. the refusal message is rendered from the SDK documentation rather than from AWS's response.
Why: the error message is part of the refusal contract clients, CLI output and recorded goldens match on. A client or test that keys on AWS's duplicate-name text does not recognise this refusal, and a user reading it is told to "use UpdateKeyGroup" when UpdateKeyGroup is exactly the call that was just refused — nonsense advice that hides the real condition (the target name is taken by another group).
Fix: UpdateKeyGroup's duplicate-name refusal path — the error construction in the UpdateKeyGroup handler under services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/ — and the shared helper that populates a modeled error's message, which must use the service's runtime message for the error class, not the shape's `documentation` trait. The identical substitution on CreateKeyGroup's duplicate-name path and GetPublicKey's not-found path is already described as Cause 8 of the open issue 20261027T001538Z-cloudfront-nine-control-plane-divergences-deletepublickey-updatekeygroup-deletefieldlevelencryptionprofile.md; that issue does NOT carry the UpdateKeyGroup entry point or any of the three check ids below, so this cause is additional to it. Audit the remaining cloudfront modeled errors for the same doc-string-as-message substitution.
Traces: cloudfront.update-key-group-already-exists-rejected, cloudfront.update-key-group-duplicate-name-fails, cloudfront.update-key-group-duplicate-name-rejected.

## Evidence
All three failing checks create two key groups over one public key (names A and B), then rename group B to name A with the group's current ETag as IfMatch.

cloudfront.update-key-group-already-exists-rejected [mismatch], check line 41:
  AssertionError: expected message 'Another key group with the same name already exists within the aws account.', got 'A key group with this name already exists. You must provide a unique name. To modify an existing key group, use UpdateKeyGroup.'

cloudfront.update-key-group-duplicate-name-fails [mismatch], check line 51: same assertion, same two strings.

cloudfront.update-key-group-duplicate-name-rejected [mismatch], check line 40: same assertion, same two strings.

Expected: HTTP 409, code KeyGroupAlreadyExists, message "Another key group with the same name already exists within the aws account." (entirely fixed text — no request- or state-derived part).
Actual: HTTP 409, code KeyGroupAlreadyExists (both correct), message "A key group with this name already exists. You must provide a unique name. To modify an existing key group, use UpdateKeyGroup." — the model's KeyGroupAlreadyExists documentation string.
The code assertion (`code == "KeyGroupAlreadyExists"`) passes in all three checks, so only the message diverges; the group names in the recordings (sim2real-cf-kg-a-*/-b-*) are per-run values and are not part of the expected message.

## Steps to Reproduce
1. CreatePublicKey with any valid RSA public key PEM.
2. CreateKeyGroup with Name=<A> and Items=[<public key id>].
3. CreateKeyGroup with Name=<B> and Items=[<public key id>]; keep its ETag.
4. UpdateKeyGroup(Id=<group B id>, IfMatch=<group B ETag>, KeyGroupConfig={Name: <A>, Items: [<public key id>]}).
   Expect: HTTP 409, code KeyGroupAlreadyExists, message "Another key group with the same name already exists within the aws account."
   Observe: HTTP 409, code KeyGroupAlreadyExists, message "A key group with this name already exists. You must provide a unique name. To modify an existing key group, use UpdateKeyGroup."
5. Confirm the rest of the contract still holds (it does today): GetKeyGroup(Id=<group B id>) still reports Name=<B> with an unchanged ETag, and UpdateKeyGroup to a genuinely free name <C> succeeds.
6. Re-replay the three check ids in trace_ids.
