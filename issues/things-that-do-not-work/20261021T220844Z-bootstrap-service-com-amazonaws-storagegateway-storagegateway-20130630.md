Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the storagegateway emulator

_Filed: 2026-10-21T22:08:44Z_
_Operation: service-com-amazonaws-storagegateway-storagegateway-20130630 | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 96 operations in this crate is an untouched scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and `src/state.rs` holds no Storage Gateway data. No client workflow can run.

Implement a FIRST, SMALL happy-path slice: the activate -> configure -> read -> list -> delete lifecycle of the two primary resources, the **gateway** and the **S3 (NFS) file share**. Wire protocol is `awsJson1_1`: `POST /` with `X-Amz-Target: StorageGateway_20130630.<OperationName>` (see OPERATIONS.md "Wire protocol").

Scope (in order):

1. **ActivateGateway** — input `ActivationKey` (required, len 1..50), `GatewayName` (required, len 2..255, pattern-constrained), `GatewayTimezone` (required, len 3..10), `GatewayRegion` (required, len 1..25), optional `GatewayType`, `TapeDriveType`, `MediumChangerType`, `Tags`. Must mint and PERSIST a new gateway: generate `GatewayId` (len 12..30, e.g. `sgw-XXXXXXXX`) and `GatewayARN` of the form `arn:aws:storagegateway:<region>:<account>:gateway/<GatewayId>` (output `GatewayARN`, len 50..500), store the name/timezone/region/type/tags, set operational state `ACTIVE`. Return the stored ARN, not an echo of the input.

2. **ListGateways** — input `Marker`, `Limit` (range 1..); output `Gateways` (list<GatewayInfo>) + `Marker` (the @paginated outputToken). Each `GatewayInfo` is built from stored state: `GatewayId`, `GatewayARN`, `GatewayName`, `GatewayType`, `GatewayOperationalState` (`ACTIVE`), `Ec2InstanceRegion`. A gateway activated in step 1 MUST appear here; a deleted one must not. Honour `Limit` and round-trip `Marker`.

3. **DescribeGatewayInformation** — input `GatewayARN` (required); output must reflect what was stored at activation: `GatewayARN`, `GatewayId`, `GatewayName`, `GatewayTimezone`, `GatewayState` (`RUNNING`), `GatewayType`, `Tags`, plus stable filler for `GatewayNetworkInterfaces`, `HostEnvironment`, `EndpointType`, `SoftwareVersion`. An unknown/never-activated ARN must fail with `InvalidGatewayRequestException` (HTTP 400), NOT a 200 with empty fields.

4. **CreateNFSFileShare** — input `ClientToken` (required, len 5..100), `GatewayARN` (required), `Role` (required, IAM-role ARN pattern), `LocationARN` (required, len 16..1400), plus optional `FileShareName`, `DefaultStorageClass`, `ObjectACL`, `ClientList`, `Squash`, `ReadOnly`, `GuessMIMETypeEnabled`, `RequesterPays`, `NFSFileShareDefaults`, `CacheAttributes`, `Tags`. Must mint `FileShareId` and `FileShareARN` (`arn:aws:storagegateway:<region>:<account>:share/<FileShareId>`), store the share against its gateway with every supplied field retained, and treat `ClientToken` as idempotency: a repeat with the same token + gateway returns the SAME `FileShareARN` without creating a second share. Unknown `GatewayARN` -> `InvalidGatewayRequestException` 400.

5. **ListFileShares** — input `GatewayARN`, `Limit`, `Marker`; output `FileShareInfoList` (list<FileShareInfo>: `FileShareType` = `NFS`, `FileShareARN`, `FileShareId`, `FileShareStatus` = `AVAILABLE`, `GatewayARN`), `Marker`/`NextMarker`. Filter by `GatewayARN` when given, list all shares otherwise; the share created in step 4 must appear, and `Limit` must bound the page.

6. **DescribeNFSFileShares** — input `FileShareARNList` (required, 1..10 ARNs); output `NFSFileShareInfoList` where each `NFSFileShareInfo` is reconstructed from stored state (`FileShareARN`, `FileShareId`, `FileShareStatus`, `GatewayARN`, `Role`, `LocationARN`, `Path` e.g. `/`, `DefaultStorageClass`, `ObjectACL`, `ClientList`, `Squash`, `ReadOnly`, `GuessMIMETypeEnabled`, `RequesterPays`, `NFSFileShareDefaults`, `Tags`). Every optional value passed to CreateNFSFileShare must come back here — this is the check that nothing is silently dropped.

7. **Teardown: DeleteFileShare** (input `FileShareARN` required, `ForceDelete` default false; output `FileShareARN`) then **DeleteGateway** (input `GatewayARN` required; output `GatewayARN`). Both must REMOVE state: after DeleteFileShare the share vanishes from ListFileShares and DescribeNFSFileShares; after DeleteGateway the gateway vanishes from ListGateways and DescribeGatewayInformation answers `InvalidGatewayRequestException` 400. Deleting a gateway must also drop its file shares.

NEVER STUBS: every one of these must compute its response from persisted state. Hardcoded bodies, echoed inputs, and minted-ARN-without-storage are failures — a create followed by a describe must return what was created.

