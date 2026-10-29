Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__s3.lambda-reads-sse-kms-object-via-customer-key-arn
Issue-Key: kms__lambda__s3.lambda-reads-sse-kms-object-via-customer-key-arn|run#8|AssertionError|assert head.get("SSEKMSKeyId") == key["arn"], (
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# kms__lambda__s3: HeadObject reports SSEKMSKeyId as a bare key id instead of the resolved KMS key ARN (1 cause)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
HeadObject

## Description
ONE cause.

Related OPEN issues that do NOT carry this check's replay key (named for context only): 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md (alias/cross-region/nonexistent SSEKMSKeyId resolution) and 20261025T181311Z-kms-s3-s3-reports-no-sse-kms-key-arn-on.md (no key ARN at all for a bucket default).

Cause 1 of 1 — S3 echoes the SSE-KMS key identifier in the form the request used instead of the resolved key ARN.
AWS (recorded behavior; S3's model types SSEKMSKeyId as a string and states no form): whatever identifier form a PutObject supplies for SSEKMSKeyId — a bare key id, a key ARN or an alias — S3 resolves it against KMS and reports the FULL key ARN ('arn:aws:kms:<region>:<account>:key/<key id>') in the object's metadata, so x-amz-server-side-encryption-aws-kms-key-id on HeadObject/GetObject and PutObject's own response always names the concrete key.
Emulator: for an object written with SSEKMSKeyId=<bare key id>, HeadObject reports SSEKMSKeyId as that same bare key id ('<key id>') rather than the resolved ARN — the value is stored and echoed verbatim, never resolved.
Why: consumers (including the Lambda in this check, and any inventory/audit tooling) compare the reported SSEKMSKeyId against a key ARN to decide which CMK an object is under. A bare id does not identify the account or region of the key, so the comparison fails and a cross-account/cross-region key cannot be told apart from a local one.
Fix: the S3 SSE-KMS metadata path — resolve the supplied key identifier through KMS when the object is written and persist/report the resolved key ARN in PutObject's response and in HeadObject/GetObject metadata.
Traces: kms__lambda__s3.lambda-reads-sse-kms-object-via-customer-key-arn.

## Evidence
"AssertionError: by-id.txt: S3 must echo the RESOLVED key ARN 'arn:aws:kms:<region>:<account>:key/b1a83f05-156c-4169-ab3e-1ed8c1cb5cb7', got 'b1a83f05-156c-4169-ab3e-1ed8c1cb5cb7'" — the assertion is head.get('SSEKMSKeyId') == key['arn'] for the object written with the bare key id. The region/account/key-id shown are this run's values; the requirement is that the reported value is the full key ARN of the resolved key, whatever identifier form the write used.

## Steps to Reproduce
CreateKey; PutObject(Bucket=<b>, Key='by-id.txt', ServerSideEncryption='aws:kms', SSEKMSKeyId='<bare key id>'); HeadObject(Bucket=<b>, Key='by-id.txt') — observe SSEKMSKeyId equal to the bare key id instead of 'arn:aws:kms:<region>:<account>:key/<key id>'.
