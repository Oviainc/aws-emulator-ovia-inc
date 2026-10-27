Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use kms__s3.kmsenvelope-mismatched-encryption-context-is-refused-when-unwrapping-the-stored-data-key
Issue-Key: kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use|run#21|AssertionError|assert refusal is not None, (
Issue-Key: kms__s3.kmsenvelope-mismatched-encryption-context-is-refused-when-unwrapping-the-stored-data-key|run#25|AssertionError|assert message == "", f"InvalidCiphertextException message was {message!r}, expected ''"
Issue-Commit: bd2652b5ddba3816e4efa759e4736f1b7b87af71

# kms__s3: the kms:ViaService key-policy condition is now silently ignored — a direct GenerateDataKey on a service-confined CMK succeeds — and the envelope-unwrap InvalidCiphertextException carries a message AWS leaves empty (2 causes)

> _Note: this issue was identified against the code base at commit `bd2652b5ddba3816e4efa759e4736f1b7b87af71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateDataKey

## Description
Two independent root causes reached through the S3 envelope-encryption checks. Cause 1 is OVERLY PERMISSIVE: a key policy that confines a CMK to one service path is not enforced at all. Cause 2 is a refusal-message-template divergence on the data-key unwrap.

Cause 1 of 2: the kms:ViaService condition on a key policy is never evaluated, so a direct GenerateDataKey on a service-confined CMK succeeds.
AWS:      Smithy: GenerateDataKey.KeyId is required (KeyIdType, @length 1..2048) and the operation models DisabledException(409, awsQueryError Disabled), KMSInvalidStateException(409), NotFoundException(404, NotFound), InvalidGrantTokenException(400), InvalidKeyUsageException(400), KeyUnavailableException(500), DependencyTimeoutException(503) — an authorization refusal surfaces as the service-common AccessDeniedException (HTTP 400). The model does not describe policy-condition evaluation, so the contract is recorded behavior: with a key policy whose crypto statement (kms:Encrypt, kms:Decrypt, kms:ReEncrypt*, kms:GenerateDataKey*) is gated by Condition StringEquals kms:ViaService = s3.<region>.amazonaws.com, the SAME caller's S3 SSE-KMS PutObject under that key must SUCCEED (the request arrives through S3's envelope path) and a direct GenerateDataKey on that key must be REFUSED with Error.Code AccessDeniedException. The message is AWS's standard authorization-denial text: its fixed text is the "User: ... is not authorized to perform: ... because ..." wording, and its variable parts are the caller ARN (from the caller identity), the action token (kms:GenerateDataKey) and the key ARN (from the request) — never literal values from one run.
Emulator: the direct GenerateDataKey returns HTTP 200 with a usable data key. No authorization refusal is produced at all, so the key policy's ViaService condition has no effect and a CMK a policy confines to S3 is freely usable by any caller on the direct KMS data plane.
Why:      Any least-privilege design that relies on kms:ViaService to pin a CMK to one service (the canonical pattern for SSE-KMS buckets) validates clean against the emulator while granting the opposite of what it says. A client cannot test that its key is unusable outside S3, and a reviewer reading a green run concludes the confinement works.
Fix:      The key-policy condition evaluation (kms:ViaService, and the via-service attribution of requests that arrive through the S3 SSE-KMS envelope path) in the KMS authorization path consulted by GenerateDataKey and the sibling crypto operations — not the GenerateDataKey happy path, which now works.
Traces:   kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use, asserting at check_kms__s3_kmsenvelope_via_service_condition_permits_the_s3_envelope_path_and_denies_the_same_callers_direct_kms_use.py:193. This is exactly the regression the still-open issue 20261024T140708Z-kms-s3-a-kms-viaservice-scoped-key-policy-is-not.md predicted ("once GenerateDataKey is implemented without condition evaluation it would be silently OVERLY PERMISSIVE"); that issue carries this check's OLD replay key (an assertion at line 198 on Error.Code 'NotImplemented'), and the check now fails at line 193 on an unexpected SUCCESS, which is why the replay report did not mark it owned. This entry records the measured overly-permissive behavior so the fix is gated on it.

Cause 2 of 2: InvalidCiphertextException is returned with the documentation blurb as its message where AWS returns an empty message, on the stored-data-key unwrap path.
AWS:      Smithy: Decrypt models InvalidCiphertextException (client, httpError 400, awsQueryError InvalidCiphertext). Recorded behavior: unwrapping a stored envelope data key with an EncryptionContext that does not match the one bound at GenerateDataKey/Encrypt time is refused with Error.Code InvalidCiphertextException, HTTP 400, and an EMPTY Error.Message — there are no variable parts because there is no text.
Emulator: the refusal carries Error.Message "The request was rejected because the specified ciphertext, or additional authenticated data incorporated into the ciphertext, such as the encryption context, is corrupted, missing, or otherwise invalid." — the API-reference prose for the exception shape, which the wire never carries.
Why:      A client-side envelope encryption library (and this check) treats the absence of a message as part of the contract; the emulator makes an information-free refusal look diagnostic, so code that distinguishes KMS refusals by message text behaves differently against AWS.
Fix:      The InvalidCiphertextException construction in the Decrypt handler / the KMS error serializer. Same serializer defect as recorded for the plain-KMS path in the companion kms issue filed in this sweep; it is recorded separately here because it is grounded in a different service's failing check.
Traces:   kms__s3.kmsenvelope-mismatched-encryption-context-is-refused-when-unwrapping-the-stored-data-key, asserting at check_kms__s3_kmsenvelope_mismatched_encryption_context_is_refused_when_unwrapping_the_stored_data_key.py:163.

## Evidence
Replay output, verbatim assertion tails (behavioral suite simparity-behavioral-ga-prod-v1):

1. [mismatch] kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use
   File "check_kms__s3_kmsenvelope_via_service_condition_permits_the_s3_envelope_path_and_denies_the_same_callers_direct_kms_use.py", line 193, in run
     assert refusal is not None, (
   AssertionError: GenerateDataKey succeeded when called directly on a key whose policy permits it only with kms:ViaService == 's3.<region>.amazonaws.com' — the condition is not being evaluated, so any caller can take data keys out of the service path the policy confines them to

   Request:  CreateKey -> PutKeyPolicy with an unconditional admin statement plus a crypto statement
             (kms:Encrypt, kms:Decrypt, kms:ReEncrypt*, kms:GenerateDataKey*) carrying
             Condition StringEquals kms:ViaService = s3.<region>.amazonaws.com;
             then S3 PutObject(ServerSideEncryption=aws:kms, SSEKMSKeyId=<that key>) as that caller (must succeed);
             then GenerateDataKey(KeyId=<that key>, KeySpec=AES_256) directly as the SAME caller.
   Expected: HTTP 400, Error.Code = AccessDeniedException on the direct call
   Actual:   HTTP 200 with a usable data key — no refusal at all

   Note on history: the open issue 20261024T140708Z-kms-s3-a-kms-viaservice-scoped-key-policy-is-not.md records this check failing at line 198 with Error.Code 'NotImplemented'. GenerateDataKey is now implemented, so the check progresses to line 193 and the divergence has changed class from a stub refusal to a silent over-permit. The replay report therefore did not mark this check as owned by that issue.

2. [mismatch] kms__s3.kmsenvelope-mismatched-encryption-context-is-refused-when-unwrapping-the-stored-data-key
   File "check_kms__s3_kmsenvelope_mismatched_encryption_context_is_refused_when_unwrapping_the_stored_data_key.py", line 163, in run
     assert message == "", f"InvalidCiphertextException message was {message!r}, expected ''"
   AssertionError: InvalidCiphertextException message was 'The request was rejected because the specified ciphertext, or additional authenticated data incorporated into the ciphertext, such as the encryption context, is corrupted, missing, or otherwise invalid.', expected ''

   Request:  GenerateDataKey(KeyId=<key>, EncryptionContext=C) -> store the wrapped key alongside an AES-GCM object in S3;
             then Decrypt(CiphertextBlob=<stored wrapped data key>, EncryptionContext=<C with a mismatched entry>)
   Expected: HTTP 400, Error.Code = InvalidCiphertextException, Error.Message = "" (empty)
   Actual:   same code and status, Error.Message = the API-reference prose for the exception shape

Smithy citations used (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json):
- GenerateDataKey: KeyId required (KeyIdType @length 1..2048); errors DisabledException(409, Disabled), KMSInvalidStateException(409), NotFoundException(404, NotFound), InvalidGrantTokenException(400), InvalidKeyUsageException(400), KeyUnavailableException(500), DependencyTimeoutException(503), DryRunOperationException(412). Authorization refusal is the service-common AccessDeniedException; kms:ViaService condition evaluation is recorded behavior, not modeled.
- Decrypt: errors InvalidCiphertextException(400, awsQueryError InvalidCiphertext), IncorrectKeyException(400), DisabledException(409), KMSInvalidStateException(409), NotFoundException(404).

## Steps to Reproduce
Boot the emulator and replay these two behavioral checks:

  kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use
  kms__s3.kmsenvelope-mismatched-encryption-context-is-refused-when-unwrapping-the-stored-data-key

Manual repro, any AWS SDK pointed at the emulator:

Cause 1:
1. k = CreateKey(KeyUsage=ENCRYPT_DECRYPT, KeySpec=SYMMETRIC_DEFAULT).
2. PutKeyPolicy(KeyId=k) with two statements: (a) an unconditional admin statement for the caller
   (kms:DescribeKey, kms:GetKeyPolicy, kms:PutKeyPolicy, kms:ScheduleKeyDeletion, kms:CancelKeyDeletion,
   kms:EnableKey, kms:DisableKey, kms:TagResource, kms:ListResourceTags) so teardown can never lock out, and
   (b) a crypto statement (kms:Encrypt, kms:Decrypt, kms:ReEncrypt*, kms:GenerateDataKey*) with
   Condition StringEquals kms:ViaService = s3.<region>.amazonaws.com.
3. CreateBucket, then PutObject(ServerSideEncryption="aws:kms", SSEKMSKeyId=k). This leg must SUCCEED.
4. As the SAME caller, call GenerateDataKey(KeyId=k, KeySpec=AES_256) directly.
   Expect Error.Code=AccessDeniedException (HTTP 400); observe HTTP 200 with a usable data key.
5. Symmetry control: the same caller's direct Encrypt/Decrypt on k must be refused the same way, while the
   S3 GetObject of the object written in step 3 must still succeed.

Cause 2:
1. k = CreateKey(); dk = GenerateDataKey(KeyId=k, KeySpec=AES_256, EncryptionContext={"purpose":"envelope", ...}).
2. AES-GCM encrypt a payload under dk.Plaintext and store the object plus dk.CiphertextBlob in S3.
3. Decrypt(CiphertextBlob=<the stored wrapped data key>, EncryptionContext=<the same map with one entry changed>).
   Expect Error.Code=InvalidCiphertextException (HTTP 400) with an EMPTY Error.Message; observe the
   API-reference prose as the message.
