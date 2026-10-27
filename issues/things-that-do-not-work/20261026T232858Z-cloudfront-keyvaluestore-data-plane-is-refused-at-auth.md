Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront-keyvaluestore.store-provisions-to-ready-status cloudfront-keyvaluestore.describe-key-value-store-created-stable-last-modified-advances cloudfront-keyvaluestore.update-keys-multiple-puts-single-etag-bump cloudfront-keyvaluestore.kvs-deleted-store-data-plane-rejected cloudfront-keyvaluestore.put-key-denied-by-iam-policy-rejected cloudfront-keyvaluestore.delete-key-denied-by-iam-policy-rejected
Issue-Shape-Id: com.amazonaws.cloudfrontkeyvaluestore#CloudFrontKeyValueStore
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|AccessDenied
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudfront-keyvaluestore data plane is refused at auth: DescribeKeyValueStore answers 403 AccessDenied "Sign the request with the emulator credentials", so all six KVS behavioral checks abort at their first data-plane call

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKeyValueStore

## Description
REOPENS 20261023T150809Z-cloudfront-keyvaluestore-data-plane-unroutable-every-read-answers.md: its fix left the call refused. That issue reported every CloudFront KeyValueStore read answering 404 UnknownService and was marked fixed; the request is now dispatched to the service, but it is rejected before the operation runs, with HTTP 403 and the error code AccessDenied. This is not described as a new defect: the same operation that issue claimed to fix is still not answerable.

This issue bundles ONE cause, and that one cause must be closed before submitting. All six cloudfront-keyvaluestore behavioral checks in this suite fail, every one of them on the very first DescribeKeyValueStore the check makes; nothing downstream of that call was ever exercised, so no second cause can be claimed from this run.

Cause 1 of 1: a request to CloudFront KeyValueStore's DescribeKeyValueStore (GET /key-value-stores/{KvsARN}) made with the harness's own, unrestricted emulator credentials is refused by the emulator's authentication layer with HTTP 403, error code `AccessDenied` and the message `Sign the request with the emulator credentials`, instead of being authenticated and executed. Failing checks: cloudfront-keyvaluestore.store-provisions-to-ready-status, cloudfront-keyvaluestore.describe-key-value-store-created-stable-last-modified-advances, cloudfront-keyvaluestore.update-keys-multiple-puts-single-etag-bump, cloudfront-keyvaluestore.kvs-deleted-store-data-plane-rejected, cloudfront-keyvaluestore.put-key-denied-by-iam-policy-rejected, cloudfront-keyvaluestore.delete-key-denied-by-iam-policy-rejected.

AWS: Smithy (services/service-com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore) — service CloudFrontKeyValueStore, apiVersion 2022-07-26, protocol restJson1, sigv4Name `cloudfront-keyvaluestore`, authSchemes [sigv4] with the endpoint ruleset overriding to **sigv4a**. DescribeKeyValueStore: GET /key-value-stores/{KvsARN}, success code 200, output requires ItemCount, TotalSizeInBytes, KvsARN, Created and the `ETag` httpHeader, with optional LastModified/Status/FailureReason. Its modeled errors are exactly the service-common set: AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404). The bare code `AccessDenied` is modeled nowhere in this service, and a correctly signed call by an identity that is allowed the action is answered 200 by AWS, never 403. Where AWS does answer 403 for this API it is the modeled shape `AccessDeniedException`, whose message is the IAM authorization sentence — fixed text `User: `, ` is not authorized to perform: `, ` on resource: `, with the caller's principal ARN, the action (`cloudfront-keyvaluestore:DescribeKeyValueStore`) and the resource ARN taken from the request and the caller's identity, plus the explicit-deny clause ` with an explicit deny in an identity-based policy` when a Deny matched. Where AWS rejects a signature rather than an identity it answers a signature error (e.g. `InvalidSignatureException` / `The request signature we calculated does not match the signature you provided...`), still not `AccessDenied`. The message `Sign the request with the emulator credentials` is emulator-authored text that AWS never emits for any error class. Hypothesis (neither the model nor the recording states the mechanism): because the endpoint ruleset selects sigv4a for this service, the SDK signs with AWS4-ECDSA-P256-SHA256 and an X-Amz-Region-Set header, and the emulator's credential check only accepts an AWS4-HMAC-SHA256 Authorization header bearing its own access key, so it treats the sigv4a-signed request as unsigned.

Emulator: HTTP 403, error code `AccessDenied`, message `Sign the request with the emulator credentials` (botocore surfaces "An error occurred (AccessDenied) when calling the DescribeKeyValueStore operation: Sign the request with the emulator credentials"). No ItemCount, TotalSizeInBytes, KvsARN, Created or Status is returned and no ETag response header is emitted. In the same checks the CloudFront control plane answers: CreateKeyValueStore returns the store with its ARN, so the refusal is specific to the cloudfront-keyvaluestore endpoint and not to the caller's credentials being absent — the identical credentials drive the control-plane calls in the same check.

Why: the entire key-value-store data plane is unusable for a client. It cannot read a store it just created, cannot observe that the store reached READY, and never obtains the ETag that every conditional write (PutKey/UpdateKeys/DeleteKey If-Match, all required members) is built from, so CloudFront-Function-at-edge workflows backed by a KVS cannot be exercised at all. The returned code `AccessDenied` matches no member of the generated SDK error union for this service, so SDK error handling falls through to a generic client error that no retry or recovery path matches, and a caller cannot distinguish a genuine authorization denial (which it would handle by fixing a policy) from the emulator refusing to authenticate it. Negative-path fidelity is unreachable too: the two IAM-denial checks never reach the PutKey/DeleteKey they expect to see denied with the modeled AccessDeniedException wording, and the deleted-store check never reaches the refusal it asserts.

