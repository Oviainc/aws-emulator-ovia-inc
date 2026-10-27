Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.cf-keyvaluestore-put-key-visible-to-function-at-edge
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|DescribeKeyValueStore|AccessDenied
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# REOPENS: cloudfront-keyvaluestore DescribeKeyValueStore is refused with an unmodeled AccessDenied ("Sign the request with the emulator credentials")

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKeyValueStore

## Description
REOPENS 20261023T150809Z-cloudfront-keyvaluestore-data-plane-unroutable-every-read-answers.md: its fix left the call refused.

This issue carries ONE cause.

Cause 1 of 1: DescribeKeyValueStore on a store the CloudFront control plane just created is refused instead of answered.

AWS: Smithy (com.amazonaws.cloudfrontkeyvaluestore#CloudFrontKeyValueStore, restJson1): DescribeKeyValueStore -> GET /key-value-stores/{KvsARN}, http code 200; the output requires ItemCount, TotalSizeInBytes, KvsARN and Created, with optional LastModified and Status, plus the ETag http header that PutKey's required If-Match is built from. The ONLY errors the operation models are AccessDeniedException (403), ConflictException (409), InternalServerException (500) and ResourceNotFoundException (404) — the message of each is the service's own text about the resource and the caller's permissions. `AccessDenied` is not a member of that union at all, so it matches no variant of the generated SDK error type.

Emulator: the call is refused with Error.Code `AccessDenied` and the message "Sign the request with the emulator credentials" (recorded behavior only: the replay harness reports the botocore code and message, not the HTTP status, so the status is unconfirmed here; the fixed issue named above recorded this same surface as HTTP 404 `UnknownService` before its fix). The message is wholly emulator-authored — it has no variable parts taken from the request or from state, and it names the emulator rather than the resource or the principal, so no AWS error message has this shape. None of ItemCount/TotalSizeInBytes/KvsARN/Created/Status and no ETag header is produced.

Why: a client that has just created a key value store through the CloudFront control plane cannot discover that the store is READY, cannot read its item count or size, and never obtains the ETag that every conditional PutKey needs, so the whole KeyValueStore data plane stays unusable with correctly signed SDK requests. Worse than the previous 404, an `AccessDenied` reads to a caller as "your credentials/policy are wrong", sending them to fix an IAM problem that does not exist. The check that exercises this — a CloudFront Function at the edge reading its associated store — aborts in setup, so the function/KVS association contract is never reached.

Fix: the emulator front door's sigv4/authentication middleware for the cloudfrontkeyvaluestore service (services/service-com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore), which must accept the same credentials every other service on this endpoint accepts and dispatch the request rather than rejecting it, and then the DescribeKeyValueStore handler plus the store-lifecycle state it reads (ItemCount/TotalSizeInBytes/Created/LastModified/Status and the ETag header).

Traces: cloudfront.cf-keyvaluestore-put-key-visible-to-function-at-edge.

## Evidence
Failing check cloudfront.cf-keyvaluestore-put-key-visible-to-function-at-edge [error]:

  botocore.exceptions.ClientError: An error occurred (AccessDenied) when calling the DescribeKeyValueStore operation: Sign the request with the emulator credentials

Expected (AWS): HTTP 200, content-type application/json, body carrying ItemCount, TotalSizeInBytes, KvsARN, Created (and Status, which transitions to READY), plus an ETag response header. If the call were genuinely unauthorized, AWS's code would be the modeled `AccessDeniedException`, never `AccessDenied`.

Actual (emulator): refusal with Error.Code `AccessDenied`, Error.Message "Sign the request with the emulator credentials" (the whole message is fixed emulator-authored text; it interpolates nothing from the request or from state). The check never gets past its setup poll for Status == READY.

Prior art: 20261023T150809Z-cloudfront-keyvaluestore-data-plane-unroutable-every-read-answers.md is marked fixed and claimed exactly this operation (its Cause 1/2/3 are DescribeKeyValueStore); the open issues 20261024T174507Z-cloudfront-keyvaluestore-data-plane-is-unreachable-every-describekeyvaluestore.md and 20261023T150809Z-cloudfront-keyvaluestore-keyvaluestore-refusals-answer-unknownservice-instead-of-resourcenotfoundexception-and.md describe the pre-fix 404 UnknownService envelope, not this AccessDenied refusal.

## Steps to Reproduce
1. Create a key value store through the CloudFront control plane: cloudfront CreateKeyValueStore(Name=<name>), take KeyValueStore.ARN.
2. With ordinary emulator credentials, call cloudfront-keyvaluestore DescribeKeyValueStore(KvsARN=<arn>).
3. Expect HTTP 200 with ItemCount/TotalSizeInBytes/KvsARN/Created/Status and an ETag header (content-type application/json).
4. Observe the call refused with Error.Code `AccessDenied` and message "Sign the request with the emulator credentials".
5. Re-replay cloudfront.cf-keyvaluestore-put-key-visible-to-function-at-edge to confirm the fix.
