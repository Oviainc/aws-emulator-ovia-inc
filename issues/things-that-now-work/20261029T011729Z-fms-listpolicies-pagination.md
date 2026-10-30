Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 45af9d085f8e6d1dcfd74ea87e1bf5fe287bf2c6

# A ListPolicies NextToken is not scoped to the collection that minted it, so an applications/protocols-list token silently truncates the policy page

_Filed: 2026-10-29T01:17:29Z_
_Operation: ListPolicies | Class: pagination_

> _Note: this issue was identified against the code base at commit `45af9d085f8e6d1dcfd74ea87e1bf5fe287bf2c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListPolicies` pages over `store.policies` and decodes its `NextToken` with `crate::state::decode_page_token`, which only weighs the SHAPE of the decoded key: "a 36-character identifier this service mints". That same decoder is the one `ListAppsLists` and `ListProtocolsLists` use, and all three mint their keys through `crate::state::mint_identifier()` (36 lowercase hex characters), so a token issued by an applications-list or protocols-list page decodes cleanly and is accepted verbatim by `ListPolicies`.

The accepted key is then used directly as the lower bound of `store.policies.range(lower..)`. Every stored policy whose `PolicyId` sorts below that foreign `ListId` is skipped, and because the walk then runs to the end of the map the response carries NO `NextToken` — so a client cannot tell the truncated page from a complete one. The handler's own comment states the opposite intent ("A token this service did not mint names no page boundary … refuse it as a bad request rather than inventing a page"), yet the check it performs cannot tell a policy page's boundary from a list page's.

This crate already knows how to scope a token to the collection that minted it: `ListThirdPartyFirewallFirewallPolicies` and `GetProtectionStatus` both key their inventory with a prefix and refuse a token whose decoded key does not carry it. `ListPolicies` keys nothing and checks nothing beyond the 36-character shape.

## Evidence

The decoder's own contract admits it cannot distinguish the two page kinds — `services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:2994-3007`:
```
/// The decoded value must still be a 36-character identifier this service mints
/// — the form the model gives both `com.amazonaws.fms#PolicyId` and
/// `com.amazonaws.fms#ListId` (36 characters matching `^[a-z0-9A-Z-]{36}$`), so
/// one check serves the policy-keyed and the list-keyed pages — and a foreign
/// token that happens to be valid hex is refused rather than resumed from as an
/// arbitrary key.
pub fn decode_page_token(token: &str) -> Option<String> {
```

The handler that states the stricter rule it does not get — `src/ops/list_policies.rs:190-203`:
```
        // A token this service did not mint names no page boundary. The model
        // declares no client-input error for ListPolicies, so refuse it as a bad
        // request rather than inventing a page.
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_page_token(token) {
```
and the unconditional use of the decoded key as the range's lower bound — `src/ops/list_policies.rs:208-229`:
```
        let lower = start.unwrap_or_default();
        let mut remaining = store.policies.range(lower..);
        ...
        let next_token = remaining
            .next()
            .map(|(policy_id, _)| crate::state::encode_page_token(policy_id));
```

The sibling token that `ListAppsLists` mints with the SAME codec — `src/ops/list_apps_lists.rs:234-236`:
```
        let next_token = remaining
            .next()
            .map(|(list_id, _)| crate::state::encode_page_token(list_id));
```
and both id spaces come from one mint — `src/state.rs:3069-3071`: `pub fn mint_identifier() -> String { mint_hex(36) }`, called for a `PolicyId` at `src/ops/put_policy.rs:613` and for a `ListId` at `src/ops/put_apps_list.rs:302`.

The in-crate precedent for scoping a token to its own collection — `src/ops/list_third_party_firewall_firewall_policies.rs:238-245`:
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
and the same rule per policy in `src/ops/get_protection_status.rs:229-236`.

Test coverage: `tests/all/pagination_token_decoding.rs` and `tests/all/apps_lists_enumeration.rs` pin only that a token which is not hex / not a 36-character identifier is refused; no test passes a token minted by one of the three operations to another.

## Steps to Reproduce

1. AssociateAdminAccount with the emulator account so the administrator gate is open.
2. PutAppsList twice, so the applications-list store holds at least two lists.
3. PutPolicy several times, so the policy store holds policies whose PolicyIds sort both below and above the ListIds minted in step 2 (both are 36 lowercase hex characters from the same mint, so they interleave).
4. Call ListAppsLists with {"DefaultLists": false, "MaxResults": 1} and keep the returned NextToken — it is hex of the next applications list's ListId.
5. Call ListPolicies with {"MaxResults": 100, "NextToken": <that token>}: the call is accepted (200) and reports only the policies whose PolicyId sorts at or after that ListId. The policies below it are missing and no NextToken comes back, so the truncated page is indistinguishable from a complete one.
6. Contrast with ListThirdPartyFirewallFirewallPolicies, which refuses a token minted for another vendor's page with InvalidInputException.
