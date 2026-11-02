Issue-Source: trace-sweep-divergence
Issue-Trace: s3-pab-remediation-put-document-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-pab-remediation-put-document-mistakes s3-pab-enforced-bucket-acl-refusal-order s3-oc-put-document-mistakes-then-corrected s3-oc-expected-bucket-owner-guard s3-pab-expected-bucket-owner-guard s3-lcp-traj-expected-bucket-owner-header
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutPublicAccessBlock|4xx->2xx|$
Issue-Key: s3|PutPublicAccessBlock|4xx->2xx|status
Issue-Key: s3|PutPublicAccessBlock|2xx->4xx|status
Issue-Key: s3|GetPublicAccessBlock|2xx->2xx|$.PublicAccessBlockConfiguration.BlockPublicAcls
Issue-Key: s3|PutBucketAcl|4xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketAcl|4xx->4xx|$.Error.Message
Issue-Key: s3|PutBucketAcl|4xx->4xx|status
Issue-Key: s3|PutBucketOwnershipControls|4xx->4xx|$.Error.Code
Issue-Key: s3|PutBucketOwnershipControls|4xx->4xx|$.Error.Message
Issue-Key: s3|GetBucketOwnershipControls|4xx->2xx|$.Error
Issue-Key: s3|GetBucketOwnershipControls|4xx->2xx|status
Issue-Key: s3|PutPublicAccessBlock|4xx->4xx|$.Error.Code
Issue-Key: s3|PutPublicAccessBlock|4xx->4xx|$.Error.Message
Issue-Key: s3|PutPublicAccessBlock|4xx->4xx|status
Issue-Key: s3|GetBucketLifecycleConfiguration|4xx->4xx|$.Error.Code
Issue-Key: s3|GetBucketLifecycleConfiguration|4xx->4xx|$.Error.Message
Issue-Key: s3|GetBucketLifecycleConfiguration|4xx->4xx|status
Issue-Related: 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 public-access-block ACL and ownership guards diverge

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPublicAccessBlock

## Description
Cause 1 of 7: malformed PublicAccessBlockConfiguration documents are accepted. AWS: 400 Code 'MalformedXML', Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text). Recorded behavior only. Emulator: 200 with an empty body. Why: the caller believes a Block Public Access posture is set from a document AWS refused. Fix: the PutPublicAccessBlock document validator. Traces: s3-pab-remediation-put-document-mistakes#task#3.

Cause 2 of 7: a valid PublicAccessBlock document is refused. AWS: 200. Recorded behavior only. Emulator: 400. Why: the remediation step of a Block Public Access workflow cannot be applied at all. Fix: the same validator, which over-rejects. Traces: s3-pab-remediation-put-document-mistakes#task#4.

Cause 3 of 7: an applied setting is not stored. AWS: GetPublicAccessBlock reports BlockPublicAcls 'true' after the Put that set it. Recorded behavior only. Emulator: 'false'. Why: an audit reads a posture weaker than the one that was configured - a security-relevant mis-report. Fix: the public-access-block store in state. Traces: s3-pab-remediation-put-document-mistakes#task#8.

Cause 4 of 7: the ACL refusal order and class are wrong. AWS: on a bucket with BlockPublicAcls enabled, PutBucketAcl with a public grant answers 403 Code 'AccessDenied', Message 'User: <caller arn> is not authorized to perform: s3:PutBucketAcl on resource: "<bucket arn>" because public ACLs are prevented by the BlockPublicAcls setting in S3 Block Public Access.' - fixed text whose caller ARN and bucket ARN come from the request and state. Recorded behavior only. Emulator: 400 Code 'AccessControlListNotSupported', Message 'The bucket does not allow ACLs' - the ownership-controls refusal, evaluated before the Block Public Access one. Why: the caller is told ACLs are disabled on the bucket when the actual blocker is Block Public Access, so remediation is aimed at the wrong setting. Fix: the refusal ordering in the PutBucketAcl authorization path. Traces: s3-pab-enforced-bucket-acl-refusal-order#task#3.

Cause 5 of 7: an empty PutBucketOwnershipControls body is reported as a schema fault. AWS: 400 Code 'MissingRequestBodyError', Message 'Request Body is empty' (fixed text). Recorded behavior only. Emulator: 400 MalformedXML with the generic schema sentence. Fix: the ownership-controls handler must detect an empty payload before schema validation. Traces: s3-oc-put-document-mistakes-then-corrected#task#3.

Cause 6 of 7: the x-amz-expected-bucket-owner guard is not enforced. AWS: a mismatching account id answers 403 AccessDenied 'Access Denied'. Recorded behavior only. Emulator: 200 - the read is served. Why: a cross-account safety check that callers rely on to prevent operating on someone else's bucket does nothing. Fix: the expected-bucket-owner check in the shared bucket-request pre-handler. Traces: s3-oc-expected-bucket-owner-guard#task#2.

Cause 7 of 7: a malformed expected-bucket-owner value is misclassified. AWS: 400 Code 'InvalidBucketOwnerAWSAccountID', Message 'The value of the expected bucket owner parameter must be an AWS Account ID... [<value as sent>]' - fixed text with the supplied value in brackets. Recorded behavior only. Emulator: either 200 (no check at all) or 403 AccessDenied 'Access Denied'. Why: a typo'd account id is reported as an authorization failure (or ignored) rather than as a malformed parameter. Fix: validate the header's shape in the same pre-handler before comparing it. Traces: s3-oc-expected-bucket-owner-guard#task#3, s3-pab-expected-bucket-owner-guard#task#6, s3-lcp-traj-expected-bucket-owner-header#task#4.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-pab-remediation-put-document-mistakes#task#3: status expected 400 got 200, empty body where AWS returns MalformedXML. #task#4: expected 200 got 400. #task#8: PublicAccessBlockConfiguration.BlockPublicAcls expected 'true' got 'false'. s3-pab-enforced-bucket-acl-refusal-order#task#3: status expected 403 got 400; Code expected 'AccessDenied' got 'AccessControlListNotSupported'; Message expected the BlockPublicAcls sentence naming the caller and bucket ARNs, got 'The bucket does not allow ACLs'. s3-oc-put-document-mistakes-then-corrected#task#3: Code expected 'MissingRequestBodyError' got 'MalformedXML'. s3-oc-expected-bucket-owner-guard#task#2: expected 403 got 200; #task#3: expected 400 InvalidBucketOwnerAWSAccountID got 200. s3-pab-expected-bucket-owner-guard#task#6 and s3-lcp-traj-expected-bucket-owner-header#task#4: expected 400 InvalidBucketOwnerAWSAccountID, got 403 AccessDenied.

## Steps to Reproduce
1. Replay s3-pab-remediation-put-document-mistakes and compare steps task#3, task#4, task#8. 2. Replay s3-pab-enforced-bucket-acl-refusal-order step task#3. 3. Replay s3-oc-put-document-mistakes-then-corrected step task#3. 4. Replay s3-oc-expected-bucket-owner-guard steps task#2/task#3 and s3-pab-expected-bucket-owner-guard step task#6 with a malformed x-amz-expected-bucket-owner.
