Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the rtbfabric emulator

_Filed: 2026-10-21T21:28:09Z_
_Operation: service-com-amazonaws-rtbfabric-rtbfabric | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The RTB Fabric (`rtbfabric`, restJson1, sigv4 name `rtbfabric`) crate is a fresh scaffold: all 36 operations under `services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/` still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return HTTP 501 `NotImplemented`, and `src/state.rs` holds `ComAmazonawsRtbfabricRtbfabricState { /* TODO: add service-specific state fields */ }` — zero state. No client workflow can run.

Implement a FIRST, SMALL happy-path slice: the gateway-pair + link lifecycle, which is this service's core loop (a requester gateway links to a responder gateway; the Link is the primary resource). Six operations, in order:

1. **CreateRequesterGateway** — `POST /requester-gateway`. Input: `vpcId` (required, `^vpc-[a-f0-9]{8,17}$`), `subnetIds` (required), `securityGroupIds` (required), `clientToken` (required, @idempotencyToken), `description`, `tags`. Output: a NEWLY MINTED `gatewayId` matching `^rtb-gw-[a-z0-9-]{1,25}$`, a generated `domainName` matching the modeled domain pattern, and `status` (`PENDING_CREATION` or `ACTIVE` — pick one and be consistent with what Get returns). Store the gateway. Honor `@idempotent`/`clientToken`: the same token + same parameters must return the SAME `gatewayId`, not a second gateway.
2. **CreateResponderGateway** — `POST /responder-gateway`. Input: `vpcId`, `subnetIds`, `securityGroupIds`, `port`, `protocol` (`HTTP|HTTPS`), `clientToken` (all required), plus optional `domainName`, `listenerConfig`, `description`, `tags`, `gatewayType`, `clientRoutingPolicy`. Output: new `gatewayId`, `status`, and `listenerConfig`/`clientRoutingPolicy` echoed from what was PERSISTED. Needed so CreateLink has a real peer.
3. **GetRequesterGateway** — `GET /requester-gateway/{gatewayId}`. Must return the stored record: `gatewayId`, `vpcId`, `subnetIds`, `securityGroupIds`, `description`, `tags`, `domainName`, `status`, `createdAt`, `updatedAt`, and `activeLinksCount`/`totalLinksCount` COMPUTED by counting the links stored against that gateway (not 0 constants). Unknown id -> `ResourceNotFoundException` (404).
4. **CreateLink** — `POST /gateway/{gatewayId}/create-link`. Input: `gatewayId` (@httpLabel), `peerGatewayId` (required), `logSettings` (required), optional `attributes`, `httpResponderAllowed`, `tags`, `timeoutInMillis` (100..5000). Must 404 if either `gatewayId` or `peerGatewayId` is not a stored gateway. On success mint a `linkId` matching `^link-[a-z0-9-]{1,25}$`, persist the link under the gateway, and return `gatewayId`, `peerGatewayId`, `linkId`, `status` (`PENDING_REQUEST`/`REQUESTED` for a fresh request), `direction`, `connectivityType` (`DEFAULT`), `createdAt`, `updatedAt`, `attributes`, `logSettings` — all from the stored record.
5. **GetLink** — `GET /gateway/{gatewayId}/link/{linkId}`. Returns exactly what CreateLink persisted, including `tags`, `httpResponderAllowed` and `timeoutInMillis` (which CreateLink's output shape does not carry — so they must be read back from state, proving the input was stored and not echoed). Unknown gateway or link -> 404 `ResourceNotFoundException`.
6. **DeleteLink** — `DELETE /gateway/{gatewayId}/link/{linkId}`. Output: `linkId` + terminal `status` (`DELETED` or `PENDING_DELETION`). After this, GetLink must 404 (or report the deleted status consistently), and `totalLinksCount`/`activeLinksCount` on GetRequesterGateway must drop accordingly. `@idempotent`: a second delete of the same link must not 500.

Every one of these six must compute its response from persisted state. No canned bodies, no echoing the request back, no minting an id that is never stored: a create followed by a get must return what was created, and a delete must be observable by the next reader. Validation already enforced by the generated schema layer (patterns, lengths, ranges) does not need re-checking in the handler; referential checks (gateway/link exists, peer exists) do, and must use the modeled `ResourceNotFoundException` (404) / `ConflictException` (409) shapes rather than 501 or 500.

## State Model

Add to `ComAmazonawsRtbfabricRtbfabricState`:

- `requester_gateways: HashMap<String /* gatewayId */, RequesterGateway>` where `RequesterGateway { gateway_id, domain_name, status: RequesterGatewayStatus, vpc_id, subnet_ids: Vec<String>, security_group_ids: Vec<String>, description: Option<String>, tags: HashMap<String,String>, created_at, updated_at }`
- `responder_gateways: HashMap<String /* gatewayId */, ResponderGateway>` where `ResponderGateway { gateway_id, status: ResponderGatewayStatus, vpc_id, subnet_ids, security_group_ids, domain_name: Option<String>, port, protocol, listener_config: Option<ListenerConfig>, gateway_type, client_routing_policy, description, tags, created_at, updated_at }`
- `links: HashMap<(String /* gatewayId */, String /* linkId */), Link>` where `Link { link_id, gateway_id, peer_gateway_id, status: LinkStatus, direction, connectivity_type, attributes, log_settings, http_responder_allowed: Option<bool>, timeout_in_millis: Option<i64>, flow_modules: Vec<ModuleConfiguration>, pending_flow_modules: Vec<ModuleConfiguration>, tags, created_at, updated_at }`
- `idempotency: HashMap<String /* clientToken */, String /* gatewayId */>` so the two Create*Gateway ops satisfy their `@idempotencyToken` contract
- `next_id: u64` (or a counter per resource kind) so `rtb-gw-...` / `link-...` ids are unique per emulator instance

`activeLinksCount` / `totalLinksCount` are derived by scanning `links` for the gateway — they are never stored fields.

The remaining 30 operations (external links, routing rules, certificates, Update*, Accept/RejectLink, the List* ops, tagging) stay 501 and will be filed as individual gap issues. The immediate follow-ons after this slice are ListRequesterGateways / ListLinks (both `@paginated` with `nextToken`) and AcceptLink, which flips a stored link from `REQUESTED` to `ACCEPTED`/`ACTIVE`.

## Evidence

OPERATIONS.md (crate root) documents all 36 operations with routes, members and errors: CreateRequesterGateway `POST /requester-gateway` (required vpcId/subnetIds/securityGroupIds/clientToken; output gatewayId/domainName/status), GetRequesterGateway `GET /requester-gateway/{gatewayId}` (output includes activeLinksCount/totalLinksCount), CreateResponderGateway `POST /responder-gateway` (required port/protocol/clientToken), CreateLink `POST /gateway/{gatewayId}/create-link` (required peerGatewayId + logSettings; output linkId `^link-[a-z0-9-]{1,25}$`, status LinkStatus), GetLink `GET /gateway/{gatewayId}/link/{linkId}` (output adds tags/httpResponderAllowed/timeoutInMillis), DeleteLink `DELETE /gateway/{gatewayId}/link/{linkId}` (output linkId+status). All six declare ResourceNotFoundException (404) and ValidationException (400); CreateLink/DeleteLink also ConflictException (409). Handler proof: `src/ops/create_requester_gateway.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 186 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateRequesterGateway"))`; every other file in `src/ops/` is the same generated stub. `src/state.rs` line 8 is `// TODO: add service-specific state fields`, so no operation could persist anything even if it returned 200. No tests assert any behavior (only `tests/state_roundtrip.rs`, a compiled example of the state API).

## Steps to Reproduce

1. POST /requester-gateway with {vpcId: "vpc-0123456789abcdef0", subnetIds: ["subnet-1"], securityGroupIds: ["sg-1"], clientToken: "tok-1", description: "req", tags: {"env":"test"}} -> expect 200 with a fresh gatewayId matching ^rtb-gw-[a-z0-9-]{1,25}$, a domainName and a status; today 501.
2. Repeat the identical POST with clientToken "tok-1" -> expect the SAME gatewayId (idempotent), not a second gateway.
3. POST /responder-gateway with {vpcId: "vpc-0123456789abcdef0", subnetIds: ["subnet-1"], securityGroupIds: ["sg-1"], port: 443, protocol: "HTTPS", clientToken: "tok-2"} -> expect 200 with a second gatewayId.
4. GET /requester-gateway/{requesterGatewayId} -> expect 200 echoing the stored vpcId, subnetIds, securityGroupIds, description, tags, and totalLinksCount == 0.
5. POST /gateway/{requesterGatewayId}/create-link with {peerGatewayId: <responderGatewayId>, logSettings: {...}, httpResponderAllowed: true, timeoutInMillis: 1500, tags: {"team":"ads"}} -> expect 200 with a fresh linkId and a non-terminal status.
6. POST /gateway/{requesterGatewayId}/create-link with peerGatewayId "rtb-gw-does-not-exist" -> expect 404 ResourceNotFoundException, not 200 and not 501.
7. GET /gateway/{requesterGatewayId}/link/{linkId} -> expect 200 with httpResponderAllowed=true, timeoutInMillis=1500 and tags {"team":"ads"} read back from state.
8. GET /requester-gateway/{requesterGatewayId} again -> expect totalLinksCount == 1 (derived from stored links).
9. DELETE /gateway/{requesterGatewayId}/link/{linkId} -> expect 200 with the linkId and a DELETED/PENDING_DELETION status.
10. GET /gateway/{requesterGatewayId}/link/{linkId} -> expect 404 ResourceNotFoundException (link gone), and GetRequesterGateway's totalLinksCount back to 0.
