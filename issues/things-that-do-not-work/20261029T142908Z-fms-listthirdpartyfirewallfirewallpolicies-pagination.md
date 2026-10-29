Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 4fd05cf1a47f5cbbe05dba0186301153f554fae3

# decode_key_token honours uppercase/mixed-case hex that encode_key_token never mints, so a NextToken this service never issued resumes a page

_Filed: 2026-10-29T14:29:08Z_
_Operation: ListThirdPartyFirewallFirewallPolicies | Class: pagination_

> _Note: this issue was identified against the code base at commit `4fd05cf1a47f5cbbe05dba0186301153f554fae3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this service decodes its `NextToken` through the one shared helper `crate::state::decode_key_token`, and the ONLY minter of such a token is `crate::state::encode_key_token`, which is `::hex::encode` and therefore emits LOWERCASE hex only. The decode side is `::hex::decode`, which accepts `A`-`F` as well as `a`-`f`. So a case-variant of a token the service minted — a string this service can never have issued — decodes to the very same store key and is served as a page boundary instead of being refused.

`ListThirdPartyFirewallFirewallPolicies` is the plainest case because its page key is `third_party_firewall_policy_key` = `<ThirdPartyFirewall>#<FirewallPolicyId>`, and both modelled vendor values contain bytes whose hex spelling carries letters (`_` is `5f`, `O` is `4f`, `N` is `4e`, `L` is `4c`, `A` is `41`), so a minted token for this listing always contains hex letters and always has case-variants. Uppercasing them produces a token this service never minted, which the handler accepts and resumes the vendor's page from (the `starts_with(&prefix)` check passes, because the decoded key is byte-identical).

The same acceptance holds for every other listing whose page key contains such a byte — a resource ARN for `ListResourceSetResources` / `ListDiscoveredResources` (`:` is `3a`, `/` is `2f`, `-` is `2d`), the `false` namespace of `ListAppsLists` / `ListProtocolsLists` (`l` is `6c`), and a shield attack key for `GetProtectionStatus` (`.` is `2e`) — since `decode_page_token`, `decode_base62_page_token`, `decode_uri_page_token`, `decode_account_page_token` and `decode_scoped_page_token` all delegate the token-to-key step to `decode_key_token` and only add their own shape check on the decoded key.

## Evidence

The only minter emits lowercase hex — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:3301-3310:
```
/// Encode an arbitrary page boundary (the store key the next page starts at) as
/// an opaque `PaginationToken`, for the paginated operations whose key is not a
/// `PolicyId`.
...
pub fn encode_key_token(key: &str) -> String {
    ::hex::encode(key)
}
```

The decoder states the rule it then breaks — src/state.rs:3312-3319:
```
/// Decode a `PaginationToken` produced by [`encode_key_token`]. `None` when the
/// token was not produced by this service: a value that is not hex, not UTF-8,
/// empty, or carries a control character is no store key of ours.
pub fn decode_key_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let is_key = !decoded.is_empty() && !decoded.chars().any(char::is_control);
    is_key.then_some(decoded)
}
```
`::hex::decode` accepts `b'A'..=b'F'` as well as `b'a'..=b'f'`, so an uppercase or mixed-case spelling — which `::hex::encode` never produces — is "hex" here and is returned as a store key.

The handler that consumes it, src/ops/list_third_party_firewall_firewall_policies.rs:242-268:
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

The key whose hex always carries letters — src/state.rs:912-914:
```
pub fn third_party_firewall_policy_key(third_party_firewall: &str, policy_id: &str) -> String {
    format!("{third_party_firewall}#{policy_id}")
}
```

The crate's own suite states the rule as "did not mint" and enumerates the spellings it refuses, with no case-varied row — tests/all/third_party_firewall_firewall_policies.rs:514-518 and :552-553:
```
/// A `NextToken` this service did not mint — or one it minted for ANOTHER
/// vendor's page — names no boundary in this vendor's policies, and is refused
/// as invalid input rather than resumed from.
#[tokio::test]
async fn refuses_a_next_token_it_did_not_mint_for_this_vendor() {
```
```
        ("not hexadecimal", "zzzz".to_string()),
        ("hexadecimal that is not UTF-8", "abcd".to_string()),
```
and the shared decode suite, tests/all/pagination_token_decoding.rs:6-9 and :230-242, likewise enumerates only "not hex at all", "hex of an odd number of digits", "hex of bytes that are not UTF-8", a control character and "a key of another page class". Nothing pins a case-varied spelling, so the acceptance is untested rather than deliberate.

## Steps to Reproduce

1. Boot the emulator and call `AssociateAdminAccount` with `{"AdminAccount": "<the emulator account id>"}` → 200.
2. Onboard a vendor and seed two firewall policies into its tenant inventory the way the crate's own suite does (tests/all/third_party_firewall_firewall_policies.rs `seeded()`): an `ONBOARD_COMPLETE` `ThirdPartyFirewallRecord` for `FORTIGATE_CLOUD_NATIVE_FIREWALL` and two `ThirdPartyFirewallPolicyRecord`s under `third_party_firewall_policies` (no API creates one — state.rs:1296-1301 documents the inventory as seeded).
3. Call `ListThirdPartyFirewallFirewallPolicies` with `{"ThirdPartyFirewall": "FORTIGATE_CLOUD_NATIVE_FIREWALL", "MaxResults": 1}` → 200 with a `NextToken` (the lowercase hex of `FORTIGATE_CLOUD_NATIVE_FIREWALL#<next id>`, which contains `5f`, `4f`, `4e`, `4c`, `41`, …).
4. Uppercase every letter of that `NextToken` (it still satisfies the modelled `PaginationToken` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, so it reaches the handler) and resend the same request with it.
5. Expected, per decode_key_token's own doc ("`None` when the token was not produced by this service") and the suite's stated rule ("A `NextToken` this service did not mint … is refused as invalid input rather than resumed from"): 400 `InvalidInputException` "NextToken is not a pagination token issued by ListThirdPartyFirewallFirewallPolicies for FORTIGATE_CLOUD_NATIVE_FIREWALL".
6. Actual: 200, serving the vendor's second page — `::hex::decode` accepted the uppercase spelling and handed back the same store key, so a token this service never issued is honoured as a cursor. The same holds for any mixed-case variant, and for every other listing whose key contains a byte whose hex carries a letter (an ARN boundary of `ListResourceSetResources`, the `false` namespace token of `ListAppsLists`/`ListProtocolsLists`, an attack key of `GetProtectionStatus`).
