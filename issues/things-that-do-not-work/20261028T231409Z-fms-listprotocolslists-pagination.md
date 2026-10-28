Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 8269d8069d160c27d0acf36557f2407fe5d81546

# A ListProtocolsLists NextToken is not scoped to the DefaultLists namespace that minted it, so the other namespace's page silently starts mid-list

_Filed: 2026-10-28T23:14:09Z_
_Operation: ListProtocolsLists | Class: pagination_

> _Note: this issue was identified against the code base at commit `8269d8069d160c27d0acf36557f2407fe5d81546`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The protocols-list twin of the same defect: `ListProtocolsLists` uses the token's decoded `ListId` as the lower bound of a range over `store.protocols_lists` — one `BTreeMap` holding BOTH the Firewall Manager MANAGED and the customer namespace — and applies `DefaultLists` only as a filter afterwards. `crate::state::decode_page_token` validates nothing but the 36-character `ListId` shape, so a token minted by a `DefaultLists=false` page is accepted by a `DefaultLists=true` request and vice versa.

Every list of the requested namespace whose `ListId` sorts below the foreign token's key is then skipped, and since the walk continues to the end of the map no `NextToken` is returned — the caller receives a silently short listing it cannot distinguish from a complete one. Two other paginated handlers in this same crate (`ListThirdPartyFirewallFirewallPolicies`, `GetProtectionStatus`) prefix-key their inventory per sub-collection and refuse a token that does not carry the prefix.

## Evidence

`services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_protocols_lists.rs:205-216`:
```
        // `protocols_lists` is ordered by `ListId`, so the boundary is a key:
        // resume at the first id at or after the token's. ...
        let lower = start.unwrap_or_default();
        // `DefaultLists` chooses the namespace: a Firewall Manager managed list
        // and a customer list are separate resources that share one id space,
        // and the stored flag is what tells them apart ...
        let mut remaining = store
            .protocols_lists
            .range(lower..)
            .filter(|(_, record)| record.default_list == input.default_lists);
```
and the shape-only token check, list_protocols_lists.rs:193-203:
```
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_page_token(token) {
```
`crate::state::decode_page_token` (`src/state.rs:2991-3007`) only requires "a 36-character identifier this service mints", which both namespaces' ids are.

The sub-collection-scoped sibling in the same crate, `src/ops/list_third_party_firewall_firewall_policies.rs:238-245`: "A token this service did not mint, or one minted for ANOTHER vendor's page, names no boundary in this vendor's policies: every key of this vendor's range carries the vendor's prefix."

The store: `src/state.rs:1111-1112` `pub protocols_lists: BTreeMap<String, ProtocolsListRecord>,` with the namespace only a record flag (`src/state.rs:1994-2005`, `protocols_list_in_namespace`).

Test coverage: `tests/all/protocols_lists_enumeration.rs` asserts the foreign-token refusal only for values that are not a decodable `ListId`, and its page walk keeps `DefaultLists` fixed; a managed protocols list is seeded directly through `seed_protocols_list(.., default_list)` (`tests/all/administrator_gated_operations.rs:110-135`), so the cross-namespace token is reachable and unpinned.

## Steps to Reproduce

1. AssociateAdminAccount with the emulator account so the gate is open.
2. Seed two MANAGED protocols lists (via `seed_protocols_list(.., true)`, the seam the suites use, since no API creates one) whose `ListId`s are the lowest and the highest in the store, and PutProtocolsList two customer lists with ids in between.
3. Call ListProtocolsLists with {"DefaultLists": false, "MaxResults": 1} and keep the returned NextToken.
4. Call ListProtocolsLists with {"DefaultLists": true, "MaxResults": 100, "NextToken": <that token>}: 200, and the managed list whose ListId sorts below the token's key is missing from the page, with no NextToken to signal it.
