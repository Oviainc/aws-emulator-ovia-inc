Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the Kinesis Video Streams emulator

_Filed: 2026-10-21T21:47:52Z_
_Operation: service-com-amazonaws-kinesisvideo-kinesisvideo-20170930 | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 32 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding no state. Implement a FIRST stateful slice around the service's primary resource, the video stream, so a realistic client script works end-to-end.

Happy path to implement, in order (protocol: `restJson1`, sigv4 signing name `kinesisvideo`, all routes POST per OPERATIONS.md):

1. **CreateStream** — `POST /createStream`. Input: `StreamName` (required, len 1..256, `^[a-zA-Z0-9_.-]+$`), `DeviceName`, `MediaType`, `KmsKeyId`, `DataRetentionInHours` (range 0..), `Tags` (map, len 1..50). Must mint and persist a stream record, and return the generated `StreamARN` matching `^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$` (e.g. `arn:aws:kinesisvideo:us-east-1:000000000000:stream/<name>/<creation-epoch-millis>`). Creating a stream whose name already exists -> `ResourceInUseException` (400). Name violating the length/pattern constraint -> `InvalidArgumentException` (400).
2. **DescribeStream** — `POST /describeStream`. Input: `StreamName` XOR `StreamARN`. Output: `StreamInfo` { `DeviceName`, `StreamName`, `StreamARN`, `MediaType`, `KmsKeyId`, `Version`, `Status`, `CreationTime`, `DataRetentionInHours` } read back from the stored record — the values must be exactly what CreateStream persisted (defaults applied: `Status` = `ACTIVE`, `DataRetentionInHours` = 0 when omitted, `KmsKeyId` = the AWS-managed default key id). Unknown stream name/ARN -> `ResourceNotFoundException` (404); neither identifier supplied -> `InvalidArgumentException` (400).
3. **ListStreams** — `POST /listStreams`. Input: `MaxResults` (1..10000), `NextToken` (len 0..1024, `^[a-zA-Z0-9+/=]*$`), `StreamNameCondition`. Output: `StreamInfoList` built from the store (same `StreamInfo` shape as DescribeStream) plus `NextToken`. Must honour `MaxResults` as a real page bound, emit an opaque resumable `NextToken` when more streams remain, omit `NextToken` on the last page, and apply `StreamNameCondition` (`BEGINS_WITH` on `ComparisonValue`) as a server-side filter.
4. **GetDataEndpoint** — `POST /getDataEndpoint`. Input: `StreamName`/`StreamARN` + `APIName` (required; one of GET_CLIP | GET_DASH_STREAMING_SESSION_URL | GET_HLS_STREAMING_SESSION_URL | GET_IMAGES | GET_MEDIA | GET_MEDIA_FOR_FRAGMENT_LIST | LIST_FRAGMENTS | PUT_MEDIA). Output: `DataEndpoint` — a URL derived from the stream that actually exists in the store (unknown stream -> `ResourceNotFoundException` 404, missing/unknown `APIName` -> `InvalidArgumentException` 400). The endpoint must point back at the emulator so data-plane clients can be pointed at it.
5. **DeleteStream** — `POST /deleteStream`. Input: `StreamARN` (required), optional `CurrentVersion`. Must remove the record from the store; when `CurrentVersion` is supplied and does not match the stored `Version`, return `VersionMismatchException` (400) and leave the stream intact. Unknown ARN -> `ResourceNotFoundException` (404). After a successful delete, DescribeStream must return 404 and ListStreams must no longer include the stream.

Every one of these five MUST compute its response from persisted state. No canned bodies, no echoing the request back, no minting an ARN that is never stored. A `CreateStream` followed by `DescribeStream`/`ListStreams` must return what was created; a `DeleteStream` must be observable by the next reader.

## State Model

Service-level state behind one lock, keyed by account+region:

