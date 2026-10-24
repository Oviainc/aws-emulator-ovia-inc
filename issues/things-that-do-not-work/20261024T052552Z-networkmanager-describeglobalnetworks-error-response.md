Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# DescribeGlobalNetworks silently drops unknown GlobalNetworkIds instead of raising ResourceNotFoundException

_Filed: 2026-10-24T05:25:51Z_
_Operation: DescribeGlobalNetworks | Class: error-response_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeGlobalNetworks models ResourceNotFoundException (HTTP 404), and the operation binds no path resource at all (GET /global-networks) — the only resource a caller can address is the GlobalNetworkIds query filter, so an id in that list that names no global network is the one and only situation in which this operation can produce its modeled 404.

Expected: DescribeGlobalNetworks(GlobalNetworkIds=[<id that was never created / was deleted>]) answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException (the service's fixed "Resource not found." message plus ResourceId/ResourceType), both when every requested id is unknown and when a known id is mixed with an unknown one.

Actual: the handler treats GlobalNetworkIds as a pure in-memory filter — `.filter(|(id, _)| requested.is_empty() || requested.iter().any(|want| want == *id))` — so an unknown id contributes nothing and the call answers HTTP 200 with an empty (or partial) GlobalNetworks list. ResourceNotFoundException is declared in DescribeGlobalNetworksErrorModeled but is never constructed anywhere in the handler, so this operation can never produce its modeled 404.

## Evidence

Smithy model (smithy_query operation=DescribeGlobalNetworks): errors include {shape: ResourceNotFoundException, httpError: 404}; http = GET /global-networks with GlobalNetworkIds bound as @httpQuery("globalNetworkIds") — there is no @httpLabel resource on this operation, so the filter list is the only addressable resource. Handler: services/service-com-amazonaws-networkmanager-networkmanager/src/ops/describe_global_networks.rs lines 160-172 (filter only; the ResourceNotFoundException variant declared at line 77 is unused). Existing test coverage asserts the CURRENT (divergent) behavior: tests/all/global_network_lifecycle.rs, test `listings_filter_by_the_ids_the_request_names`, rows "an id that names nothing" -> [] and "a known and an unknown id" -> [known] both expect HTTP 200, so the gap is unguarded and that test will need to change with the fix. No existing filed issue covers DescribeGlobalNetworks id resolution (the open backlog covers ARN scoping, the delete/update state machine and PutResourcePolicy).

## Steps to Reproduce

1. POST /global-networks with {"Description":"kept"} and note the minted GlobalNetworkId
2. GET /global-networks?globalNetworkIds=global-network-00000000000000000 (an id of the right shape that was never minted)
3. Observe HTTP 200 with {"GlobalNetworks": []}; AWS answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException
4. GET /global-networks?globalNetworkIds=<known id>&globalNetworkIds=global-network-00000000000000000
5. Observe HTTP 200 listing only the known network, where the modeled contract raises the 404 for the unresolvable id
