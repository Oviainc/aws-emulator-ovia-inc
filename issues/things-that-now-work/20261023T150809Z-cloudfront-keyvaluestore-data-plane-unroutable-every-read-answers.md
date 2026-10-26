Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-keyvaluestore-absent-store-error-precedence
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-keyvaluestore-absent-store-error-precedence cloudfront-keyvaluestore-precondition-and-quota-rejections cloudfront-keyvaluestore-put-get-describe-etag-chain cloudfront-keyvaluestore-updatekeys-batch-list-and-delete cloudfront-keyvaluestore-put-key-empty-value-accepted-and-counted
Issue-Shape-Id: com.amazonaws.cloudfrontkeyvaluestore#CloudFrontKeyValueStore
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|$
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|$.Created
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|$.ItemCount
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|$.KvsARN
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|$.LastModified
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|$.Status
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|$.TotalSizeInBytes
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|$headers.content-type
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|$headers.etag
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|2xx->4xx|status
Issue-Key: cloudfront-keyvaluestore|GetKey|2xx->4xx|$.ItemCount
Issue-Key: cloudfront-keyvaluestore|GetKey|2xx->4xx|$.Key
Issue-Key: cloudfront-keyvaluestore|GetKey|2xx->4xx|$.TotalSizeInBytes
Issue-Key: cloudfront-keyvaluestore|GetKey|2xx->4xx|$.Value
Issue-Key: cloudfront-keyvaluestore|GetKey|2xx->4xx|$headers.content-type
Issue-Key: cloudfront-keyvaluestore|GetKey|2xx->4xx|status
Issue-Key: cloudfront-keyvaluestore|ListKeys|2xx->4xx|$.Items
Issue-Key: cloudfront-keyvaluestore|ListKeys|2xx->4xx|$headers.content-type
Issue-Key: cloudfront-keyvaluestore|ListKeys|2xx->4xx|status
Issue-Key: cloudfront-keyvaluestore|ListKeys|2xx->4xx|$.NextToken
Issue-Commit: 326899db14d8647fedf8ea1301a88431f0a1aba6

# CloudFront KeyValueStore data plane unroutable: every read answers 404 UnknownService instead of the modeled restJson1 result

