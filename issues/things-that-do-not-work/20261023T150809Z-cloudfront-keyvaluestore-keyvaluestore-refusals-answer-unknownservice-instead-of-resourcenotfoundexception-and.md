Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-keyvaluestore-absent-store-error-precedence
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-keyvaluestore-absent-store-error-precedence cloudfront-keyvaluestore-put-get-describe-etag-chain cloudfront-keyvaluestore-precondition-and-quota-rejections
Issue-Shape-Id: com.amazonaws.cloudfrontkeyvaluestore#CloudFrontKeyValueStore
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|4xx->4xx|$.Message
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|4xx->4xx|$headers.content-type
Issue-Key: cloudfront-keyvaluestore|DescribeKeyValueStore|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cloudfront-keyvaluestore|GetKey|4xx->4xx|$.Message
Issue-Key: cloudfront-keyvaluestore|GetKey|4xx->4xx|$headers.content-type
Issue-Key: cloudfront-keyvaluestore|GetKey|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cloudfront-keyvaluestore|ListKeys|4xx->4xx|$.Message
Issue-Key: cloudfront-keyvaluestore|ListKeys|4xx->4xx|$headers.content-type
Issue-Key: cloudfront-keyvaluestore|ListKeys|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cloudfront-keyvaluestore|PutKey|4xx->4xx|$.Message
Issue-Key: cloudfront-keyvaluestore|PutKey|4xx->4xx|$headers.content-type
Issue-Key: cloudfront-keyvaluestore|PutKey|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cloudfront-keyvaluestore|PutKey|4xx->4xx|status
Issue-Key: cloudfront-keyvaluestore|ListKeys|4xx->4xx|$.message
Issue-Key: cloudfront-keyvaluestore|ListKeys|4xx->4xx|status
Issue-Commit: 326899db14d8647fedf8ea1301a88431f0a1aba6

# KeyValueStore refusals answer UnknownService instead of ResourceNotFoundException and ValidationException

