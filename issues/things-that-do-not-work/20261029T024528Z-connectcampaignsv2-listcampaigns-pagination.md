Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 847417f95205f58c5dcb38f2d402ae29c05d6bc7

# A nextToken is not scoped to the listing that minted it, so a ListConnectInstanceIntegrations cursor silently truncates the ListCampaigns page instead of being refused

_Filed: 2026-10-29T02:45:28Z_
_Operation: ListCampaigns | Class: pagination_

> _Note: this issue was identified against the code base at commit `847417f95205f58c5dcb38f2d402ae29c05d6bc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both paged lists of this crate mint and resolve their `nextToken` through the SAME codec (`crate::state::encode_next_token` / `decode_next_token`), which encodes nothing but a bare `u64` sequence value. But the two lists read that value out of two DIFFERENT, independent counters on the store: `ListCampaigns` pages on the campaign creation sequence (`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::next_sequence`) and `ListConnectInstanceIntegrations` pages on the integration sequence (`next_integration_sequence`). Both counters start at 0 and advance independently, so the two cursor spaces overlap completely.

The consequence is that a cursor minted by one list is accepted by the other and resolves to a position in a collection it was never computed against. `cursor_page` takes `start = items.position(|item| sequence_of(item) > after)`, so a `nextToken` whose value happens to be 2 — a perfectly ordinary integration cursor — makes `ListCampaigns` skip every campaign whose creation sequence is <= 2 and answer HTTP 200 with a SHORT page (or an empty one, with no `nextToken`, which a client reads as "the end of the list"). Campaigns the account holds silently vanish from the listing. The same happens in the other direction: a campaign cursor truncates an integration page.

That is neither of the two outcomes the crate says a `nextToken` has: it is not a valid resume point, and it is not refused. The crate's own stated rule is that a token it did not mint resolves to no position at all and is a `ValidationException`; nothing states that a token minted for a DIFFERENT collection is a valid resume point for this one, and the handler cannot tell the two apart because the token carries no indication of which listing issued it.

## Evidence

The token is a bare sequence value, with nothing identifying the collection — src/state.rs:643-645:
```
pub fn encode_next_token(last_sequence: u64) -> String {
    ::hex::encode(last_sequence.to_string())
}
```

One shared window computation, fed two different sequence spaces:
- src/ops/list_campaigns.rs:183-188:
```
        let window = crate::state::cursor_page(
            &matching,
            |(sequence, _)| *sequence,
            input.next_token.as_deref(),
            input.max_results,
        )
```
- src/ops/list_connect_instance_integrations.rs:236-241:
```
        let window = crate::state::cursor_page(
            &integrations,
            |stored| stored.sequence,
            input.next_token.as_deref(),
            input.max_results,
        )
```

The two sequence spaces are separate counters on the same store — src/state.rs:1909-1916:
```
    regions: HashMap<String, RegionStore>,
    next_sequence: u64,
    next_dial_sequence: u64,
    ...
    next_integration_sequence: u64,
```
and the doc above them only promises uniqueness WITHIN a kind, not across the two cursor spaces — src/state.rs:1901-1903: "The sequence counters stay store-wide: an id or cursor value is minted from one counter whatever region takes it, so no two campaigns, dial requests, profile requests or integrations anywhere in the store share one."

The resume step is a pure positional skip, so an out-of-collection value silently shortens the page — src/state.rs:708-717:
```
        let start = match next_token {
            None => 0,
            Some(token) => {
                let after = decode_next_token(token).ok_or(UnissuedCursor)?;
                items
                    .iter()
                    .position(|item| sequence_of(item) > after)
                    .unwrap_or(items.len())
            }
        };
```

The crate states that the only two fates of a token are "a position" or a refusal — src/state.rs:698-701: "A cursor names a position in the stored order rather than a record, so an item removed between two pages does not invalidate a token already handed out. A token this codec did not mint resolves to no position at all, which is [`UnissuedCursor`] rather than a page." — and src/ops/list_campaigns.rs:180: "A token this service did not mint is a bad request."

Test coverage: `tests/all/cursor_paging_is_one_rule.rs` is the one place cross-list token handling could be pinned, and it only exercises MALFORMED tokens — tests/all/cursor_paging_is_one_rule.rs:319-325:
```
    let tokens: Vec<(&str, String)> = vec![
        ("a word", "next-page".to_string()),
        ("a decimal position", "2".to_string()),
        ("hex of a non-number", ::hex::encode("second")),
        ("hex of a negative number", ::hex::encode("-2")),
        ("odd-length hex", "abc".to_string()),
    ];
```
No case sends a token the SIBLING list minted, so nothing pins the current behaviour.

## Steps to Reproduce

1. Onboard a Connect instance (`PUT /v2/connect-instance/{id}/onboarding`) and `PUT /v2/campaigns` three times against it, so the store holds campaigns at creation sequences 0, 1 and 2.
2. Onboard a second instance and `PUT /v2/connect-instance/{id2}/integrations` three times (e.g. three distinct `{"lambda": {"functionArn": ...}}` configs), so the integration store holds sequences 0, 1 and 2.
3. Call `GET /v2/connect-instance/{id2}/integrations?maxResults=1` and copy the `nextToken` it returns (hex of "0", i.e. "30").
4. Call `POST /v2/campaigns-summary` with `{"nextToken": "30"}`.
5. Observe HTTP 200 with a `campaignSummaryList` that OMITS the first campaign — the integration cursor was resolved as a campaign position — instead of the HTTP 400 `ValidationException` the crate states a token that names no position in this collection gets.
6. Repeat in the other direction: take a `nextToken` from `POST /v2/campaigns-summary` with `{"maxResults": 1}` and pass it as `GET /v2/connect-instance/{id2}/integrations?nextToken=...`; integrations are silently dropped from the page.
