Issue-Source: trace-sweep-divergence
Issue-Trace: kafkaconnect-list-filters-and-pagination
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafkaconnect-list-filters-and-pagination kafkaconnect-worker-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.kafkaconnect#KafkaConnect
Issue-Key: kafkaconnect|CreateWorkerConfiguration|2xx->2xx|$.creationTime
Issue-Key: kafkaconnect|CreateWorkerConfiguration|2xx->2xx|$.latestRevision.creationTime
Issue-Key: kafkaconnect|DescribeWorkerConfiguration|2xx->2xx|$.creationTime
Issue-Key: kafkaconnect|DescribeWorkerConfiguration|2xx->2xx|$.latestRevision.creationTime
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# kafkaconnect serializes worker-configuration creationTime as an epoch number instead of the modeled ISO-8601 date-time

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkerConfiguration

## Description
Two causes, one serializer fault: every kafkaconnect timestamp member is emitted as a bare epoch-seconds float rather than the ISO-8601 string the model requires.

Cause 1 of 2: CreateWorkerConfiguration's creationTime (top level and inside latestRevision) is a number.
AWS: Smithy: CreateWorkerConfiguration output member `creationTime` targets __timestampIso8601 with @timestampFormat("date-time"), as does the matching `creationTime` of the latestRevision summary. On the wire that is an RFC-3339 / ISO-8601 string (e.g. '2024-05-01T12:34:56.000Z'), not a JSON number, and the recording asserts the member's shape rather than a fixed instant.
Emulator: emits a JSON float — $.creationTime and $.latestRevision.creationTime both come back as epoch seconds with a fractional part (recorded example value 1792739195.2732673), failing the shape assertion.
Why: a client decoding a modeled date-time string gets a type error or a silently wrong value; SDKs and user code that format or compare the field break, and the fractional epoch is not a representation AWS ever returns for this member.
Fix: the kafkaconnect response serializer's timestamp handling — honour @timestampFormat("date-time") for __timestampIso8601 members and render them as ISO-8601 strings. (The actual instant is environment-specific and is not a requirement; only the format is.)
Traces: kafkaconnect-list-filters-and-pagination#setup#1 (cluster also covers kafkaconnect-list-filters-and-pagination#setup#2 and kafkaconnect-worker-configuration-lifecycle#setup#1).

Cause 2 of 2: DescribeWorkerConfiguration returns the same numeric timestamps on the read path.
AWS: Smithy: DescribeWorkerConfiguration output member `creationTime` targets __timestampIso8601 with @timestampFormat("date-time"), and the latestRevision description carries a creationTime of the same shape — ISO-8601 strings.
Emulator: $.creationTime and $.latestRevision.creationTime are epoch-seconds floats (recorded example 1792739316.7587702).
Why: the defect is not confined to the create response, so a client re-reading a worker configuration also cannot parse its creation time; the whole kafkaconnect read path is affected.
Fix: same serializer path, applied to all kafkaconnect operations returning __timestampIso8601 members (also observed on CustomPluginSummary timestamps).
Traces: kafkaconnect-worker-configuration-lifecycle#task#2.

## Evidence
Cause 1 — kafkaconnect-list-filters-and-pagination#setup#1, CreateWorkerConfiguration: status 200 both sides; $.creationTime value_mismatch (expected a shaped ISO-8601 date-time, got 1792739195.2732673); $.latestRevision.creationTime value_mismatch (same). Same cluster: #setup#2 and kafkaconnect-worker-configuration-lifecycle#setup#1.

Cause 2 — kafkaconnect-worker-configuration-lifecycle#task#2, DescribeWorkerConfiguration: status 200 both sides; $.creationTime value_mismatch (expected a shaped ISO-8601 date-time, got 1792739316.7587702); $.latestRevision.creationTime value_mismatch (same).

Model: CreateWorkerConfigurationResponse.creationTime / DescribeWorkerConfigurationResponse.creationTime -> __timestampIso8601, timestampFormat date-time.

## Steps to Reproduce
1. Boot the emulator and replay fixtures kafkaconnect-list-filters-and-pagination and kafkaconnect-worker-configuration-lifecycle.
2. Cause 1: POST /v1/worker-configurations with a name and propertiesFileContent (list-filters-and-pagination#setup#1) and inspect the response — creationTime and latestRevision.creationTime are JSON numbers, not ISO-8601 strings.
3. Cause 2: GET /v1/worker-configurations/{arn} (worker-configuration-lifecycle#task#2) and inspect the same two members — again JSON numbers.
