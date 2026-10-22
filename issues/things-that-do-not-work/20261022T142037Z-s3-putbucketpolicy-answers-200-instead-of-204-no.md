Issue-Source: trace-sweep-divergence
Issue-Trace: accessanalyzer-resource-scan-and-lookup
Issue-Step: setup#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: accessanalyzer-resource-scan-and-lookup
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: s3|PutBucketPolicy|2xx->2xx|status
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# S3 PutBucketPolicy answers 200 instead of 204 No Content

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketPolicy

## Description
1 cause.

Cause 1 of 1: PutBucketPolicy (fixture accessanalyzer-resource-scan-and-lookup, step setup#4).
AWS: Smithy: PutBucketPolicy http {method: PUT, uri: /{Bucket}?policy}, output shape Unit (no members, no body); the model's declared code is 200 but the operation has no output to carry, and recorded behavior only - the recording shows AWS answering 204 with an empty body for a successful bucket-policy put.
Emulator: answers 200 (status: expected 204, got 200).
Why: PutBucketPolicy success is observable only through its status, so a client, proxy or test that treats 204 as the success signal for this call (or that attempts to parse a body because the status promised one) behaves differently against the emulator than against AWS; this also makes any cross-service setup that asserts the recorded status - such as the Access Analyzer resource-scan fixture that seeds a bucket policy here - fail before it reaches the operation under test.
Fix: the status selection in PutBucketPolicy's response serializer (emit 204 for this no-output bucket subresource PUT rather than 200).
Traces: accessanalyzer-resource-scan-and-lookup#setup#4.

## Evidence
Replay of trace set simparity-wire-aws-v1: accessanalyzer-resource-scan-and-lookup#setup#4 PutBucketPolicy, expected status 204, actual status 200; detail 'status: expected 204, got 200'; divergence_key service=s3|PutBucketPolicy|status=204->200|keys=status, verdict mismatch. The cluster carries an empty stub_path and no owning issue, and the run reported state_reset_failures: 0, so the step did not run against stale state. Open S3-adjacent issues (e.g. the s3files PutBucketVersioning NotImplemented issue) concern other operations and do not cover this status.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture accessanalyzer-resource-scan-and-lookup.
2. Step setup#4 PUTs /{Bucket}?policy (PutBucketPolicy) with a bucket policy document on the bucket created earlier in the fixture.
3. Observe the response status is 200 where the recording has 204 with an empty body.
4. The step passes once the successful put answers 204.
