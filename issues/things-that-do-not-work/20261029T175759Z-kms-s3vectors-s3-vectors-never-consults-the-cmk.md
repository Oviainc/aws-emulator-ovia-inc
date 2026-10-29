Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3vectors.disabled-kms-key-refuses-new-vector-buckets-and-indexes-until-re-enabled
Issue-Key: kms__s3vectors.disabled-kms-key-refuses-new-vector-buckets-and-indexes-until-re-enabled|_error#28|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED: {result!r}")
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# kms__s3vectors: S3 Vectors never consults the CMK's key state — CreateIndex and CreateVectorBucket under a DISABLED customer key succeed instead of AccessDeniedException (1 cause)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIndex

## Description
ONE cause.

Cause 1 of 1 — S3 Vectors treats a vector bucket's/index's encryptionConfiguration as a stored flag: creating an index or bucket while the named customer CMK is DISABLED is accepted.
AWS (recorded behavior; the model carries no key-state semantics for s3vectors): once DisableKey has moved the CMK to KeyState Disabled, three calls are refused with AccessDeniedException and each message ends in a per-call ' (kms_request_id=<uuid>)' suffix identifying the KMS call S3 Vectors made on the caller's behalf — the suffix's presence and canonical-UUID form are the requirement, its value is per-run:
  * CreateIndex INHERITING the bucket's encryption -> message 'Insufficient access to perform asynchronous indexing. Ensure that the key used to encrypt the index still exists, is active, and has a resource policy granting access to the S3 service principal 'indexing.s3vectors.amazonaws.com'.' (fixed text),
  * CreateIndex naming the key explicitly -> message '<key ARN> is disabled.' (the key ARN from the request),
  * CreateVectorBucket naming the key -> message '<key ARN> is disabled.'
Nothing is created by a refused call: GetIndex on the refused names answers NotFoundException 'The specified index could not be found' and GetVectorBucket answers NotFoundException 'The specified vector bucket could not be found'. After EnableKey the identical CreateIndex succeeds and the index inherits {'sseType': 'aws:kms', 'kmsKeyArn': <key ARN>}.
Emulator: CreateIndex under the disabled key returns HTTP 200 with an indexArn — no KMS call is made, so the key's state is irrelevant and the resource is created. The positive-control legs passed first (the bucket and index echoed the CMK encryptionConfiguration, and put/query/get vectors worked through it), so the divergence is specifically the absent key-state dependency.
Why: disabling the CMK is the control a customer uses to stop a vector store from being written or indexed. An emulator that keeps creating indexes under a disabled key makes that control look ineffective-but-harmless, hides the AccessDeniedException the client must handle, and leaves resources the refused calls should never have produced.
Fix: the S3 Vectors CreateIndex/CreateVectorBucket handlers — resolve the effective CMK (explicit or inherited) against KMS, refuse AccessDeniedException with the measured message when the key is not usable, and create nothing.
Traces: kms__s3vectors.disabled-kms-key-refuses-new-vector-buckets-and-indexes-until-re-enabled.

## Evidence
"AssertionError: expected the call to be refused, but it SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'indexArn': 'arn:aws:s3vectors:<region>:<account>:bucket/<bucket>/index/<index>'}" at the first post-DisableKey leg (CreateIndex inheriting the bucket's encryption). Immediately before it the check confirmed DescribeKey reports KeyState 'Disabled', and before that: GetVectorBucket and GetIndex both echoed {'sseType': 'aws:kms', 'kmsKeyArn': <key ARN>}, PutVectors/QueryVectors/GetVectors all worked. The expected refusals are code 'AccessDeniedException' with the messages quoted in the description, each closing with ' (kms_request_id=<canonical UUID>)'.

## Steps to Reproduce
CreateKey; CreateVectorBucket with encryptionConfiguration={'sseType':'aws:kms','kmsKeyArn':<key ARN>}; CreateIndex inheriting it; put/query/get vectors (all succeed); DisableKey; then (a) CreateIndex inheriting the bucket's encryption, (b) CreateIndex naming the key explicitly, (c) CreateVectorBucket naming the key — observe HTTP 200 for each instead of AccessDeniedException, and the resources exist afterwards. EnableKey and repeat (a) to see it succeed.
