Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.deploy-kms-key-with-policy-cfn-enforced kms.encryption-context-aad-binding-integrity kms.kms-cross-region-decrypt-of-a-single-region-ciphertext-rejected kms.mrk-replica-key-rotation-independent-server-managed
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.deploy-kms-key-with-policy-cfn-enforced|_error#10|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded with {result!r}")
Issue-Key: kms.encryption-context-aad-binding-integrity|run#16|AssertionError|assert empty_code == ERR_VALIDATION, (
Issue-Key: kms.kms-cross-region-decrypt-of-a-single-region-ciphertext-rejected|run#25|AssertionError|assert code_foreign_arn == ERR_CROSS_REGION, (
Issue-Key: kms.mrk-replica-key-rotation-independent-server-managed|run#16|AssertionError|assert replica_denied_msg == (
Issue-Commit: d558c4fc1331790598d7d8dd6ea4bd673681392c

# KMS: 4 net-new root causes — a key-policy explicit Deny is never applied to Decrypt, an empty-string EncryptionContext value is misclassified, a foreign-Region key ARN answers NotFoundException, and the MRK-replica EnableKeyRotation refusal carries no message (4 causes)

> _Note: this issue was identified against the code base at commit `d558c4fc1331790598d7d8dd6ea4bd673681392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Decrypt

## Description
Four distinct net-new root causes in the KMS emulator, each grounded in a failing behavioral check of this suite that the report left unclaimed. None of these four replay keys is carried by an open issue; where an open issue is adjacent it is named in the cause.

Cause 1 of 4 — an explicit Deny statement in a key policy is not applied to Decrypt, so the data plane serves plaintext the policy forbids.
AWS: a KMS key policy is evaluated on every data-plane call. An explicit Deny for kms:Decrypt on the calling principal beats the Allow of kms:* in the same document, and Decrypt is refused with AccessDeniedException (HTTP 400). The message's fixed text is "User: <caller ARN> is not authorized to perform: kms:Decrypt on resource: <key ARN> with an explicit deny in a resource-based policy"; its variable parts are the caller ARN from the request's signing identity and the key ARN from state. Removing only that Deny statement with PutKeyPolicy must make the IDENTICAL ciphertext decrypt successfully again. Smithy: Decrypt carries the service-common AccessDeniedException, and the policy is server state reachable through GetKeyPolicy/PutKeyPolicy, so this is a server-side authorization decision, not a client-side one. This check reaches the key through a CloudFormation AWS::KMS::Key whose KeyPolicy property carries both statements, and GetKeyPolicy already returns the deployed document verbatim — so the policy IS stored; it is simply never consulted.
Emulator: Decrypt returns HTTP 200 with the plaintext. Replay evidence: `AssertionError: expected a ClientError but the call succeeded with {'KeyId': 'arn:aws:kms:...:key/...', 'Plaintext': b'cfn-policy-probe', 'EncryptionAlgorithm': 'SYMMETRIC_DEFAULT', ... 'HTTPStatusCode': 200}`. Because the refusal never happens, the explicit-deny message assertion and the PutKeyPolicy restore leg are left ungraded and must also hold once the gate exists.
Why: every least-privilege and break-glass design that relies on an explicit Deny in a key policy appears to work against the emulator and does not hold in production — the emulator hides exactly the refusal it exists to surface, and a developer validating a deny-by-default key policy gets a false pass. Related open issue 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md describes the absence of identity/grant authorization on Encrypt/Decrypt/GenerateDataKey; it does not carry this check's replay key and does not cover the resource-policy explicit-Deny precedence path or the CloudFormation-deployed KeyPolicy that feeds it.
Fix: an authorization gate ahead of the Decrypt handler in services/service-com-amazonaws-kms-trentservice that evaluates the stored key policy with explicit-Deny precedence, plus the AccessDeniedException serializer for its message template.
Traces: kms.deploy-kms-key-with-policy-cfn-enforced.

Cause 2 of 4 — an EncryptionContext entry with an empty-string VALUE is treated as an AAD mismatch instead of a malformed input, on both Decrypt and Encrypt.
AWS: an empty-string encryption-context value is not a legal encryption context, so the request is refused at input validation and never reaches the AAD authentication step. The refusal is ValidationException with the message "Invalid EncryptionContext" — fixed text, no variable part. The proof it is validation and not authentication is that ENCRYPT, where there is no ciphertext to authenticate against, is refused identically. Smithy: Decrypt.EncryptionContext and Encrypt.EncryptionContext target EncryptionContextType (a map); the model does not constrain the value's length, so the empty-value rule is recorded behavior only (measured us-west-2, 2026-09) — but the operations' modeled faults make the distinction load-bearing: InvalidCiphertextException (httpError 400) is the AAD/authentication fault and is modeled on Decrypt only, while Encrypt has no InvalidCiphertextException at all.
Emulator: Decrypt with {"tenant": "acme", "purpose": ""} is refused with 'InvalidCiphertextException'. Replay evidence: `AssertionError: Decrypt with an empty-string context value must raise ValidationException (the context is malformed, not merely mismatched), got 'InvalidCiphertextException'`. The check stops there, so the Encrypt leg of the same rule is ungraded; it must produce ValidationException / "Invalid EncryptionContext" too, and cannot produce InvalidCiphertextException because that fault is not modeled on Encrypt.
Why: a client cannot distinguish "my encryption context is malformed — fix the caller" from "this ciphertext is not mine or has been tampered with — raise a security event". The two have opposite operational responses, and the emulator collapses the first into the second. On Encrypt the emulator would have to invent an unmodeled fault to answer this way at all.
Fix: EncryptionContext input validation shared by the Encrypt and Decrypt handlers, ahead of AAD authentication, with the ValidationException serializer emitting the constant message "Invalid EncryptionContext". Note: open issue 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md lists this check id under its message-text cause (it asserts InvalidCiphertextException carries an EMPTY Message); that is a different assertion in the same check file and does not cover the error-class misclassification described here.
Traces: kms.encryption-context-aad-binding-integrity.

Cause 3 of 4 — a key ARN from another Region presented to this Region's KMS endpoint is refused NotFoundException instead of AccessDeniedException.
AWS: single-Region key material does not travel. A ciphertext minted by an ordinary CMK in Region A, presented to the Region-B KMS endpoint with the Region-A key ARN as KeyId, is refused with AccessDeniedException (HTTP 400) — KMS decodes the key the ciphertext names, finds it is not a key this endpoint can authorize, and refuses on AUTHORIZATION, not on lookup. The same AccessDeniedException is returned when the caller names a real Region-B key for Region-A bytes, and when no KeyId is given. Measured us-west-2 -> us-east-1, 2026-09-10 (recorded behavior; the check's own plan had predicted NotFoundException and IncorrectKeyException and real AWS corrected it). Smithy: Decrypt models both NotFoundException (httpError 404, awsQueryError NotFound) and the service-common AccessDeniedException, so the two are distinguishable responses and the choice between them is the contract. The boundary is about MATERIAL and not about routing: a multi-Region replica at the Region-B endpoint must still open its primary's Region-A ciphertext.
Emulator: NotFoundException. Replay evidence: `AssertionError: a region-A key ARN presented to the us-east-1 endpoint must be refused with AccessDeniedException, got 'NotFoundException'`. The check stops at the first of three refusal legs, so the Region-B-key leg, the no-KeyId leg and the multi-Region-replica positive control are ungraded and must also hold.
Why: NotFoundException reads as "this key does not exist — provision it", which is false and sends an operator to create a duplicate key; AccessDeniedException reads as "this endpoint cannot authorize that key — you are talking to the wrong Region", which is the actual fault. A client that distinguishes a missing key from a cross-Region misroute takes the wrong branch, and the 404-vs-400 status difference also changes SDK retry classification.
Fix: cross-Region key-identifier resolution in the Decrypt handler's key-resolution path — a well-formed ARN whose Region is not this endpoint's Region must be classified as an authorization refusal (AccessDeniedException, HTTP 400), not a lookup miss, while shared multi-Region material continues to resolve.
Traces: kms.kms-cross-region-decrypt-of-a-single-region-ciphertext-rejected.

Cause 4 of 4 — EnableKeyRotation on a multi-Region REPLICA key returns the right code with no message text.
AWS: automatic rotation is a shared property of a multi-Region key set that KMS manages server-side. EnableKeyRotation is accepted only on the PRIMARY; on a replica it is refused with UnsupportedOperationException (Smithy: modeled on EnableKeyRotation, httpError 400, awsQueryError code UnsupportedOperation) and the message's fixed text is "<arn> is a multi-region replica key which is not valid for EnableKeyRotation." — the only variable part is the replica key's ARN, taken from state.
Emulator: the error CODE is correct (UnsupportedOperationException) but the Message is empty/absent. Replay evidence: the assertion `replica_denied_msg == f"{replica_arn} is a multi-region replica key which is not valid for EnableKeyRotation."` fails with a bare `AssertionError` whose detail (the actual message) renders as nothing, i.e. the emulator sent no message text. The check stops there, so the downstream legs are ungraded and must also hold: GetKeyRotationStatus must report KeyRotationEnabled False on both primary and replica initially; EnableKeyRotation(RotationPeriodInDays=180) on the primary must take effect (Smithy: RotationPeriodInDays range 90..2560); and KMS must synchronize the setting to the replica's OWN GetKeyRotationStatus record (KeyRotationEnabled true, RotationPeriodInDays 180) in the other Region without the caller ever calling EnableKeyRotation there.
Why: the message is the only thing that tells an operator WHY the rotation call was refused and which key to call instead — UnsupportedOperationException alone is unactionable, and a client surfacing the AWS text to a user shows an empty error. The absent message is also the structural property this check measures.
Fix: the EnableKeyRotation handler's multi-Region replica guard and its UnsupportedOperationException serializer in services/service-com-amazonaws-kms-trentservice — populate the message from the template above with the replica ARN from state. (Related fixed issue 20261024T102105Z-kms-enablekeyrotation-stub.md implemented the operation and the refusal code is now correct; this is the message contract on top of it, not a re-refusal.)
Traces: kms.mrk-replica-key-rotation-independent-server-managed.

## Evidence
Cause 1 — kms.deploy-kms-key-with-policy-cfn-enforced [mismatch]:
  File "check_kms_deploy_kms_key_with_policy_cfn_enforced.py", line 103, in run
    err = _error(lambda: kms.decrypt(KeyId=key_id, CiphertextBlob=ciphertext))
  AssertionError: expected a ClientError but the call succeeded with {'KeyId': 'arn:aws:kms:us-west-2:000000000000:key/4008139e-...', 'Plaintext': b'cfn-policy-probe', 'EncryptionAlgorithm': 'SYMMETRIC_DEFAULT', 'KeyMaterialId': 'ef80f6cc...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Key policy in effect (deployed via CFN AWS::KMS::Key, and read back verbatim by GetKeyPolicy, so the assertion on the stored document passed):
    Sid AllowPrincipalAdmin  Effect Allow  Principal {AWS: <caller role ARN>}  Action kms:*     Resource *
    Sid DenyDecrypt          Effect Deny   Principal {AWS: <caller role ARN>}  Action kms:Decrypt Resource *
  Expected: AccessDeniedException, HTTP 400, Message "User: <caller ARN> is not authorized to perform: kms:Decrypt on resource: <key ARN> with an explicit deny in a resource-based policy".
  Actual: HTTP 200 + Plaintext. Positive control Encrypt under the same policy correctly succeeded.

Cause 2 — kms.encryption-context-aad-binding-integrity [mismatch]:
  File "check_kms_encryption_context_aad_binding_integrity.py", line 122, in run
    assert empty_code == ERR_VALIDATION, (...)
  AssertionError: Decrypt with an empty-string context value must raise ValidationException (the context is malformed, not merely mismatched), got 'InvalidCiphertextException'
  Input: Encrypt(KeyId=<key ARN>, Plaintext=b'aad-bound-payload', EncryptionContext={'tenant':'acme','purpose':'invoice'}), then Decrypt(..., EncryptionContext={'tenant':'acme','purpose':''}).
  Expected: ValidationException / Message "Invalid EncryptionContext" — identically for the Encrypt form of the same input.
  Actual: InvalidCiphertextException. (The five genuine AAD deviations and the omitted-context leg did reach InvalidCiphertextException and passed the code assertion.)

Cause 3 — kms.kms-cross-region-decrypt-of-a-single-region-ciphertext-rejected [mismatch]:
  File "check_kms_kms_cross_region_decrypt_of_a_single_region_ciphertext_rejected.py", line 169, in run
    assert code_foreign_arn == ERR_CROSS_REGION, (...)
  AssertionError: a region-A key ARN presented to the us-east-1 endpoint must be refused with AccessDeniedException, got 'NotFoundException'
  Input: single-Region CMK in region A; ct_a = Encrypt at the region-A endpoint; then Decrypt(CiphertextBlob=ct_a, KeyId=<region-A key ARN>) against the region-B endpoint with the same credentials.
  Expected: AccessDeniedException, HTTP 400 (measured us-west-2 -> us-east-1, 2026-09-10).
  Actual: NotFoundException (Smithy httpError 404).
  Ungraded behind the stop: Decrypt(ct_a, KeyId=<real region-B key ARN>) must also be AccessDeniedException; region-B key must remain Enabled and usable; ct_a must still open at the region-A endpoint; and a multi-Region replica at region B MUST open its primary's region-A ciphertext (the control that separates isolation from a blanket cross-Region refusal).

Cause 4 — kms.mrk-replica-key-rotation-independent-server-managed [mismatch]:
  File "check_kms_mrk_replica_key_rotation_independent_server_managed.py", line 50, in run
    assert replica_denied_msg == (f"{replica_arn} is a multi-region replica key which is not valid for EnableKeyRotation."), replica_denied_msg
  AssertionError            <- the assertion detail (the actual Message) renders empty: the emulator sent no message text
  Input: CreateKey(MultiRegion=True) in region A, ReplicateKey to region B, wait for the replica to reach Enabled, then EnableKeyRotation(KeyId=<id>, RotationPeriodInDays=90) at the region-B endpoint.
  Expected: UnsupportedOperationException (HTTP 400) with Message "<replica key ARN> is a multi-region replica key which is not valid for EnableKeyRotation." — fixed text, ARN from state.
  Actual: UnsupportedOperationException (code correct) with an empty Message.

## Steps to Reproduce
Replay the behavioral suite and re-run the four check ids in trace_ids. Per cause, against the emulator:

Cause 1: Deploy a CloudFormation stack with one AWS::KMS::Key whose KeyPolicy has an Allow of kms:* and an explicit Deny of kms:Decrypt, both for the calling principal's role ARN. GetKeyPolicy(KeyId, PolicyName='default') returns the document verbatim. Encrypt(KeyId, Plaintext=b'cfn-policy-probe') succeeds (positive control). Then Decrypt(KeyId, CiphertextBlob=<that ciphertext>) — observe HTTP 200 + Plaintext instead of AccessDeniedException. Then PutKeyPolicy with only the Allow statement and confirm the identical Decrypt succeeds (the restore leg).

Cause 2: CreateKey(KeyUsage=ENCRYPT_DECRYPT, KeySpec=SYMMETRIC_DEFAULT). Encrypt(KeyId, Plaintext=b'aad-bound-payload', EncryptionContext={'tenant':'acme','purpose':'invoice'}). Then (a) Decrypt(CiphertextBlob=<ct>, KeyId, EncryptionContext={'tenant':'acme','purpose':''}) and (b) Encrypt(KeyId, Plaintext=b'aad-bound-payload', EncryptionContext={'tenant':'acme','purpose':''}) — both must be ValidationException / "Invalid EncryptionContext"; observe InvalidCiphertextException on (a).

Cause 3: CreateKey in region A (MultiRegion false) and ct_a = Encrypt(KeyId=<A ARN>, Plaintext=b'single-region-material-must-not-travel') at the region-A endpoint. With the same credentials call Decrypt(CiphertextBlob=ct_a, KeyId=<A ARN>) against a DIFFERENT region's KMS endpoint — observe NotFoundException instead of AccessDeniedException. Repeat with KeyId=<a real key ARN created in region B> and with KeyId omitted. Then create a multi-Region primary in A, ReplicateKey into B, and confirm the replica DOES open the primary's region-A ciphertext at the region-B endpoint.

Cause 4: CreateKey(MultiRegion=True) in region A; ReplicateKey(KeyId, ReplicaRegion=<region B>); poll describe_key at region B until KeyState=Enabled; EnableKeyRotation(KeyId, RotationPeriodInDays=90) at the region-B endpoint — inspect Error.Message, which is empty where AWS sends "<replica ARN> is a multi-region replica key which is not valid for EnableKeyRotation.". Then EnableKeyRotation(RotationPeriodInDays=180) on the primary and poll GetKeyRotationStatus at BOTH endpoints: both must report KeyRotationEnabled true with RotationPeriodInDays 180.
