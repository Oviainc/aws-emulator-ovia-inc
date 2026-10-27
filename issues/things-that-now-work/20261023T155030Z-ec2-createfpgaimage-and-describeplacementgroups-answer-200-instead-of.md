Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-fpga-image-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-fpga-image-error-paths ec2-fpga-image-mistakes ec2-image-copy-and-fpga-invalid-input-errors ec2-t12-post-delete-not-found-probes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateFpgaImage|4xx->2xx|$.Response
Issue-Key: ec2|CreateFpgaImage|4xx->2xx|$headers.content-type
Issue-Key: ec2|CreateFpgaImage|4xx->2xx|status
Issue-Key: ec2|DescribePlacementGroups|4xx->2xx|$.Response
Issue-Key: ec2|DescribePlacementGroups|4xx->2xx|$headers.content-type
Issue-Key: ec2|DescribePlacementGroups|4xx->2xx|status
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 CreateFpgaImage and DescribePlacementGroups answer 200 instead of an error

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFpgaImage

## Description
Two causes, both the same defect shape: a request AWS refuses with 400 is answered 200 with no error envelope at all, so the caller is told a failure succeeded.

Cause 1 of 2 — CreateFpgaImage against an unreachable S3 storage location.
AWS: 400 with Code 'InaccessibleStorageLocation' (recorded message example: 'Error accessing resource in S3. Check permissions, naming, and ensure the bucket is within the same region as the API endpoint. Bucket: <bucket>, Key: <key>.'). Smithy: CreateFpgaImage binds no error shapes in the EC2 model, so the status/code pair is recorded behavior only, taken from the recorded response.
Emulator: 200, and the body contains no <Response> error envelope ('$.Response: missing ... got None'). It accepts the create as if the bitstream location were valid.
Trace: ec2-fpga-image-error-paths#task#2 (also ec2-fpga-image-mistakes#task#1, ec2-image-copy-and-fpga-invalid-input-errors#task#4).

Cause 2 of 2 — DescribePlacementGroups naming a placement group that was already deleted.
AWS: 400 with Code 'InvalidPlacementGroup.Unknown' (recorded message example: "The Placement Group '<name>' is unknown."). Recorded behavior only.
Emulator: 200 — the deleted placement group is still resolvable, so the post-delete not-found probe succeeds instead of failing.
Trace: ec2-t12-post-delete-not-found-probes#task#19.

Why: a 200 on a request that must fail is the worst divergence direction, because no client-side retry, error handler or validation path ever runs. For cause 1, a caller that submits a bad or cross-region S3 bitstream location is told the FPGA image creation was accepted and only discovers the problem (if ever) much later, instead of getting an immediate, actionable 400 naming the bucket and key. For cause 2, a client using DescribePlacementGroups as its existence probe concludes a deleted placement group still exists — so delete-then-verify and idempotent-create flows take the wrong branch, and tests that assert on the post-delete 400 pass against AWS and silently invert against the emulator.

Fix: input validation and state lookup in the ec2 service crate — CreateFpgaImage's handler must validate/reach the S3 storage location before accepting and raise the 400, and DescribePlacementGroups' handler must stop resolving placement groups that DeletePlacementGroup removed (and must raise the unknown-group 400 for an unresolvable name).

Related open issue: 20261023T102725Z-ec2-skips-input-validation-and-accepts-requests-aws.md covers the same family but carries neither of these two replay keys (owned_by: [] on both clusters), so they are filed here.

Traces: ec2-fpga-image-error-paths#task#2 (representative), ec2-t12-post-delete-not-found-probes#task#19.

## Evidence
Replay of simparity-wire-aws-v1, 2 divergence clusters, both expected 400 and answered 200.

Cause 1 — divergence_key 'service=ec2|CreateFpgaImage|status=400->200|keys=$.Response,$headers.content-type,status', fixture ec2-fpga-image-error-paths step task#2, affected_count 3 (also ec2-fpga-image-mistakes#task#1, ec2-image-copy-and-fpga-invalid-input-errors#task#4):
  "status: expected 400, got 200"
  "$.Response: missing (expected {'Errors': {'Error': {'Code': 'InaccessibleStorageLocation', 'Message': 'Error accessing resource in S3. Check permissions, naming, and ensure the bucket is within the same region as the API endpoint. Bucket: sim2real-probe-ec2-fpga-bucket-7f1bd67f, Key: bitstreams/design.tar.'}}, 'RequestID': ...}, got None)"

Cause 2 — divergence_key 'service=ec2|DescribePlacementGroups|status=400->200|keys=$.Response,$headers.content-type,status', fixture ec2-t12-post-delete-not-found-probes step task#19:
  "status: expected 400, got 200"
  "$.Response: missing (expected {'Errors': {'Error': {'Code': 'InvalidPlacementGroup.Unknown', 'Message': \"The Placement Group 'sim2real-probe-ec2-pd-pg-a37c9b4e' is unknown.\"}}, 'RequestID': ...}, got None)"

The bucket/key and placement-group names in the recorded messages are fixture-generated examples; the requirements asserted are the 400 status and the Error/Code values.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus().
2. Replay fixture ec2-fpga-image-error-paths, step task#2: CreateFpgaImage pointing at an inaccessible S3 bucket/key. The emulator returns 200 with no error envelope; the recording has 400 InaccessibleStorageLocation.
3. Replay fixture ec2-t12-post-delete-not-found-probes to step task#19: after the placement group is deleted, DescribePlacementGroups for its name returns 200 instead of the recorded 400 InvalidPlacementGroup.Unknown.
