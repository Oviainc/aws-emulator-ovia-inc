Issue-Source: trace-sweep-divergence
Issue-Trace: kms-asymmetric-key-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-asymmetric-key-lifecycle kms-asymmetric-key-sign-verify kms-asymmetric-sign-verify kms-derive-shared-secret kms-key-material-import kms-key-rotation-management kms-mac-generate-verify kms-asymmetric-key-tag-workflow kms-data-key-generation kms-create-describe-update-list kms-key-description-update-and-cancel-deletion kms-update-key-description-lifecycle kms-update-key-description-roundtrip kms-symmetric-encrypt-decrypt
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|CreateKey|2xx->4xx|status
Issue-Key: kms|CreateKey|2xx->4xx|$.KeyMetadata
Issue-Key: kms|Decrypt|2xx->2xx|$.KeyOrigin
Issue-Related: 20261022T131242Z-kms-cancelkeydeletion-does-not-restore-the-key-leaving.md
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# KMS key metadata is not honoured: asymmetric CreateKey rejected, UpdateKeyDescription dropped, Decrypt omits KeyOrigin

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKey

## Description
Four replay causes in the KMS key-metadata surface: the emulator only accepts symmetric key creation, it does not persist a key's description update, and it omits a field the recorded Decrypt response carries. All four are metadata the emulator stores or projects, not error-path behaviour.

Cause 1 of 4 — CreateKey rejects every non-symmetric KeySpec.
AWS: CreateKey accepts KeySpec (Smithy: CreateKey.KeySpec, enum KeySpec; the deprecated CustomerMasterKeySpec is its alias) for asymmetric RSA/ECC, HMAC and key-agreement specs, with KeyUsage set accordingly (SIGN_VERIFY, GENERATE_VERIFY_MAC, KEY_AGREEMENT), and answers HTTP 200 with CreateKeyResponse{KeyMetadata}. None of CreateKey's modelled errors (UnsupportedOperationException, LimitExceededException, MalformedPolicyDocumentException, TagException, InvalidArnException, the CustomKeyStore/Xks family — all @httpError 400) applies to a well-formed request for a standard spec in the default key store.
Emulator: HTTP 400 for these CreateKey requests, so every asymmetric, HMAC and key-agreement fixture fails at its very first step.
Why: no client can create anything but a symmetric key. Sign/Verify, GenerateMac/VerifyMac, DeriveSharedSecret, key-material import and key rotation are all unreachable, since each needs a key of the matching spec to exist first.
Fix: CreateKey's validation and the key record it writes — the handler must accept the modelled KeySpec/KeyUsage combinations and persist the resulting key metadata.
Traces: kms-asymmetric-key-lifecycle#setup#1 (expected 200, got 400), which also blocks kms-asymmetric-key-sign-verify#setup#1, kms-asymmetric-sign-verify#setup#1, kms-derive-shared-secret#setup#1 and #setup#2, kms-key-material-import#setup#1, kms-key-rotation-management#setup#2, kms-mac-generate-verify#setup#1.

Cause 2 of 4 — CreateKey for an RSA signing key returns no KeyMetadata at all.
AWS: a CreateKey with KeySpec RSA_2048, KeyUsage SIGN_VERIFY, a Description and Tags answers 200 and the body's KeyMetadata carries the created key's identity and capability: KeyId, Arn, AWSAccountId, CreationDate, Enabled, KeyState Enabled, KeyManager CUSTOMER, Origin AWS_KMS, MultiRegion, the echoed Description, KeySpec/CustomerMasterKeySpec RSA_2048 and the SigningAlgorithms list for that spec (Smithy: CreateKey -> KeyMetadata).
Emulator: HTTP 400 and $.KeyMetadata missing entirely, so the caller learns neither the key id nor which signing algorithms it may use.
Why: a client cannot even discover the key it asked for, let alone pick a signing algorithm; tag-on-create workflows fail at step one.
Fix: as Cause 1 — CreateKey's validation plus the KeyMetadata serializer, which must derive SigningAlgorithms from the accepted KeySpec.
Traces: kms-asymmetric-key-tag-workflow#task#1 (expected 200 with full KeyMetadata, got 400 with none), also kms-data-key-generation#task#7.

