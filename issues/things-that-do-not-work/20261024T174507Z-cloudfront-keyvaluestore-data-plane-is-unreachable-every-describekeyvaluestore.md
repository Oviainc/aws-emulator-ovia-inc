Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront-keyvaluestore.delete-key-denied-by-iam-policy-rejected cloudfront-keyvaluestore.describe-key-value-store-created-stable-last-modified-advances cloudfront-keyvaluestore.kvs-deleted-store-data-plane-rejected cloudfront-keyvaluestore.put-key-denied-by-iam-policy-rejected cloudfront-keyvaluestore.store-provisions-to-ready-status cloudfront-keyvaluestore.update-keys-multiple-puts-single-etag-bump
Issue-Shape-Id: com.amazonaws.cloudfrontkeyvaluestore#CloudFrontKeyValueStore
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|UnknownService
Issue-Commit: d62274a9170c1371c4ddc8e9211094959c6d8c1a

# cloudfront-keyvaluestore data plane is unreachable: every DescribeKeyValueStore answers 404 UnknownService, so all six KVS behavioral checks abort at their first data-plane call

> _Note: this issue was identified against the code base at commit `d62274a9170c1371c4ddc8e9211094959c6d8c1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKeyValueStore

## Description
This issue bundles ONE cause and that one cause must be closed before submitting. All six cloudfront-keyvaluestore behavioral checks in this suite fail, every one of them on the same single divergence at the first call it makes to the CloudFront KeyValueStore data plane; nothing downstream of that call was ever exercised, so no further cause can be claimed from this run.

Cause 1 of 1: a request addressed to the cloudfront-keyvaluestore (CloudFront KeyValueStore) API is never dispatched to a service by the emulator's front door — it is refused with the unmodeled error code UnknownService and the message "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)". The failing call in each check is the data-plane DescribeKeyValueStore(KvsARN=<arn>) that the check uses to read the store's readiness/metadata and to obtain the ETag that PutKey/UpdateKeys/DeleteKey require for If-Match. In the same checks CreateKeyValueStore on the cloudfront control plane succeeds and returns the store ARN, so the refusal is specific to the KeyValueStore endpoint, not to the caller's credentials.
AWS: Smithy (services/service-com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore): service CloudFrontKeyValueStore, protocol restJson1, authScheme sigv4; DescribeKeyValueStore -> GET /key-value-stores/{KvsARN}, http code 200, output requires ItemCount, TotalSizeInBytes, KvsARN, Created and the ETag httpHeader, with optional LastModified/Status/FailureReason. Its only modeled errors are AccessDeniedException(403), ConflictException(409), InternalServerException(500) and ResourceNotFoundException(404) (the service-common error set); UnknownService is modeled nowhere in this service. The sibling operations the checks go on to use are modeled the same way: UpdateKeys -> POST /key-value-stores/{KvsARN}/keys with required If-Match header and required ItemCount/TotalSizeInBytes/ETag output, DeleteKey -> DELETE /key-value-stores/{KvsARN}/keys/{Key} with required If-Match, GetKey/ListKeys -> GET under the same prefix.
Emulator: HTTP 404 with error code UnknownService for the DescribeKeyValueStore call (botocore surfaces "An error occurred (UnknownService) when calling the DescribeKeyValueStore operation: could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"). None of ItemCount, TotalSizeInBytes, KvsARN, Created or Status is returned and no ETag header is emitted.
Why: a client cannot read a store it just created, cannot learn that it has reached READY, and never obtains the ETag that every conditional write (PutKey/UpdateKeys/DeleteKey If-Match) is built from — so the entire key-value-store data plane is unusable, including the CloudFront-Function-at-edge workflows that depend on it. The returned code matches no member of the generated SDK error union, so it is treated as an infrastructure/server fault and retried, and the 404 status misroutes callers into not-found handling. Negative-path behavior cannot be reached either: the two IAM-denial checks never get as far as the DeleteKey/PutKey they mean to see denied, and the deleted-store check never gets to the refusal it asserts.
Fix: the emulator front door's request routing / endpoint resolution for the cloudfrontkeyvaluestore service crate (services/service-com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore) — it must resolve this API from the request's sigv4 credential scope and its /key-value-stores/{KvsARN} path — plus the endpoint the emulator announces for that service (the SDK derives an account-prefixed host from the KVS ARN); then the DescribeKeyValueStore handler, its restJson1 serialization (including the ETag response header) and the store state/lifecycle it reads.
Related open issues that describe the same unroutable data plane but do NOT carry these behavioral replay keys: 20261023T140640Z-cloudfront-9-fidelity-divergences-19-control-plane-operations.md (its Cause 5, keyed to cloudfront.* checks), 20261023T150809Z-cloudfront-keyvaluestore-data-plane-unroutable-every-read-answers.md and 20261023T150809Z-cloudfront-keyvaluestore-keyvaluestore-refusals-answer-unknownservice-instead-of-resourcenotfoundexception-and.md (both wire-corpus, keyed to fixture steps only).
Traces: cloudfront-keyvaluestore.store-provisions-to-ready-status, cloudfront-keyvaluestore.describe-key-value-store-created-stable-last-modified-advances, cloudfront-keyvaluestore.update-keys-multiple-puts-single-etag-bump, cloudfront-keyvaluestore.kvs-deleted-store-data-plane-rejected, cloudfront-keyvaluestore.put-key-denied-by-iam-policy-rejected, cloudfront-keyvaluestore.delete-key-denied-by-iam-policy-rejected.

## Evidence
All six checks error identically, on the operation DescribeKeyValueStore:

  botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the DescribeKeyValueStore operation: could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)

Expected (per the Smithy model): HTTP 200, a restJson1 body carrying the required ItemCount, TotalSizeInBytes, KvsARN and Created members (plus optional LastModified/Status), and an ETag response header. Actual: HTTP 404 with error code UnknownService and no modeled member at all.

What each check could therefore never reach:
- store-provisions-to-ready-status: asserts a fresh READY store reports ItemCount == 0 and TotalSizeInBytes == 0.
- describe-key-value-store-created-stable-last-modified-advances: asserts Created is stable across writes while LastModified advances.
- update-keys-multiple-puts-single-etag-bump: asserts a single UpdateKeys with 5 Puts yields ItemCount 5, one ETag bump, and ListKeys/GetKey read-back.
- kvs-deleted-store-data-plane-rejected: asserts the data plane refuses calls against a deleted store.
- put-key-denied-by-iam-policy-rejected / delete-key-denied-by-iam-policy-rejected: assert AccessDeniedException for an identity with an explicit Deny, with the key surviving.
In every case the failure happens before the asserted behavior, on the DescribeKeyValueStore used to obtain the store's ETag/readiness. CreateKeyValueStore on the cloudfront control plane succeeded in the same checks (the ARN was returned), so only the KeyValueStore endpoint is unroutable.

## Steps to Reproduce
1. Create a key value store on the CloudFront control plane: cloudfront CreateKeyValueStore(Name=<unique>, Comment='...') and take KeyValueStore.ARN.
2. With a cloudfront-keyvaluestore client, call DescribeKeyValueStore(KvsARN=<arn>).
   Expect: HTTP 200, content-type application/json, body with ItemCount/TotalSizeInBytes/KvsARN/Created (and Status transitioning to READY), plus an ETag response header.
   Observe: HTTP 404 with error code UnknownService and the message 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'.
3. Any follow-on data-plane call is unreachable for the same reason: PutKey(KvsARN=<arn>, Key='k', Value='v', IfMatch=<etag from step 2>), UpdateKeys(KvsARN=<arn>, IfMatch=<etag>, Puts=[...]), GetKey, ListKeys, DeleteKey.
4. Re-replay the six checks listed in trace_ids to confirm the fix.
