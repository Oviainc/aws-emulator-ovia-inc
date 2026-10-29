Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 6cdf7a4bb780a1593da31764ba2b1e15a7af6027

# decode_page_token honours hex spellings encode_page_token never mints (uppercase/mixed-case hex), so a NextToken this service never issued is served as a page

_Filed: 2026-10-29T12:19:06Z_
_Operation: DescribeGlobalNetworks | Class: pagination_

> _Note: this issue was identified against the code base at commit `6cdf7a4bb780a1593da31764ba2b1e15a7af6027`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing of this crate decodes `NextToken` through the single helper `crate::state::decode_page_token`, and each one states the rule it is enforcing: a token this service did not mint names no position in the listing, so it must be refused on the member rather than answered with a page.

The minter and the reader disagree on the token alphabet. `encode_page_token` writes the token with `hex::encode`, which emits LOWERCASE hex only. `decode_page_token` reads it with `hex::decode`, which accepts upper-, lower- and mixed-case hex digits alike. A token whose hex digits have been re-cased — `6E65...` for the `6e65...` the service minted — therefore decodes to the same bytes, passes the `networkmanager:` prefix check, and is honoured as a resume point, even though no call of this service ever handed that string out.

The crate treats the hex alphabet as significant elsewhere: the shared refusal suite crosses every listing with tokens "differing in alphabet (not hex at all, lowercase hex, uppercase hex)" and asserts a 400 for "uppercase hex of foreign text" — it is refused for its TEXT, not its case, so the re-cased spelling of a token the service DID mint slips through untested. Tightening the decoder to the one alphabet the minter writes leaves every existing assertion in that suite (and in the paging suites, which always feed back the exact token they were handed) unchanged.

## Evidence

The rule each listing states — src/ops/describe_global_networks.rs:153-158:
```
        // An opaque token this service did not mint cannot name a key offset, so it is a
        // client error rather than an empty page.
        let after = match input.next_token.as_deref() {
            Some(token) => Some(
                crate::state::decode_page_token_or_reject(token)
                    .map_err(DescribeGlobalNetworksErrorModeled::ValidationException)?,
```
The minter writes lowercase hex, the reader accepts any case — src/state.rs:8745-8764:
```
/// The prefix every pagination token this service mints carries, so a token minted by
/// something else (or a truncated one) is recognised as unparseable.
const PAGE_TOKEN_PREFIX: &str = "networkmanager:";
...
fn encode_page_token(last_key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{last_key}"))
}
...
pub fn decode_page_token(token: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    text.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}
```
(`hex::encode` emits `0-9a-f` only; `hex::decode` accepts `A-F` as well, so `decode_page_token(encode_page_token(k).to_uppercase()) == Some(k)`.)

The shared refusal suite shows the alphabet is deliberately part of the class under test, yet only ever re-cases FOREIGN text — tests/all/unminted_page_token_refusal.rs:15-20:
```
//! The tables below span that class: every listing the service pages with a token — all 23
//! of them, reached at the method and URI the model binds each to (`smithy_query
//! query="digest"`) — crossed with tokens differing in alphabet (not hex at all, lowercase
//! hex, uppercase hex), in length (odd-length hex, and the modelled 2048-character bound)
```
tests/all/unminted_page_token_refusal.rs:174-177:
```
        // "not-ours" and "NOT-OURS", in hex: readable text carrying no prefix of this
        // service, in each hex alphabet.
        ("lowercase hex of foreign text", "6e6f742d6f75727300".to_string()),
        ("uppercase hex of foreign text", "4E4F542D4F555253".to_string()),
```
The helper all 23 listings share — src/state.rs:8776-8785 (`decode_page_token_or_reject`) — so the laxity is service-wide; `DescribeGlobalNetworks` is the first operation of the checklist whose handler calls it.

## Steps to Reproduce

1. CreateGlobalNetwork twice so the listing holds two records
2. GET /global-networks?maxResults=1 → 200 with a NextToken, e.g. `6e6574776f726b6d616e616765723a676c6f62616c2d6e6574776f726b2d…` (lowercase hex of `networkmanager:global-network-…`)
3. Upper-case every hex digit of that token and GET /global-networks?maxResults=1&nextToken=<THE UPPERCASED TOKEN> → 200, resuming the listing, although no response of this service ever contained that string: src/state.rs:8762 decodes it because `hex::decode` accepts `A-F`
4. Expected from the handler's own rule (src/ops/describe_global_networks.rs:153-154) and from the refusal the suite pins for every token the service did not mint: ValidationException (400), Reason FieldValidationFailed, one Fields entry naming NextToken