Cause 3 of 4 — UpdateKeyDescription does not persist the new description.
AWS: UpdateKeyDescription (Smithy: input KeyId required, Description required @length 0..8192, output Unit) replaces the key's description, and a following DescribeKey reports KeyMetadata.Description as the value just written.
Emulator: the update is acknowledged, but DescribeKey answers 200 still carrying the description the key was created with.
Why: a client that renames a key and then reads it back sees the write silently lost — key inventories, console-style listings and any read-after-write check disagree with what the caller was told succeeded.
Fix: the UpdateKeyDescription handler's write into the key store (the describe projection already reads the field).
Traces: kms-create-describe-update-list#task#4 (expected the updated description, got the original), spanning kms-create-describe-update-list#task#6, kms-key-description-update-and-cancel-deletion#task#4 and #task#8, kms-update-key-description-lifecycle#task#4 and #task#6, kms-update-key-description-roundtrip#task#4 and #task#6.

Cause 4 of 4 — Decrypt omits KeyOrigin from its response.
AWS: recorded behavior only — the recorded Decrypt response for a symmetric AWS_KMS-origin key includes KeyOrigin = "AWS_KMS" alongside KeyId and Plaintext. The on-disk Smithy model's DecryptResponse does not declare KeyOrigin (it declares KeyId, Plaintext, EncryptionAlgorithm, CiphertextForRecipient, KeyMaterialId), so the recording is the only evidence for the field; its value tracks the key's Origin rather than being a constant.
Emulator: Decrypt answers 200 with a correct plaintext but no KeyOrigin member.
Why: a client that reads KeyOrigin to tell an AWS_KMS-backed key from an external or custom-key-store one gets nothing to read and cannot make that distinction from the decrypt result.
Fix: the Decrypt response serializer, projecting the decrypting key's origin.
Traces: kms-symmetric-encrypt-decrypt#task#4 ($.KeyOrigin missing, expected 'AWS_KMS').

No open issue covers these; the open kms TagResource NotImplemented issue (20261022T081518Z-kms-implement-kms-tagresource...) is a different operation. state_reset_failures was 0 for this run.

## Evidence
Replay of trace set simparity-wire-aws-v1 (64 fixtures, 475 steps, score 37.1%, state_reset_failures=0). Four divergence clusters:
- kms-asymmetric-key-lifecycle#setup#1 CreateKey: expected 200, got 400; affects 8 fixture setups (kms-asymmetric-key-sign-verify, kms-asymmetric-sign-verify, kms-derive-shared-secret x2, kms-key-material-import, kms-key-rotation-management, kms-mac-generate-verify).
- kms-asymmetric-key-tag-workflow#task#1 CreateKey: expected 200, got 400; $.KeyMetadata missing — recorded body had KeySpec/CustomerMasterKeySpec 'RSA_2048', KeyUsage 'SIGN_VERIFY', KeyState 'Enabled', KeyManager 'CUSTOMER', Origin 'AWS_KMS', MultiRegion false and the six RSASSA SigningAlgorithms. Also kms-data-key-generation#task#7.
- kms-create-describe-update-list#task#4 DescribeKey 200/200: $.KeyMetadata.Description expected the '-updated-' value written by UpdateKeyDescription, got the original creation description. Also kms-create-describe-update-list#task#6, kms-key-description-update-and-cancel-deletion#task#4/#task#8, kms-update-key-description-lifecycle#task#4/#task#6, kms-update-key-description-roundtrip#task#4/#task#6.
- kms-symmetric-encrypt-decrypt#task#4 Decrypt 200/200: $.KeyOrigin missing, expected 'AWS_KMS'.
Smithy (com-amazonaws-kms-trentservice): CreateKey.KeySpec enum KeySpec / KeyUsage enum KeyUsageType -> CreateKeyResponse{KeyMetadata}; UpdateKeyDescription{KeyId required, Description required @length 0..8192} -> Unit; DecryptResponse{KeyId, Plaintext, EncryptionAlgorithm, CiphertextForRecipient, KeyMaterialId} — no KeyOrigin modelled, so Cause 4 rests on the recording.

## Steps to Reproduce
1. Boot the emulator from the mainline checkout and replay trace set simparity-wire-aws-v1.
2. Call CreateKey with KeySpec RSA_2048 and KeyUsage SIGN_VERIFY (and again with HMAC_256/GENERATE_VERIFY_MAC and an ECC spec): each answers HTTP 400 instead of 200 with KeyMetadata (Causes 1 and 2).
3. Call CreateKey with the default symmetric spec, then UpdateKeyDescription with a new Description, then DescribeKey: KeyMetadata.Description is still the creation-time value (Cause 3).
4. Encrypt then Decrypt with that symmetric key: the Decrypt response has no KeyOrigin member (Cause 4).
5. Replay kms-asymmetric-key-lifecycle, kms-asymmetric-key-tag-workflow, kms-create-describe-update-list and kms-symmetric-encrypt-decrypt to confirm the fix.
