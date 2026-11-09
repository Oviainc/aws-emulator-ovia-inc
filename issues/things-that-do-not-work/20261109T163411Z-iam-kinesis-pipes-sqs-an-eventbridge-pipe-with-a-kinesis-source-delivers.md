Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__pipes__sqs.kinesis-source-filter-on-decoded-data-field-and-input-template-rendering
Issue-Key: iam__kinesis__pipes__sqs.kinesis-source-filter-on-decoded-data-field-and-input-template-rendering|run#17|AssertionError|assert sorted(parsed, key=lambda b: str(b.get("t"))) == expected, (
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# An EventBridge Pipe with a Kinesis source delivers nothing to its target, so data-field filtering and input-template rendering are unobservable

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1: a RUNNING pipe whose source is a Kinesis data stream never polls or delivers records to its target.

AWS: a pipe with a Kinesis stream source, a FilterCriteria pattern over the decoded `data` field and a target input template delivers exactly the records whose decoded payload matches the pattern, rendered through the template; records that do not match (and records whose payload is not JSON) are dropped. For a two-matching/two-non-matching batch the target receives exactly the two rendered matches, within a short settling window. Smithy: pipes#CreatePipe takes Source, Target, SourceParameters.FilterCriteria.Filters[].Pattern and TargetParameters.InputTemplate — the delivery semantics are service behaviour, recorded behavior only, and the recording shows the two matching records arriving rendered.

Emulator: within 180 s the target receives nothing at all — the filter cannot be distinguished from a pipe that never runs.

Why: a client cannot use Pipes with a Kinesis source at all: no record is forwarded, so the whole pipeline silently loses data while the control plane reports the pipe RUNNING. Nothing downstream of the pipe (filtering correctness, template rendering, batching) can be relied on or even tested.

Fix: the Kinesis source poller in the Pipes execution engine — it must read shard records, decode the payload for filter evaluation, render the input template and deliver to the target.

Traces: iam__kinesis__pipes__sqs.kinesis-source-filter-on-decoded-data-field-and-input-template-rendering.

## Evidence
Expected: the two kind=keep records arrive at the target, rendered; the kind=drop and non-JSON records do not.
Actual: AssertionError: FilterCriteria {'data': {'kind': ['keep']}} must deliver exactly the two kind=keep records, rendered, and filter the kind=drop and non-JSON records; within 180s the target got []

## Steps to Reproduce
1. Create a Kinesis stream, an SQS target queue and a pipe role.
2. CreatePipe with the stream as Source, the queue as Target, SourceParameters.FilterCriteria matching on the decoded data field, and a TargetParameters.InputTemplate; wait for RUNNING.
3. PutRecords with two matching and two non-matching payloads.
4. Poll the target queue; observe nothing is delivered.