> _Note: this issue was identified against the code base at commit `326899db14d8647fedf8ea1301a88431f0a1aba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKeyValueStore

## Description
No request addressed to the cloudfront-keyvaluestore data plane is dispatched to a service by the emulator's front door: every one is refused with the unmodeled error code `UnknownService` and the body "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)", served with content-type application/x-amz-json-1.1 although the service is restJson1. This issue bundles SIX causes (the six success-path operation surfaces the wire corpus exercises) and all six must be closed before submitting; the matching refusal-path surfaces are filed separately as the KeyValueStore refusals bundle. The open behavioral issue 20261023T140640Z-cloudfront-9-fidelity-divergences-19-control-plane-operations.md describes the same unroutable data plane in its Cause 5, but it carries only behavioral check ids and none of the wire replay keys below.

Cause 1 of 6: DescribeKeyValueStore on a store that exists and has reached READY.
AWS: Smithy: CloudFront KeyValueStore is restJson1; DescribeKeyValueStore -> GET /key-value-stores/{KvsARN}, http code 200, output requires ItemCount, TotalSizeInBytes, KvsARN and Created plus the ETag http header, with optional LastModified/Status; modeled errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404). A restJson1 response body is content-type application/json.
Emulator: 404 with {"__type": "UnknownService", "message": "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} and content-type application/x-amz-json-1.1; none of ItemCount/TotalSizeInBytes/KvsARN/Created/Status is present and no ETag header is emitted, so a poll waiting for Status == READY can never be satisfied.
Why: a client cannot discover that a store it just created is ready, cannot read its item count/size, and never receives the ETag that PutKey's required If-Match is built from, so the whole data plane is unusable; the unmodeled code also matches no member of the generated SDK error union and is retried as a server fault.
Fix: the emulator front door's request routing/endpoint resolution for the cloudfrontkeyvaluestore service (service-com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore), which must resolve this API from its sigv4 credential scope and its /key-value-stores/{KvsARN} path, plus the endpoint it announces for it; then the DescribeKeyValueStore handler and the store state it reads.
Traces: cloudfront-keyvaluestore-absent-store-error-precedence#task#2 (cluster also covers cloudfront-keyvaluestore-precondition-and-quota-rejections#task#2, cloudfront-keyvaluestore-put-get-describe-etag-chain#task#2, cloudfront-keyvaluestore-updatekeys-batch-list-and-delete#task#2).

Cause 2 of 6: DescribeKeyValueStore read back after a key was written.
AWS: Smithy: the same operation; after a PutKey the response reports the updated ItemCount and TotalSizeInBytes, a refreshed LastModified and the store's new ETag header (the recording chains that ETag into the next conditional write).
Emulator: the same 404 UnknownService envelope; ItemCount, TotalSizeInBytes, KvsARN, Created, LastModified, Status and the ETag header are all absent.
Why: a client cannot observe the effect of its writes or obtain the fresh ETag required to continue the optimistic-concurrency chain.
Fix: as cause 1 — routing for the cloudfrontkeyvaluestore crate, then DescribeKeyValueStore's handler/state so metadata reflects written keys.
Traces: cloudfront-keyvaluestore-put-get-describe-etag-chain#task#7.

Cause 3 of 6: DescribeKeyValueStore used as the readiness gate in fixture setup.
AWS: the operation answers 200 and reports Status, which transitions to READY for a newly created store.
Emulator: 404 UnknownService, so the setup poll ends after one attempt with the UnknownService body instead of a READY store.
Why: every scenario that must wait for a store to become usable before writing to it aborts in setup, so the rest of the data-plane surface is never reachable.
Fix: as cause 1; the Status member must be served from the store's lifecycle state.
Traces: cloudfront-keyvaluestore-put-key-empty-value-accepted-and-counted#setup#2.

Cause 4 of 6: GetKey for a key that exists.
AWS: Smithy: GetKey -> GET /key-value-stores/{KvsARN}/keys/{Key}, http code 200, output requires Key, Value (sensitive), ItemCount and TotalSizeInBytes; modeled errors AccessDeniedException(403)/ConflictException(409)/InternalServerException(500)/ResourceNotFoundException(404).
Emulator: 404 UnknownService with content-type application/x-amz-json-1.1; Key, Value, ItemCount and TotalSizeInBytes are all missing.
Why: no stored value can be read back, so a CloudFront Function's key-value reads cannot be set up or verified, and the failure looks like a client/endpoint misconfiguration rather than an API result.
Fix: as cause 1, then the GetKey handler and the key store it reads.
Traces: cloudfront-keyvaluestore-put-get-describe-etag-chain#task#4 (cluster also covers cloudfront-keyvaluestore-put-get-describe-etag-chain#task#6 and cloudfront-keyvaluestore-put-key-empty-value-accepted-and-counted#task#4).

Cause 5 of 6: ListKeys for a store holding keys.
AWS: Smithy: ListKeys -> GET /key-value-stores/{KvsARN}/keys, http code 200, output Items (list of Key/Value pairs) and NextToken; MaxResults defaults to 10.
Emulator: 404 UnknownService; Items is absent and the content-type is application/x-amz-json-1.1.
Why: a client cannot enumerate a store's contents, so reconciliation of what was written is impossible.
Fix: as cause 1, then the ListKeys handler and its listing over key state.
Traces: cloudfront-keyvaluestore-precondition-and-quota-rejections#task#7 (cluster also covers cloudfront-keyvaluestore-updatekeys-batch-list-and-delete#task#4, #task#7 and #task#10).

Cause 6 of 6: ListKeys paginated page (MaxResults smaller than the key count).
AWS: Smithy: ListKeys is paginated with inputToken/outputToken NextToken, pageSize MaxResults, items Items; a truncated page answers 200 with the page's Items and a NextToken for the next call.
Emulator: 404 UnknownService; both Items and NextToken are absent, so pagination cannot start.
Why: a paginating client gets a hard failure instead of a page, and no multi-page enumeration of a store is possible.
Fix: as cause 1, then ListKeys' pagination (token minting and page slicing) in the cloudfrontkeyvaluestore crate.
Traces: cloudfront-keyvaluestore-updatekeys-batch-list-and-delete#task#5.

## Evidence
Cause 1: status expected 200, got 404; $.Created/$.ItemCount/$.KvsARN/$.LastModified/$.Status/$.TotalSizeInBytes all missing; $headers.etag missing; $headers.content-type expected 'application/json', got 'application/x-amz-json-1.1'; "poll ended: 404 UnknownService cannot satisfy until $.Status in ['READY'] after 1 attempt(s)", body {'__type': 'UnknownService', 'message': 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'} (recorded example values: Status 'READY', ItemCount 0, TotalSizeInBytes 0, KvsARN 'arn:aws:cloudfront::000000000000:key-value-store/<uuid>').
Cause 2: status expected 200, got 404; $.ItemCount (recorded 1), $.TotalSizeInBytes (recorded 26), $.Created, $.LastModified, $.Status, $.KvsARN missing; $headers.etag missing.
Cause 3: status expected 200, got 404; poll ended: 404 UnknownService cannot satisfy until $.Status in ['READY'] after 1 attempt(s).
Cause 4: status expected 200, got 404; $.Key, $.Value, $.ItemCount, $.TotalSizeInBytes missing (recorded example Key 'alpha', Value 'alpha-value', ItemCount 1, TotalSizeInBytes 16).
Cause 5: status expected 200, got 404; $.Items missing (recorded example [{'Key': 'alpha', 'Value': 'alpha-value'}]).
Cause 6: status expected 200, got 404; $.Items missing (recorded example [{'Key': 'alpha', 'Value': 'alpha-value'}, {'Key': 'beta', 'Value': 'beta-value'}]) and $.NextToken missing.
Every cause shares the identical actual response: HTTP 404, content-type application/x-amz-json-1.1, __type UnknownService.

## Steps to Reproduce
1. Create a key value store through the cloudfront control plane (CreateKeyValueStore) and take its ARN.
2. Cause 1/3: call cloudfront-keyvaluestore DescribeKeyValueStore(KvsARN=<arn>) and poll for Status READY -> expect 200 with ItemCount/TotalSizeInBytes/KvsARN/Created/Status and an ETag header, content-type application/json; observe 404 UnknownService with content-type application/x-amz-json-1.1.
3. Cause 4: PutKey(KvsARN=<arn>, Key='alpha', Value='alpha-value', IfMatch=<etag>) then GetKey(KvsARN=<arn>, Key='alpha') -> expect 200 with Key/Value/ItemCount/TotalSizeInBytes; observe 404 UnknownService.
4. Cause 2: DescribeKeyValueStore(KvsARN=<arn>) again -> expect 200 with the updated ItemCount/TotalSizeInBytes and a new ETag; observe 404 UnknownService.
5. Cause 5/6: ListKeys(KvsARN=<arn>) and ListKeys(KvsARN=<arn>, MaxResults=<fewer than the stored key count>) -> expect 200 with Items (and NextToken on the truncated page); observe 404 UnknownService.
6. Replay the fixture steps listed under Traces to confirm.
