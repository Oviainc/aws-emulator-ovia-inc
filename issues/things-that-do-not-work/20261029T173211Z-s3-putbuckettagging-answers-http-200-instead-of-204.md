Issue-Source: trace-sweep-divergence
Issue-Trace: application-insights-component-configuration-lifecycle
Issue-Step: setup#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-insights-component-configuration-lifecycle application-insights-workload-lifecycle
Issue-Shape-Id: com.amazonaws.applicationinsights#EC2WindowsBarleyService
Issue-Key: s3|PutBucketTagging|2xx->2xx|status
Issue-Commit: 04aee419340dca63f4b07449b9fd0b6ed9270cf9

# s3 PutBucketTagging answers HTTP 200 instead of 204 No Content

> _Note: this issue was identified against the code base at commit `04aee419340dca63f4b07449b9fd0b6ed9270cf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketTagging

## Description
Cause 1 of 1: PutBucketTagging returns the wrong success status.
AWS: a successful PUT /{Bucket}?tagging is answered HTTP 204 No Content with an empty body. Recorded behavior: 204. (Smithy: PutBucketTagging has http {method: PUT, uri: '/{Bucket}?tagging', code: 200} and output Unit — the model's declared code is 200, but the recording shows real S3 answering 204, so the model's code is not the wire requirement here.)
Emulator: answers HTTP 200 for the same successful tagging write.
Why: a client or test that asserts the documented 204 (or that distinguishes 'no content' from a body-bearing 200 when deciding whether to parse the response) sees a status S3 never sends; tooling that treats any non-204 as an unexpected result on this operation fails against the emulator even though the tags were stored. The tag write itself does take effect, so the impact is confined to the status line.
Fix: the s3 PutBucketTagging handler/serializer — return 204 with an empty body on success instead of 200.
Traces: application-insights-component-configuration-lifecycle#setup#2 and application-insights-workload-lifecycle#setup#2 (same cluster, both setup steps of applicationinsights fixtures that tag a bucket).

## Evidence
Replay of simparity-wire-aws-v1: cluster service=s3|PutBucketTagging|status=204->200, detail 'status: expected 204, got 200'. Representative step application-insights-component-configuration-lifecycle#setup#2; also affected application-insights-workload-lifecycle#setup#2 (affected_count 2). Response body is empty on both sides — only the status line diverges.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Fixture application-insights-component-configuration-lifecycle, step setup#2: PUT /{Bucket}?tagging with a valid Tagging payload for an existing bucket.
3. Observe the response status is 200; the recorded S3 response is 204 No Content.
4. The same divergence reproduces at application-insights-workload-lifecycle#setup#2.
