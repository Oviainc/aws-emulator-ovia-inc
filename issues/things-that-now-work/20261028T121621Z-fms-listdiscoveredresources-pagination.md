Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# ListDiscoveredResources accepts a NextToken of any other page class and serves a page from an arbitrary key

_Filed: 2026-10-28T12:16:21Z_
_Operation: ListDiscoveredResources | Class: pagination_

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListDiscoveredResources pages over resource URIs (ARNs) keyed in a BTreeMap, exactly like its sibling ListResourceSetResources. Every other paginated read of this crate validates the SHAPE of the decoded key before using it as a lower bound (`decode_page_token` = a 36-character minted id, `decode_base62_page_token` = a 22-character Base62Id, `decode_account_page_token` = an AWSAccountId, `decode_uri_page_token` = an ARN, and GetProtectionStatus / ListThirdPartyFirewallFirewallPolicies additionally require their own key prefix). ListDiscoveredResources alone calls the raw `crate::state::decode_key_token`, which accepts ANY hex string that decodes to non-empty, control-character-free UTF-8.

Consequence: a token this service minted for a different page class — or any hex-encoded printable string — is accepted and used verbatim as the range lower bound. The handler then answers HTTP 200 with a page that silently starts in the middle of (or past the end of) the inventory, instead of the `InvalidInputException` its own model declares and its own comment promises. A token whose decoded value sorts after every ARN (e.g. hex of `zzz` = `7a7a7a`) yields `Items: []` with no `NextToken`, i.e. a 200 claiming the account owns no resources of that type — an error turned into a wrong success. The URI-keyed decoder that would refuse it already exists in the crate and is used by the sibling handler that pages over the same kind of key.

## Evidence

Handler that skips the shape check — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_discovered_resources.rs:216-218`:
```
        // A token this service did not mint names no page boundary.
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_key_token(token) {
```
and the bound it is used as, `list_discovered_resources.rs:245-246`:
```
        let lower = start.unwrap_or_default();
        let mut remaining = inventory.range(lower..);
```

Sibling handler over the SAME kind of key, which does apply the check — `src/ops/list_resource_set_resources.rs:215-220`:
```
        // A token this service did not mint for a resource-membership page names
        // no page boundary. The refusal is a bad request, not an invented page:
        // the modelled `InvalidInputException` is exactly "the parameters of the
        // request were invalid".
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_uri_page_token(token) {
```

The helper that states the rule — `src/state.rs:2953-2966`:
```
/// Decode a URI-keyed `PaginationToken` produced by [`encode_key_token`], for
/// the paginated operation whose store key is a member resource's URI. ...
/// The decoded value must still be an ARN ([`parse_arn`]) ... so a
/// token minted for a policy, account or resource-set page, or any foreign token
/// that happens to be valid hex, is refused rather than resumed from as an
/// arbitrary key.
pub fn decode_uri_page_token(token: &str) -> Option<String> {
    let decoded = decode_key_token(token)?;
    parse_arn(&decoded).is_some().then_some(decoded)
}
```
and the raw decoder it wraps — `src/state.rs:2912-2916` — which checks only hex/UTF-8/non-control, no key shape.

The crate's own statement of the shared rule — `tests/all/pagination_token_decoding.rs:6-13`:
```
//! The rule: a `NextToken` names a page boundary exactly when it is the hex
//! encoding of a UTF-8 store key of that page class's own shape; any other value
//! names no boundary, so the request is refused rather than answered with an
//! invented page, and a refusal reports nothing and changes nothing.
//!
//! The decode step itself (`crate::state::decode_key_token`) is one helper every
//! specialised decoder delegates to, and each page class adds only its own shape
//! check on the decoded key
```

Test coverage: `tests/all/discovered_resources.rs:1027-1042` is the only token-refusal case for this operation, and it sends `"NextToken": "not-a-token-this-service-minted"` — not valid hex, so it is rejected by `decode_key_token` itself and the missing shape check is never exercised. No test pins the foreign-but-valid-hex case for this operation.

## Steps to Reproduce

1. Start the emulator and call AssociateAdminAccount with the emulator account so the administrator gate is open.
2. Create an EC2 VPC through the EC2 emulator so ListDiscoveredResources has something to report: POST ListDiscoveredResources with {"MemberAccountIds": [<account>], "ResourceType": "AWS::EC2::VPC"} and observe Items contains the VPC's ARN.
3. Repeat the call with "NextToken": "7a7a7a" (the hex of the string `zzz`, which is not an ARN and names no key of this page).
4. Observed: HTTP 200 with Items: [] and no NextToken — the token was decoded to the arbitrary key `zzz`, which sorts after every ARN, so the whole inventory is skipped.
5. Expected (the rule the sibling ListResourceSetResources applies through decode_uri_page_token, and the modelled error this operation declares): HTTP 400 InvalidInputException, "NextToken is not a pagination token issued by ListDiscoveredResources".
6. For contrast, send the same "NextToken": "7a7a7a" to ListResourceSetResources: it is refused with InvalidInputException.
