Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-vfd-s3-reference-route-and-destination-filters
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-vfd-s3-reference-route-and-destination-filters
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: s3|GetBucketPolicy|2xx->4xx|$.Id
Issue-Related: 20261102T111455Z-s3-bucket-policy-plane-accepts-malformed-policies-and.md
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# S3 GetBucketPolicy answers 404 XML for a bucket whose policy was accepted

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketPolicy

## Description
Cause 1 of 1: a bucket policy written by PutBucketPolicy cannot be read back.
AWS: Smithy: GetBucketPolicy is GET /{Bucket}?policy with http code 200 and output member Policy bound as the httpPayload; PutBucketPolicy is PUT /{Bucket}?policy with required Policy payload. A successful read answers HTTP 200 with the stored policy document as the body and Content-Type 'application/json' — for the recorded request the log-delivery policy with Version '2012-10-17', Id 'AWSLogDeliveryWrite20150319' and two statements (AWSLogDeliveryWrite1 allowing s3:PutObject to delivery.logs.amazonaws.com on <bucket>/AWSLogs/<account>/*, and AWSLogDeliveryAclCheck1 allowing s3:GetBucketAcl on the bucket), each with the recorded aws:SourceAccount / s3:x-amz-acl / aws:SourceArn conditions. The bucket name, account id and the policy body itself come from the request; the status, the JSON content type and the round-trip guarantee are the requirement.
Emulator: HTTP 404 with Content-Type 'application/xml' and no policy body — the read reports that no policy exists on a bucket whose policy was just accepted.
Why: every flow that configures a bucket for log/flow-log delivery writes a policy and then reads it back to confirm; the emulator makes that confirmation impossible, and the downstream VPC flow-log delivery that depends on the policy cannot be validated. The wrong content type additionally breaks clients that decode the 200 body as JSON, since an S3 policy read is the one S3 GET whose success body is JSON rather than XML.
Fix: the bucket-policy state (PutBucketPolicy must persist the document against the bucket) and the GetBucketPolicy handler/serializer, which must answer 200 with the stored document and Content-Type application/json, reserving 404 NoSuchBucketPolicy (XML error body) for buckets with no policy.
Traces: ec2-vfd-s3-reference-route-and-destination-filters#task#10.

## Evidence
Replayed wire corpus simparity-wire-aws-v1, fixture ec2-vfd-s3-reference-route-and-destination-filters step task#10 (service s3, operation GetBucketPolicy): expected status 200 with $.Version '2012-10-17', $.Id 'AWSLogDeliveryWrite20150319', the two-statement $.Statement list and $headers.content-type 'application/json'; actual status 404, no Version/Id/Statement in the body and $headers.content-type 'application/xml'.

## Steps to Reproduce
1. Create the bucket and PUT the AWSLogDeliveryWrite20150319 policy with PutBucketPolicy (earlier steps of fixture ec2-vfd-s3-reference-route-and-destination-filters).
2. Call GET /<bucket>?policy (step task#10).
3. Observe HTTP 404 with Content-Type application/xml instead of HTTP 200 with the stored JSON policy document.
