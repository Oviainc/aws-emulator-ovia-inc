Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 6bea01b02671f5bc5cdfd807295cdb4bfa0eb328

# decode_page_token honours cursor spellings encode_page_token never writes, so a token this service never issued is served as a page

_Filed: 2026-10-29T10:55:53Z_
_Operation: ListIdentityProviders | Class: pagination_

> _Note: this issue was identified against the code base at commit `6bea01b02671f5bc5cdfd807295cdb4bfa0eb328`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every list operation of this service pages through `crate::state::paginate`, whose cursor is `hex("<list kind>:<offset>")`. The minting side writes the offset through `usize`'s `Display`, so the only spellings it can produce are canonical lowercase-hex envelopes carrying a bare decimal offset with no sign and no leading zeros.

The reading side is wider than the writing side in two ways: `offset.parse::<usize>()` accepts a leading `+` and any number of leading zeros (`"+1"`, `"0000000001"` all parse to 1), and `::hex::decode` accepts upper-case and mixed-case hex digits. So a client that fabricates `hex("identity-providers/:+1")` — or upper-cases a token it was given — is served a page at that position instead of being refused, which is the opposite of what the helper's own documentation promises.

The doc of `paginate` states the rule the code breaks: a `NextToken` is honoured only when it was "minted by the previous page of the SAME list ... and rejected otherwise". A `+1`-spelled cursor was minted by no page of any list, yet it is honoured.

## Evidence

The rule the helper states — `src/state.rs:2781-2786`:
```
/// `MaxResults` is the page size; a `NextToken` is this service's opaque cursor, minted
/// by the previous page of the SAME list (`kind` names that list: the operation and
/// whatever selected it) and rejected otherwise — a token is not a client-chosen value,
/// so an unparseable or foreign one is a `ValidationException` rather than a silent
/// restart from the beginning.
```
and `src/state.rs:2824-2827`: "What makes a cursor trustworthy here is the `kind` check in [`decode_page_token`]: a token is only honoured by the list that minted it, so a forged or stale one is refused rather than applied to a different list."

The ONLY spelling the minter writes — `src/state.rs:2828-2830`:
```
fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{kind}:{offset}"))
}
```

The reader accepts more — `src/state.rs:2835-2843`:
```
fn decode_page_token(kind: &str, token: &str) -> Result<usize, StateError> {
    let invalid = || StateError::Validation("NextToken is not a valid pagination token".to_string());
    let decoded = ::hex::decode(token).map_err(|_| invalid())?;
    ...
    offset.parse::<usize>().map_err(|_| invalid())
}
```
`usize::from_str` accepts an optional `+` sign and leading zeros, and `hex::decode` accepts `A-F` as well as `a-f`, so three families of token the minter cannot produce are honoured.

Reached from every list handler, e.g. `src/ops/list_identity_providers.rs:170-181` (`crate::state::paginate(&matching, &format!("identity-providers/{}", crate::state::filter_signature(&filters)), input.max_results, input.next_token.as_deref())`).

Test coverage stops short of these spellings — `tests/all/list_instances.rs:511-514` pins only four refusal classes:
```
("a token that is not hex", json!({ "NextToken": "not-a-token" })),
("a hex token from no list of ours", json!({ "NextToken": "6f746865723a30" })),
("a token minted by another list", json!({ "NextToken": foreign })),
("a token minted under other filters", json!({ "NextToken": other_filters })),
```
and `tests/all/license_server_endpoints.rs:467-483` / `tests/all/identity_provider_subscriptions.rs:1081-1085` pin the same four. None sends a right-kind token with a non-canonical offset or an upper-cased envelope.

## Steps to Reproduce

1. Register three identity providers so the list has more than one page.
2. POST /identity-provider/ListIdentityProviders with `{"MaxResults":1}` and note that the returned `NextToken` is `hex("identity-providers/:1")`.
3. Fabricate a token this service never issued: hex-encode the string `identity-providers/:+1` (or `identity-providers/:0000000001`), and send it as `NextToken`.
4. Observe HTTP 200 with the second page instead of the `ValidationException` the helper's own doc says a token not minted by a previous page of this list gets.
5. Also take a genuine token and upper-case its hex digits; observe it is still accepted, although `encode_page_token` only ever emits lower-case hex.
