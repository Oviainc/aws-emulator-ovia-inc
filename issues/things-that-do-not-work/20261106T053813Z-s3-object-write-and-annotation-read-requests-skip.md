Issue-Source: trace-sweep-divergence
Issue-Trace: s3-sseinv-kms-key-reference-forms-echo-verbatim-and-dangling-key
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-sseinv-kms-key-reference-forms-echo-verbatim-and-dangling-key s3w-object-validation-error-envelopes s3w-object-annotation-validation-errors
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObject|4xx->2xx|$.Error
Issue-Key: s3|PutObject|4xx->2xx|status
Issue-Key: s3|ListObjectAnnotations|4xx->2xx|$.Error
Issue-Key: s3|ListObjectAnnotations|4xx->2xx|status
Issue-Commit: 4e248580a2e5ff0df0d7326e549cf11eb17f0a22

# s3 object-write and annotation-read requests skip reference validation and answer 200 instead of the modeled 400 refusal

> _Note: this issue was identified against the code base at commit `4e248580a2e5ff0df0d7326e549cf11eb17f0a22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject,ListObjectAnnotations

## Description
Two net-new s3 causes share one fault: a request whose referenced/illegal input AWS refuses up front is instead executed and answered 200 OK with a success body, so the modeled 400 refusal envelope never reaches the client. This issue bundles 2 causes; BOTH must be fixed before it is closed.

Cause 1 of 2 - PutObject with an SSE-KMS key that does not exist is accepted.
AWS: a PutObject that asks for server-side encryption with a customer KMS key (x-amz-server-side-encryption: aws:kms plus x-amz-server-side-encryption-aws-kms-key-id) is resolved against KMS before the object is stored. When the named key does not exist, S3 relays KMS's failure and refuses the write with HTTP 400 and error code 'KMS.NotFoundException'; the message's fixed text is "Key '" + <the key identifier exactly as supplied in the request> + "' does not exist" - the only variable part is that key identifier, taken from the request, and the envelope also carries RequestId/HostId. Smithy: PutObject.SSEKMSKeyId (member of PutObjectRequest, httpHeader x-amz-server-side-encryption-aws-kms-key-id, target com.amazonaws.s3#SSEKMSKeyId, sensitive string); the s3 model binds no dedicated error for this relay, so the status/code/message above are 'recorded behavior only', from the recorded response of this step.
Emulator: the dangling-key write returns HTTP 200 with a normal PutObject success response and no Error element at all - the key reference is never resolved against KMS state, so the object is created under a key that does not exist.
Why: a client that relies on S3 refusing an unencryptable write believes its data was written encrypted under a real CMK. Error handling keyed on KMS.NotFoundException (key typo, deleted key, wrong-account key) never fires, and the bucket silently accumulates objects whose encryption reference cannot be resolved on read.
Fix: the PutObject handler's SSE-KMS validation - resolve the requested key id/ARN against KMS state before accepting the body and relay the not-found failure as the 400 KMS.NotFoundException envelope (code, message template, RequestId/HostId) instead of storing the object.
Traces: s3-sseinv-kms-key-reference-forms-echo-verbatim-and-dangling-key#task#8 (representative), s3w-object-validation-error-envelopes#task#9 (same cluster).
Related open issues that do NOT cover this step (neither carries its replay key): 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md and 20261103T024024Z-kms-s3-s3-s-sse-kms-default-encryption.md.

Cause 2 of 2 - ListObjectAnnotations accepts an invalid annotation-prefix.
AWS: ListObjectAnnotations validates the annotation-prefix query parameter and refuses an invalid one with the modeled InvalidPrefix error - HTTP 400, code 'InvalidPrefix', message 'The annotation prefix you have provided is invalid.' That message is entirely fixed text with no variable part; the envelope also carries RequestId/HostId. Smithy: ListObjectAnnotations (GET /{Bucket}/{Key+}?annotation) binds errors InvalidPrefix (client, httpError 400), NoSuchBucket, NoSuchKey; shape com.amazonaws.s3#InvalidPrefix is documented 'The annotation prefix you provided is invalid.'; input member AnnotationPrefix -> httpQuery 'annotation-prefix'.
Emulator: the same request answers HTTP 200 with a ListObjectAnnotationsOutput body and no Error element - annotation-prefix is passed through to the listing without any validity check, so the modeled InvalidPrefix error is unreachable.
Why: a client cannot distinguish a malformed prefix from a prefix that legitimately matches nothing: it reads an empty/unfiltered annotation list as truth instead of being told its selector is invalid, so prefix bugs pass silently and annotation-driven logic runs on the wrong result set.
Fix: the ListObjectAnnotations handler's input validation - check the annotation-prefix selector and return the modeled InvalidPrefix (400) envelope with AWS's exact message before performing the listing.
Traces: s3w-object-annotation-validation-errors#task#6.
Related open issue that does NOT cover this step: 20261030T053639Z-s3-refusals-use-the-wrong-error-code-and.md (covers this fixture's max-annotation-results message, task#7/#8, not the unvalidated prefix).

## Evidence
Cause 1 - fixture s3-sseinv-kms-key-reference-forms-echo-verbatim-and-dangling-key, step task#8, PutObject.
  Expected: status 400, body $.Error = {Code: 'KMS.NotFoundException', Message: "Key '<requested-kms-key-arn>' does not exist", RequestId: <shaped>, HostId: <shaped>}.
  Actual:   status 200, $.Error missing (no error envelope; write succeeded).
  Replay detail: "status: expected 400, got 200 / $.Error: missing (expected {'Code': 'KMS.NotFoundException', 'Message': \"Key 'arn:aws:kms:us-west-2:000000000000:key/4f5fb9d9-3e64-4ee1-a07a-0cf0bcfee9f9' does not exist\"...}, got None)". The arn/uuid, RequestId and HostId in that line are this recording's values filling the template, not requirements; the requirements are status 400, code KMS.NotFoundException, and the message's fixed text "Key '...' does not exist" around the request's own key identifier.
  Cluster also covers: s3w-object-validation-error-envelopes#task#9.

Cause 2 - fixture s3w-object-annotation-validation-errors, step task#6, ListObjectAnnotations.
  Expected: status 400, body $.Error = {Code: 'InvalidPrefix', Message: 'The annotation prefix you have provided is invalid.', RequestId: <shaped>, HostId: <shaped>}.
  Actual:   status 200, $.Error missing (listing returned normally).
  Replay detail: "status: expected 400, got 200 / $.Error: missing (expected {'Code': 'InvalidPrefix', 'Message': 'The annotation prefix you have provided is invalid.', ...}, got None)". The recorded RequestId/HostId are per-run values, not requirements; the code and the full message text are.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture s3-sseinv-kms-key-reference-forms-echo-verbatim-and-dangling-key up to step task#8 - i.e. PutObject to the fixture's bucket with x-amz-server-side-encryption: aws:kms and x-amz-server-side-encryption-aws-kms-key-id set to a syntactically valid KMS key ARN for which no key exists. Observe 200 + success body; AWS answers 400 with code KMS.NotFoundException and message "Key '<that arn>' does not exist". Same divergence at s3w-object-validation-error-envelopes#task#9.
3. Cause 2: replay fixture s3w-object-annotation-validation-errors up to step task#6 - GET /{Bucket}/{Key}?annotation&annotation-prefix=<invalid prefix>. Observe 200 + ListObjectAnnotationsOutput; AWS answers 400 with code InvalidPrefix and message 'The annotation prefix you have provided is invalid.'
