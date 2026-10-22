Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9e4273988d175f89e8a017aa0737ee7107d62a89

# Bootstrap the backup-gateway emulator

_Filed: 2026-10-21T23:48:12Z_
_Operation: service-com-amazonaws-backupgateway-backuponpremises-v20210101 | Class: missing_

> _Note: this issue was identified against the code base at commit `9e4273988d175f89e8a017aa0737ee7107d62a89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 handlers in `services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow can run. Implement a FIRST minimal slice: the stateful lifecycle of the service's primary resource, the gateway (awsJson1_0, `POST /` with `X-Amz-Target: BackupOnPremises_v20210101.<Operation>`).

Happy path to implement, in order (fields per OPERATIONS.md):

1. **CreateGateway** — input `ActivationKey` (required, 1..50, `^[0-9a-zA-Z\-]+$`), `GatewayDisplayName` (required, 1..100, `^[a-zA-Z0-9-]*$`), `GatewayType` (required, only `BACKUP_VM`), optional `Tags` (list of Tag{Key,Value}). Must mint and PERSIST a new gateway record keyed by a generated gateway id, and return the generated `GatewayArn` in the modeled form `arn:aws:backup-gateway:<region>:<account>:gateway/<id>` (len 50..180, matches the modeled pattern). The ARN must be derived from the stored record, not echoed from input.

2. **GetGateway** — input `GatewayArn` (required). Must look the ARN up in the store and return `Gateway` as a `GatewayDetails` built from the persisted record: `GatewayArn`, `GatewayDisplayName`, `GatewayType`, `HypervisorId` (absent until associated), `LastSeenTime`, `MaintenanceStartTime`, `SoftwareVersion`, `VpcEndpoint`. An ARN that was never created, or one already deleted, must fail with `ResourceNotFoundException` (HTTP 404) — not 200 with an empty body.

3. **ListGateways** — input `MaxResults` (range 1..), `NextToken` (1..1000). Must enumerate the stored gateways, returning each as a `Gateway` summary (`GatewayArn`, `GatewayDisplayName`, `GatewayType`, `HypervisorId`, `LastSeenTime`). Respect `MaxResults` by truncating the page and returning an opaque `NextToken` only when more records remain; a token supplied on input must resume after the previous page and the final page must omit `NextToken`. A gateway created in step 1 must appear here; one deleted in step 5 must not.

4. **UpdateGatewayInformation** — input `GatewayArn` (required), `GatewayDisplayName`. Must mutate the stored record in place (only the members present in the request) and return `GatewayArn`; a subsequent GetGateway must observe the new display name. Unknown ARN -> `ResourceNotFoundException` (404).

5. **DeleteGateway** — input `GatewayArn` (required). Must remove the record from the store and return `GatewayArn`. After it, GetGateway must return `ResourceNotFoundException` (404) and ListGateways must no longer include it.

Also in this first slice, so tagging round-trips on the created gateway:

6. **TagResource / ListTagsForResource** — TagResource takes `ResourceARN` (required) + `Tags` (required) and must merge the tags into the stored gateway's tag map (same key overwrites) and return `ResourceARN`; ListTagsForResource takes `ResourceArn` and must return the persisted `Tags` (including any passed to CreateGateway), with `ResourceNotFoundException` (404) for an unknown ARN.

No canned, hardcoded or echoed responses: every one of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (never a freshly constructed engine), and the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker must be deleted from each file touched. Input constraints the model declares (length, pattern, enum `BACKUP_VM`, integer ranges) must be enforced as 400-class validation rather than silently accepted.

## State Model

In `src/state.rs`, on the service state struct:

- `gateways: BTreeMap<String /* GatewayArn */, Gateway>` — insertion-ordered-by-key map so ListGateways paginates deterministically.
- `Gateway { arn: String, gateway_id: String, display_name: String, gateway_type: String /* BACKUP_VM */, activation_key: String, hypervisor_arn: Option<String>, hypervisor_id: Option<String>, last_seen_time: SystemTime, created_time: SystemTime, software_version: String, vpc_endpoint: Option<String>, maintenance_start_time: Option<MaintenanceStartTime>, bandwidth_rate_limit_intervals: Vec<BandwidthRateLimitInterval>, tags: BTreeMap<String /* Key */, String /* Value */> }`
- `MaintenanceStartTime { hour_of_day: i32, minute_of_hour: i32, day_of_week: Option<i32>, day_of_month: Option<i32> }` (stored here so the later PutMaintenanceStartTime/GetGateway slice has a home).
- `next_gateway_id: u64` — monotonic counter feeding the `gateway/<id>` ARN suffix, so ARNs are unique and reproducible.

Pagination tokens for ListGateways should encode the last-returned ARN (opaque string) and be validated against the map rather than stored server-side.

Everything outside this slice (hypervisor import/update/delete, virtual-machine listing and metadata sync, bandwidth rate-limit schedule, property mappings, associate/disassociate, UpdateGatewaySoftwareNow, TestHypervisorConfiguration, UntagResource) stays at 501 and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate lists 25 operations and their contracts: CreateGateway (ActivationKey/GatewayDisplayName/GatewayType required -> GatewayArn, no errors), GetGateway (@readonly, GatewayArn -> Gateway: GatewayDetails, ResourceNotFoundException 404), ListGateways (@paginated items=Gateways, MaxResults/NextToken -> Gateways + NextToken), UpdateGatewayInformation (GatewayArn + GatewayDisplayName -> GatewayArn, 404/409), DeleteGateway (@idempotent, GatewayArn -> GatewayArn, 404), TagResource/ListTagsForResource (ResourceARN + Tags, 404). Shape members for GatewayDetails, Gateway and Tag are from the Smithy model (smithy_query shape=GatewayDetails,Gateway,Tag). All-stub status confirmed by `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matching all 25 handler files; README.md documents the 501 scaffold behavior, the awsJson1_0 wiring and the shared-engine state API. No tests assert any of this behavior today (only the generated tests/state_roundtrip.rs example).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `backup-gateway` at it.
2. CreateGateway with ActivationKey="ACTIVATION-KEY123", GatewayDisplayName="my-gateway", GatewayType="BACKUP_VM", Tags=[{Key:"env",Value:"test"}] -> expect 200 with a generated GatewayArn matching ^arn:aws:backup-gateway(:[a-zA-Z-0-9]+){3}/[a-zA-Z-0-9]+$; today: HTTP 501 NotImplemented.
3. GetGateway with that GatewayArn -> expect 200 and Gateway.GatewayDisplayName=="my-gateway", Gateway.GatewayType=="BACKUP_VM", Gateway.GatewayArn equal to the created ARN.
4. ListGateways -> expect the new gateway in Gateways; repeat with MaxResults=1 after creating a second gateway and confirm one item plus a NextToken, then pass that NextToken and get the remaining gateway with no NextToken.
5. ListTagsForResource with the gateway ARN -> expect Tags to contain env=test; TagResource with Tags=[{Key:"owner",Value:"qa"}] then ListTagsForResource -> expect both tags.
6. UpdateGatewayInformation with GatewayDisplayName="my-gateway-2" then GetGateway -> expect the updated display name.
7. DeleteGateway with the ARN -> expect 200 echoing GatewayArn; then GetGateway on it -> expect ResourceNotFoundException with HTTP 404, and ListGateways -> expect it absent.
8. GetGateway on a well-formed but never-created ARN -> expect ResourceNotFoundException (404), not 200 or 501.
