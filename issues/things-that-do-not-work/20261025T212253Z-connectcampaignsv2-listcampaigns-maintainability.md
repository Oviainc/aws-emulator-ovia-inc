Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# The cursor-window computation is implemented twice (ListCampaigns and ListConnectInstanceIntegrations) although the token codec is already centralized in state.rs

_Filed: 2026-10-25T21:22:53Z_
_Operation: ListCampaigns | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::encode_next_token` / `decode_next_token` are deliberately the one place the opaque cursor is encoded and decoded, but the RULE built around them — default page size, "a token this service did not mint is a ValidationException", `position(sequence > after)` as the page start, `start + page_size` clamped to the item count, and `next_token = last item's sequence only when the page did not reach the end` — is written out twice, once in `ListCampaigns::handle` and once in `ListConnectInstanceIntegrations::handle`. The two copies are character-for-character the same logic over a different item vector, so they can drift: a fix to one (say, the empty-last-page or off-by-one behaviour) would silently leave the other paging differently. Lifting the window computation next to the codec in `state.rs` (taking the already-collected slice, the optional token and the optional `maxResults`, and handing back `(page, next_token)`) removes the second copy and leaves every response byte-identical.

## Evidence

state.rs already claims the cursor as one rule — src/state.rs:340-358:
```
/// Encode a `ListCampaigns` cursor: the creation sequence number of the last
/// campaign on the page just returned, hex-encoded so the token is opaque rather
/// than a readable position.
...
pub fn encode_next_token(last_sequence: u64) -> String
...
pub fn decode_next_token(token: &str) -> Option<u64>
```

Copy 1 — src/ops/list_campaigns.rs:144-147, 186-219:
```
const DEFAULT_MAX_RESULTS: usize = 50;
...
let start = match input.next_token.as_deref() {
    None => 0,
    Some(token) => {
        let Some(after) = crate::state::decode_next_token(token) else { /* ValidationException: "nextToken is not a cursor this service issued" */ };
        matching.iter().position(|(sequence, _)| *sequence > after).unwrap_or(matching.len())
    }
};
let page_size = input.max_results.and_then(|max| usize::try_from(max).ok()).unwrap_or(DEFAULT_MAX_RESULTS);
let end = start.saturating_add(page_size).min(matching.len());
let page = &matching[start..end];
... next_token: if end < matching.len() { page.last().map(|(sequence, _)| crate::state::encode_next_token(*sequence)) } else { None },
```

Copy 2 — src/ops/list_connect_instance_integrations.rs:156-159, 206-242:
```
const DEFAULT_MAX_RESULTS: usize = 50;
...
let start = match input.next_token.as_deref() {
    None => 0,
    Some(token) => {
        let Some(after) = crate::state::decode_next_token(token) else { /* ValidationException: "nextToken is not a cursor this service issued" */ };
        integrations.iter().position(|stored| stored.sequence > after).unwrap_or(integrations.len())
    }
};
let page_size = input.max_results.and_then(|max| usize::try_from(max).ok()).unwrap_or(DEFAULT_MAX_RESULTS);
let end = start.saturating_add(page_size).min(integrations.len());
let page = &integrations[start..end];
... next_token: if end < integrations.len() { page.last().map(|stored| crate::state::encode_next_token(stored.sequence)) } else { None },
```

Existing tests that pin both copies and must stay unchanged: tests/all/campaign_lifecycle.rs ("paging over any page size walks every campaign exactly once, with no `nextToken` on the last page", module doc line 16) and tests/all/connect_instance_integration_list_paging.rs.

## Steps to Reproduce

1. Read src/ops/list_campaigns.rs:186-219 and src/ops/list_connect_instance_integrations.rs:206-242 side by side: the cursor decode, page-start search, page-size default, clamp and next-token emission are the same five steps duplicated.
2. Change one of them (e.g. make the start search inclusive) and observe that nothing in the other list operation or its tests notices — the two paginators can disagree.
