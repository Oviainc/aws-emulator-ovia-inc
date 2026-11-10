Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing
Issue-Key: cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing|run#35|Timeout|time.sleep(2.0)
Issue-Key: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis|_poll_until#51|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing|_poll_until#46|Timeout|time.sleep(interval_s)
Issue-Commit: 90bce746fe9817893a23b18e5ce8b053a3578430

# cloudfront__kinesis: real-time log delivery never completes — all three real-time-log checks exhaust the suite's 180 s budget (no edge answer, no Kinesis record)

> _Note: this issue was identified against the code base at commit `90bce746fe9817893a23b18e5ce8b053a3578430`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRealtimeLogConfig

## Description
Cause 1 of 1: a distribution carrying a real-time log configuration never produces the observable effect AWS guarantees, so every check that grades the real-time log pipeline polls until the suite's budget is exhausted and is reported as a 180 s timeout. Failing checks: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis, cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing, cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing.

AWS: Smithy (cloudfront 2020-05-31) models CreateRealtimeLogConfig POST /2020-05-31/realtime-log-config -> 201 with required Name, SamplingRate, Fields and EndPoints (EndPoint.StreamType="Kinesis" with KinesisStreamConfig{RoleARN, StreamARN}); it models no validation of the role or stream at create time, and the delivery itself is not modeled — recorded behavior only. The recorded behavior is: CreateRealtimeLogConfig accepts the config; a distribution whose cache behavior sets RealtimeLogConfigArn reaches Status=Deployed; a viewer request to Distribution.DomainName is answered by the edge (200 for an OAC-permitted S3 origin with a CachingDisabled cache policy); and, at SamplingRate=100, each such viewer request yields exactly one tab-delimited log line on the configured Kinesis stream, in CloudFront's own canonical order of the configured Fields (so cs-uri-stem = the requested path, sc-status = "200", x-edge-response-result-type = "Miss", timestamp inside the request window, c-ip non-empty). When the delivery role lacks or is explicitly denied kinesis:PutRecord/PutRecords, the edge still answers the viewer request normally and the stream stays empty — that difference between a permitted and a denied role is the contract the two negative checks grade, and granting the missing permission afterwards makes the same stream start receiving.

Emulator: none of the three checks reaches any of its assertions. Each is reported as "Error: Check timed out after 180.0s" with no response ever graded: the sequence of waiting for Status=Deployed, GETting the published distribution domain until it answers 200, and draining GetRecords for a log line never converges, so the loops run to the harness cap. The positive case therefore produces no record and the negatives are vacuous — a permitted role and a denied role are indistinguishable because both yield an empty stream. Note the open issue 20261109T181319Z-cloudfront-kinesis-real-time-log-delivery-is-unreachable.md names these same three check ids, but it is keyed on an immediate PublishedUrlRejected refusal of the published <id>.cloudfront.net DomainName; these runs no longer fail that way — they now hang — so that issue does not own this failure and does not cover it.

Why: a client that configures CloudFront real-time logging gets no telemetry at all and no signal distinguishing a correct pipeline from a broken IAM role: the stream is empty either way, log-driven dashboards, alarms and consumers of the log stream see nothing, and the call that is expected to complete in seconds instead hangs the caller's poll loop until its own deadline.

Fix: the CloudFront edge request router (the distribution data plane that must answer a viewer request on the host:port the emulator actually serves and publishes as Distribution.DomainName) and the real-time log delivery path that, per served viewer request, renders one tab-delimited line over the configured Fields in CloudFront's canonical order and writes it to the configured Kinesis stream through the configured RoleARN — including honouring an absent or explicitly denied kinesis:PutRecord/PutRecords on that role by delivering nothing while still serving the request. Also the distribution lifecycle that must settle Status to Deployed so the wait terminates.

Traces: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis (positive: one request -> exactly one line), cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing and cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing (negatives: control stream receives every request, the permission-less/denied stream receives nothing, then receives once the permission is granted).

## Evidence
Expected (recorded AWS behavior): CreateRealtimeLogConfig 201 storing the requested field set and SamplingRate=100; the distribution reaches Status=Deployed; GET https://<Distribution.DomainName>/<key> answers 200 with the origin bytes; GetRecords on the configured single-shard stream returns exactly one record whose data is one tab-delimited line with cs-uri-stem="/<key>", sc-status="200", x-edge-response-result-type="Miss", a timestamp inside the request window and a non-empty c-ip. With a role lacking (or explicitly denied) kinesis:PutRecord/PutRecords, the edge still answers 200 and GetRecords returns zero records, and after the permission is granted the same stream starts receiving lines naming only that behaviour's path.

Actual (emulator): all three checks end as "Error: Check timed out after 180.0s". No log line is ever read from the Kinesis stream, so no assertion on the line's fields, status, result type, timestamp or viewer IP is ever reached, and neither negative can observe the permitted-vs-denied difference it grades (both sides stay empty).

## Steps to Reproduce
1. Kinesis CreateStream (ShardCount=1); wait for StreamStatus=ACTIVE and note StreamARN.
2. IAM CreateRole trusting cloudfront.amazonaws.com and PutRolePolicy allowing kinesis:DescribeStream(Summary) and kinesis:PutRecord/PutRecords on that stream ARN (for the negatives, create a twin role with the Describe actions only, or with an explicit Deny on PutRecord/PutRecords).
3. CloudFront CreateRealtimeLogConfig with SamplingRate=100, Fields=[timestamp, c-ip, cs-uri-stem, sc-status, x-edge-response-result-type] and EndPoints=[{StreamType: Kinesis, KinesisStreamConfig: {RoleARN, StreamARN}}].
4. Create an S3 bucket with one object, an OriginAccessControl, and CreateDistribution over that S3 origin with CachePolicyId=4135ea2d-6df8-44a3-9df3-4b5a84be39ad and DefaultCacheBehavior.RealtimeLogConfigArn set to the config ARN (for the negatives, add a second cache behavior "broken/*" bound to the permission-less config).
5. Wait for GetDistribution Status=Deployed, open a TRIM_HORIZON shard iterator on the stream, then GET https://<Distribution.DomainName>/<key>.
6. Poll GetRecords: no record naming the requested key ever arrives, and the check runs to the harness's 180 s cap instead of grading the single log line (and, for the negatives, instead of observing that only the permitted side delivers).