Fix: the emulator front door's SigV4 authentication/credential verification for the cloudfrontkeyvaluestore service crate — it must accept the signature scheme the SDK actually uses for this service (the endpoint ruleset's sigv4a, signing name `cloudfront-keyvaluestore`) for the emulator's own credentials and let the request through; it must never answer a non-modeled `AccessDenied` code with emulator-authored prose, and when it does deny, it must raise the service's modeled AccessDeniedException with AWS's IAM wording. Then the DescribeKeyValueStore handler, its restJson1 serialization (including the required ETag response header) and the store state/lifecycle it reads must answer 200 with the required members.

Related open issues that describe the KVS data plane but do NOT carry these behavioral replay keys and do not cover this auth refusal: 20261024T174507Z-cloudfront-keyvaluestore-data-plane-is-unreachable-every-describekeyvaluestore.md (keyed to the earlier UnknownService refusal), 20261023T150809Z-cloudfront-keyvaluestore-keyvaluestore-refusals-answer-unknownservice-instead-of-resourcenotfoundexception-and.md (wire corpus, keyed to fixture steps), and 20261023T140640Z-cloudfront-9-fidelity-divergences-19-control-plane-operations.md (its KVS cause is keyed to cloudfront.* checks).

Traces: all six check ids listed above; each errors on DescribeKeyValueStore before reaching its assertion.

## Evidence
All six cloudfront-keyvaluestore checks error identically, on operation DescribeKeyValueStore:

  botocore.exceptions.ClientError: An error occurred (AccessDenied) when calling the DescribeKeyValueStore operation: Sign the request with the emulator credentials

Expected (per the Smithy model, for the harness's unrestricted credentials): HTTP 200, content-type application/json, a body carrying the required ItemCount, TotalSizeInBytes, KvsARN and Created members (plus optional LastModified/Status), and an `ETag` response header.
Actual: HTTP 403 with error code `AccessDenied` — a code modeled nowhere in CloudFrontKeyValueStore (its only client-side 403 is AccessDeniedException) — and the message `Sign the request with the emulator credentials`, which is entirely emulator-authored fixed text with no variable part; AWS's 403 for this API is AccessDeniedException whose message is the IAM sentence "User: <caller principal ARN> is not authorized to perform: cloudfront-keyvaluestore:<Action> on resource: <KvsARN>" (fixed text as quoted; principal ARN, action and resource ARN are values from the request and the emulator's IAM state), with " with an explicit deny in an identity-based policy" appended when an explicit Deny matched.

The previously-fixed issue 20261023T150809Z-cloudfront-keyvaluestore-data-plane-unroutable-every-read-answers.md reported this same operation answering 404 UnknownService. The routing symptom is gone, but the call is still refused and still never executed — the status and code changed from 404/UnknownService to 403/AccessDenied.

What each check could therefore never reach:
- store-provisions-to-ready-status: a fresh READY store reports ItemCount == 0 and TotalSizeInBytes == 0 on the data plane.
- describe-key-value-store-created-stable-last-modified-advances: Created stays fixed across writes while LastModified advances.
- update-keys-multiple-puts-single-etag-bump: one UpdateKeys with several Puts yields the new ItemCount, a single ETag bump, and ListKeys/GetKey read-back.
- kvs-deleted-store-data-plane-rejected: the data plane refuses calls against a deleted store.
- put-key-denied-by-iam-policy-rejected / delete-key-denied-by-iam-policy-rejected: an identity with an explicit Deny gets AccessDeniedException with AWS's explicit-deny wording while the key survives, and unrestricted credentials still succeed.
In every case the failure is on the DescribeKeyValueStore used to obtain the store's readiness/ETag, before the asserted behavior. CreateKeyValueStore on the CloudFront control plane succeeded in the same checks with the same credentials.

## Steps to Reproduce
1. With the emulator's own credentials, create a store on the CloudFront control plane: cloudfront CreateKeyValueStore(Name=<unique>, Comment='...') and take KeyValueStore.ARN. It succeeds.
2. With a cloudfront-keyvaluestore client using the SAME credentials (botocore signs this service per its endpoint ruleset, i.e. sigv4a), call DescribeKeyValueStore(KvsARN=<arn>).
   Expect: HTTP 200, content-type application/json, body with ItemCount/TotalSizeInBytes/KvsARN/Created (Status READY once provisioned) and an `ETag` response header.
   Observe: HTTP 403 with error code `AccessDenied` and the message 'Sign the request with the emulator credentials'; no modeled member and no ETag header.
3. Every follow-on data-plane call is unreachable for the same reason, because each needs the ETag from step 2: PutKey(KvsARN, Key, Value, IfMatch=<etag>), UpdateKeys(KvsARN, IfMatch=<etag>, Puts=[...]), GetKey, ListKeys, DeleteKey.
4. Confirm the denial is not identity-based: the same credentials are accepted by the cloudfront control plane in step 1, and no Deny policy exists at this point.
5. Re-replay the six checks listed in trace_ids to confirm the fix.
