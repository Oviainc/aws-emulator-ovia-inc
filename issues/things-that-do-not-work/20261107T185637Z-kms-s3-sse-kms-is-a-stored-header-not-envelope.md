Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.kmsenvelope-deny-of-generate-data-key-blocks-the-default-encrypted-write-while-earlier-objects-stay-readable kms__s3.sep-kms-key-scope-sse-kms-object-unreadable-while-its-key-is-disabled-rejected kms__s3.bucket-key-switches-the-kms-encryption-context-from-object-arn-to-bucket-arn
Issue-Key: kms__s3.kmsenvelope-deny-of-generate-data-key-blocks-the-default-encrypted-write-while-earlier-objects-stay-readable|run#29|AssertionError|assert refusal is not None, (
Issue-Key: kms__s3.sep-kms-key-scope-sse-kms-object-unreadable-while-its-key-is-disabled-rejected|_poll#11|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last {last!r}")
Issue-Key: kms__s3.bucket-key-switches-the-kms-encryption-context-from-object-arn-to-bucket-arn|_poll_until#36|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting for {what}; last saw {last!r}")
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# SSE-KMS is a stored header, not envelope encryption: S3 never calls KMS, so key state, key policy and encryption context have no effect

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
One root cause, grounded in three failing checks this report does NOT mark ALREADY FILED or OWNED. Open issues 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md and 20261103T024024Z-kms-s3-s3-s-sse-kms-default-encryption.md own OTHER checks of this service and do not carry these replay keys.

Cause 1 of 1: S3 performs no KMS call on an SSE-KMS write or read, so every KMS-side control is inert.
AWS: an SSE-KMS write makes S3 call kms:GenerateDataKey on the named (or bucket-default) CMK, and an SSE-KMS read makes it call kms:Decrypt. Three consequences follow, and all three are what the recordings assert: (a) once kms:GenerateDataKey is denied to the writer on the bucket's default CMK, PutObject is refused (403 AccessDenied / KMS.AccessDeniedException) while objects written earlier stay readable; (b) while the CMK is Disabled, GetObject of an object encrypted with it is refused 400 KMS.DisabledException / "<key-arn> is disabled" and succeeds again after EnableKey; (c) the encryption context S3 sends is the bucket ARN when BucketKeyEnabled=true and the object ARN when it is false, so a key policy whose kms:EncryptionContext condition names only the bucket ARN starts refusing writes as soon as the bucket key is switched off.
Emulator: none of that happens. PutObject still succeeds 60 s after kms:GenerateDataKey was denied; GetObject still returns the plaintext 90-120 s after DisableKey; and PutObject never starts being refused after BucketKeyEnabled is set to false (the check saw only a successful ETag). The emulator stores the SSE-KMS headers and the key id as metadata and encrypts nothing with that key, so the key's state, its policy and the encryption context are all unobservable.
Why: a client's entire KMS-based control plane over its S3 data is fiction: disabling or scheduling deletion of a CMK does not make data inaccessible, revoking kms:GenerateDataKey does not stop writes, and an encryption-context-scoped key policy does not scope anything. Any test of crypto-shredding, key compromise response, or bucket-key cost/semantics gets a false pass.
Fix: the S3 object write/read path's KMS integration — call GenerateDataKey on write (with the encryption context chosen by BucketKeyEnabled: bucket ARN when true, object ARN when false) and Decrypt on read, and propagate KMS's refusals (AccessDenied, DisabledException) as the operation's error.
Traces: kms__s3.kmsenvelope-deny-of-generate-data-key-blocks-the-default-encrypted-write-while-earlier-objects-stay-readable, kms__s3.sep-kms-key-scope-sse-kms-object-unreadable-while-its-key-is-disabled-rejected, kms__s3.bucket-key-switches-the-kms-encryption-context-from-object-arn-to-bucket-arn.

## Evidence
(a) 'PutObject still succeeded 60s after kms:GenerateDataKey was denied on the bucket's default CMK — S3 is not minting a data key per write, so the default encryption is a stored setting rather than cryptography'.
(b) 'GetObject of key A's object refused after DisableKey not observed within 90s; last ("OK", "")'.
(c) 'timed out after 120s waiting for PutObject to start being refused after BucketKeyEnabled=false — S3 must switch to sending the OBJECT ARN as the encryption context, which the key policy denies'.

## Steps to Reproduce
1) (a) Set a bucket default of aws:kms with a CMK, write an object, deny kms:GenerateDataKey to the writer in the key policy, then PutObject again and re-read the earlier object.
2) (b) Write an SSE-KMS object, DisableKey, then GetObject; EnableKey and GetObject again.
3) (c) Give the CMK a policy whose Allow is conditioned on kms:EncryptionContext:aws:s3:arn equal to the bucket ARN; write with BucketKeyEnabled=true (succeeds), then set BucketKeyEnabled=false and write again.
