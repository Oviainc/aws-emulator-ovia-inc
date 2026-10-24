Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# A stream whose Kinesis target does not exist still settles to ACTIVE: FAILED/IMPAIRED and statusReason are unreachable

_Filed: 2026-10-24T00:42:23Z_
_Operation: CreateStream | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Aurora DSQL models the failure of a CDC stream as stream STATE, not as a create-time refusal: `StreamStatus` carries FAILED and IMPAIRED, `GetStream` returns an optional `statusReason` (`StatusReason` = `error` + `updatedAt`), and `StreamFailureErrorCode` enumerates exactly the target problems a created stream runs into — KINESIS_STREAM_NOT_FOUND ("The specified Kinesis stream was not found"), ROLE_ACCESS_DENIED, KINESIS_ACCESS_DENIED, KINESIS_KMS_ACCESS_DENIED, KINESIS_THROUGHPUT_EXCEEDED, KINESIS_OVERSIZE_RECORD, CLUSTER_CMK_INACCESSIBLE, INTERNAL_ERROR.

Expected: a stream created against a `targetDefinition.kinesis.streamArn` that names no existing Kinesis stream (or a role/target this service cannot write to) must not sit in ACTIVE forever — it settles into FAILED/IMPAIRED and `GetStream` reports `statusReason` with the matching `error` (e.g. KINESIS_STREAM_NOT_FOUND) and an `updatedAt` timestamp, so a client that polls GetStream after CreateStream learns the delivery target is unusable.

Actual: `create_stream.rs` queues one unconditional transition for every new stream (`pending: next_status = STREAM_STATUS_ACTIVE`, lines 319-333) with no inspection of the Kinesis target beyond the same-account pass-role gate, and `state.rs` defines only STREAM_STATUS_CREATING / STREAM_STATUS_ACTIVE. `get_stream.rs` then hardcodes `status_reason: None` (lines 197-199, comment: "a stream the clock is driving to ACTIVE has none"). So every stream — including one targeting `arn:aws:kinesis:us-east-1:<account>:stream/does-not-exist` — reports CREATING then ACTIVE with no `statusReason`, and FAILED, IMPAIRED and the whole `StatusReason`/`StreamFailureErrorCode` surface are unreachable on every code path. The target's existence is never cross-checked against the Kinesis emulator's streams either, so nothing ever distinguishes a deliverable stream from an undeliverable one.

## Evidence

Spec (smithy_query): `StreamStatus` = CREATING | ACTIVE | DELETING | DELETED | FAILED | IMPAIRED; `GetStreamOutput.statusReason` (StatusReason: required `error` of `StreamFailureErrorCode`, required `updatedAt`); `StreamFailureErrorCode` values include KINESIS_STREAM_NOT_FOUND and ROLE_ACCESS_DENIED; CreateStream doc: "The stream captures database changes and delivers them to the specified target destination." Handler: `src/ops/create_stream.rs` lines 319-336 (status CREATING, single pending transition to ACTIVE, target stored but never checked), `src/ops/get_stream.rs` lines 196-199 (`status_reason: None` hardcoded), `src/state.rs` lines 60-62 (only CREATING/ACTIVE stream statuses exist). Tests: `tests/all/stream_lifecycle.rs::the_clock_moves_a_new_stream_from_creating_to_active` asserts the ACTIVE settle for all streams; `stream_target_authorization.rs` covers only the cross-account pass-role 403. No test asserts FAILED/IMPAIRED or a `statusReason`, and no test uses a nonexistent Kinesis stream ARN (a Kinesis emulator crate exists at `services/service-com-amazonaws-kinesis-kinesis-20131202`).

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false}, wait for the cluster to settle to ACTIVE.
2. POST /stream/{clusterIdentifier} with {"targetDefinition": {"kinesis": {"streamArn": "arn:aws:kinesis:us-east-1:<account>:stream/never-created", "roleArn": "arn:aws:iam::<account>:role/dsql-cdc"}}, "ordering": "UNORDERED", "format": "JSON"} → 200, status CREATING.
3. GET /stream/{clusterIdentifier}/{streamIdentifier} after the clock settles → emulator answers status ACTIVE with no `statusReason`.
4. Expected: the stream reports FAILED (or IMPAIRED) with `statusReason` = {"error": "KINESIS_STREAM_NOT_FOUND", "updatedAt": <timestamp>} rather than ACTIVE, since the delivery target does not exist.
