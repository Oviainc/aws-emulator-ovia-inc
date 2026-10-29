Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0e1d5242aac04ba2a44c3cb18c53433536c8022f

# A ListDomains nextToken carries no record of the status filter it was issued under, so resuming it with a different filter silently drops domains that listing never served

_Filed: 2026-10-28T20:44:47Z_
_Operation: ListDomains | Class: pagination_

> _Note: this issue was identified against the code base at commit `0e1d5242aac04ba2a44c3cb18c53433536c8022f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDomains` narrows the records it enumerates by two request values: the Region (from the credential scope) and the `status` query filter. Its pagination cursor records only the first. `Page::resolve` is handed `domains_page_scope(&region)`, which is `format!("domains/{region}")`, and `decode_page_token` accepts any token whose scope string and tag match — so a cursor minted by `/domain?status=DELETING` is accepted verbatim by `/domain` or `/domain?status=ACTIVE`.

Because the cursor is a bare "resume strictly after this domainId" position, resuming it under a different filter skips every domain whose id sorts at or before the cursor key — including domains the second request's own filter has never served. Concretely: with two DELETING domains and an ACTIVE domain whose id sorts lowest, `/domain?status=DELETING&maxResults=1` issues a cursor after the first DELETING domain; replaying that cursor on unfiltered `/domain` drops the ACTIVE domain from the listing entirely, with no error and no indication it was skipped.

This is the crate contradicting its own rule in two places. `Page`'s own doc states that "a `nextToken` is a resume point only when this service issued it FOR THIS collection", and the sibling listing follows it: `subscriptions_page_scope(region, domain_id)` folds the narrowing parameter (`domainId`) into the scope precisely so that a cursor of one domain's subscriptions is refused by another's — a case `tests/all/pagination_token_issuance.rs` asserts. `domains_page_scope` does not do the same for `status`, and its doc asserts something that is only true if `status` is ignored: "the Region is the only part of that scope two live collections can differ in".

## Evidence

The cursor scope omits the filter — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:1375-1382`:
```
/// The collection a `ListDomains` cursor enumerates: ONE account's domains in ONE
/// Region, since this endpoint is regional and the listing reports the calling account's
/// domains alone. Every record of this emulator belongs to the one canonical account
/// ([`owning_account_id`]), so the Region is the only part of that scope two live
/// collections can differ in, and so the only part the key carries.
pub fn domains_page_scope(region: &str) -> String {
    format!("domains/{region}")
}
```

The sibling listing DOES fold its narrowing parameter in — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:1384-1388`:
```
/// The collection a `ListSubscriptions` cursor enumerates: the subscriptions of ONE
/// domain in ONE Region, which is the only collection that listing reports.
pub fn subscriptions_page_scope(region: &str, domain_id: &str) -> String {
    format!("subscriptions/{region}/{domain_id}")
}
```

The rule both are supposed to obey — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:1490-1492`:
```
/// cursor primitives it is built on ([`encode_page_token`] / [`decode_page_token`] and
/// the `*_page_scope` constructors): absent `maxResults` means a full page and a value
/// outside the bound is refused rather than clamped; a `nextToken` is a resume point only
/// when this service issued it FOR THIS collection; and a page issues a cursor only when
/// a matching record is left after it.
```
and the only thing the decode actually checks — `src/state.rs:1459-1460`:
```
    (token_scope == scope && page_token_tag(scope, after_key) == tag)
        .then(|| after_key.to_string())
```

`status` really is a narrowing parameter of this listing — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/list_domains.rs:169-186`:
```
        let page = match crate::state::Page::resolve(
            crate::state::domains_page_scope(&region),
            input.max_results,
            input.next_token.as_deref(),
        ) {
...
        let mut matching = store
            .domains_owned_by(&region, &account_id)
            .filter(|domain| wanted.is_none_or(|status| domain.status == status))
            .filter(|domain| page.starts_after(&domain.domain_id));
```

Test coverage: `tests/all/pagination_token_issuance.rs:228-238` refuses "a cursor of the subscriptions collection" and "a cursor of this collection in another Region", and `tests/all/shared_pagination_policy.rs:174-191` only ever pairs `status=DELETED` with an EMPTY listing — no test resumes a cursor across two different `status` values, so this path is unpinned.

## Steps to Reproduce

1. Create three domains in one Region; give two of them a `webAppSetupConfiguration` so their DeleteDomain passes through the non-terminal DELETING state, and note that the third (ACTIVE) domain's `domainId` sorts lowest of the three.
2. DELETE /domain/{id} for the two web-app domains, so both are in DELETING.
3. GET /domain?status=DELETING&maxResults=1 and capture the issued nextToken (a second DELETING domain remains, so a token is issued).
4. GET /domain?nextToken=<that token> with no status filter.
5. Observe HTTP 200 and a listing that omits the ACTIVE domain, which this unfiltered listing has never served — the cursor was accepted even though it was issued for a different enumerated collection, where a ListSubscriptions cursor crossing collections is refused.
