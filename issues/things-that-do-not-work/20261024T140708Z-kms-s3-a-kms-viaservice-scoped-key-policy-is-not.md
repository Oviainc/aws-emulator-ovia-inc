Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use
Issue-Key: kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use|run#23|AssertionError|assert code == "AccessDeniedException", (
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# A kms:ViaService-scoped key policy is not enforced on direct KMS use: GenerateDataKey outside the permitted service path answers NotImplemented instead of AccessDeniedException (1 cause)

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateDataKey

## Description
Cause 1 of 1: the `kms:ViaService` condition on a key policy is never evaluated, because the operation it gates is a scaffold stub.
AWS:      Smithy: GenerateDataKey takes a required `KeyId` (KeyIdType, @length 1..2048) and models DisabledException(409)/KMSInvalidStateException(409)/NotFoundException(404)/InvalidGrantTokenException(400); authorization refusals surface as the service-common `AccessDeniedException`. The contract this check pins is the standard `kms:ViaService` condition semantics: a key policy that allows the crypto actions (`kms:Encrypt`, `kms:Decrypt`, `kms:ReEncrypt*`, `kms:GenerateDataKey*`) ONLY when the request arrives through the named service (here S3's envelope path) must permit the S3 SSE-KMS write for that caller and must REFUSE the same caller's direct `GenerateDataKey` on the same key with `AccessDeniedException`. The model does not state the condition evaluation, so this is recorded behavior: expected Error.Code `AccessDeniedException` on the direct call.
Emulator: the direct GenerateDataKey call returns Error.Code `NotImplemented` ("operation GenerateDataKey is scaffolded but not implemented in this emulator"), so the two halves of the contract are indistinguishable — the permitted service path and the denied direct path both fail for the same unrelated reason, and the ViaService condition is never consulted.
Why:      A client validating least-privilege envelope-encryption design cannot verify that a key is usable only through S3: the emulator neither permits the envelope path nor denies the direct path on authorization grounds. Any policy that relies on `kms:ViaService` to confine a CMK to one service reads as untestable, and once GenerateDataKey is implemented without condition evaluation it would be silently OVERLY PERMISSIVE — direct crypto use of a service-confined key would succeed.
Fix:      The GenerateDataKey handler in the KMS crate, plus the key-policy condition evaluation (`kms:ViaService`) in the KMS authorization path that the S3 SSE-KMS envelope call must satisfy. Related open issues: `20261024T102105Z-kms-generatedatakey-stub.md` names the stub, and `20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md` names the missing authorization evaluation generally; neither carries this check's replay key or the ViaService contract, so this records it.
Traces:   kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use, asserting at check_kms__s3_kmsenvelope_via_service_condition_permits_the_s3_envelope_path_and_denies_the_same_callers_direct_kms_use.py:198. Handler: services/service-com-amazonaws-kms-trentservice/src/ops/generate_data_key.rs.

## Evidence
Replay output, verbatim assertion tail:

[mismatch] kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use
  File "check_kms__s3_kmsenvelope_via_service_condition_permits_the_s3_envelope_path_and_denies_the_same_callers_direct_kms_use.py", line 198, in run
    assert code == "AccessDeniedException", (
  AssertionError: expected AccessDeniedException for a direct GenerateDataKey outside the permitted service path, got 'NotImplemented'

Expected: Error.Code = AccessDeniedException for the direct GenerateDataKey, under a key policy whose crypto statement (kms:Encrypt, kms:Decrypt, kms:ReEncrypt*, kms:GenerateDataKey*) is conditioned on kms:ViaService naming S3, while the same caller's S3 SSE-KMS PutObject under that key succeeds.
Actual:   Error.Code = NotImplemented ("operation GenerateDataKey is scaffolded but not implemented in this emulator").

Smithy citation: GenerateDataKey input KeyId required (KeyIdType, @length 1..2048); modeled errors DisabledException(409, awsQueryError Disabled), KMSInvalidStateException(409), NotFoundException(404), InvalidGrantTokenException(400), InvalidKeyUsageException(400) — authorization refusal is the service-common AccessDeniedException. The ViaService condition behavior is recorded behavior only.

## Steps to Reproduce
Boot the emulator and replay the behavioral check:

  kms__s3.kmsenvelope-via-service-condition-permits-the-s3-envelope-path-and-denies-the-same-callers-direct-kms-use

Manual repro with any AWS SDK pointed at the emulator:
1. CreateKey, then PutKeyPolicy on it with two statements: an unconditional admin statement (kms:DescribeKey, kms:GetKeyPolicy, kms:PutKeyPolicy, kms:ScheduleKeyDeletion, kms:CancelKeyDeletion, kms:EnableKey, kms:DisableKey, kms:TagResource, kms:ListResourceTags) so teardown can never be locked out, and a crypto statement (kms:Encrypt, kms:Decrypt, kms:ReEncrypt*, kms:GenerateDataKey*) with Condition StringEquals kms:ViaService = s3.<region>.amazonaws.com.
2. CreateBucket, then PutObject with ServerSideEncryption=aws:kms and SSEKMSKeyId=<that key>. This leg must SUCCEED (the request arrives via S3).
3. As the SAME caller, call GenerateDataKey(KeyId=<that key>, KeySpec=AES_256) directly.
   Expect Error.Code=AccessDeniedException; observe Error.Code=NotImplemented.
