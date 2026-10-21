Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CloudTrail Data (cloudtrail-data) emulator

_Filed: 2026-10-21T21:38:12Z_
_Operation: service-com-amazonaws-cloudtraildata-cloudtraildataservice | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole service is a fresh scaffold: the single bound operation, `PutAuditEvents`, carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns HTTP 501 `NotImplemented` while holding zero state. Implement a first functional, stateful slice.

Because this service models exactly ONE operation (`POST /PutAuditEvents`, restJson1, signing name `cloudtrail-data`), the happy path IS that operation, exercised end to end across repeated calls so that state accumulated by one call is observable by the next.

## Happy path to implement

1. **PutAuditEvents (ingest a batch)** — Input: `auditEvents` (list<AuditEvent>, 1..100, required; each entry `id` (1..128, `^[-_A-Za-z0-9]+$`, required), `eventData` (String, required), `eventDataChecksum` (optional base64-SHA256 of eventData)), `channelArn` (`@httpQuery(channelArn)`, pattern `^arn:.*$`, required), `externalId` (`@httpQuery(externalId)`, 2..1224, optional). Output: `successful` (list<AuditEventResultEntry>: `id` = caller's original id, `eventID` = CloudTrail-assigned id, both required) and `failed` (list<ResultErrorEntry>: `id`, `errorCode`, `errorMessage`, all required) — both members are `@required` and must ALWAYS be present (empty list when nothing in that bucket).
   - For each accepted event, mint a fresh `eventID` (UUID-shaped, matching `^[-_A-Za-z0-9]+$`), STORE the ingested record under the channel, and echo the pair back in `successful`.
   - Per-event failures are PARTIAL failures, not operation errors: an event whose `eventDataChecksum` does not match base64-SHA256 of `eventData` goes into `failed` with `errorCode=InvalidChecksum`; an event whose `eventData` is not valid JSON / is missing required fields goes into `failed` with `errorCode=InvalidData`; an oversized field goes into `failed` with `errorCode=FieldTooLong`. The same request can return both `successful` and `failed` entries.
2. **PutAuditEvents (unknown channel)** — a `channelArn` that is well formed but was never registered must fail with the modeled `ChannelNotFound` (HTTP 400), not 200 and not 501.
3. **PutAuditEvents (malformed ARN)** — a `channelArn` that does not look like a channel ARN (`arn:aws:cloudtrail:<region>:<account>:channel/<id>`) must fail with `InvalidChannelARN` (HTTP 400).
4. **PutAuditEvents (duplicate ids within one batch)** — two or more entries sharing the same `id` in a single request must fail the WHOLE request with `DuplicatedAuditEventId` (HTTP 400); nothing from that request may be persisted.
5. **PutAuditEvents (external id / ownership)** — when the registered channel requires an external id, a request with a missing or mismatched `externalId` must fail with `ChannelInsufficientPermission` (HTTP 400); a request whose caller account differs from the channel owner's account in the ARN must likewise fail with `ChannelInsufficientPermission`.

Every one of these responses must be COMPUTED FROM PERSISTED STATE — no canned success, no echoing the input back as a response, no minting an eventID without storing the record. A second `PutAuditEvents` against the same channel must see the events the first one stored (used for the ingest counter / duplicate-id behaviour and for any later read path), and a request against a channel that was never registered must fail rather than succeed.

Channels themselves are created by the *CloudTrail* control plane (`CreateChannel`), which is a different service crate. For this slice, the emulator must look the channel up in shared/seedable state keyed by channel ARN (and accept the channel ARN suffix / channel ID form as well, per the model doc "The ARN or ID (the ARN suffix) of a channel"), so that a channel created via CloudTrail — or seeded for tests — is ingestible here.

## State Model

```text
CloudTrailDataState {
    // channel ARN (and its ID suffix, aliased) -> channel registration
    channels: HashMap<String /*channelArn*/, Channel>,
}

Channel {
    arn: String,              // arn:aws:cloudtrail:<region>:<account>:channel/<uuid>
    channel_id: String,       // ARN suffix, accepted as an alias for arn on input
    owner_account_id: String, // compared against caller account -> ChannelInsufficientPermission
    required_external_id: Option<String>, // Some(..) -> externalId must match
    source: String,           // e.g. "Custom" / event source name
    ingested: Vec<IngestedEvent>,       // append-only ingest log
    seen_source_ids: HashSet<String>,   // caller-supplied AuditEvent.id values already ingested
}

IngestedEvent {
    source_id: String,        // AuditEvent.id supplied by the caller
    event_id: String,         // eventID minted by the emulator, returned in successful[]
    event_data: String,       // raw JSON payload as submitted
    event_data_checksum: Option<String>,
    ingested_at: SystemTime,
}
```

Resolution order for a request: normalise `channelArn` (full ARN or bare channel ID) -> `InvalidChannelARN` if it is not a channel ARN/ID -> `ChannelNotFound` if absent from `channels` -> `ChannelInsufficientPermission` on owner-account or `externalId` mismatch -> `DuplicatedAuditEventId` if `auditEvents` contains repeated `id`s -> per-event validation, appending to `ingested`/`seen_source_ids` for accepted events and to `failed` for rejected ones.

Any behaviour beyond this slice (checksum algorithm edge cases, 1 MB total-payload limit, throttling, `ChannelUnsupportedSchema` schema validation, `UnsupportedOperationException` region gating) stays out of scope and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md (services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/OPERATIONS.md) lists exactly 1 operation: `PutAuditEvents` — `POST /PutAuditEvents`, restJson1, sigv4 name `cloudtrail-data`; input `auditEvents` (list<AuditEvent>, len 1..100, required), `channelArn` (@httpQuery, pattern ^arn:.*$, required), `externalId` (@httpQuery, len 2..1224); output `successful` (list<AuditEventResultEntry>, required) and `failed` (list<ResultErrorEntry>, required); errors ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException (all HTTP 400). smithy_query on AuditEvent/AuditEventResultEntry/ResultErrorEntry confirms the member names and semantics used above (AuditEvent.id/eventData/eventDataChecksum; AuditEventResultEntry.id/eventID; ResultErrorEntry.id/errorCode/errorMessage with documented codes FieldTooLong, FieldNotFound, InvalidChecksum, InvalidData, InvalidRecipient, InvalidEventSource, AccountNotSubscribed, Throttling, InternalFailure). The sole handler services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/src/ops/put_audit_events.rs line 3-6 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 175 returns `Err(crate::scaffolded::Scaffolded::not_implemented("PutAuditEvents"))`, so the service answers HTTP 501 for its only operation and persists nothing.

## Steps to Reproduce

1. Register (or seed) a CloudTrail Lake channel, e.g. arn:aws:cloudtrail:us-east-1:111122223333:channel/0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9, owned by account 111122223333 with no required external id.
2. Call PutAuditEvents with channelArn=<that ARN> and auditEvents=[{id:"evt-1",eventData:"{\"eventSource\":\"custom.app\",\"eventName\":\"Login\"}"}] — expect HTTP 200 with successful=[{id:"evt-1",eventID:<newly minted id>}] and failed=[] (both members present).
3. Call PutAuditEvents again on the same channel with two events whose ids are distinct and one carrying a deliberately wrong eventDataChecksum — expect 200 with the good event in successful[] and the bad one in failed[] with errorCode=InvalidChecksum.
4. Verify the ingest is stateful: the channel's stored ingest log now holds 3 accepted events (not re-minted per call), and eventIDs returned earlier are stable/unique.
5. Call PutAuditEvents with auditEvents containing two entries both with id="dup-1" — expect HTTP 400 DuplicatedAuditEventId and NO new events persisted.
6. Call PutAuditEvents with channelArn=arn:aws:cloudtrail:us-east-1:111122223333:channel/does-not-exist — expect HTTP 400 ChannelNotFound.
7. Call PutAuditEvents with channelArn="not-an-arn" style invalid value — expect HTTP 400 InvalidChannelARN.
8. Seed a channel that requires externalId="secret-123" and call PutAuditEvents without externalId — expect HTTP 400 ChannelInsufficientPermission.
