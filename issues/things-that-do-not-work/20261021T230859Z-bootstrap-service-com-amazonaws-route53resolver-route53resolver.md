Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 36a9702edf97929891a031643d156f194fedffe9

# Bootstrap the Route53Resolver emulator

_Filed: 2026-10-21T23:08:59Z_
_Operation: service-com-amazonaws-route53resolver-route53resolver | Class: missing_

> _Note: this issue was identified against the code base at commit `36a9702edf97929891a031643d156f194fedffe9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 72 operations in `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/` is an unmodified scaffold (`//! SCAFFOLDED, NOT IMPLEMENTED:` + `Err(crate::scaffolded::Scaffolded::not_implemented(..))`) answering HTTP 501, and the service holds no state. Implement a FIRST, SMALL, stateful slice around the primary resource — the Resolver endpoint — so a realistic client script works end-to-end.

Wire protocol (OPERATIONS.md 'Wire protocol'): awsJson1_1, `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`, sigv4 signing name `route53resolver`.

## Happy path to implement (in order)

1. **CreateResolverEndpoint** — input `CreatorRequestId` (required), `SecurityGroupIds` (required), `Direction` (required: INBOUND | INBOUND_DELEGATION | OUTBOUND), `IpAddresses` (required, list<IpAddressRequest>, len 2..20), optional `Name`, `ResolverEndpointType` (IPV4|IPV6|DUALSTACK), `Protocols`, `Tags`. Must mint a new `Id` (e.g. `rslvr-out-<16 hex>`), build `Arn` (`arn:aws:route53resolver:<region>:<account>:resolver-endpoint/<id>`), persist the record, and return the stored `ResolverEndpoint` with `Id`, `Arn`, `Name`, `SecurityGroupIds`, `Direction`, `IpAddressCount` (= number of IpAddresses supplied), `HostVPCId` (derived from the request subnets), `Status` (`OPERATIONAL` is acceptable for the emulator), `StatusMessage`, `CreationTime`/`ModificationTime` as RFC3339 strings, `ResolverEndpointType`, `Protocols`, `CreatorRequestId`.
2. **GetResolverEndpoint** — input `ResolverEndpointId` (required); must look the record up in state and return the SAME `ResolverEndpoint` values that CreateResolverEndpoint returned (same Id/Arn/Name/Direction/IpAddressCount/CreationTime). Unknown id => `ResourceNotFoundException` (HTTP 400, modeled).
3. **ListResolverEndpoints** — input `MaxResults` (1..100), `NextToken`, `Filters`; output `ResolverEndpoints` + `NextToken`. Must enumerate the endpoints actually stored, honour `MaxResults` by truncating and emitting an opaque `NextToken` that resumes where the previous page stopped (@paginated inputToken=NextToken, outputToken=NextToken, items=ResolverEndpoints), and reject a token it did not mint with `InvalidNextTokenException`.
4. **UpdateResolverEndpoint** — input `ResolverEndpointId` (required), optional `Name`, `ResolverEndpointType`, `UpdateIpAddresses`, `Protocols`; must mutate the persisted record (only the members present in the request), bump `ModificationTime`, and return the updated `ResolverEndpoint`. A subsequent GetResolverEndpoint must observe the new `Name`. Unknown id => `ResourceNotFoundException`.
5. **ListResolverEndpointIpAddresses** — input `ResolverEndpointId` (required), `MaxResults`, `NextToken`; output `IpAddresses` (list<IpAddressResponse>: `IpId`, `SubnetId`, `Ip`, `Ipv6`, `Status`, `StatusMessage`, `CreationTime`, `ModificationTime`) + `NextToken`. Must return exactly the IP addresses stored at create/update time, with the `IpId`s assigned by the emulator. Unknown id => `ResourceNotFoundException`.
6. **DeleteResolverEndpoint** — input `ResolverEndpointId` (required); must remove the record from state (or mark it DELETING/DELETED and drop it) and return the final `ResolverEndpoint`. After this, GetResolverEndpoint must fail with `ResourceNotFoundException` and ListResolverEndpoints must no longer include the endpoint. Deleting twice => `ResourceNotFoundException`.

Every one of these six must compute its response from persisted state. No canned structs, no echoing the input back, no minting an Id without storing it: a create followed by a get/list must return what was created, and a delete must be observable by the next caller.

## State Model

In `src/state.rs` (`ComAmazonawsRoute53resolverRoute53resolverState`, reached via `crate::state::service_state(&*self.engine)`):

