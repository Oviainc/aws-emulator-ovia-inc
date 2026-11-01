Issue-Source: trace-sweep-divergence
Issue-Trace: s3w-object-validation-error-envelopes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3w-object-validation-error-envelopes
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|CreateBucket|4xx->2xx|$
Issue-Key: s3|PutObjectTagging|4xx->2xx|$
Issue-Related: 20261030T053639Z-s3-answers-success-where-aws-refuses-the-request.md
Issue-Commit: d7a2f366149cfae4c7c153e56172220f0cdfc550

# S3 accepts invalid bucket names and reserved aws: tag keys with 200 instead of refusing

> _Note: this issue was identified against the code base at commit `d7a2f366149cfae4c7c153e56172220f0cdfc550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucket

## Description
Two net-new root causes, both a request AWS refuses with HTTP 400 and a flat XML <Error> envelope that the emulator instead accepts with a 200 and an empty body. A fixer must close both.

Cause 1 of 2: CreateBucket accepts a syntactically invalid bucket name.
AWS: a bucket name that violates the S3 bucket naming rules (here: an upper-case letter and an underscore) is refused before the bucket is created, with HTTP 400 and the flat envelope Code 'InvalidBucketName', Message 'The specified bucket is not valid.' plus the BucketName, RequestId and HostId members. Fixed text: the Code string and the message sentence exactly as above; variable parts: BucketName is the name from the request URI, RequestId/HostId are per-response values. Smithy: CreateBucket http {method PUT, uri /{Bucket}, code 200} with Bucket an httpLabel targeting BucketName; the model constrains neither the name nor this refusal, so the 400/InvalidBucketName classification is recorded behavior.
Emulator: answers HTTP 200 with an empty body — the bucket is created under a name AWS cannot host (replay: "status: expected 400, got 200" and "$: empty_body").
Why: a client that relies on S3 to validate names records a bucket that no real account can ever address (virtual-hosted addressing of such a name is impossible), and a test or provisioning path that asserts the refusal silently 'passes' while leaving an unusable bucket behind.
Fix: the CreateBucket handler's bucket-name validation, which must reject non-conforming names with the 400 InvalidBucketName envelope before any state is written.
Traces: s3w-object-validation-error-envelopes#task#5, s3w-object-validation-error-envelopes#task#6

Cause 2 of 2: PutObjectTagging accepts a tag key using the reserved 'aws:' prefix.
AWS: a TagSet entry whose Key begins with the reserved prefix 'aws:' is refused with HTTP 400 and the flat envelope Code 'InvalidTag', Message 'Your TagKey cannot be prefixed with aws:' plus the TagKey, RequestId and HostId members. Fixed text: the Code string and that message sentence; variable parts: TagKey is the offending key from the request body, RequestId/HostId are per-response values. Smithy: PutObjectTagging http {method PUT, uri /{Bucket}/{Key+}?tagging, code 200}, Tagging is the required httpPayload; the model declares no operation errors, so the 400/InvalidTag classification is recorded behavior.
Emulator: answers HTTP 200 with an empty body and stores the reserved tag (replay: "status: expected 400, got 200" and "$: empty_body").
Why: 'aws:'-prefixed keys are reserved for AWS-generated tags; accepting them lets a caller shadow system tags, and any client asserting the documented refusal sees success. The stored tag then reappears on GetObjectTagging as if AWS had allowed it.
Fix: the PutObjectTagging request validation (the shared tag-set validator), which must refuse a reserved-prefix key with the 400 InvalidTag envelope.
Traces: s3w-object-validation-error-envelopes#task#11

The open issue 20261030T053639Z-s3-answers-success-where-aws-refuses-the-request.md covers other success-instead-of-refusal steps (e.g. PutBucketReplication, PutObject storage class) but does not carry either replay key above, so neither of these causes is exercised by it.

## Evidence
Cause 1 — s3w-object-validation-error-envelopes#task#5 (also task#6), CreateBucket: expected 400 with body {'Error': {'Code': 'InvalidBucketName', 'Message': 'The specified bucket is not valid.', 'BucketName': '<name from request>', 'RequestId': ..., 'HostId': ...}}; actual 200 with an empty body. divergence_key service=s3|CreateBucket|status=400->200|keys=$,status.
Cause 2 — s3w-object-validation-error-envelopes#task#11, PutObjectTagging: expected 400 with body {'Error': {'Code': 'InvalidTag', 'Message': 'Your TagKey cannot be prefixed with aws:', 'TagKey': 'aws:reserved', 'RequestId': ..., 'HostId': ...}}; actual 200 with an empty body. divergence_key service=s3|PutObjectTagging|status=400->200|keys=$,status.
Smithy (com.amazonaws.s3#AmazonS3): CreateBucket errors are only BucketAlreadyExists/BucketAlreadyOwnedByYou (409); PutObjectTagging models no errors — both refusals are recorded behavior.

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus) and inspect fixture s3w-object-validation-error-envelopes:
1. step task#5 (and task#6) — PUT /{Bucket} with a bucket name containing an upper-case letter and an underscore. Expect HTTP 400 with the InvalidBucketName <Error> envelope; the emulator answers 200 with no body and creates the bucket.
2. step task#11 — PUT /{Bucket}/{Key}?tagging with a Tagging payload whose TagSet contains Key 'aws:reserved'. Expect HTTP 400 with the InvalidTag <Error> envelope naming TagKey; the emulator answers 200 with no body and stores the tag.
