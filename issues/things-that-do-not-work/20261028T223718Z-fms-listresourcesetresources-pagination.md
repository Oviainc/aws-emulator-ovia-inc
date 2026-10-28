Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: dbfb16c0807859a588c77add63fc25981d8f50b1

# A ListResourceSetResources NextToken is not scoped to the resource set that minted it, so another set's token silently drops members from the page

_Filed: 2026-10-28T22:37:18Z_
_Operation: ListResourceSetResources | Class: pagination_

> _Note: this issue was identified against the code base at commit `dbfb16c0807859a588c77add63fc25981d8f50b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListResourceSetResources` pages over ONE resource set's membership, held as that record's own `BTreeSet<String>` of URIs. Its `NextToken` is the bare hex of the next member's URI (`encode_key_token(uri)`) and carries no record of which set it was minted for, and the handler's token check is only a SHAPE check (`decode_uri_page_token` = "the decoded value is an ARN").

So a token minted while paging set A is accepted when sent with `Identifier` naming set B: the handler uses the decoded ARN verbatim as the lower bound of B's membership range, answers HTTP 200, and silently omits every member of B whose URI sorts before that ARN. The last page carries no `NextToken`, so the caller sees a complete-looking walk that under-reports B's membership — an error turned into a wrong success.

The crate already states and applies the stronger rule in the two sibling listings whose request carries the scope their page is partitioned by: `GetProtectionStatus` and `ListThirdPartyFirewallFirewallPolicies` both refuse "a token this service did not mint, OR one minted for ANOTHER <policy/vendor>'s page" by requiring the decoded key to carry that scope's own prefix. `ListResourceSetResources` is exactly the same situation — one page per addressed resource set — and takes only the family-wide shape check.

## Evidence

The handler: shape check only, then the decoded key used verbatim as the bound — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_resource_set_resources.rs:215-220`:
```
        // A token this service did not mint for a resource-membership page names
        // no page boundary. The refusal is a bad request, not an invented page:
        // the modelled `InvalidInputException` is exactly "the parameters of the
        // request were invalid".
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_uri_page_token(token) {
```
and `list_resource_set_resources.rs:243-244` / `:259`:
```
        let lower = start.unwrap_or_default();
        let mut remaining = record.resources.range(lower..);
...
        let next_token = remaining.next().map(|uri| crate::state::encode_key_token(uri));
```
(`decode_uri_page_token`, `src/state.rs:3036-3039`, checks only `parse_arn(&decoded).is_some()` — nothing about which set the key belongs to.)

The sibling that states and applies the scoped rule — `src/ops/list_third_party_firewall_firewall_policies.rs:238-245`:
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
and the second one — `src/ops/get_protection_status.rs:229-236`:
```
        // A token this service did not mint, or one minted for ANOTHER policy's
        // page, names no boundary in this policy's attacks: every key of this
        // policy's range carries the policy's prefix.
        let prefix = crate::state::shield_attack_key_prefix(&input.policy_id);
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_key_token(token)
                .filter(|key| key.starts_with(&prefix))
```

The suite that cares about exactly this isolation, yet never exercises the cross-set token — `tests/all/resource_set_resources.rs:322-324`:
```
/// The operation reports exactly the set's membership, each resource carrying the
/// account its ARN names, in ascending URI order — for every row of the table,
/// and without reporting the membership of another set in the same store.
```
with a deliberate decoy set at `resource_set_resources.rs:331-340` ("A second set holding a resource the first one does not, so a handler that reported 'every resource in the store' would fail here"). The token-refusal test covers only other page FAMILIES — `resource_set_resources.rs:632-656`:
```
    // Tokens that are no membership boundary: a value that is not a token at
    // all, and well-formed tokens for OTHER page families (a resource set's
    // `Base62Id`, an account id, a `PolicyId`-shaped key) together with the hex
    // of a value that is not an ARN — none of which may be resumed from as a
    // member URI.
```
No row sends a token minted by ListResourceSetResources for a DIFFERENT set, so a scoped refusal leaves every existing assertion passing.

## Steps to Reproduce

1. POST / AWSFMS_20180101.AssociateAdminAccount with {"AdminAccount": "<the emulator account id>"} — 200.
2. Create two EC2 instances in the EC2 emulator so two member ARNs exist, one sorting before the other.
3. PutResourceSet twice (sets A and B), each with {"ResourceTypeList": ["AWS::EC2::Instance"]}.
4. BatchAssociateResource both instance ARNs into set A, and both into set B, so A and B hold the same two members.
5. ListResourceSetResources with {"Identifier": "<A>", "MaxResults": 1} — 200 with the first member and a NextToken (the hex of the SECOND member's ARN).
6. ListResourceSetResources with {"Identifier": "<B>", "NextToken": "<that token>"} — observed: HTTP 200 reporting only B's second member, with no NextToken, so B's first member is silently dropped and the walk looks complete.
7. Expected (the rule the sibling GetProtectionStatus / ListThirdPartyFirewallFirewallPolicies state for their own per-resource pages): HTTP 400 InvalidInputException, "NextToken is not a pagination token issued by ListResourceSetResources" for that set.
