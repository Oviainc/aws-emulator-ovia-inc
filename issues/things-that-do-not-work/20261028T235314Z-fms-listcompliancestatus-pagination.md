Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 964c2ed1f803ffeae6e06375ca6c19c7cce42b54

# A ListComplianceStatus NextToken is not scoped to the PolicyId that minted it, so another policy's token silently empties (or truncates) the page

_Filed: 2026-10-28T23:53:14Z_
_Operation: ListComplianceStatus | Class: pagination_

> _Note: this issue was identified against the code base at commit `964c2ed1f803ffeae6e06375ca6c19c7cce42b54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListComplianceStatus is a policy-narrowed listing: `PolicyId` is a required input and the page walks only the accounts THAT policy applies to (`store.policy_scope_accounts(&policy.policy)`). The page token it mints and accepts, however, carries nothing but a bare account id: it is minted with `crate::state::encode_page_token(account)` and decoded with `crate::state::decode_account_page_token`, whose only check is `is_account_id(&decoded)`.

So a token this operation issued for policy A is accepted verbatim when it is replayed with `PolicyId` = B (and so is a token minted by ListMemberAccounts / ListAdminAccountsForOrganization / ListAdminsManagingAccount, which share the same codec). The handler then resumes B's listing at `scope.range(lower..)` from an account that was never a boundary in B's scope, so every account of B sorting before that id is silently dropped from the response — commonly producing an empty final page with no NextToken for a policy that has compliance status to report.

The crate already knows how to prevent exactly this: the two sibling handlers whose page is narrowed by a request member prefix their store key with that member and REFUSE a token that does not carry the prefix. ListComplianceStatus is the one narrowed listing that skips that scoping, so a cross-policy token is answered with a wrong page instead of the refusal its siblings state.

## Evidence

The unscoped decode — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_compliance_status.rs:194`:
```
        // A token this service did not mint names no page boundary. This page is
        // keyed by member account, so only an account-keyed token names one; the
        // model declares no client-input error for `ListComplianceStatus`, so a
        // foreign token is refused as a bad request rather than resumed from as
        // an arbitrary key ...
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_account_page_token(token) {
```
and the resume, `list_compliance_status.rs:216`:
```
        let scope = store.policy_scope_accounts(&policy.policy);
        let lower = start.unwrap_or_default();
        let mut remaining = scope.range(lower..);
```
with the token minted at `list_compliance_status.rs:230`:
```
        let next_token = remaining
            .next()
            .map(|account| crate::state::encode_page_token(account));
```

The decoder carries no policy stamp — `services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:3052`:
```
pub fn decode_account_page_token(token: &str) -> Option<String> {
    let decoded = decode_key_token(token)?;
    is_account_id(&decoded).then_some(decoded)
}
```

Sibling that DOES scope its token to the narrowing member — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_protection_status.rs:229` (also `PolicyId`-narrowed):
```
        // A token this service did not mint, or one minted for ANOTHER policy's
        // page, names no boundary in this policy's attacks: every key of this
        // policy's range carries the policy's prefix.
        let prefix = crate::state::shield_attack_key_prefix(&input.policy_id);
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_key_token(token)
                .filter(|key| key.starts_with(&prefix))
```
and `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_third_party_firewall_firewall_policies.rs:238`:
```
        // A token this service did not mint, or one minted for ANOTHER vendor's
        // page, names no boundary in this vendor's policies: every key of this
        // vendor's range carries the vendor's prefix.
        let prefix =
            crate::state::third_party_firewall_policy_key_prefix(&input.third_party_firewall);
```

Test coverage leaves the gap open: `services/service-com-amazonaws-fms-awsfms-20180101/tests/all/compliance_status.rs:966` only exercises tokens of another CLASS —
```
    // token minted for a page keyed by something else (a `PolicyId`).
    let foreign = [
        ("not-hex", "not.a.token.this.service.minted".to_string()),
        ...
        ("policy-keyed-token", hex::encode(&policy_id)),
```
— and `services/service-com-amazonaws-fms-awsfms-20180101/tests/all/pagination_token_decoding.rs:76` (`page_classes()`) lists only ListAdminsManagingAccount, ListMemberAccounts and ListPolicies, so no test replays an account-keyed token across two policies.

## Steps to Reproduce

1. AssociateAdminAccount so the administrator gate passes.
2. PutPolicy twice: policy A with `IncludeMap` = {"ACCOUNT": ["111111111111", "999988887777"]} and policy B with `IncludeMap` = {"ACCOUNT": ["222222222222"]}.
3. Call ListComplianceStatus with `PolicyId` = A and `MaxResults` = 1; note the returned `NextToken` (it is hex of "999988887777").
4. Replay that same `NextToken` with `PolicyId` = B.
5. Observed: HTTP 200 with an empty `PolicyComplianceStatusList` and no `NextToken` — account 222222222222 is dropped. Expected (per the rule get_protection_status.rs:229 and list_third_party_firewall_firewall_policies.rs:238 apply): the token is not one this operation minted for policy B, so it is refused.