- `streams: Map<StreamName, Stream>` — the primary index, name is unique per account/region.
- `stream_arn_index: Map<StreamARN, StreamName>` — so every op that accepts `StreamARN` resolves to the same record as the name form.
- `Stream { stream_name: String, stream_arn: String, device_name: Option<String>, media_type: Option<String>, kms_key_id: String, version: String (opaque alnum token, regenerated on every mutation — UpdateStream/UpdateDataRetention later), status: Status (CREATING|ACTIVE|UPDATING|DELETING), creation_time: Timestamp, data_retention_in_hours: i32, tags: Map<String,String> }`.
- `next_arn_seq` (or creation-epoch-millis) used for the trailing numeric segment of the ARN so ARNs satisfy the modeled pattern and are stable for the record's lifetime.
- Pagination tokens for ListStreams: opaque base64 (`^[a-zA-Z0-9+/=]*$`) encoding the last-returned stream name, resolved against the sorted key set rather than a snapshot copy.

The other 27 operations (signaling channels, edge configuration, notification/image-generation/media-storage configuration, tagging) stay stubbed at 501 for now and are tracked as individual gap issues later. Keeping the state model above shaped per-resource (a parallel `channels: Map<ChannelName, Channel>` will slot in next) is enough for them to be layered on.

Done = an integration test that runs CreateStream -> DescribeStream -> ListStreams -> GetDataEndpoint -> DeleteStream -> DescribeStream(404) against the emulator and sees correct, stateful bodies at every step — not merely 2xx.

## Evidence

OPERATIONS.md for this crate documents all 32 operations with their routes, input/output members and errors: CreateStream (`POST /createStream`, required `StreamName`, output `StreamARN`, errors incl. ResourceInUseException/InvalidArgumentException), DescribeStream (`POST /describeStream`, output `StreamInfo`, ResourceNotFoundException 404), ListStreams (`POST /listStreams`, @paginated(inputToken=NextToken, outputToken=NextToken, items=StreamInfoList), MaxResults 1..10000), GetDataEndpoint (`POST /getDataEndpoint`, required `APIName` enum, output `DataEndpoint`), DeleteStream (`POST /deleteStream`, required `StreamARN`, optional `CurrentVersion`, VersionMismatchException 400). The `StreamInfo` member list (DeviceName, StreamName, StreamARN, MediaType, KmsKeyId, Version, Status, CreationTime, DataRetentionInHours) comes from smithy_query on shape `StreamInfo` in smithy-model/com-amazonaws-kinesisvideo-kinesisvideo-20170930.json. Proof of the all-stub state: `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/` matches all 32 handler files (every file in src/ops/ except mod.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. No tests assert any of this behavior.

## Steps to Reproduce

1. aws kinesisvideo create-stream --stream-name demo-stream --media-type video/h264 --data-retention-in-hours 24 --endpoint-url $EMULATOR  # expect 200 with a StreamARN; today 501 NotImplemented
2. aws kinesisvideo describe-stream --stream-name demo-stream --endpoint-url $EMULATOR  # expect StreamInfo with StreamName=demo-stream, MediaType=video/h264, DataRetentionInHours=24, Status=ACTIVE, the same StreamARN, a Version and a CreationTime
3. aws kinesisvideo list-streams --max-results 1 --endpoint-url $EMULATOR  # expect exactly one StreamInfo plus a NextToken when a second stream exists; repeat with --next-token to drain
4. aws kinesisvideo get-data-endpoint --stream-name demo-stream --api-name PUT_MEDIA --endpoint-url $EMULATOR  # expect a DataEndpoint URL for the stored stream
5. aws kinesisvideo describe-stream --stream-name no-such-stream --endpoint-url $EMULATOR  # expect ResourceNotFoundException (HTTP 404)
6. aws kinesisvideo delete-stream --stream-arn <ARN> --current-version bogus1 --endpoint-url $EMULATOR  # expect VersionMismatchException (HTTP 400), stream still present
7. aws kinesisvideo delete-stream --stream-arn <ARN> --current-version <Version from describe> --endpoint-url $EMULATOR  # expect 200
8. aws kinesisvideo describe-stream --stream-name demo-stream --endpoint-url $EMULATOR  # expect ResourceNotFoundException (404); list-streams no longer returns it
