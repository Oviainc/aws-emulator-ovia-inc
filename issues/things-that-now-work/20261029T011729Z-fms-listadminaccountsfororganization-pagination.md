Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 45af9d085f8e6d1dcfd74ea87e1bf5fe287bf2c6

# An account-keyed NextToken is interchangeable across the three account listings, so a ListMemberAccounts token silently truncates the administrator list

_Filed: 2026-10-29T01:17:29Z_
_Operation: ListAdminAccountsForOrganization | Class: pagination_

> _Note: this issue was identified against the code base at commit `45af9d085f8e6d1dcfd74ea87e1bf5fe287bf2c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAdminAccountsForOrganization`, `ListMemberAccounts` and `ListAdminsManagingAccount` all decode their `NextToken` with `crate::state::decode_account_page_token` and all mint it with `crate::state::encode_page_token`. That decoder checks only that the decoded key is an `AWSAccountId` — it cannot say WHICH of the three listings the boundary belongs to — yet the three enumerate three different collections:

* `ListAdminAccountsForOrganization` pages `store.admin_account_summaries(now)` (the organization's administrators only);
* `ListMemberAccounts` pages `store.member_accounts()` (every account the control plane was told about);
* `ListAdminsManagingAccount` pages `store.managing_admin_accounts(now)` (the administrators that manage the caller).

`member_accounts()` is a strict superset of the administrators, so a `NextToken` minted by `ListMemberAccounts` normally names an account that sorts beyond several administrators. Handed to `ListAdminAccountsForOrganization` it is accepted and used straight as the lower bound of `summaries.range(lower..)`: every administrator whose account id sorts below it is dropped, and because the walk then runs to the end the response carries NO `NextToken`, so the short page is indistinguishable from a complete one. Each handler's own comment claims the opposite ("A token this service did not mint names no page boundary"), but the check it performs cannot tell one listing's boundary from another's.

The crate already demonstrates the scoping discipline elsewhere: `ListThirdPartyFirewallFirewallPolicies` and `GetProtectionStatus` prefix their keys with the sub-collection they belong to and refuse a token whose decoded key does not carry that prefix.

## Evidence

The decoder that weighs only the account-id shape — `services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:3041-3055`:
```
/// The decoded value must still be an `AWSAccountId` ([`is_account_id`]), so a
/// token minted for a policy page — or any foreign token that happens to be
/// valid hex — is refused rather than resumed from as an arbitrary key.
pub fn decode_account_page_token(token: &str) -> Option<String> {
    let decoded = decode_key_token(token)?;
    is_account_id(&decoded).then_some(decoded)
}
```
(it names only the POLICY page as the foreign class it rules out; nothing distinguishes the three account-keyed pages from each other.)

The three handlers that share it, each claiming a rule the shared check does not give them:
- `src/ops/list_admin_accounts_for_organization.rs:183-197` — "A token this service did not mint names no page boundary. The model declares no client-input error for this operation, so refuse it as a bad request rather than inventing a page for it." … `match crate::state::decode_account_page_token(token)`
- `src/ops/list_member_accounts.rs:174-187` — "A token this service did not mint names no page boundary." … `match crate::state::decode_account_page_token(token)`
- `src/ops/list_admins_managing_account.rs:160-180` — "A token this service did not mint names no page boundary, so the request's parameters are invalid" … `match crate::state::decode_account_page_token(token)`

The foreign key is used straight as the range's lower bound — `src/ops/list_admin_accounts_for_organization.rs:210-231`:
```
        let summaries = store.admin_account_summaries(now);
        let lower = start.unwrap_or_default();
        let mut remaining = summaries.range(lower..);
        ...
        let next_token = remaining
            .next()
            .map(|(account, _)| crate::state::encode_page_token(account));
```
while `ListMemberAccounts` mints its token from a DIFFERENT collection with the same codec — `src/ops/list_member_accounts.rs:192-205`:
```
        let accounts = store.member_accounts();
        ...
        let next_token = remaining
            .next()
            .map(|account_id| crate::state::encode_page_token(account_id));
```
and those collections are not the same set: `src/state.rs:1406` `pub fn admin_account_summaries(&self, now: SystemTime) -> BTreeMap<String, AdminAccountSummary>` holds only administrators, whereas `src/state.rs:1544` `pub fn member_accounts(&self) -> BTreeSet<String>` also adds "every account id a stored policy names under the [`SCOPE_ID_TYPE_ACCOUNT`] key of its `IncludeMap` or `ExcludeMap`" (`src/state.rs:1528-1531`).

The in-crate precedent for refusing a token minted for another sub-collection — `src/ops/list_third_party_firewall_firewall_policies.rs:238-245` ("or one minted for ANOTHER vendor's page, names no boundary …") and `src/ops/get_protection_status.rs:229-236`.

Test coverage: `tests/all/admin_accounts_for_organization.rs` and `tests/all/member_accounts.rs` walk each listing's own pages and pin only that a non-hex / non-account-id token is refused; no test hands one listing's token to another.

## Steps to Reproduce

1. AssociateAdminAccount with the emulator account (so an active default administrator exists and the gate is open).
2. PutAdminAccount for a delegated administrator account id that sorts LOW (for example "100000000001").
3. PutPolicy with an IncludeMap of {"ACCOUNT": ["900000000009"]}, so member_accounts() also reports a high-sorting account that is not an administrator.
4. Call ListMemberAccounts with {"MaxResults": 1}; follow the pages until the returned NextToken is hex of "900000000009".
5. Call ListAdminAccountsForOrganization with {"MaxResults": 100, "NextToken": <that token>}: the call is accepted (200), the AdminAccounts list is empty (every administrator sorts below 900000000009) and no NextToken is returned — the caller cannot tell this from "the organization has no administrators".
6. Contrast with ListThirdPartyFirewallFirewallPolicies, which refuses a token minted for another vendor's page with InvalidInputException.
