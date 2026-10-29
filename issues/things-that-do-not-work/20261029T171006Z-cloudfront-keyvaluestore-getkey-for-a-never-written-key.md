Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront-keyvaluestore.get-key-nonexistent-key-rejected
Issue-Shape-Id: com.amazonaws.cloudfrontkeyvaluestore#CloudFrontKeyValueStore
Issue-Key: cloudfront-keyvaluestore.get-key-nonexistent-key-rejected|run#5|AssertionError|assert err == ("ResourceNotFoundException", "No such Key exists"), (
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudfront-keyvaluestore GetKey for a never-written key is refused at auth with unmodeled 403 AccessDenied "Sign the request with the emulator credentials" instead of 404 ResourceNotFoundException "No such Key exists"

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetKey

## Description
REOPENS 20261023T150809Z-cloudfront-keyvaluestore-data-plane-unroutable-every-read-answers.md: its fix left the call refused. That (now fixed) issue claimed the CloudFront KeyValueStore data plane reads — DescribeKeyValueStore, GetKey (its Cause 4) and ListKeys — would answer their modeled restJson1 results instead of being refused by the front door with 404 UnknownService. The routing refusal is gone, but GetKey is still refused before the operation runs: it now answers HTTP 403 with the error code `AccessDenied`. This is not described as a new defect — the same operation that issue claimed to fix is still not answerable.

This issue bundles ONE cause, and that one cause must be closed before submitting. The emulator never reached GetKey's not-found path in this run, so no second cause can be claimed from it.

Cause 1 of 1: GetKey against an existing, READY key-value store for a key that was never written is rejected by the emulator's authentication/credential layer with HTTP 403, error code `AccessDenied` and the message `Sign the request with the emulator credentials`, instead of being authenticated, executed, and answered with the service's modeled not-found refusal. The call is made with the harness's own unrestricted emulator credentials — the same credentials that the CloudFront control plane accepts for CreateKeyValueStore and DescribeKeyValueStore (control plane) earlier in the very same check. Failing check: cloudfront-keyvaluestore.get-key-nonexistent-key-rejected.

AWS: Smithy (services/service-com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore) — service CloudFrontKeyValueStore, protocol restJson1, sigv4Name `cloudfront-keyvaluestore`, authSchemes [sigv4] with the endpoint ruleset overriding to sigv4a. GetKey: GET /key-value-stores/{KvsARN}/keys/{Key}, success code 200, readonly; input requires the httpLabel members KvsARN (@length 1..2048) and Key (@length 1..1024); output requires Key, Value (sensitive), ItemCount and TotalSizeInBytes. Its modeled errors are exactly AccessDeniedException (403), ConflictException (409), InternalServerException (500) and ResourceNotFoundException (404). For a key that is absent from an existing store, AWS answers the modeled ResourceNotFoundException with HTTP 404 and the message `No such Key exists` — that message is entirely fixed text with no variable part (recorded behavior: the behavioral check asserts exactly ('ResourceNotFoundException', 'No such Key exists')). The bare code `AccessDenied` is modeled nowhere in this service: its only client-side 403 is the modeled shape `AccessDeniedException`, whose message is the IAM authorization sentence — fixed text `User: `, ` is not authorized to perform: `, ` on resource: `, with the caller's principal ARN, the action (`cloudfront-keyvaluestore:GetKey`) and the resource ARN taken from the request and the caller's identity, plus ` with an explicit deny in an identity-based policy` when an explicit Deny matched. Where AWS rejects a signature rather than an identity it answers a signature error (e.g. InvalidSignatureException, `The request signature we calculated does not match the signature you provided...`), still never the bare `AccessDenied`. The message `Sign the request with the emulator credentials` is emulator-authored prose AWS emits for no error class. Hypothesis (neither the model nor the recording states the mechanism): because the endpoint ruleset selects sigv4a for this service, the SDK signs with AWS4-ECDSA-P256-SHA256 plus an X-Amz-Region-Set header, and the emulator's credential check only accepts an AWS4-HMAC-SHA256 Authorization header bearing its own access key, so it treats the correctly signed request as unsigned.

Emulator: HTTP 403, error code `AccessDenied`, message `Sign the request with the emulator credentials` (botocore surfaces "An error occurred (AccessDenied) when calling the GetKey operation: Sign the request with the emulator credentials"). No ResourceNotFoundException is raised and, on the positive-control path of the same check, no Key/Value/ItemCount/TotalSizeInBytes is ever returned, because the data-plane call is refused before the handler runs.

Why: a client cannot distinguish "this key does not exist" from "you are not allowed / not signed in". Code-based error handling built on the generated SDK error union for this service never matches `AccessDenied` (it is not a member), so an absent key falls through to a generic client error instead of the ResourceNotFoundException branch a caller would use to create the key or fall back to a default; cache-miss and read-through logic over a KeyValueStore is therefore impossible to exercise. Equally, a genuine authorization denial becomes indistinguishable from an emulator-internal refusal, so a caller cannot tell it must fix a policy. Because the refusal precedes the handler, the whole GetKey surface — including its success path — is unusable.

