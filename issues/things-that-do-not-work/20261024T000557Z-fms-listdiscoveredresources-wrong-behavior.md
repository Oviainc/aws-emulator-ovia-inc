Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# ListDiscoveredResources can never report a resource: no code path ever populates the discovery inventory

_Filed: 2026-10-24T00:05:56Z_
_Operation: ListDiscoveredResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation returns 'an array of resources in the organization's accounts that are available to be associated with a resource set'. The handler reads `store.discovered_resources`, which NO operation in the crate ever writes — it is only ever populated by the crate's own tests reaching into the store directly. So for every client, for every MemberAccountIds/ResourceType combination, the operation answers 200 with an empty `Items` list and no NextToken, whereas AWS reports the organization's matching resources (e.g. AWS::EC2::VPC / AWS::EC2::Instance / AWS::NetworkFirewall::Firewall resources owned by the named member account). The gap also degrades BatchAssociateResource, whose organization-account set is partly derived from the same inventory.

Expected: the inventory is derived from the resources the engine actually holds for the relevant services (the repo already does cross-service store reads elsewhere), so a resource created in e.g. the EC2 emulator becomes discoverable and associable. Actual: the inventory is permanently empty, which makes the operation indistinguishable from an unimplemented one while still answering 200.

## Evidence

Model: ListDiscoveredResources takes MemberAccountIds + ResourceType and outputs `Items` (list<DiscoveredResource> with URI, AccountId, Type, Name). Handler: `src/ops/list_discovered_resources.rs` lines 231-254 filter `store.discovered_resources`. State: `src/state.rs` field `discovered_resources` (line 413) — grep of `src/` shows no handler inserting into it; the only writers are the test helpers in `tests/all/administrator_gated_operations.rs` (`with_store(...)` seeding) . Cross-service precedent for sourcing it: `services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/list_s3_buckets.rs`. Test coverage: `tests/all/administrator_gated_operations.rs::list_discovered_resources_filters_and_pages_the_inventory` only passes because it seeds the store by hand, so no test covers the client-visible path.

## Steps to Reproduce

1. Boot the emulator and call AssociateAdminAccount.
2. Create resources the organization owns through the other emulators (e.g. an EC2 VPC or instance in the calling account).
3. POST ListDiscoveredResources with {"MemberAccountIds":["<account>"],"ResourceType":"AWS::EC2::VPC"}.
4. Observed: 200 with "Items": []. Expected: 200 with a DiscoveredResource entry per matching resource (URI/ARN, AccountId, Type), pageable by MaxResults/NextToken.
