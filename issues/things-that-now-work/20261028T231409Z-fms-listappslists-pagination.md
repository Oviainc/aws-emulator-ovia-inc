Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 8269d8069d160c27d0acf36557f2407fe5d81546

# A ListAppsLists NextToken is not scoped to the DefaultLists namespace that minted it, so the other namespace's page silently starts mid-list

_Filed: 2026-10-28T23:14:09Z_
_Operation: ListAppsLists | Class: pagination_

> _Note: this issue was identified against the code base at commit `8269d8069d160c27d0acf36557f2407fe5d81546`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAppsLists` pages over `store.apps_lists`, a single `BTreeMap` that holds BOTH namespaces (Firewall Manager MANAGED lists and customer lists) under one `ListId` key space; `DefaultLists` is applied as a FILTER after the token's key is used as the range's lower bound. The token itself is only shape-checked (`crate::state::decode_page_token` accepts any 36-character `ListId`), so a token minted by a `DefaultLists=false` page is accepted verbatim by a `DefaultLists=true` request (and the reverse).

The result is a silently truncated listing: the resumed page starts at the key the OTHER namespace's walk had reached, so every list of the requested namespace whose `ListId` sorts below that key is skipped, and because the walk then runs to the end the response carries no `NextToken` — a client cannot tell the short page from a complete one.

This crate already knows how to scope a token to the sub-collection that minted it when two sub-collections share one key space: `ListThirdPartyFirewallFirewallPolicies` and `GetProtectionStatus` both key their inventory with a prefix and refuse a token whose decoded key does not carry it. `ListAppsLists` keys nothing by namespace and checks nothing, although its own comment states that the two namespaces are separate resources sharing one id space.

## Evidence

The handler's own statement that the two namespaces share one id space, `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_apps_lists.rs:209-220`:
```
        // `apps_lists` is ordered by `ListId`, so the boundary is a key: resume at
        // the first id at or after the token's. With no token the lower bound is
        // the empty id, which sorts before every `ListId`.
        let lower = start.unwrap_or_default();
        // `DefaultLists` chooses the namespace: a Firewall Manager managed list and
        // a customer list are separate resources that share one id space, and the
        // stored flag is what tells them apart ...
        let mut remaining = store
            .apps_lists
            .range(lower..)
            .filter(|(_, record)| record.default_list == input.default_lists);
```
and the token check that weighs only the SHAPE of the key, list_apps_lists.rs:194-207:
```
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_page_token(token) {
```
with `crate::state::decode_page_token`'s own contract, `services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:2991-3007`: "The decoded value must still be a 36-character identifier this service mints — the form the model gives both `com.amazonaws.fms#PolicyId` and `com.amazonaws.fms#ListId` ... so one check serves the policy-keyed and the list-keyed pages" — i.e. nothing in it distinguishes the managed from the customer namespace.

The sibling in the SAME crate that does scope a token to its sub-collection, `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_third_party_firewall_firewall_policies.rs:238-245`:
```
        // A token this service did not mint, or one minted for ANOTHER vendor's
        // page, names no boundary in this vendor's policies: every key of this
        // vendor's range carries the vendor's prefix.
        let prefix =
            crate::state::third_party_firewall_policy_key_prefix(&input.third_party_firewall);
        let start = match input.next_token.as_deref() {
            Some(token) => {
                match crate::state::decode_key_token(token).filter(|key| key.starts_with(&prefix)) {
```
and the same rule per policy in `src/ops/get_protection_status.rs:229-236` ("or one minted for ANOTHER policy's page, names no boundary in this policy's attacks").

The store keeps both namespaces in one map: `src/state.rs:1109` `pub apps_lists: BTreeMap<String, AppsListRecord>,` with the namespace only a record flag (`src/state.rs:1975-1992`, `apps_list_in_namespace`).

Test coverage: `tests/all/apps_lists_enumeration.rs:474-493` asserts only tokens that are not hex / not a 36-character `ListId` are refused, and the page walk keeps `DefaultLists` constant (`tests/all/apps_lists_enumeration.rs:219-235`), so no existing test pins the cross-namespace token. A managed list is reachable in the store — `tests/all/apps_list_reporting.rs:164` seeds one with `default_list: true`.

## Steps to Reproduce

1. AssociateAdminAccount with the emulator account so the gate is open.
2. Seed two MANAGED applications lists whose `ListId`s sort as the lowest and the highest of the store (the managed namespace has no creating API; `tests/all/apps_list_reporting.rs:164` seeds one the same way), and PutAppsList a customer list whose `ListId` sorts between them.
3. Call ListAppsLists with {"DefaultLists": false, "MaxResults": 1}: the customer list is reported; because another customer list follows, note the returned NextToken (it is hex of the next CUSTOMER list's ListId).
4. Call ListAppsLists with {"DefaultLists": true, "MaxResults": 100, "NextToken": <that token>}: the call is accepted (200) and reports only the managed lists whose ListId sorts at or after that key — the lower managed list is missing and no NextToken is returned, so the truncated page is indistinguishable from a complete one.
5. Contrast with ListThirdPartyFirewallFirewallPolicies, which refuses a token minted for another vendor's page with InvalidInputException.