Fix: the emulator front door's SigV4 authentication/credential verification for the cloudfrontkeyvaluestore service crate — it must accept the signature scheme the SDK actually uses for this service (the endpoint ruleset's sigv4a, signing name `cloudfront-keyvaluestore`) for the emulator's own credentials and let the request through, and it must never answer a non-modeled `AccessDenied` code with emulator-authored prose; when it genuinely denies, it must raise the service's modeled AccessDeniedException with AWS's IAM wording. Then the GetKey handler, its key-lookup against store state, and its restJson1 error serialization must answer 404 ResourceNotFoundException with the message `No such Key exists` for an absent key and 200 with the required Key/Value/ItemCount/TotalSizeInBytes members for a present one.

Related: the open issue 20261026T232858Z-cloudfront-keyvaluestore-data-plane-is-refused-at-auth.md describes the same auth refusal on DescribeKeyValueStore and carries only six other behavioral check ids; it does not carry this check's replay key and does not name GetKey's not-found refusal. The open 20261024T174507Z-cloudfront-keyvaluestore-data-plane-is-unreachable-every-describekeyvaluestore.md and 20261023T150809Z-cloudfront-keyvaluestore-keyvaluestore-refusals-answer-unknownservice-instead-of-resourcenotfoundexception-and.md are keyed to the earlier UnknownService routing refusal.

Traces: cloudfront-keyvaluestore.get-key-nonexistent-key-rejected (asserts on the GetKey refusal; 45 further checks in this service abort on the same auth refusal at DescribeKeyValueStore and are already owned by 20261026T232858Z-cloudfront-keyvaluestore-data-plane-is-refused-at-auth.md).

## Evidence
Failing check cloudfront-keyvaluestore.get-key-nonexistent-key-rejected (verdict: mismatch):

  AssertionError: expected ResourceNotFoundException for a never-written key, got ('AccessDenied', 'Sign the request with the emulator credentials')

raised at check_cloudfront_keyvaluestore_get_key_nonexistent_key_rejected.py:23 on
  assert err == ("ResourceNotFoundException", "No such Key exists")

Expected (modeled + recorded): HTTP 404, restJson1 error with code `ResourceNotFoundException` and message `No such Key exists` (all fixed text, no variable part) for GetKey(KvsARN=<arn of a READY store>, Key='never-written').
Actual: HTTP 403, error code `AccessDenied` — modeled nowhere in CloudFrontKeyValueStore, whose only client-side 403 is AccessDeniedException — with the message `Sign the request with the emulator credentials`, which is wholly emulator-authored fixed text with no variable part. AWS's 403 for this API is AccessDeniedException with the IAM sentence "User: <caller principal ARN> is not authorized to perform: cloudfront-keyvaluestore:GetKey on resource: <KvsARN>" (quoted parts fixed; principal ARN, action and resource ARN are values from the request and the emulator's IAM state), plus " with an explicit deny in an identity-based policy" when an explicit Deny matched.

The refusal is not identity-based: in the same check, with the SAME credentials, the CloudFront control plane answered CreateKeyValueStore with the store and its ARN and the readiness poll on DescribeKeyValueStore (control plane) reached READY. Only the cloudfront-keyvaluestore data-plane endpoint refuses.

The positive control of the same check — PutKey(Key='was-written', Value='present', IfMatch=<etag>) then GetKey reading back Value == 'present' — is never reached, so GetKey's success path is unverified as well.

The previously-fixed 20261023T150809Z-cloudfront-keyvaluestore-data-plane-unroutable-every-read-answers.md reported this same GetKey operation answering 404 UnknownService (its Cause 4). The routing symptom is gone; the call is still refused and still never executed — status/code moved from 404/UnknownService to 403/AccessDenied.

## Steps to Reproduce
1. With the emulator's own credentials, create a store on the CloudFront control plane: cloudfront CreateKeyValueStore(Name=<unique>, Comment='...'); take KeyValueStore.ARN. It succeeds.
2. Poll the control plane until the store reports Status READY.
3. With a cloudfront-keyvaluestore (data plane) client using the SAME credentials (botocore signs this service per its endpoint ruleset, i.e. sigv4a), call GetKey(KvsARN=<arn>, Key='never-written').
   Expect: HTTP 404, restJson1 error code `ResourceNotFoundException`, message `No such Key exists`.
   Observe: HTTP 403, error code `AccessDenied`, message `Sign the request with the emulator credentials`.
4. Positive control, also blocked by the same refusal: DescribeKeyValueStore(KvsARN=<arn>) to get the ETag, PutKey(KvsARN=<arn>, Key='was-written', Value='present', IfMatch=<etag>), then GetKey(KvsARN=<arn>, Key='was-written') -> expect HTTP 200 with Key/Value/ItemCount/TotalSizeInBytes and Value == 'present'; observe the same 403 AccessDenied.
5. Re-replay cloudfront-keyvaluestore.get-key-nonexistent-key-rejected to confirm the fix.
