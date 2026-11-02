Issue-Source: trace-sweep-divergence
Issue-Trace: s3-cfoac-oac-grant-policy-document-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-cfoac-oac-grant-policy-document-mistakes s3-pab-block-public-policy-classifies-the-document s3-pab-reference-walk-names-the-partial-bucket
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketPolicy|4xx->2xx|$
Issue-Key: s3|PutBucketPolicy|4xx->2xx|status
Issue-Key: s3|GetBucketPolicy|2xx->2xx|$.Statement[*]
Issue-Key: s3|PutBucketPolicy|2xx->4xx|status
Issue-Key: s3|GetBucketPolicy|2xx->4xx|$.Statement
Issue-Key: s3|GetBucketPolicy|2xx->4xx|$.Version
Issue-Key: s3|GetBucketPolicy|2xx->4xx|$headers.content-type
Issue-Key: s3|GetBucketPolicy|2xx->4xx|status
Issue-Key: s3|GetBucketPolicyStatus|4xx->2xx|$.Error
Issue-Key: s3|GetBucketPolicyStatus|4xx->2xx|status
Issue-Related: 20261030T053639Z-s3-authorization-session-and-key-encoding-paths-diverge.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 bucket policy plane accepts malformed policies and misreports reads

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketPolicy

## Description
Cause 1 of 5: a policy whose Action applies to no resource in its statement is accepted. AWS: 400 Code 'MalformedPolicy', Message 'Action does not apply to any resource(s) in statement', with <Detail> 'Action "<action>" in Statement "<sid>"' where action and sid come from the submitted document. Recorded behavior only (S3 models no errors for this operation). Emulator: 204 No Content - the policy is stored. Why: a grant that cannot work (typo'd action, object action on a bucket ARN) is silently accepted, so the caller believes access is configured. Fix: the PutBucketPolicy document validator. Traces: s3-cfoac-oac-grant-policy-document-mistakes#task#4 (cluster of 4 steps).

Cause 2 of 5: the stored bad statement is then served, with the wrong content type. AWS: GetBucketPolicy returns the policy as application/json and the refused statement is absent. Recorded behavior only. Emulator: content-type 'text/plain' and the extra statement (Action 's3:GetObjects') is present. Why: clients that parse by content type mis-handle the body, and the policy they read is not the policy AWS would hold. Fix: the GetBucketPolicy serializer's content-type, plus Cause 1 for the body. Traces: s3-cfoac-oac-grant-policy-document-mistakes#task#8.

Cause 3 of 5: a non-public policy is refused 403 under BlockPublicPolicy. AWS: a statement scoped by aws:PrincipalAccount is not public, so the Put answers 204 even with BlockPublicPolicy enabled. Recorded behavior only. Emulator: 403. Why: a legitimate same-account grant cannot be applied while Block Public Access is on. Fix: the public-policy classifier consulted by PutBucketPolicy. Traces: s3-pab-block-public-policy-classifies-the-document#task#7.

Cause 4 of 5: the policy then cannot be read back. AWS: 200 with Version '2012-10-17' and the Statement array, content-type application/json. Recorded behavior only. Emulator: 404 and content-type application/xml. Why: write-then-read of a bucket policy does not round-trip. Fix: the bucket-policy store/read path in state (follows from Cause 3 for this step, but the content-type is independent). Traces: s3-pab-block-public-policy-classifies-the-document#task#9.

Cause 5 of 5: GetBucketPolicyStatus reports a status for a bucket with no policy. AWS: 404 Code 'NoSuchBucketPolicy', Message 'The bucket policy does not exist' with <BucketName> = the bucket addressed. Recorded behavior only. Emulator: 200 with no Error element. Why: a caller auditing public exposure reads a policy status that does not exist. Fix: the GetBucketPolicyStatus absence branch. Traces: s3-pab-block-public-policy-classifies-the-document#task#11, s3-pab-reference-walk-names-the-partial-bucket#task#8.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-cfoac-oac-grant-policy-document-mistakes#task#4: status expected 400 got 204, body empty where AWS returns MalformedPolicy 'Action does not apply to any resource(s) in statement' with Detail 'Action "s3:GetObject" in Statement "AllowCloudFrontServicePrincipal"' (cluster of 4). #task#8: $.Statement[1] extra (the s3:GetObjects statement) and content-type expected 'application/json' got 'text/plain'. s3-pab-block-public-policy-classifies-the-document#task#7: expected 204 got 403. #task#9: expected 200 with Version/Statement and content-type application/json, got 404 application/xml. #task#11: expected 404 NoSuchBucketPolicy 'The bucket policy does not exist', got 200.

## Steps to Reproduce
1. Replay s3-cfoac-oac-grant-policy-document-mistakes and compare steps task#4 and task#8. 2. Replay s3-pab-block-public-policy-classifies-the-document steps task#7, task#9 and task#11.
