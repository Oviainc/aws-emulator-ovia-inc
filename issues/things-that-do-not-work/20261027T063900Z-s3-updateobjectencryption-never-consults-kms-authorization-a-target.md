Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.update-object-encryption-error-preconditions
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.update-object-encryption-error-preconditions|_error#27|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: de2fdfddb99280a454e9752c3aabdc5314d728a2

# S3 UpdateObjectEncryption never consults KMS authorization: a target CMK whose key policy explicitly denies kms:Encrypt/kms:GenerateDataKey* is accepted with HTTP 200 instead of 403 AccessDenied (1 cause)

> _Note: this issue was identified against the code base at commit `de2fdfddb99280a454e9752c3aabdc5314d728a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateObjectEncryption

## Description
One net-new root cause in the s3 service. UpdateObjectEncryption's syntactic preconditions now work (the missing-object NoSuchKey and malformed-ARN InvalidRequest assertions in the grounding check both pass), but the operation still performs no authorization against the target KMS key: a CMK whose own key policy carries an explicit Deny on the actions S3 needs to re-encrypt the object is accepted and the call answers 200.

Cause 1 of 1: UpdateObjectEncryption does not evaluate the target CMK's key policy, so an SSE-KMS re-encryption onto a key the caller cannot use succeeds. Failing check: s3.update-object-encryption-error-preconditions.

AWS:      Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): UpdateObjectEncryption is PUT /{Bucket}/{Key+}?encryption with Bucket and Key as required @httpLabel members and ObjectEncryption as the required @httpPayload; it models exactly three errors — AccessDenied (403), InvalidRequest (400), NoSuchKey (404) — and a 200 success whose only output member is RequestCharged. The model does not describe the authorization evaluation, so that part is recorded behavior: S3 must perform the KMS operation the re-encryption needs (for a small object, kms:Encrypt directly) under the CALLER's identity against the target key, and when the key's resource-based policy carries an explicit Deny for that action the request is refused with Error.Code AccessDenied / HTTP 403, with the KMS denial relayed as the message: "User: <caller ARN> is not authorized to perform: kms:Encrypt on resource: <target key ARN> with an explicit deny in a resource-based policy". Fixed text: "User: ", " is not authorized to perform: ", " on resource: ", " with an explicit deny in a resource-based policy". Variable parts: the caller ARN (from the request's signing identity), the KMS action S3 actually attempted, and the target key ARN (from ObjectEncryption.SSEKMS.KMSKeyArn in the request). A refused call must also be a no-op: HeadObject must still report the object's prior encryption (SSE-S3, i.e. ServerSideEncryption absent or AES256) and must NOT carry an SSEKMSKeyId. Lifting the Deny and repeating the identical call must then succeed and flip HeadObject to ServerSideEncryption=aws:kms with that key.

Emulator: the call returns HTTP 200 with an empty XML body (content-length 113) and no error at all. The grounding check establishes the precondition first — it polls KMS directly until both GenerateDataKey and Encrypt on that key are refused AccessDeniedException, and that polling now SUCCEEDS, so KMS itself does enforce the Deny — and only then issues UpdateObjectEncryption, which is accepted anyway. S3 therefore never attempts (or never honours the result of) the KMS call under the caller's identity.

Why:      A client can re-encrypt an object onto a CMK that explicitly denies it the key operations, and be told it worked. The object's reported encryption state and the actual KMS grant model silently disagree: every subsequent read of that object is either served from a key the caller may not use, or fails later with no connection to the request that caused it. Negative paths every SDK surfaces as a 403 become successes, so key-policy-based blast-radius controls (an explicit Deny on a CMK — the one construct that cannot be overridden by an Allow elsewhere) validate clean against the emulator while granting the opposite of what they say.

Fix:      The UpdateObjectEncryption handler's authorization step: after the input validation it already performs, it must perform the KMS operation the re-encryption requires under the caller's identity against ObjectEncryption.SSEKMS.KMSKeyArn, map a KMS authorization refusal onto S3's AccessDenied (403) relaying the KMS message, and leave the object's encryption state untouched on refusal.

Traces:   s3.update-object-encryption-error-preconditions, asserting at check_s3_update_object_encryption_error_preconditions.py:105 (via _error at line 200). The open issue 20261027T001031Z-s3-7-net-new-root-causes-access-point.md names this check in its cause 5, but on a different replay key (a timeout inside _wait_for_deny_effective, i.e. the check never used to get past the KMS deny calibration) and for a different, broader defect (no precondition validation at all, including the ARN-syntax and object-existence classes that now pass). The replay report did not mark this check owned. This entry records the remaining, narrower cause so a fix is gated on it.

## Evidence
Replay output, verbatim assertion tail (behavioral suite simparity-behavioral-ga-prod-v1):

[mismatch] s3.update-object-encryption-error-preconditions
  File "check_s3_update_object_encryption_error_preconditions.py", line 105, in run
    code, message = _error(
        lambda: s3.update_object_encryption(
            Bucket=bucket, Key=key, ObjectEncryption={"SSEKMS": {"KMSKeyArn": denied_key_id}}
        )
    )
  File "check_s3_update_object_encryption_error_preconditions.py", line 200, in _error
    raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned
    {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/xml', 'content-length': '113', ...}, 'RetryAttempts': 0}}

Expected on that call: HTTP 403, Error.Code = AccessDenied, Error.Message =
  "User: <caller ARN> is not authorized to perform: kms:Encrypt on resource: <target key ARN> with an explicit deny in a resource-based policy"
(the ARNs are filled from the request/caller identity; the quoted clauses are the fixed text).
Actual: HTTP 200, empty XML body, no error.

What the same check establishes BEFORE that call, and which now passes — so the precondition is sound and the 200 is S3's:
- PutObject then HeadObject: the object defaults to SSE-S3 (ServerSideEncryption absent or AES256).
- UpdateObjectEncryption on a missing key with an ARN-shaped KMS key -> 404 NoSuchKey, message "The specified key does not exist." PASSES.
- UpdateObjectEncryption with KMSKeyArn="not-a-valid-kms-key-arn-string" -> 400 InvalidRequest, message "Requests modifying object encryption configuration to SSE-KMS require a valid target kms key arn: Malformed ARN - doesn't start with 'arn:'" PASSES, and HeadObject still reports SSE-S3.
- CreateKey with a policy whose second statement is Effect=Deny, Principal="*", Action=["kms:GenerateDataKey*", "kms:Encrypt"], Resource="*"; then the check polls KMS until BOTH kms:GenerateDataKey and kms:Encrypt on that key are refused AccessDeniedException. This polling COMPLETES, i.e. KMS enforces the Deny.

Smithy citation used (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json):
- UpdateObjectEncryption: PUT /{Bucket}/{Key+}?encryption, success code 200; Bucket (BucketName, @httpLabel, required), Key (ObjectKey, @httpLabel, required, @length min 1), ObjectEncryption (@httpPayload, required), VersionId (?versionId), RequestPayer / ExpectedBucketOwner / ContentMD5 / ChecksumAlgorithm headers; httpChecksum requestChecksumRequired. Modeled errors: AccessDenied (client, httpError 403), InvalidRequest (client, 400), NoSuchKey (client, 404). Output: RequestCharged only.

## Steps to Reproduce
Boot the emulator and replay this behavioral check:

  s3.update-object-encryption-error-preconditions

Manual repro, any AWS SDK pointed at the emulator:
1. CreateBucket; PutObject(Bucket=b, Key=k, Body=<a few hundred bytes>). HeadObject must report SSE-S3
   (ServerSideEncryption absent or AES256). Keep its ETag.
2. k_denied = CreateKey with a key policy of two statements:
   (a) Sid=EnableRootFullAccess, Allow kms:* for arn:aws:iam::<account>:root on Resource "*";
   (b) Sid=DenyGenerateDataKeyAndEncryptForEveryone, Deny ["kms:GenerateDataKey*", "kms:Encrypt"] for
       Principal "*" on Resource "*".
3. Confirm the precondition against KMS directly: GenerateDataKey(KeyId=k_denied, KeySpec=AES_256) and
   Encrypt(KeyId=k_denied, Plaintext=b"probe-bytes") must BOTH be refused AccessDeniedException (poll until
   they are; they are today).
4. Call UpdateObjectEncryption(Bucket=b, Key=k, ObjectEncryption={"SSEKMS": {"KMSKeyArn": k_denied}}).
   Expect HTTP 403 / Error.Code=AccessDenied with the relayed KMS message
   "User: <caller ARN> is not authorized to perform: kms:Encrypt on resource: <k_denied> with an explicit
   deny in a resource-based policy"; observe HTTP 200 and an empty XML body.
5. Then assert the refusal was a no-op: HeadObject must still report SSE-S3 and must NOT carry SSEKMSKeyId.
6. Isolation control: PutKeyPolicy on k_denied with only the root-full-access statement, wait until KMS
   permits GenerateDataKey and Encrypt again, and repeat the IDENTICAL call from step 4 — it must now
   succeed, and HeadObject must report ServerSideEncryption=aws:kms with SSEKMSKeyId=k_denied, the same
   ETag as step 1 and unchanged bytes.
