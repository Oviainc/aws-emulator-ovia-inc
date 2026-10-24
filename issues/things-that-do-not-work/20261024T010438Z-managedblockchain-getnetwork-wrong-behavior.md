Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Resource store is not partitioned by Region: a network/member/node/accessor created in one Region is readable and mutable from every other

_Filed: 2026-10-24T01:04:38Z_
_Operation: GetNetwork | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Managed Blockchain is a regional service: a network, member, node, proposal or accessor created through the us-east-1 endpoint does not exist in eu-west-1, where GetNetwork/GetMember/GetNode/GetProposal/GetAccessor answer ResourceNotFoundException (404) and the List* operations return an empty page.

The emulator keeps ONE global store for the whole service. `crate::state::request_region(ctx)` is read off the request's SigV4 credential scope but is used only to MINT ARNs and endpoints; nothing keys state by region and no lookup compares the stored region with the caller's. Expected: CreateNetwork signed for us-east-1 followed by GetNetwork signed for eu-west-1 → 404 ResourceNotFoundException, and ListNetworks in eu-west-1 → empty. Actual: the second call answers 200 with the full Network record, whose Arn (and VpcEndpointServiceName / OrderingServiceEndpoint) still name us-east-1, so one call reports a resource as living in a Region the caller is not addressing. The same holds for every id-addressed read and for the mutations (DeleteNode, UpdateNode, RejectInvitation, VoteOnProposal, DeleteAccessor) — a resource created in one Region can be mutated from another.

Note the tag plane is already region-correct by accident (it resolves by the stored ARN string, so another Region's ARN is not found — tests/all/resource_tags.rs:500), which makes the id-addressed path's region blindness an internal inconsistency as well.

## Evidence

Spec/well-known behavior: the model's `aws.api#service` endpointPrefix `managedblockchain` with per-Region endpoints, and the Region segment of every ARN this service mints (`crate::state::network_arn` → arn:aws:managedblockchain:<region>:<account>:networks/<id>), establish that AMB resources are per-Region; cross-Region reads of a resource id 404 on real AWS.
Handler/state: services/service-com-amazonaws-managedblockchain-taigawebservice/src/state.rs — `service_state()` returns a single `engine::ServiceState<ComAmazonawsManagedblockchainTaigawebserviceState>` with `networks`/`invitations`/`accessors` maps keyed by id only; `request_region()` is documented as "The region this request was signed for" and is passed solely to the *_arn / endpoint helpers. Handlers such as src/ops/get_network.rs, get_member.rs, get_node.rs, get_proposal.rs, get_accessor.rs and the List* handlers discard `ctx` entirely (`let _ = ctx;`) and look resources up by id.
Tests: no test signs two different regions and asserts isolation; tests/all/network_and_node_lifecycle.rs and update_node_log_publishing.rs pin a single REGION (eu-west-1) and only assert the ARNs carry it. tests/all/resource_tags.rs:500 asserts a foreign-Region ARN is not found for tagging, which is the behavior the id path lacks.

## Steps to Reproduce

1. Boot the emulator and create a network with a client signed for us-east-1: east = boto3.client('managedblockchain', region_name='us-east-1', endpoint_url=<emulator>); nid = east.create_network(...)['NetworkId'] (ClientRequestToken, Name, Framework='HYPERLEDGER_FABRIC', FrameworkVersion='2.2', VotingPolicy, MemberConfiguration).
2. Create a second client signed for another Region: west = boto3.client('managedblockchain', region_name='eu-west-1', endpoint_url=<emulator>).
3. Call west.get_network(NetworkId=nid). Observe HTTP 200 with Network.Arn containing 'us-east-1'; expected 404 ResourceNotFoundException.
4. Call west.list_networks(). Observe the us-east-1 network listed; expected an empty Networks list.
5. Repeat with an accessor (east.create_accessor → west.get_accessor / west.delete_accessor): the foreign-Region call succeeds and mutates state.
