Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7f122c3e96febe26bb2311916e16539819a4dcf1

# Bootstrap the s3outposts emulator

_Filed: 2026-10-21T22:19:22Z_
_Operation: service-com-amazonaws-s3outposts-s3outposts | Class: stub_

> _Note: this issue was identified against the code base at commit `7f122c3e96febe26bb2311916e16539819a4dcf1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 5 operations in `services/service-com-amazonaws-s3outposts-s3outposts/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. Implement a FIRST stateful slice covering the endpoint lifecycle of S3 on Outposts (restJson1, sigv4 signing name `s3-outposts`).

### Happy path to implement (in order)

1. **CreateEndpoint** — `POST /S3Outposts/CreateEndpoint`. Input: `OutpostId` (required, `^(op-[a-f0-9]{17}|\d{12}|ec2)$`), `SubnetId` (required, `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$`), `SecurityGroupId` (required, `^sg-([0-9a-f]{8}|[0-9a-f]{17})$`), optional `AccessType` (`CustomerOwnedIp` | `Private`, default `Private`), optional `CustomerOwnedIpv4Pool` (`^ipv4pool-coip-([0-9a-f]{17})$`). Must mint a 19-char alphanumeric endpoint id, PERSIST an `Endpoint` record keyed by (OutpostId, EndpointId), and return `EndpointArn` = `arn:aws:s3-outposts:<region>:<account>:outpost/<OutpostId>/endpoint/<EndpointId>` built from the request's region/account — not a canned string. If an outpost is not known to the store, fail `ResourceNotFoundException` (404); a duplicate endpoint for the same subnet/security-group should fail `ConflictException` (409).
2. **ListEndpoints** — `GET /S3Outposts/ListEndpoints`. Input: `MaxResults` (0..100), `NextToken` (1..1024). Output: `Endpoints` (list of `Endpoint`) + `NextToken`. Must return exactly the endpoints persisted by CreateEndpoint for the caller's account, each with `EndpointArn`, `OutpostsId`, `CidrBlock`, `Status`, `CreationTime`, `NetworkInterfaces`, `VpcId`, `SubnetId`, `SecurityGroupId`, `AccessType`, `CustomerOwnedIpv4Pool` populated from the stored record. Honour `MaxResults` and round-trip `NextToken` (opaque cursor matching `^[A-Za-z0-9\+\:\/\=\?\#-_]+$`); omit `NextToken` on the last page.
3. **ListOutpostsWithS3** — `GET /S3Outposts/ListOutpostsWithS3`. Output: `Outposts` (list of `Outpost` with `OutpostArn`, `S3OutpostArn`, `OutpostId`, `OwnerId`, `CapacityInBytes`) + `NextToken`. Must list the outposts in the store (seed a small fixed set at state init so CreateEndpoint has a valid parent), paginated by `MaxResults`/`NextToken`.
4. **DeleteEndpoint** — `DELETE /S3Outposts/DeleteEndpoint?endpointId=..&outpostId=..`. Empty output. Must remove the persisted endpoint; unknown `endpointId`/`outpostId` → `ResourceNotFoundException` (404). After deletion ListEndpoints must no longer return it.
5. **ListSharedEndpoints** — `GET /S3Outposts/ListSharedEndpoints?outpostId=..`. Same `Endpoints`/`NextToken` output, filtered to endpoints on the given `OutpostId` that are shared (owner account ≠ caller account). Returning an empty list for an outpost with no shared endpoints is correct; unknown outpost → `ResourceNotFoundException` (404).

Every response must be computed from persisted state — no hardcoded ARNs, no echoing the input back without storing it, no random id without a stored record. Endpoint `Status` should progress deterministically (create → `Available` immediately, or `Pending` then `Available`); `CreationTime` is the real creation instant stored at create time.

## State Model

In `src/state.rs` (`ComAmazonawsS3outpostsS3outpostsState`):

