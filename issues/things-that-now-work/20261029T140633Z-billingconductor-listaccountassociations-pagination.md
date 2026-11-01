Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: cc96b89474242e07623dda2d895b89486f0507c0

# decode_next_token honours uppercase/mixed-case hex spellings encode_next_token never mints, so a NextToken this service never issued is served as a cut point

_Filed: 2026-10-29T14:06:33Z_
_Operation: ListAccountAssociations | Class: pagination_

> _Note: this issue was identified against the code base at commit `cc96b89474242e07623dda2d895b89486f0507c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated read of this service resolves its `NextToken` through `crate::state::decode_next_token`, whose documented contract is that it answers `None` for any value that is not a token this service minted for that listing — and the surrounding module states that the only thing a caller may do with a token is "hand it back, unchanged".

`encode_next_token` mints the token with `::hex::encode`, which emits LOWERCASE hex digits only. `decode_next_token` reads it back with `::hex::decode`, which accepts `0-9`, `a-f` AND `A-F`. So a caller that re-types or upper-cases a token — a spelling this service never produced and the caller therefore did not hand back unchanged — is still served the page, instead of the 400 `ValidationException` with reason `CANNOT_PARSE` that `invalid_next_token` is there to answer a token the service did not mint with.

The tag check cannot catch it: the tag is computed over the DECODED payload bytes, which are identical for either spelling, so the integrity check `page_token_tag` was added for ("it tells a token that was minted whole from one that was not") passes.

This affects every paginated read of the service, since all of them share the one helper: ListAccountAssociations, ListBillingGroups, ListBillingGroupCostReports, ListCustomLineItems, ListCustomLineItemVersions, ListPricingPlans, ListPricingRules, ListPricingPlansAssociatedWithPricingRule, ListPricingRulesAssociatedToPricingPlan, ListResourcesAssociatedToCustomLineItem and GetBillingGroupCostReport.

## Evidence

The documented contract the code breaks — `src/state.rs:4022-4026`:
```
/// The cursor `token` carries for the listing `scope`, or `None` when `token` is not one
/// this service minted for that listing — a value longer than any token it mints, one that
/// is not hex, is too short to carry a tag, does not agree with the tag it carries, does
/// not decode to UTF-8, carries no scope at all, or carries a different listing's scope.
pub fn decode_next_token(scope: &str, token: &str) -> Option<String> {
```
and `src/state.rs:3997-3999`:
```
/// A pagination token is OPAQUE — the only thing a caller may do with one is hand it back,
/// unchanged, to the listing that returned it — so the cursor does not travel in the clear
/// as the store key it is, and the scope travels with it: ...
```

What is minted — LOWERCASE hex only, `src/state.rs:4008-4013`:
```
pub fn encode_next_token(scope: &str, cursor: &str) -> String {
    let payload = format!("{scope}{PAGE_TOKEN_SEPARATOR}{cursor}");
    let mut encoded = page_token_tag(payload.as_bytes());
    encoded.extend_from_slice(payload.as_bytes());
    ::hex::encode(encoded)
}
```

What is accepted — case-insensitive hex, `src/state.rs:4030`:
```
    let encoded = ::hex::decode(token).ok()?;
```

The integrity check that cannot see the difference (it digests the decoded payload, not the spelling) — `src/state.rs:4043-4050`:
```
/// [`PAGE_TOKEN_TAG_BYTES`] bytes of the SHA-256 digest of a token's payload.
///
/// An integrity check over a value this service produced and the caller hands back
/// unchanged, not a secret: it tells a token that was minted whole from one that was not.
fn page_token_tag(payload: &[u8]) -> Vec<u8> {
```

The refusal that is skipped — `src/state.rs:4079-4087` (`invalid_next_token`, reason `CANNOT_PARSE`), reached from `src/state.rs:4126`:
```
        Some(token) => Some(decode_next_token(scope, token).ok_or_else(invalid_next_token)?),
```

The caller cited — `src/ops/list_account_associations.rs:259-261`:
```
        let (linked_accounts, next_token) =
            crate::state::paginate(Self::NAME, selected, input.next_token.as_deref(), None)
                .map_err(ListAccountAssociationsErrorModeled::ValidationException)?;
```

Test coverage: `tests/all/pagination_token_opacity.rs` drives every paginated read against a table of unminted values, and its one "token with ONE character changed" case changes the digit's VALUE, never its case — `tests/all/pagination_token_opacity.rs:440-451`:
```
/// `token` with one character changed — a token a client corrupted in transit or in a log.
fn corrupted(token: &str) -> String {
    let last = token.chars().count().saturating_sub(1);
    token
        .chars()
        .enumerate()
        .map(|(index, character)| match (index == last, character) {
            (true, '0') => '1',
            (true, _) => '0',
            (false, _) => character,
        })
        .collect()
```
so the suite asserts only that a token is made of ascii hex digits (`tests/all/pagination_token_opacity.rs:633-635`) and nothing pins acceptance of a case-variant.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`).
2. Create enough state for a listing to page (e.g. two billing groups with one account each, so ListAccountAssociations has two rows).
3. POST /list-account-associations and keep the `NextToken` of the first page (it is lowercase hex).
4. Upper-case every character of that token — a spelling this service never minted.
5. POST /list-account-associations with that upper-cased value as `NextToken`.
6. Observe HTTP 200 and a served page, where `decode_next_token`'s own contract ("`None` when `token` is not one this service minted for that listing") requires the 400 `ValidationException` with Reason `CANNOT_PARSE` that `tests/all/pagination_token_opacity.rs` asserts for every other unminted value.
