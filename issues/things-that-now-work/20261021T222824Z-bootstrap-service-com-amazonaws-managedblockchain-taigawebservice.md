Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the ManagedBlockchain emulator

_Filed: 2026-10-21T22:28:24Z_
_Operation: service-com-amazonaws-managedblockchain-taigawebservice | Class: missing_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 27 operations in `services/service-com-amazonaws-managedblockchain-taigawebservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. The service holds no state, so no client workflow works at all.

Implement a FIRST, SMALL happy-path slice for the primary resource (Network, and the Node that runs on it). Wire protocol is `restJson1` (SDK id `ManagedBlockchain`, signing name `managedblockchain`). The six operations below, in order, must all compute their responses from persisted state in the shared engine store (`crate::state::service_state(&*self.engine)`), never from canned or echoed values:

1. **CreateNetwork** — `POST /networks`. Input: `ClientRequestToken` (required, @idempotencyToken), `Name` (required, 1..64, pattern \S), `Description`, `Framework` (required, ETHEREUM|HYPERLEDGER_FABRIC), `FrameworkVersion` (required), `FrameworkConfiguration`, `VotingPolicy` (required), `MemberConfiguration` (required — its `Name` and `FrameworkConfiguration` are @required), `Tags`. Must mint and STORE a new network id (1..32 chars) plus the first member described by `MemberConfiguration`, and return BOTH `NetworkId` and `MemberId`. Repeating the same `ClientRequestToken` must return the same ids rather than creating a second network.
2. **GetNetwork** — `GET /networks/{NetworkId}`. Returns the stored `Network`: `Id`, `Name`, `Description`, `Framework`, `FrameworkVersion`, `FrameworkAttributes`, `VotingPolicy`, `Status` (AVAILABLE), `CreationDate` (date-time), `Tags`, `Arn`, `VpcEndpointServiceName`. Every field must reflect what CreateNetwork persisted. Unknown id => `ResourceNotFoundException` (404), not 501 and not an empty 200.
3. **ListNetworks** — `GET /networks`. Returns `Networks` as `NetworkSummary` entries (`Id`, `Name`, `Description`, `Framework`, `FrameworkVersion`, `Status`, `CreationDate`, `Arn`) built from the same store, so a just-created network appears. Honour the `name`, `framework` and `status` query filters and `maxResults` (1..10) with a `NextToken` that actually resumes.
4. **CreateNode** — `POST /networks/{NetworkId}/nodes`. Input: `ClientRequestToken` (required), `NetworkId` (@httpLabel, required), `MemberId`, `NodeConfiguration` (required: `InstanceType` required, `AvailabilityZone`, `StateDB`, `LogPublishingConfiguration`), `Tags`. Must reject an unknown `NetworkId` with `ResourceNotFoundException` (404), store the node under its network/member, and return the minted `NodeId`.
5. **GetNode** — `GET /networks/{NetworkId}/nodes/{NodeId}` (optional `memberId` query). Returns the stored `Node`: `NetworkId`, `MemberId`, `Id`, `InstanceType`, `AvailabilityZone`, `FrameworkAttributes`, `LogPublishingConfiguration`, `StateDB`, `Status` (AVAILABLE), `CreationDate`, `Tags`, `Arn`. `InstanceType`/`AvailabilityZone` must equal what CreateNode persisted.
6. **DeleteNode** — `DELETE /networks/{NetworkId}/nodes/{NodeId}`. Removes the node from the store (empty 200 body). A following GetNode must fail with `ResourceNotFoundException` (404) and ListNodes must no longer include it; deleting an unknown node id is also a 404.

Also implement **ListNodes** (`GET /networks/{NetworkId}/nodes`) if it comes for free from the node store, so the delete can be verified by listing; it is otherwise acceptable to leave it stubbed for the first slice.

State must survive across requests in the ONE shared engine (per the crate README: reach it via `self.engine`, never `InMemoryEngine::new()` inside a handler). Ids, ARNs and CreationDate are generated once at create time and stored — not recomputed per read.

## State Model

Store a single struct on the service state, e.g.:

- `networks: HashMap<String /*NetworkId*/, Network>` where `Network { id, name, description, framework, framework_version, framework_attributes, framework_configuration, voting_policy, status: NetworkStatus, creation_date: DateTime, tags: HashMap<String,String>, arn: String, vpc_endpoint_service_name: String, members: HashMap<String /*MemberId*/, Member>, nodes: HashMap<String /*NodeId*/, Node> }`
- `Member { id, network_id, name, description, framework_attributes, log_publishing_configuration, status: MemberStatus, creation_date, tags, arn, kms_key_arn, is_owned: bool }` — the owning member created by CreateNetwork from `MemberConfiguration`.
- `Node { id, network_id, member_id: Option<String>, instance_type, availability_zone, framework_attributes, log_publishing_configuration, state_db, status: NodeStatus, creation_date, tags, arn }`
- `idempotency: HashMap<String /*ClientRequestToken*/, CreatedIds{ network_id, member_id, node_id }>` so repeated tokens replay the original ids.

ARNs follow `arn:aws:managedblockchain:<region>:<account>:networks/<NetworkId>` (and `.../nodes/<NodeId>`), matching the modeled pattern `^arn:.+:.+:.+:.+:.+$`.

The remaining 21 operations (members, proposals/votes, invitations, accessors, tagging, updates) stay 501 for now and are tracked as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) documents all 27 operations with their routes, input/output members and errors: CreateNetwork `POST /networks` -> `NetworkId`+`MemberId`; GetNetwork `GET /networks/{NetworkId}` -> `Network`; ListNetworks `GET /networks` -> `Networks`+`NextToken` (@paginated, maxResults 1..10); CreateNode `POST /networks/{NetworkId}/nodes` -> `NodeId`; GetNode `GET /networks/{NetworkId}/nodes/{NodeId}` -> `Node`; DeleteNode `DELETE /networks/{NetworkId}/nodes/{NodeId}` -> no members, all declaring `ResourceNotFoundException (HTTP 404)` for unknown ids. smithy_query on shapes Network/Node/Member/MemberConfiguration/NodeConfiguration/NetworkSummary/NodeSummary gives the field lists cited above (Network.Id/Name/Framework/FrameworkVersion/VotingPolicy/Status/CreationDate/Tags/Arn; NodeConfiguration.InstanceType @required). Census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 27 handler files, i.e. the entire service answers 501 and persists nothing.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `managedblockchain` client at it.
2. CreateNetwork with ClientRequestToken=tok-1, Name=my-net, Framework=HYPERLEDGER_FABRIC, FrameworkVersion=2.2, a VotingPolicy.ApprovalThresholdPolicy and a MemberConfiguration{Name=org1, FrameworkConfiguration=...}; expect 200 with a NetworkId and a MemberId (currently 501).
3. Repeat the identical CreateNetwork call with ClientRequestToken=tok-1 and expect the SAME NetworkId/MemberId back (idempotency), not a second network.
4. GetNetwork with that NetworkId and expect Network.Name=my-net, Framework=HYPERLEDGER_FABRIC, FrameworkVersion=2.2, Status=AVAILABLE, a CreationDate and an Arn.
5. ListNetworks and expect the new network in Networks as a NetworkSummary with the same Id/Name/Framework; ListNetworks with name=nope must return an empty list.
6. CreateNode on that NetworkId with MemberId from step 2 and NodeConfiguration{InstanceType=bc.t3.small, AvailabilityZone=us-east-1a}; expect 200 with a NodeId.
7. GetNode with that NetworkId/NodeId and expect Node.InstanceType=bc.t3.small, AvailabilityZone=us-east-1a, Status=AVAILABLE, NetworkId/MemberId matching the create.
8. DeleteNode with that NetworkId/NodeId (expect 200), then GetNode again and expect ResourceNotFoundException with HTTP 404.
9. GetNetwork with NetworkId=n-doesnotexist00000000000000000 and expect ResourceNotFoundException (404), not 501.
