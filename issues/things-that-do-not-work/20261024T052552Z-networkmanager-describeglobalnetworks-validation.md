Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# DescribeGlobalNetworks accepts more than 10 GlobalNetworkIds, which the member documents as its maximum

_Filed: 2026-10-24T05:25:51Z_
_Operation: DescribeGlobalNetworks | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The GlobalNetworkIds filter is documented as "The IDs of one or more global networks. The maximum is 10." The handler applies no bound on the list length: it unwraps the list and uses it as a filter of any size.

Expected: a request carrying 11 or more globalNetworkIds values is refused with the modeled ValidationException (HTTP 400) naming the GlobalNetworkIds member, before any listing is computed.

Actual: the request is accepted and a 200 page is returned for an arbitrarily long id list.

## Evidence

Smithy model documentation for the member (smithy_query query=doc shape=DescribeGlobalNetworksRequest member=GlobalNetworkIds): "The IDs of one or more global networks. The maximum is 10." — a documented constraint the modeled GlobalNetworkIdList does not express as @length, exactly like the device/link descriptor maxima this crate already enforces in handler code (src/ops/create_device.rs `descriptor_problem`, src/ops/create_link.rs `descriptor_problem`). Handler: src/ops/describe_global_networks.rs lines 157-168 — `input.global_network_ids.unwrap_or_default()` is used directly as a filter with no length check. grep of src/ and tests/ for a list-length check (`ids.len()`, `requested.len()`, "maximum is 10") finds only the doc comment, so nothing enforces or asserts it.

## Steps to Reproduce

1. GET /global-networks?globalNetworkIds=global-network-00000000000000001&globalNetworkIds=...&globalNetworkIds=global-network-00000000000000011 (11 distinct ids)
2. Observe HTTP 200 instead of the documented refusal (ValidationException, HTTP 400, naming GlobalNetworkIds)
