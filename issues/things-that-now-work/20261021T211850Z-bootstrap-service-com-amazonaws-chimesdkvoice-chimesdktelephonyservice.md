Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Amazon Chime SDK Voice (chimesdkvoice) emulator

_Filed: 2026-10-21T21:18:50Z_
_Operation: service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 96 operations in `services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented`, and the crate holds no state. No client workflow can succeed.

Implement a FIRST, SMALL, stateful slice: the Voice Connector lifecycle, which is the service's primary resource (restJson1, SDK id `Chime SDK Voice`, endpoint prefix `voice-chime`). Five operations, in order:

1. **CreateVoiceConnector** — `POST /voice-connectors` -> 201. Input: `Name` (required, len 1..256, pattern `^[a-zA-Z0-9 _.-]+$`), `RequireEncryption` (required boolean), optional `AwsRegion` (enum, default `us-east-1`), `Tags`, `IntegrationType`, `NetworkType`. Must mint a new `VoiceConnectorId` matching `^([a-z0-9]{21,22}|<uuid>)$`, persist the record, and return the stored `VoiceConnector` with `VoiceConnectorId`, `Name`, `AwsRegion`, `RequireEncryption`, `OutboundHostName` (derived, e.g. `<id>.voiceconnector.chime.aws`), `VoiceConnectorArn`, `CreatedTimestamp` and `UpdatedTimestamp` (ISO-8601).
2. **GetVoiceConnector** — `GET /voice-connectors/{VoiceConnectorId}` -> 200. Must look the id up in state and return the EXACT record created (same name, region, encryption flag, outbound host name, timestamps). Unknown id -> `NotFoundException` (HTTP 404).
3. **ListVoiceConnectors** — `GET /voice-connectors` -> 200. Must return every connector currently in state in `VoiceConnectors`, honouring `max-results` (1..100) and returning/accepting `next-token` so a paged walk terminates and yields each connector exactly once.
4. **UpdateVoiceConnector** — `PUT /voice-connectors/{VoiceConnectorId}` -> 200. Required `Name` and `RequireEncryption` must be written into the stored record, `UpdatedTimestamp` refreshed, and the updated `VoiceConnector` returned; a following GetVoiceConnector must show the new values. Unknown id -> `NotFoundException` (404).
5. **DeleteVoiceConnector** — `DELETE /voice-connectors/{VoiceConnectorId}` -> 204 with an empty body. Must remove the record; a following GetVoiceConnector must return `NotFoundException` (404) and the connector must be absent from ListVoiceConnectors. Unknown id -> 404.

Every one of these must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the input back, no minting an id that is never stored. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line only when the handler is real.

Validation in scope for this slice: reject a missing/empty `Name`, a `Name` violating the length or pattern constraint, a `MaxResults` outside 1..100, and a `VoiceConnectorId` that does not match the id pattern — each as `BadRequestException` (HTTP 400), not a 501 or a silent 200.

## State Model

In `src/state.rs`, on the service state struct:

- `voice_connectors: HashMap<String /* VoiceConnectorId */, VoiceConnector>` where `VoiceConnector { voice_connector_id: String, name: String, aws_region: String /* enum value, default us-east-1 */, outbound_host_name: String, require_encryption: bool, voice_connector_arn: String, integration_type: Option<String>, network_type: Option<String>, created_timestamp: OffsetDateTime, updated_timestamp: OffsetDateTime, tags: Vec<(String, String)> }`
- `next_connector_seq: u64` (or equivalent) used only to mint ids deterministically; the minted id MUST be inserted into `voice_connectors` in the same locked section that returns it.

List pagination reads a stable ordering of `voice_connectors` (e.g. insertion order or sorted by id) and encodes the opaque `NextToken` as the position/last-id within that ordering.

All 91 other operations (phone numbers, SIP media applications, SIP rules, proxy sessions, voice profiles, voice connector groups and the per-connector configuration sub-resources) stay 501 for now and will be filed as individual gap issues.

## Evidence

`OPERATIONS.md` (crate root, 96 operations, `## Wire protocol`: restJson1, SDK id `Chime SDK Voice`, sigv4 name `chime`, endpoint prefix `voice-chime`) documents the exact routes and members used above: `## CreateVoiceConnector` (`POST /voice-connectors -> 201`, required `Name`/`RequireEncryption`, output `VoiceConnector`), `## GetVoiceConnector` (`GET /voice-connectors/{VoiceConnectorId} -> 200`, errors include `NotFoundException (HTTP 404)`), `## ListVoiceConnectors` (`GET /voice-connectors -> 200`, `@paginated(inputToken=NextToken, outputToken=NextToken)`, `NextToken`/`MaxResults` range 1..100 as `next-token`/`max-results` query params), `## UpdateVoiceConnector` (`PUT /voice-connectors/{VoiceConnectorId} -> 200`, required `Name` and `RequireEncryption`), `## DeleteVoiceConnector` (`DELETE /voice-connectors/{VoiceConnectorId} -> 204`, no output members, `NotFoundException`/`ConflictException`). `smithy_query shape=VoiceConnector` gives the output members to persist: `VoiceConnectorId`, `AwsRegion` (doc: "Default: us-east-1"), `Name`, `OutboundHostName`, `RequireEncryption`, `CreatedTimestamp`/`UpdatedTimestamp` (`Iso8601Timestamp`, date-time), `VoiceConnectorArn`, `IntegrationType`, `NetworkType`. Stub confirmed: `src/ops/create_voice_connector.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented and`, and the same marker is present across `src/ops/`. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `chime-sdk-voice` client at it.
2. `CreateVoiceConnector(Name="demo-connector", RequireEncryption=true, AwsRegion="us-east-1")` -> expect 201 with a `VoiceConnector` carrying a generated `VoiceConnectorId`, the submitted Name/RequireEncryption, an `OutboundHostName`, an ARN and ISO-8601 CreatedTimestamp/UpdatedTimestamp. Today: HTTP 501 NotImplemented.
3. `GetVoiceConnector(VoiceConnectorId=<id from step 2>)` -> expect 200 returning the identical stored record.
4. `ListVoiceConnectors(MaxResults=1)` -> expect 200 with the connector present; page with the returned `next-token` until it is absent, each connector appearing exactly once.
5. `UpdateVoiceConnector(VoiceConnectorId=<id>, Name="demo-renamed", RequireEncryption=false)` -> expect 200; then `GetVoiceConnector(<id>)` must report Name="demo-renamed", RequireEncryption=false and a bumped UpdatedTimestamp.
6. `DeleteVoiceConnector(VoiceConnectorId=<id>)` -> expect 204 empty body; then `GetVoiceConnector(<id>)` must fail with `NotFoundException` (HTTP 404) and `ListVoiceConnectors()` must no longer include it.
7. `CreateVoiceConnector(Name="bad name!!", RequireEncryption=true)` -> expect `BadRequestException` (HTTP 400) for the `^[a-zA-Z0-9 _.-]+$` pattern violation.
