Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: cc22e59eeda2de4fd1330e74811c0c72a53dba7a

# decode_next_token honours cursor spellings encode_next_token never writes ("+N", leading zeros), so a token this service did not issue is served as a page

_Filed: 2026-10-29T09:59:26Z_
_Operation: ListConnectInstanceIntegrations | Class: pagination_

> _Note: this issue was identified against the code base at commit `cc22e59eeda2de4fd1330e74811c0c72a53dba7a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both paged lists of this crate resume from a cursor decoded by the single helper `crate::state::decode_next_token`. Its own doc states the rule it is supposed to implement: a token "not produced by" `encode_next_token` must decode to `None`. The body breaks that rule, because the final step is `str::parse::<u64>()`, which accepts spellings the minting side never writes:

* `encode_next_token` writes `hex::encode(last_sequence.to_string())`, and `u64::to_string()` emits a bare decimal with no sign and no leading zeros — so `hex::encode("5")` is the ONLY encoding of position 5 this service ever hands out;
* `u64::from_str`, however, also accepts an optional leading `+` and any number of leading zeros, so `hex::encode("+5")` and `hex::encode("005")` both decode to `Some(5)`.

The effect is not a cosmetic one. `cursor_page` documents that "A token this codec did not mint resolves to no position at all, which is [`UnissuedCursor`] rather than a page", and both handlers map `UnissuedCursor` to HTTP 400 `ValidationException`. A fabricated `nextToken` of the two shapes above skips every record at or before that sequence number and answers HTTP 200 with a SILENTLY TRUNCATED page instead of refusing the token — the caller cannot tell a short page caused by a bogus cursor from a genuine end of the list, because the response also carries no `nextToken` once the window reaches the end.

`ListCampaigns` reaches the same helper through the same call, so the one decode site serves both listings.

## Evidence

The minting side never writes a sign or a leading zero — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:824-826:

```
pub fn encode_next_token(last_sequence: u64) -> String {
    ::hex::encode(last_sequence.to_string())
}
```

The decode side states the rule and then breaks it — src/state.rs:828-835:

```
/// Decode a cursor produced by [`encode_next_token`], or `None` when the token
/// was not produced by it.
pub fn decode_next_token(token: &str) -> Option<u64> {
    String::from_utf8(::hex::decode(token).ok()?)
        .ok()?
        .parse()
        .ok()
}
```

`.parse()` here is `u64::from_str`, which accepts `"+5"` and `"005"` as 5 — neither of which `last_sequence.to_string()` can produce.

The rule the window computation relies on — src/state.rs:879-882 (doc of `cursor_page`):

```
/// A cursor names a position in the stored order rather than a record, so an item
/// removed between two pages does not invalidate a token already handed out. A
/// token this codec did not mint resolves to no position at all, which is
/// [`UnissuedCursor`] rather than a page.
```

and the position it computes from a decoded cursor — src/state.rs:889-898:

```
    let start = match next_token {
        None => 0,
        Some(token) => {
            let after = decode_next_token(token).ok_or(UnissuedCursor)?;
            items
                .iter()
                .position(|item| sequence_of(item) > after)
                .unwrap_or(items.len())
```

The handler that turns the refusal into the 400 — src/ops/list_connect_instance_integrations.rs:236-247:

```
        let window = crate::state::cursor_page(
            &integrations,
            |stored| stored.sequence,
            input.next_token.as_deref(),
            input.max_results,
        )
        .map_err(|unissued| {
            ListConnectInstanceIntegrationsErrorModeled::ValidationException(ValidationException {
                message: unissued.message(),
```

Test coverage: tests/all/cursor_paging_is_one_rule.rs:312-325 is the suite that pins "a token the service did not mint resolves to no position at all", and its table enumerates exactly five un-minted spellings — none of them a `+`-signed or leading-zero decimal:

```
    let tokens: Vec<(&str, String)> = vec![
        ("a word", "next-page".to_string()),
        ("a decimal position", "2".to_string()),
        ("hex of a non-number", ::hex::encode("second")),
        ("hex of a negative number", ::hex::encode("-2")),
        ("odd-length hex", "abc".to_string()),
    ];
```

so the `hex::encode("+2")` / `hex::encode("02")` rows are unpinned and currently answered as a page.

## Steps to Reproduce

1. Onboard a Connect instance (`PUT /v2/connect-instance/{id}/onboarding`) and wait for its job to reach SUCCEEDED.
2. Put three Lambda integrations on it with `PUT /v2/connect-instance/{id}/integrations`, so the store holds integrations at three consecutive sequence numbers (say 0, 1, 2).
3. Call `GET /v2/connect-instance/{id}/integrations` with no token: all three come back and no `nextToken` is handed out.
4. Compute a token this service never minted: hex-encode the ASCII text `+1` (`2b31`) — or `01` (`3031`).
5. Call `GET /v2/connect-instance/{id}/integrations?nextToken=2b31`. Expected per src/state.rs:828-829 and the `cursor_page` doc at src/state.rs:879-882: HTTP 400 `ValidationException` carrying "nextToken is not a cursor this service issued". Actual: HTTP 200 with only the third integration, the first two silently dropped and no indication the cursor was not one of this service's own.
6. Repeat the same token against `POST /v2/campaigns-summary` for the other listing that calls the same helper.
