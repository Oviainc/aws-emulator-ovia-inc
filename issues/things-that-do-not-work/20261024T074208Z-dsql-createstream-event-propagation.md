Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# A DSQL CDC stream never delivers change records to its target Kinesis stream

_Filed: 2026-10-24T07:42:08Z_
_Operation: CreateStream | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `CreateStream` with a `targetDefinition.kinesis` creates a change-data-capture stream whose entire documented purpose is delivery — the model's own operation documentation says the stream "captures database changes and delivers them to the specified target destination", and `KinesisTargetDefinition` carries the `streamArn` to write to plus the `roleArn` whose permission to write is assumed. On real Aurora DSQL, once the stream reports ACTIVE, committed row changes in the cluster appear as records on the named Kinesis stream in the configured `format` (JSON) and `ordering` (UNORDERED), so a client can `GetRecords` from that Kinesis stream and observe the changes.

Actual: `CreateStream` persists the `targetDefinition` on the cluster's stream record and settles the stream to ACTIVE, and that is the end of it. Nothing in the crate ever writes to the named Kinesis stream: there is no reference to the Kinesis emulator anywhere in `services/service-com-amazonaws-dsql-dsql/src` (the only uses of the target are the idempotency fingerprint `fingerprint_target`, the pass-role authorization gate, and the `GetStream` echo). A stream is therefore a purely decorative record: it reports ACTIVE forever and delivers nothing, so no cross-service CDC workflow (create cluster -> create stream -> change data -> read records off Kinesis) can be exercised.

Note on sequencing: this gap sits behind the already-open CRITICAL data-plane issue (`20261023T121409Z-dsql-clusters-report-active-with-a-published-endpoint`) — with no PostgreSQL wire listener there are no committed changes to capture yet. It is filed separately because it is a distinct, additional piece of behavior (delivery into the emulated Kinesis service) that will still be missing once a data plane exists, and because it is a different class from the already-filed `CreateStream`/wrong-behavior issue, which is about the stream's own FAILED/IMPAIRED status rather than about record delivery.

## Evidence

Spec: `smithy_query(query="operation", operation="CreateStream")` — input `targetDefinition` (union `TargetDefinition` with `kinesis` -> `KinesisTargetDefinition{streamArn @required, roleArn @required}`), `format` (StreamFormat: JSON), `ordering` (StreamOrdering: UNORDERED); the operation doc states the stream delivers database changes to the specified target. `StreamFailureErrorCode` further names delivery-time target failures (KINESIS_THROUGHPUT_EXCEEDED, KINESIS_OVERSIZE_RECORD), which only exist because delivery is the behavior.

Handler: `src/ops/create_stream.rs` lines 319-342 store `target_definition` on `crate::state::Stream` and queue a transition to ACTIVE; the only other readers of the target are `fingerprint_target` (lines 536-542) and `cross_account_denial` (lines 400-404). `src/state.rs` `Stream` holds `target_definition` but no delivery machinery, and no handler references the Kinesis emulator crate (`services/service-com-amazonaws-kinesis-kinesis-20131202`).

Tests: `tests/all/stream_lifecycle.rs`, `tests/all/stream_listing.rs` and `tests/all/stream_target_authorization.rs` assert only the control-plane envelope (CREATING -> ACTIVE, listing, the cross-account 403 and the target echo). No test asserts that any record reaches the target Kinesis stream.

## Steps to Reproduce

1. Create a Kinesis stream in the emulator: kinesis CreateStream(StreamName="dsql-cdc") and wait for ACTIVE.
2. POST /cluster {"deletionProtectionEnabled": false}; wait for the cluster to settle to ACTIVE.
3. POST /stream/{clusterIdentifier} with {"targetDefinition": {"kinesis": {"streamArn": "arn:aws:kinesis:us-east-1:<account>:stream/dsql-cdc", "roleArn": "arn:aws:iam::<account>:role/dsql-cdc"}}, "ordering": "UNORDERED", "format": "JSON"} -> 200, status CREATING, then ACTIVE.
4. Produce a database change on the cluster (blocked today by the missing PostgreSQL data plane; equivalently, any committed change on real DSQL).
5. kinesis GetShardIterator + GetRecords against "dsql-cdc": the emulator returns no records ever, where real Aurora DSQL delivers JSON change records for the committed change.
6. GetStream still reports status ACTIVE, so nothing signals that the stream delivers nothing.
