Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.cli-s3-select-vs-athena-query-comparison
Issue-Key: athena__glue__s3.cli-s3-select-vs-athena-query-comparison|run#12|AssertionError|assert select_error == (
Issue-Commit: 94ad28198b2e505ea0de135561be7aa2e71a7bc3

# S3 SelectObjectContent is not routed at all: the ?select&select-type=2 POST falls through to the generic router fallback (NotImplemented + router message) instead of AWS's 405/MethodNotAllowed refusal or a 200 event stream (1 cause)

> _Note: this issue was identified against the code base at commit `94ad28198b2e505ea0de135561be7aa2e71a7bc3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SelectObjectContent

## Description
One root cause, grounded in one failing behavioral check of suite simparity-behavioral-ga-prod-v1.

Related open issues that do NOT cover this failure (the replay suite did not mark this check as OWNED by either, and neither describes the router fallthrough or the required message text): 20261025T031335Z-s3-selectobjectcontent-answers-notimplemented-instead-of-the-recorded.md (describes the SelectObjectContent *handler* answering the scaffold-stub message 'operation SelectObjectContent is scaffolded but not implemented in this emulator') and 20261025T032458Z-s3-remaining-divergences-8-root-causes-selectobjectcontent-routing.md. The emulator's observed message has since changed to the generic router fallthrough message, which shows the request is no longer reaching the SelectObjectContent handler at all; and the check now asserts the error MESSAGE as well as the code, which neither issue states as a requirement.

Cause 1 of 1 - the SelectObjectContent request is not dispatched to the operation, and the refusal it does get is neither of AWS's two outcomes.

AWS: Smithy com.amazonaws.s3#SelectObjectContent - http { method: POST, uri: "/{Bucket}/{Key+}?select&select-type=2", code: 200 }; required input members Bucket (@httpLabel), Key (@httpLabel, @length min 1), Expression, ExpressionType (enum), InputSerialization, OutputSerialization; output member Payload targets the streaming event stream SelectObjectContentEventStream (@httpPayload, @streaming); errors: [] - the operation models no errors of its own. So the modeled outcome is HTTP 200 with an event stream carrying the selected records. Recorded behavior only: AWS discontinued S3 Select for accounts that had not enabled it before the cutoff, and such an account answers every SelectObjectContent request with HTTP 405 and error code MethodNotAllowed whose message is the fixed text 'The specified method is not allowed against this resource.' (that message is entirely fixed text for this error class - it interpolates nothing from the request or from state). Either outcome is AWS-faithful: 200 + event stream, or 405/MethodNotAllowed with that exact message.

Emulator: the request is refused with error code 'NotImplemented' and the message 'operation POST /<bucket>/<key> is not implemented in this emulator' - i.e. the message names an HTTP method and a raw request path rather than an operation, which is the emulator's generic unrouted-request fallback, not an operation-level stub response. The query-string literals ?select&select-type=2 that bind this URI pattern are evidently not part of the router's match, so a POST to /{Bucket}/{Key+} never resolves to SelectObjectContent. Neither the status, the code, nor the message matches AWS. Controls in the same check on the same S3 path all succeed (CreateBucket, PutBucketEncryption with SSEAlgorithm=AES256, PutObject with ServerSideEncryption=AES256, and a GetObject that reads back the same bytes containing '2,bob,silver'), so S3 is not broadly broken - this is specific to the SelectObjectContent route.

Why: a client that probes S3 Select and branches on MethodNotAllowed (405) to fall back to another read path - a plain GetObject, or a query engine - never sees that code, so the fallback never fires and the call surfaces as an unmodeled exception. Worse, because the refusal is the router's generic 'method+path is not implemented' answer rather than an operation-scoped one, a caller cannot distinguish 'this account does not have S3 Select' from 'this emulator does not know this route' from 'this request was malformed'. Any SDK or tool that routes on the ?select subresource (including `aws s3api select-object-content`) is affected identically.

Fix: s3 request routing for the ?select&select-type=2 subresource so a POST /{Bucket}/{Key+}?select&select-type=2 dispatches to the SelectObjectContent operation, plus that operation's handler/serializer, which must then answer either the modeled 200 SelectObjectContentEventStream or the recorded refusal HTTP 405 with Error.Code 'MethodNotAllowed' and Error.Message 'The specified method is not allowed against this resource.' - never the generic NotImplemented router fallback.

Traces: athena__glue__s3.cli-s3-select-vs-athena-query-comparison (the check asserts (code, message) == ('MethodNotAllowed', 'The specified method is not allowed against this resource.') and observed ('NotImplemented', 'operation POST /<bucket>/<key> is not implemented in this emulator')).

Service-attribution note: the leading token of this check's id is the composite 'athena__glue__s3' (the check declares SERVICES = ['athena','glue','s3']), which is not a single service token and cannot be matched by exact equality against the bootstrap sets. The failing call is S3's SelectObjectContent, issued before any Athena or Glue call in the check, so this issue is filed under service 's3', which IS bootstrapped. The later Athena leg of the same check is separately blocked by Athena not being bootstrapped and is NOT part of this issue.

## Evidence
Expected (AWS), either outcome:
  (a) HTTP 200 with a Payload event stream whose Records decode to the selected row ['2','bob','silver']; or
  (b) HTTP 405 with Error.Code == 'MethodNotAllowed' and Error.Message == 'The specified method is not allowed against this resource.' (fixed text; no variable parts).

Actual (emulator): ClientError with Error.Code == 'NotImplemented' and Error.Message == 'operation POST /<bucket>/<key> is not implemented in this emulator'. In the recorded run the variable parts of that message were the generated bucket name and the object key ('/sim2real-s3select-athena-cd13eea2beeb/data/rows.csv'); the fixed part of the emulator's message is 'operation POST ' + path + ' is not implemented in this emulator'. Because the message interpolates the method and path instead of naming the operation, the request never reached the SelectObjectContent handler - it was answered by the generic unrouted-request fallback.

Failing assertion verbatim:
  assert select_error == (
      "MethodNotAllowed",
      "The specified method is not allowed against this resource.",
  ), f"SelectObjectContent refusal (code, message) {select_error!r}"
  AssertionError: SelectObjectContent refusal (code, message) ('NotImplemented', 'operation POST /sim2real-s3select-athena-cd13eea2beeb/data/rows.csv is not implemented in this emulator')

Passing controls in the same check, same emulator, same bucket:
  s3.create_bucket -> OK
  s3.put_bucket_encryption(Rules=[{ApplyServerSideEncryptionByDefault:{SSEAlgorithm:'AES256'}}]) -> OK
  s3.put_object(Key='data/rows.csv', ServerSideEncryption='AES256') -> OK
  s3.get_object(Key='data/rows.csv') -> body contains '2,bob,silver'

Model citation: Smithy com.amazonaws.s3#SelectObjectContent, http {method: POST, uri: '/{Bucket}/{Key+}?select&select-type=2', code: 200}, output Payload -> SelectObjectContentEventStream (@httpPayload @streaming), errors: [].

## Steps to Reproduce
1. Boot the emulator and point botocore (or the aws CLI) at it.
2. s3.create_bucket(Bucket=b) (with CreateBucketConfiguration.LocationConstraint set when the region is not us-east-1).
3. s3.put_bucket_encryption(Bucket=b, ServerSideEncryptionConfiguration={'Rules': [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}}]}).
4. s3.put_object(Bucket=b, Key='data/rows.csv', Body=b'id,name,category\n1,alice,gold\n2,bob,silver\n3,carol,bronze\n', ServerSideEncryption='AES256').
5. s3.get_object(Bucket=b, Key='data/rows.csv') -> succeeds, body contains '2,bob,silver' (positive control: S3 access and the object's bytes are fine).
6. s3.select_object_content(Bucket=b, Key='data/rows.csv', Expression="SELECT s.id, s.name, s.category FROM S3Object s WHERE s.id = '2'", ExpressionType='SQL', InputSerialization={'CSV': {'FileHeaderInfo': 'USE'}}, OutputSerialization={'CSV': {}}) - equivalently `aws s3api select-object-content --bucket b --key data/rows.csv ...`.
7. Observe Error.Code 'NotImplemented' with the message 'operation POST /<bucket>/data/rows.csv is not implemented in this emulator' (a router fallthrough that names the method and path, not the operation). Expect either HTTP 200 with a Payload event stream, or HTTP 405 with Error.Code 'MethodNotAllowed' and Error.Message 'The specified method is not allowed against this resource.'
8. Re-replay athena__glue__s3.cli-s3-select-vs-athena-query-comparison.