> _Note: this issue was identified against the code base at commit `326899db14d8647fedf8ea1301a88431f0a1aba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKeyValueStore

## Description
Every refusal the cloudfront-keyvaluestore data plane should produce is replaced by the emulator front door's unmodeled `UnknownService` error: HTTP 404 with x-amzn-errortype: UnknownService, content-type application/x-amz-json-1.1 and no Message member, because the request is never dispatched to the service at all. This issue bundles FIVE causes (the five refusal surfaces the wire corpus exercises) and all five must be closed before submitting; the success-path surfaces of the same unroutable data plane are filed separately as the KeyValueStore data-plane-unroutable bundle. The open behavioral issue 20261023T140640Z-cloudfront-9-fidelity-divergences-19-control-plane-operations.md covers the same unroutable data plane in its Cause 5 but carries no wire replay keys, and it says nothing about which refusal each operation owes.

Cause 1 of 5: DescribeKeyValueStore against a Key-Value-Store ARN that does not exist.
AWS: Smithy: DescribeKeyValueStore models ResourceNotFoundException with httpError 404 and a single Message member; the service is restJson1, so the error body is content-type application/json and the error code travels in x-amzn-errortype. Recorded behavior shows the Message "No such Key-Value-Store exists" (an example of that shape's text).
Emulator: 404 whose x-amzn-errortype is UnknownService, body content-type application/x-amz-json-1.1 and no Message at all — the status coincides, the error identity does not.
Why: a client cannot distinguish 'this store does not exist' from 'the emulator does not know this service'; code switching on ResourceNotFoundException never matches, and an unmodeled code is treated as an infrastructure fault and retried.
Fix: the emulator front door's routing/endpoint resolution for the cloudfrontkeyvaluestore service (service-com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore) so the request reaches the service, plus that crate's restJson1 error serializer and the store lookup in DescribeKeyValueStore.
Traces: cloudfront-keyvaluestore-absent-store-error-precedence#task#3.

Cause 2 of 5: GetKey against an absent store (error precedence: the missing store, not the missing key).
AWS: Smithy: GetKey -> GET /key-value-stores/{KvsARN}/keys/{Key}, modeled errors include ResourceNotFoundException(404); the recording shows the store-level refusal with Message "No such Key-Value-Store exists" taking precedence over any key lookup.
Emulator: 404 UnknownService, x-amz-json-1.1, no Message.
Why: a client reading a key cannot tell that the store itself is gone, so retry/repair logic aimed at re-creating the store never triggers.
Fix: as cause 1, then GetKey's lookup order in the cloudfrontkeyvaluestore crate (resolve the store before the key).
Traces: cloudfront-keyvaluestore-absent-store-error-precedence#task#4 (cluster also covers cloudfront-keyvaluestore-absent-store-error-precedence#task#8 and cloudfront-keyvaluestore-put-get-describe-etag-chain#task#8).

Cause 3 of 5: ListKeys against an absent store.
AWS: Smithy: ListKeys models ResourceNotFoundException(404) (and ValidationException(400)); an unknown store yields the 404 with the store-not-found Message.
Emulator: 404 UnknownService, x-amz-json-1.1, no Message.
Why: an enumeration against a deleted store surfaces as a broken service rather than a missing resource.
Fix: as cause 1, then ListKeys' store resolution in the cloudfrontkeyvaluestore crate.
Traces: cloudfront-keyvaluestore-absent-store-error-precedence#task#5.

Cause 4 of 5: PutKey against an absent store / with an If-Match that cannot be satisfied.
AWS: Smithy: PutKey -> PUT /key-value-stores/{KvsARN}/keys/{Key} with required If-Match header; modeled errors include ValidationException(400), ResourceNotFoundException(404), ServiceQuotaExceededException(402) and ConflictException(409). Recorded behavior: the call is refused with HTTP 400 and Message "Pre-Condition failed during update of Key-Value-Store" (the recorded text is an example of that refusal).
Emulator: HTTP 404 with x-amzn-errortype UnknownService, content-type application/x-amz-json-1.1 and no Message — both the status and the error identity diverge.
Why: a conditional-write client cannot distinguish a precondition failure (retry after re-reading the ETag) from a non-existent resource or a broken service, so the optimistic-concurrency contract is unimplementable.
Fix: as cause 1, then PutKey's If-Match/precondition evaluation and its error mapping in the cloudfrontkeyvaluestore crate.
Traces: cloudfront-keyvaluestore-absent-store-error-precedence#task#6 (cluster also covers cloudfront-keyvaluestore-precondition-and-quota-rejections#task#3).

Cause 5 of 5: ListKeys with MaxResults above the modeled maximum is not validated.
AWS: Smithy: ListKeysRequest.MaxResults is an httpQuery Integer with @range min 1 max 50 (default 10) and ListKeys models ValidationException with httpError 400; because the rule is modeled, the refusal is the standard constraint report naming the member and its bound — recorded: 400 with message "1 validation error detected: Value '51' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50".
Emulator: HTTP 404 with x-amzn-errortype UnknownService and the message "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)", content-type application/x-amz-json-1.1.
Why: an out-of-range page size is accepted-or-rejected indistinguishably from a routing failure, so a client never learns which member it must correct, and the 404 status misroutes it into not-found handling.
Fix: as cause 1, then input validation for ListKeys (the @range constraint on MaxResults) and the ValidationException serialization in the cloudfrontkeyvaluestore crate.
Traces: cloudfront-keyvaluestore-absent-store-error-precedence#task#7.

## Evidence
Cause 1 (DescribeKeyValueStore, absent store): status 404 as expected but $.Message missing (recorded example 'No such Key-Value-Store exists'), $headers.x-amzn-errortype got 'UnknownService', $headers.content-type expected 'application/json', got 'application/x-amz-json-1.1'.
Cause 2 (GetKey, absent store): same three divergences — $.Message missing, x-amzn-errortype 'UnknownService', content-type 'application/x-amz-json-1.1'.
Cause 3 (ListKeys, absent store): same three divergences.
Cause 4 (PutKey, absent store/stale If-Match): status expected 400, got 404; $.Message missing (recorded example 'Pre-Condition failed during update of Key-Value-Store'); x-amzn-errortype 'UnknownService'; content-type 'application/x-amz-json-1.1'.
Cause 5 (ListKeys MaxResults=51): status expected 400, got 404; $.message expected "1 validation error detected: Value '51' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50", got 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'; x-amzn-errortype 'UnknownService'; content-type 'application/x-amz-json-1.1'.

## Steps to Reproduce
1. Pick a Key-Value-Store ARN that was never created (or was deleted), e.g. arn:aws:cloudfront::<account>:key-value-store/<random uuid>.
2. Cause 1: cloudfront-keyvaluestore DescribeKeyValueStore(KvsARN=<absent arn>) -> expect 404 ResourceNotFoundException (x-amzn-errortype ResourceNotFoundException, content-type application/json, Message naming the missing store); observe 404 UnknownService with no Message.
3. Cause 2: GetKey(KvsARN=<absent arn>, Key='alpha') -> expect the store-level 404 ResourceNotFoundException; observe 404 UnknownService.
4. Cause 3: ListKeys(KvsARN=<absent arn>) -> expect 404 ResourceNotFoundException; observe 404 UnknownService.
5. Cause 4: PutKey(KvsARN=<absent arn>, Key='alpha', Value='v', IfMatch=<any etag>) -> expect 400 with the precondition-failure Message; observe 404 UnknownService.
6. Cause 5: ListKeys(KvsARN=<any arn>, MaxResults=51) -> expect 400 ValidationException reporting maxResults against its max of 50; observe 404 UnknownService.
7. Replay the fixture steps listed under Traces to confirm.
