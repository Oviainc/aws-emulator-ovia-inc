Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# CreateEndpoint accepts and persists CustomerOwnedIpv4Pool on a Private-access endpoint

_Filed: 2026-10-24T18:36:31Z_
_Operation: CreateEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a CoIP pool belongs only to an endpoint whose access type is `CustomerOwnedIp`. `CreateEndpointRequest$AccessType` documents the two choices as mutually exclusive network paths — "To use the Amazon Web Services VPC, choose Private. To use the endpoint with an on-premises network, choose CustomerOwnedIp. If you choose CustomerOwnedIp, you must also provide the customer-owned IP address pool (CoIP pool)" — and `CustomerOwnedIpv4Pool` is documented as the pool "IP addresses are allocated from ... for the endpoint". A `Private` endpoint allocates its addresses from the VPC subnet, so a request that pairs `AccessType: Private` (or omits AccessType, which defaults to Private) with a `CustomerOwnedIpv4Pool` is a contradictory input the service refuses with its modeled ValidationException (400); it certainly never materialises an endpoint that reports a CoIP pool it does not use.

Actual: the handler enforces only the one direction of the rule (CustomerOwnedIp without a pool -> ValidationException). The reverse pairing falls through untouched: `access_type` is set to `Private` and `customer_owned_ipv4_pool: input.customer_owned_ipv4_pool` is stored verbatim on the `EndpointRecord`. CreateEndpoint answers 200, and every later `ListEndpoints` / `ListSharedEndpoints` reports `Endpoint{ AccessType: "Private", CustomerOwnedIpv4Pool: "ipv4pool-coip-..." }` — a combination the real service never emits, since `Endpoint$CustomerOwnedIpv4Pool` is "the ID of the customer-owned IPv4 address pool used for the endpoint" and a Private endpoint uses none. A client that branches on the presence of `CustomerOwnedIpv4Pool` to decide whether an endpoint is reachable from the on-premises network is misled.

## Evidence

Spec: smithy_query(query="shape", shape="CreateEndpointRequest") — AccessType doc ties the CoIP pool to `CustomerOwnedIp` and notes "Private is the default access type value"; smithy_query(query="shape", shape="Endpoint") — `CustomerOwnedIpv4Pool` is "The ID of the customer-owned IPv4 address pool used for the endpoint". CreateEndpoint models ValidationException (HTTP 400) for exactly this class of refusal.
Handler: services/service-com-amazonaws-s3outposts-s3outposts/src/ops/create_endpoint.rs:174-189 checks only `access_type == CustomerOwnedIp && customer_owned_ipv4_pool.is_none()`; lines 246-266 then store `access_type` and `customer_owned_ipv4_pool` unconditionally, and src/state.rs:248-274 (`EndpointRecord::to_endpoint`) copies the pool straight into every listing projection.
Tests: tests/all/endpoint_lifecycle.rs::customer_owned_ip_without_a_coip_pool_is_refused (lines 1088-1146) enumerates only "CustomerOwnedIp with no pool" (400), "CustomerOwnedIp with its pool" (200), "Private needs no pool" (200) and "no AccessType at all" (200) — no case sends a pool alongside Private, so the inconsistent pairing is wholly untested.

## Steps to Reproduce

1. Start the emulator with an Outpost provisioned: S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248"
2. POST /S3Outposts/CreateEndpoint with {"OutpostId":"op-0a1b2c3d4e5f60718","SubnetId":"subnet-0a1b2c3d4e5f60718","SecurityGroupId":"sg-0a1b2c3d4e5f60718","AccessType":"Private","CustomerOwnedIpv4Pool":"ipv4pool-coip-0a1b2c3d4e5f60713"}
3. Expected: 400 ValidationException — a CoIP pool cannot be supplied for a Private-access endpoint
4. Observed: 200 with an EndpointArn
5. GET /S3Outposts/ListEndpoints?maxResults=100 and look at the new endpoint: it reports AccessType=Private together with CustomerOwnedIpv4Pool=ipv4pool-coip-0a1b2c3d4e5f60713
6. Repeat step 2 with the AccessType member omitted entirely (defaults to Private): the pool is stored and reported the same way
