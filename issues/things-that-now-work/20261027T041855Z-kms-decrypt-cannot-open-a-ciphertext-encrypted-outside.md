Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.kms-ciphertext-produced-locally-with-the-published-rsa-public-key-decrypts-in-kms
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|Decrypt|InvalidCiphertextException
Issue-Commit: 855066f75c7ad2828bfa689b40045f4c4c8b1f3f

# KMS Decrypt cannot open a ciphertext encrypted OUTSIDE KMS with the public key GetPublicKey published: every raw RSAES_OAEP blob is refused InvalidCiphertextException (1 cause)

> _Note: this issue was identified against the code base at commit `855066f75c7ad2828bfa689b40045f4c4c8b1f3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Decrypt

## Description
1 cause in this issue; it must be closed before submitting.

Cause 1 of 1: Decrypt has no asymmetric (raw RSA-OAEP) decryption path — it only opens the proprietary AES-GCM envelope the emulator's own Encrypt produced, so a ciphertext built outside KMS from the DER SubjectPublicKeyInfo that GetPublicKey published is refused instead of decrypted.

AWS: Decrypt is documented to decrypt ciphertext produced outside KMS by the public half of an asymmetric KMS key — Smithy doc for Decrypt (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json): "The Decrypt operation also decrypts ciphertext that was encrypted outside of KMS by the public key in an KMS asymmetric KMS key." Smithy: DecryptRequest.CiphertextBlob -> CiphertextType, blob, @length 1..6144; DecryptRequest.EncryptionAlgorithm -> EncryptionAlgorithmSpec enum {RSAES_OAEP_SHA_1, RSAES_OAEP_SHA_256, SM2PKE, SYMMETRIC_DEFAULT}, documented as "required only when the ciphertext was encrypted under an asymmetric KMS key"; DecryptRequest.KeyId -> KeyIdType @length 1..2048. GetPublicKey.PublicKey is "a DER-encoded X.509 public key, also known as SubjectPublicKeyInfo (SPKI), as defined in RFC 5280", and GetPublicKey.EncryptionAlgorithms is documented as "critical. If a public key encrypts data outside of KMS by using an unsupported encryption algorithm, the ciphertext cannot be decrypted" — i.e. encrypting outside KMS with an ADVERTISED algorithm must decrypt inside KMS. So for an RSA_3072 / KeyUsage=ENCRYPT_DECRYPT CMK whose GetPublicKey advertises exactly {RSAES_OAEP_SHA_1, RSAES_OAEP_SHA_256}, a plaintext OAEP-encrypted locally under that SPKI with MGF1/SHA-256 and presented to Decrypt with EncryptionAlgorithm=RSAES_OAEP_SHA_256 must be answered HTTP 200, with Plaintext equal to the original bytes, KeyId equal to the key ARN (the resolved ARN, not the identifier the request used) and EncryptionAlgorithm echoing RSAES_OAEP_SHA_256. The same bytes presented under the WRONG declared digest (RSAES_OAEP_SHA_1 bytes claimed as RSAES_OAEP_SHA_256) are the only leg that must be refused, and there the requirement is HTTP 400 with error code InvalidCiphertextException; AWS carries no message for that error on Decrypt (the body's message is absent — the recorded refusal renders as "An error occurred (InvalidCiphertextException) when calling the Decrypt operation:" with nothing after the colon), so the message is not the divergence here, the status and code on the VALID ciphertext are.

Emulator: GetPublicKey is faithful — services/service-com-amazonaws-kms-trentservice/src/ops/get_public_key.rs computes the public half from the private key the store holds (Cmk::public_key()), so the published SPKI really is the counterpart of the private half. Decrypt is not: services/service-com-amazonaws-kms-trentservice/src/ops/decrypt.rs states in its own header "IMPLEMENTED: opens the ciphertext envelope `Encrypt` produced" and its handle() begins by requiring that envelope — crate::state::parse_envelope(blob), and failing that returns invalid_ciphertext(); the plaintext is then recovered only via crate::state::unseal(key, &envelope, ...), an AES-GCM open. There is no RSA/OAEP branch anywhere in the handler, and EncryptionAlgorithm is never used as a cryptographic parameter. A genuine RSAES_OAEP_SHA_256 ciphertext is therefore not recognised as an envelope at all and the call is answered HTTP 400 InvalidCiphertextException with an empty message, on the FIRST (valid) decrypt — so the digest-discrimination leg and the effect-absent leg behind it are unreachable.

Why: the whole point of an asymmetric ENCRYPT_DECRYPT CMK is that a party holding only the published public key can encrypt without any KMS call and KMS alone can open it. In this emulator that one-way path is impossible: any client that encrypts with the key KMS published — the AWS SDK's own offline-encrypt pattern, an envelope-encryption library, a device or partner that was handed the SPKI — gets a hard 400 InvalidCiphertextException and reads it as "my ciphertext is corrupt" or "the public key is not the key's public key", when both are fine. Worse, the only ciphertexts the emulator can decrypt are ones it minted itself, so a target can pass every KMS-encrypt/KMS-decrypt round-trip while holding key material that is not an RSA key pair at all; the OAEP digest is also not a real parameter, so a client that mis-declares the digest is not told so for the right reason. Any code path that derives trust from "KMS can open this, therefore KMS holds the private half" is silently wrong.

Fix: belongs in the Decrypt handler and the key-material layer it calls — services/service-com-amazonaws-kms-trentservice/src/ops/decrypt.rs must branch on the resolved key's spec family before requiring crate::state::parse_envelope: for an asymmetric ENCRYPT_DECRYPT key it must treat CiphertextBlob as a raw RSA ciphertext and open it with the stored private key (crate::state::Cmk material, the same private key get_public_key.rs derives the published SPKI from) under the OAEP parameters named by input.encryption_algorithm (MGF1 and the OAEP digest both taken from RSAES_OAEP_SHA_1 / RSAES_OAEP_SHA_256, label empty), keeping the existing envelope path for symmetric keys. The modelled refusals keep their current meaning: an OAEP open that fails under the declared algorithm stays HTTP 400 InvalidCiphertextException with no message, a KeyId naming a different key stays IncorrectKeyException, and an algorithm the key's spec does not advertise is InvalidKeyUsageException. The response must report KeyId as the key ARN and echo the EncryptionAlgorithm actually used.

Not covered by any open issue: 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md covers asymmetric Encrypt/ReEncrypt and authorization, not Decrypt of an externally produced ciphertext; the fixed 20261023T084512Z-kms-key-identifier-resolution-and-decrypt-response-fidelity.md touched Decrypt only for alias resolution and the KeyOrigin response member and never claimed an asymmetric decrypt path, so this is a net-new defect rather than a reopen.

## Evidence
Behavioral suite replay (248 checks, 132 passed; emulator serving, bootstrap_signal=false). Failing check kms.kms-ciphertext-produced-locally-with-the-published-rsa-public-key-decrypts-in-kms, verdict [error] — the ONLY failure in the report not marked ALREADY FILED or OWNED.

Replay output:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.InvalidCiphertextException: An error occurred (InvalidCiphertextException) when calling the Decrypt operation:

Expected (AWS): HTTP 200 from Decrypt, with Plaintext == the bytes encrypted locally, KeyId == the created key's ARN, EncryptionAlgorithm == "RSAES_OAEP_SHA_256".
Actual (emulator): HTTP 400, error code InvalidCiphertextException, empty message — raised on the FIRST Decrypt, the one presenting a VALID RSAES_OAEP_SHA_256 ciphertext.

What the check established before that call, and which the emulator answered correctly:
  - CreateKey(KeyUsage=ENCRYPT_DECRYPT, KeySpec=RSA_3072) reaches KeyState Enabled.
  - GetPublicKey returns KeySpec RSA_3072, KeyUsage ENCRYPT_DECRYPT, EncryptionAlgorithms == {RSAES_OAEP_SHA_1, RSAES_OAEP_SHA_256}, and the DER SPKI loads as a 3072-bit RSA public key.
So the published public key is right; only Decrypt cannot use it. No KMS Encrypt is involved anywhere — the ciphertext is produced entirely client-side from fresh randomness, which is why no wire fixture can grade this.

Source confirming the cause (citations, not a patch):
  services/service-com-amazonaws-kms-trentservice/src/ops/decrypt.rs:4 — "IMPLEMENTED: opens the ciphertext envelope `Encrypt` produced"
  services/service-com-amazonaws-kms-trentservice/src/ops/decrypt.rs:400 — `let Some(envelope) = crate::state::parse_envelope(blob) else { return Err(invalid_ciphertext()); };`  (unconditional, before any key-spec branch)
  services/service-com-amazonaws-kms-trentservice/src/ops/decrypt.rs:535 — `crate::state::unseal(key, &envelope, input.encryption_context.as_ref())` — AES-GCM only
  services/service-com-amazonaws-kms-trentservice/src/ops/decrypt.rs:579-584 — `invalid_ciphertext()` builds InvalidCiphertextException { message: None }
  services/service-com-amazonaws-kms-trentservice/src/ops/get_public_key.rs:8-12, 392-406 — the published public key IS computed from the stored private key, so the two halves exist and disagree only because Decrypt never uses them.

Model cited (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json):
  Decrypt input CiphertextBlob (CiphertextType blob, @length 1..6144), EncryptionAlgorithm (EncryptionAlgorithmSpec: RSAES_OAEP_SHA_1 | RSAES_OAEP_SHA_256 | SM2PKE | SYMMETRIC_DEFAULT), KeyId (KeyIdType @length 1..2048)
  Decrypt output KeyId, Plaintext, EncryptionAlgorithm, CiphertextForRecipient, KeyMaterialId
  Decrypt errors include InvalidCiphertextException (client, HTTP 400), IncorrectKeyException (400), InvalidKeyUsageException (400)
  GetPublicKey output PublicKey = DER X.509 SubjectPublicKeyInfo (RFC 5280); EncryptionAlgorithms doc: a public key encrypting outside KMS with a supported algorithm must be decryptable.

## Steps to Reproduce
1. Call kms.CreateKey(KeyUsage="ENCRYPT_DECRYPT", KeySpec="RSA_3072") and poll DescribeKey until KeyState == "Enabled".
2. Call kms.GetPublicKey(KeyId=<key ARN>) and load the returned PublicKey bytes as a DER SubjectPublicKeyInfo RSA public key (e.g. cryptography's serialization.load_der_public_key). Confirm KeySpec=RSA_3072 and EncryptionAlgorithms == {RSAES_OAEP_SHA_1, RSAES_OAEP_SHA_256}.
3. WITHOUT any KMS Encrypt call, encrypt a short plaintext locally with that public key using RSA-OAEP, MGF1(SHA-256), digest SHA-256, no label.
4. Call kms.Decrypt(CiphertextBlob=<those bytes>, KeyId=<key ARN>, EncryptionAlgorithm="RSAES_OAEP_SHA_256").
   Emulator: HTTP 400 InvalidCiphertextException with an empty message.
   AWS: HTTP 200 with Plaintext == the original bytes, KeyId == the key ARN, EncryptionAlgorithm == "RSAES_OAEP_SHA_256".
5. Then verify the digest is a real parameter, not a label: encrypt the SAME plaintext with OAEP/MGF1(SHA-1)/SHA-1 and call Decrypt with EncryptionAlgorithm="RSAES_OAEP_SHA_256" — this leg must be HTTP 400 InvalidCiphertextException (no message) — and the identical bytes with EncryptionAlgorithm="RSAES_OAEP_SHA_1" must return 200 with the original plaintext. Finally re-Decrypt the step-3 ciphertext to confirm the failed attempt consumed nothing, and DescribeKey to confirm KeyState=Enabled / KeySpec=RSA_3072 unchanged.
6. Re-replay kms.kms-ciphertext-produced-locally-with-the-published-rsa-public-key-decrypts-in-kms to confirm the fix.