- `outposts: BTreeMap<String /*OutpostId*/, OutpostRecord>` where `OutpostRecord { outpost_id, outpost_arn, s3_outpost_arn, owner_id (12-digit account), capacity_in_bytes: i64 }` — seeded with at least one outpost (e.g. `op-<17 hex>`) so CreateEndpoint has a valid parent.
- `endpoints: BTreeMap<(String /*OutpostId*/, String /*EndpointId*/), EndpointRecord>` where `EndpointRecord { endpoint_id (19 alnum chars), endpoint_arn, outposts_id, cidr_block, status: EndpointStatus (Pending|Available|Deleting|Create_Failed|Delete_Failed), creation_time: SystemTime, network_interfaces: Vec<String /*NetworkInterfaceId*/>, vpc_id, subnet_id, security_group_id, access_type (CustomerOwnedIp|Private), customer_owned_ipv4_pool: Option<String>, failed_reason: Option<String>, owner_account: String }`.
- `list_tokens` (or a deterministic encode/decode of the last-returned key) so `NextToken` is an opaque cursor resolvable on the next ListEndpoints / ListOutpostsWithS3 / ListSharedEndpoints call.

Done = an integration test that creates an endpoint on a seeded outpost, sees it in ListEndpoints with the same ARN/SubnetId/SecurityGroupId/AccessType it was created with, pages ListOutpostsWithS3 with MaxResults=1, deletes the endpoint, and observes it gone — all with correct bodies, not merely 2xx.

Operations and edge cases outside this slice (throttling, OutpostOfflineException simulation, access-denied semantics) stay stubbed/unimplemented and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-s3outposts-s3outposts/OPERATIONS.md` lists all 5 modeled operations with routes, required inputs and errors (CreateEndpoint POST /S3Outposts/CreateEndpoint; DeleteEndpoint DELETE with httpQuery endpointId/outpostId; ListEndpoints, ListOutpostsWithS3, ListSharedEndpoints all @paginated with NextToken/MaxResults). `src/ops/create_endpoint.rs` lines 3-6 carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and line 181 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateEndpoint"))`; the other four files are the same scaffold. Shape members for `Endpoint` (EndpointArn, OutpostsId, CidrBlock, Status, CreationTime, NetworkInterfaces, VpcId, SubnetId, SecurityGroupId, AccessType, CustomerOwnedIpv4Pool, FailedReason), `Outpost` (OutpostArn, S3OutpostArn, OutpostId, OwnerId, CapacityInBytes) and the `EndpointStatus` enum values come from smithy_query on the model at `services/service-com-amazonaws-s3outposts-s3outposts/smithy-model/com-amazonaws-s3outposts-s3outposts.json`. No tests exist beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Point an AWS SDK/CLI client at the emulator with endpoint prefix s3-outposts.
2. aws s3outposts list-outposts-with-s3 --max-results 10  -> expect a seeded Outpost with OutpostId/OutpostArn/S3OutpostArn/OwnerId; today: HTTP 501 NotImplemented.
3. aws s3outposts create-endpoint --outpost-id <seeded op-id> --subnet-id subnet-0123456789abcdef0 --security-group-id sg-0123456789abcdef0 --access-type Private -> expect 200 with EndpointArn .../outpost/<OutpostId>/endpoint/<19-char id>; today: 501.
4. aws s3outposts list-endpoints --max-results 10 -> expect exactly one Endpoint whose EndpointArn matches the create response and whose SubnetId/SecurityGroupId/AccessType/CreationTime/Status reflect what was stored; today: 501.
5. aws s3outposts list-outposts-with-s3 --max-results 1 -> expect one item plus a NextToken that, passed back as --next-token, returns the following page and no NextToken on the last page.
6. aws s3outposts list-shared-endpoints --outpost-id <seeded op-id> --max-results 10 -> expect 200 with only endpoints owned by another account (empty list is fine); unknown outpost id -> 404 ResourceNotFoundException.
7. aws s3outposts delete-endpoint --endpoint-id <id from create> --outpost-id <seeded op-id> -> expect 200 empty body; today: 501.
8. aws s3outposts list-endpoints --max-results 10 -> expect the endpoint to be gone; repeat delete-endpoint -> expect 404 ResourceNotFoundException.
