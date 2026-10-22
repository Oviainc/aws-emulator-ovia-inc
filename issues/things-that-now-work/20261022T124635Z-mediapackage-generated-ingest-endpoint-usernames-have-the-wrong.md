Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-channel-lifecycle-tags-logs
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-channel-lifecycle-tags-logs mediapackage-conflict-error-paths mediapackage-harvest-job-workflow mediapackage-listing-and-validation mediapackage-origin-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage|CreateChannel|2xx->2xx|$.hlsIngest.ingestEndpoints[*].username
Issue-Key: mediapackage|DescribeChannel|2xx->2xx|$.hlsIngest.ingestEndpoints[*].username
Issue-Key: mediapackage|ListChannels|2xx->2xx|$.channels[arn="arn
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# MediaPackage generated ingest-endpoint usernames have the wrong shape

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
This issue bundles 3 causes that share ONE fault: the credential the emulator mints for each hlsIngest ingest endpoint does not have the shape real MediaPackage generates, so EVERY channel projection that carries hlsIngest diverges. A fixer must close all 3 causes below before submitting.

Cause 1 of 3 - CreateChannel (fixture mediapackage-channel-lifecycle-tags-logs, step task#1).
AWS: Smithy: IngestEndpoint.Username (jsonName 'username', target SensitiveString, @sensitive, doc 'The system generated username for ingest authentication'); the model puts no length/pattern constraint on it, so recorded behavior only - every recorded CreateChannel response returns, for each of the two ingest endpoints, a 32-character lowercase-hexadecimal username. The fixture asserts the SHAPE of that member, not a literal value.
Emulator: returns a 24-character lowercase-hex string for both ingestEndpoints[0].username and ingestEndpoints[1].username, which fails the shape assertion (replay detail: value_mismatch, expected '<shaped-not-asserted>', got e.g. a 24-hex string).
Why: a client that stores or validates the ingest credential (or a test that pins its format) sees a credential of a width AWS never issues; anything that round-trips the username through a fixed-width field or a format check breaks against the emulator but works on AWS.
Fix: the ingest-credential generator behind the CreateChannel handler / channel state, so generated usernames (and the paired password) are produced with MediaPackage's generated-credential width.
Traces: mediapackage-channel-lifecycle-tags-logs#task#1 (cluster also covers mediapackage-conflict-error-paths#setup#1, mediapackage-harvest-job-workflow#setup#1, mediapackage-listing-and-validation#setup#1, mediapackage-listing-and-validation#setup#2, mediapackage-origin-endpoint-lifecycle#setup#1).

Cause 2 of 3 - DescribeChannel (fixture mediapackage-channel-lifecycle-tags-logs, step task#2).
AWS: Smithy: DescribeChannel -> DescribeChannelResponse.HlsIngest.IngestEndpoints[].Username, same shape as above; a Describe echoes the credential stored at create time.
Emulator: the same mis-shaped 24-hex username is echoed by DescribeChannel, confirming the defect is in generation/storage and not in one serializer.
Why: the read path a client uses to retrieve the credential it must configure in its encoder returns a credential of the wrong shape.
Fix: same generator; the Describe projection should need no change once the stored value is correct.
Traces: mediapackage-channel-lifecycle-tags-logs#task#2 (cluster also covers mediapackage-conflict-error-paths#task#5).

Cause 3 of 3 - ListChannels (fixture mediapackage-channel-lifecycle-tags-logs, step task#3).
AWS: Smithy: ListChannels -> ListChannelsResponse.Channels[] of Channel, whose HlsIngest.IngestEndpoints[].Username is the same member; the list projection carries the same credential as Describe.
Emulator: each listed channel's ingestEndpoints[0..1].username carry the same 24-hex value, so the list projection diverges for every channel.
Why: a client enumerating channels to collect ingest credentials gets the wrong-shaped credential for all of them.
Fix: same generator / channel state; ListChannels' Channel projection reuses it.
Traces: mediapackage-channel-lifecycle-tags-logs#task#3 (cluster also covers mediapackage-listing-and-validation#task#3).

Note: fixture mediapackage-credential-rotation#task#2 shows the same 24-hex-vs-32-hex gap after RotateIngestEndpointCredentials, but that step asserts a LITERAL generated credential value that only a live run can produce, so it is not filed as a cause here - it is corroboration only. No open issue covers this; the three open MediaPackage-family issues are for mediapackagev2 and mediapackage-vod, which are different services.

## Evidence
Replay (trace set simparity-wire-aws-v1), all steps HTTP 200 expected and 200 actual, body member diverges:
- mediapackage-channel-lifecycle-tags-logs#task#1 CreateChannel: $.hlsIngest.ingestEndpoints[0].username value_mismatch (expected '<shaped-not-asserted>', got '0a1a58891025e134fa71fa6e'); same for [1].username ('726c6ca94b4f670814192efc'). 6 steps affected.
- mediapackage-channel-lifecycle-tags-logs#task#2 DescribeChannel: identical two members diverge. 2 steps affected.
- mediapackage-channel-lifecycle-tags-logs#task#3 ListChannels: $.channels[arn="arn:aws:mediapackage:us-west-2:000000000000:channels/sim2real-probe-mediapackage-mp-channel-818202c7"].hlsIngest.ingestEndpoints[0..1].username diverge the same way. 2 steps affected.
Expected shape (recorded): 32-char lowercase hex per username. Actual: 24-char lowercase hex. The recorded literal values are examples, not requirements; the width/alphabet is the requirement.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. CreateChannel with any id (fixture mediapackage-channel-lifecycle-tags-logs step task#1) and read $.hlsIngest.ingestEndpoints[*].username - it is 24 hex chars instead of MediaPackage's 32.
3. DescribeChannel (task#2) and ListChannels (task#3) on the same channel echo the same mis-shaped username.