## State Model

In `ComAmazonawsStoragegatewayStoragegateway20130630State` (src/state.rs), behind the shared engine lock:

- `gateways: HashMap<String /*GatewayARN*/, Gateway>` where `Gateway { arn, gateway_id, name, timezone, region, gateway_type, operational_state: String /*ACTIVE*/, gateway_state: String /*RUNNING*/, activation_key, host_environment, endpoint_type, software_version, ec2_instance_id: Option<String>, ec2_instance_region: Option<String>, tags: Vec<Tag>, file_share_arns: Vec<String>, created_at }`
- `file_shares: HashMap<String /*FileShareARN*/, FileShare>` where `FileShare { arn, file_share_id, share_type: "NFS"|"SMB", status: String /*AVAILABLE*/, gateway_arn, client_token, role, location_arn, path, file_share_name, default_storage_class, object_acl, client_list: Vec<String>, squash, read_only: bool, guess_mime_type_enabled: bool, requester_pays: bool, kms_encrypted: bool, kms_key: Option<String>, encryption_type: Option<String>, nfs_defaults: Option<NFSFileShareDefaults>, cache_attributes: Option<CacheAttributes>, notification_policy, audit_destination_arn, bucket_region, tags: Vec<Tag> }`
- `client_tokens: HashMap<(String /*GatewayARN*/, String /*ClientToken*/), String /*FileShareARN*/>` for CreateNFSFileShare idempotency
- `next_id: u64` (or a seeded RNG) so `GatewayId`/`FileShareId` are unique and ARNs are derived deterministically from region + account in the request context

All other operations (volumes, tapes, SMB shares, cache reports, bandwidth limits, domain join, tags, maintenance windows, ...) stay 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md at the crate root documents all 96 operations, including the exact signatures cited above: ActivateGateway (line 27, required ActivationKey/GatewayName/GatewayTimezone/GatewayRegion -> GatewayARN), CreateNFSFileShare (line 99, required ClientToken/GatewayARN/Role/LocationARN -> FileShareARN), DeleteFileShare (line 171), DeleteGateway (line 177), DescribeGatewayInformation (line 261), DescribeNFSFileShares (line 273, FileShareARNList 1..10 -> NFSFileShareInfoList), ListFileShares (line 386, @paginated Marker/NextMarker/FileShareInfoList), ListGateways (line 400, @paginated Marker/Marker/Gateways). Member lists for GatewayInfo, FileShareInfo and NFSFileShareInfo come from the Smithy model (smithy_query shape). Every declared error set is `InternalServerError (500)` + `InvalidGatewayRequestException (400)`, so unknown-ARN lookups have exactly one modeled 400 to return.

All-stubs confirmed: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches every operation file in services/service-com-amazonaws-storagegateway-storagegateway-20130630/src/ops/ (96 of 96, e.g. activate_gateway.rs:4, list_gateways.rs:4, create_nfs_file_share.rs:4, delete_gateway.rs:4). No handler reads or writes state, and no integration test exists beyond the generated tests/state_roundtrip.rs.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `storagegateway` client at it (awsJson1_1, X-Amz-Target: StorageGateway_20130630.<Op>).
2. ActivateGateway with ActivationKey="key-123456", GatewayName="my-gw", GatewayTimezone="GMT-5:00", GatewayRegion="us-east-1", GatewayType="FILE_S3" -> expect 200 with a well-formed GatewayARN; today: HTTP 501 NotImplemented.
3. ListGateways -> expect the new gateway in Gateways with matching GatewayId/GatewayARN/GatewayName and GatewayOperationalState=ACTIVE.
4. DescribeGatewayInformation with that GatewayARN -> expect GatewayName="my-gw", GatewayTimezone="GMT-5:00", GatewayType="FILE_S3", GatewayState=RUNNING.
5. CreateNFSFileShare with ClientToken="token1", that GatewayARN, Role="arn:aws:iam::123456789012:role/gw-role", LocationARN="arn:aws:s3:::my-bucket", ClientList=["10.0.0.0/8"], ReadOnly=true, Squash="RootSquash" -> expect 200 with a FileShareARN; repeat the identical call and expect the SAME FileShareARN with no second share created.
6. ListFileShares with that GatewayARN -> expect exactly one FileShareInfo (FileShareType=NFS, FileShareStatus=AVAILABLE, matching FileShareARN/GatewayARN).
7. DescribeNFSFileShares with FileShareARNList=[that ARN] -> expect Role, LocationARN, ClientList, ReadOnly=true and Squash="RootSquash" returned exactly as supplied.
8. DescribeGatewayInformation with an ARN that was never activated -> expect InvalidGatewayRequestException with HTTP 400, not 200 and not 501.
9. DeleteFileShare with that FileShareARN -> expect 200 echoing the ARN, then ListFileShares returns an empty FileShareInfoList and DescribeNFSFileShares no longer returns the share.
10. DeleteGateway with that GatewayARN -> expect 200, then ListGateways no longer lists it and DescribeGatewayInformation on it returns InvalidGatewayRequestException 400.
