Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# The ClientToken replay/conflict walk is open-coded in ten handlers

_Filed: 2026-10-28T02:37:10Z_
_Operation: CreateVpcAttachment | Class: maintainability_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation of this crate that models `@idempotencyToken` implements the same four-step rule inline: normalise the token (`as_deref().filter(|t| !t.is_empty())`), scan the relevant store for a record whose `client_token` equals it, compare the stored record field-by-field against the request to decide "same request", then either replay the stored record's shape or answer `ConflictException` with the message "client token {token:?} already created ... with a different request" naming the record's id and resource type. Ten copies exist (nine creates plus PutCoreNetworkPolicy), each differing only in the store it scans and the per-kind field comparison, so the shared half — token normalisation, the lookup, the replay/conflict branch and the refusal's shape — can drift between them. Extracting it as one helper parameterised by the store iterator and a "same request" predicate leaves every response byte-identical.

## Evidence

The same walk, ten times (identical structure, same message):
- src/ops/create_vpc_attachment.rs:247-299 (`.find(|record| record.client_token.as_deref() == Some(token))` … "client token {token:?} already created attachment {}, with a different request")
- src/ops/create_connect_attachment.rs:272-321
- src/ops/create_site_to_site_vpn_attachment.rs:254 (same `.find`)
- src/ops/create_direct_connect_gateway_attachment.rs:251-296
- src/ops/create_transit_gateway_route_table_attachment.rs:266-308
- src/ops/create_transit_gateway_peering.rs:220-253
- src/ops/create_connect_peer.rs:260-297
- src/ops/create_core_network.rs:209-243
- src/ops/create_core_network_prefix_list_association.rs:206-239
- src/ops/put_core_network_policy.rs:183-216

Existing coverage that must stay green unchanged: tests/all/attachment_lifecycle.rs:782 and :819 ("vpc-token", "connect-token"), tests/all/transit_gateway_route_table_attachment_lifecycle.rs:1024-1162 ("token-one"/"token-two"/"token-three"), tests/all/site_to_site_vpn_attachment_lifecycle.rs:594-684, tests/all/direct_connect_gateway_attachment_lifecycle.rs:1163-1190, tests/all/transit_gateway_peering.rs:86-643, tests/all/connect_peer_lifecycle.rs:914, tests/all/core_network_lifecycle.rs:598, tests/all/core_network_policy_versions.rs:708-740, tests/all/core_network_prefix_list_association.rs:468-488, plus the bounds suites create_vpc_attachment_bounds.rs:421-474, create_connect_peer_bounds.rs:186-419 and transit_gateway_peering_create_bounds.rs:114-236.

## Steps to Reproduce

1. grep -n "client_token.as_deref() == Some(token)" services/service-com-amazonaws-networkmanager-networkmanager/src/ops/*.rs → ten handlers
2. Read any two of them (e.g. create_vpc_attachment.rs:247-299 and create_connect_peer.rs:260-297): token normalisation, the store scan, the same-request replay and the ConflictException text are the same logic written twice
3. A fix that changed the refusal in one copy only — or that fixed an empty-token edge in one — would silently diverge from the other nine