- `resolver_endpoints: HashMap<String /* endpoint id, e.g. rslvr-out-abc123 */, ResolverEndpointRecord>`
- `ResolverEndpointRecord { id: String, arn: String, name: Option<String>, creator_request_id: String, security_group_ids: Vec<String>, direction: ResolverEndpointDirection, host_vpc_id: Option<String>, status: ResolverEndpointStatus, status_message: Option<String>, endpoint_type: Option<ResolverEndpointType>, protocols: Vec<Protocol>, outpost_arn: Option<String>, preferred_instance_type: Option<String>, ip_addresses: Vec<IpAddressRecord>, tags: Vec<Tag>, creation_time: String /* RFC3339 */, modification_time: String }`
  - `IpAddressCount` is DERIVED as `ip_addresses.len()`, never stored independently.
- `IpAddressRecord { ip_id: String, subnet_id: String, ip: Option<String>, ipv6: Option<String>, status: IpAddressStatus, status_message: Option<String>, creation_time: String, modification_time: String }`
- `creator_request_id_index: HashMap<String /* CreatorRequestId */, String /* endpoint id */>` so a repeated CreatorRequestId returns the existing endpoint instead of a duplicate (or `ResourceExistsException`, which CreateResolverEndpoint models).
- `endpoint_id_seq: u64` — monotonic counter used to mint stable, unique endpoint and IP ids, so ids are reproducible within a run and never collide.

All of it lives behind the single shared engine handle (`state.lock()`), so every operation in the crate observes the same store across requests.

Out of scope for this first slice (stays 501 and will be filed as individual gap issues later): resolver rules and rule associations, query log configs and their associations, DNS Firewall (domain lists, rule groups, rules, associations), Outpost resolvers, DNSSEC/resolver configs, resource policies, and tagging operations.

## Evidence

OPERATIONS.md for this crate: 'Wire protocol' section (awsJson1_1, X-Amz-Target: Route53Resolver.<OperationName>, sdk id Route53Resolver); '## CreateResolverEndpoint' (required CreatorRequestId/SecurityGroupIds/Direction/IpAddresses len 2..20, output ResolverEndpoint); '## GetResolverEndpoint' and '## DeleteResolverEndpoint' (input ResolverEndpointId required, output ResolverEndpoint, ResourceNotFoundException); '## ListResolverEndpoints' and '## ListResolverEndpointIpAddresses' (@paginated inputToken=NextToken, outputToken=NextToken; MaxResults range 1..100; InvalidNextTokenException); '## UpdateResolverEndpoint' (ResolverEndpointId required plus optional Name/ResolverEndpointType/UpdateIpAddresses/Protocols). Smithy model shapes `ResolverEndpoint` (Id, Arn, Name, SecurityGroupIds, Direction, IpAddressCount, HostVPCId, Status, StatusMessage, CreationTime, ModificationTime, ResolverEndpointType, Protocols) and `IpAddressResponse` (IpId, SubnetId, Ip, Ipv6, Status, StatusMessage, CreationTime, ModificationTime) give the exact response members. Handler census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-route53resolver-route53resolver/src/ops/` matches all 72 files, including create_resolver_endpoint.rs, get_resolver_endpoint.rs, list_resolver_endpoints.rs, list_resolver_endpoint_ip_addresses.rs, update_resolver_endpoint.rs and delete_resolver_endpoint.rs — no tests assert any of this behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI route53resolver client at it.
2. CreateResolverEndpoint with CreatorRequestId=req-1, Name=my-outbound, Direction=OUTBOUND, SecurityGroupIds=[sg-123], IpAddresses=[{SubnetId: subnet-a, Ip: 10.0.0.10}, {SubnetId: subnet-b, Ip: 10.0.1.10}] — expect 200 with ResolverEndpoint.Id, Arn, Status, IpAddressCount=2; today the call answers HTTP 501 NotImplemented.
3. GetResolverEndpoint with the returned Id — expect the same Name/Direction/Arn/IpAddressCount/CreationTime that Create returned.
4. ListResolverEndpoints with MaxResults=1 after creating two endpoints — expect one endpoint plus a NextToken, and the second endpoint on the follow-up page with that token.
5. ListResolverEndpointIpAddresses with the Id — expect two IpAddressResponse entries whose SubnetId/Ip match the create request and that carry emulator-assigned IpIds.
6. UpdateResolverEndpoint with the Id and Name=my-outbound-renamed — expect the updated ResolverEndpoint, and a following GetResolverEndpoint must report Name=my-outbound-renamed with a bumped ModificationTime.
7. DeleteResolverEndpoint with the Id — expect 200; then GetResolverEndpoint on that Id must fail with ResourceNotFoundException and ListResolverEndpoints must not list it.
8. GetResolverEndpoint with an id that was never created (e.g. rslvr-out-doesnotexist) — expect ResourceNotFoundException, not 501.
