Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: ef111eb3ea31ef4faca303fd106b7202052fa3ed

# decode_page_token honours nextToken hex spellings encode_page_token never writes (uppercase/mixed-case hex), so a token this service never issued is served as a page instead of refused

_Filed: 2026-10-29T13:26:04Z_
_Operation: ListFeeds | Class: pagination_

> _Note: this issue was identified against the code base at commit `ef111eb3ea31ef4faca303fd106b7202052fa3ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this service (ListFeeds, ListDictionaries, SearchFixtures) shares one token codec in `crate::state`. `encode_page_token` emits the token through `::hex::encode`, which writes LOWERCASE hex only, so every token this service ever hands out is lowercase hex. `decode_page_token` reads it back with `::hex::decode`, which is documented as case-insensitive (hex 0.4.3: "Both, upper and lower case characters are valid in the input string and can even be mixed (e.g. f9b4ca, F9B4CA and f9B4Ca are all valid strings)"). An uppercase or mixed-case re-spelling of a real token therefore decodes to the identical plaintext `1:<digest>:<last id>`, matches the layout marker and the criteria digest, and is accepted as a resume point — although it is a byte string this service never issued in any request.

That contradicts the rule the codec's own documentation states for `PageTokenRejection::Malformed` ("for a token this service never issues in any request") and the rule the crate's pagination test states in its own words ("A token this service never issued is refused rather than read as a cursor"). The consequence is the opposite answer on the wire: HTTP 200 with a page, where the stated rule is the modelled `ValidationException` (HTTP 400) carrying `PAGE_TOKEN_MALFORMED_MESSAGE`. The decode path is otherwise strict about every field it reads — layout marker, digest length, non-empty id, digest equality — so the case leniency is the one spelling it accepts that it never produces. Note the asymmetry inside the same function: an uppercase INNER digest is caught (`digest != page_criteria_digest(criteria)` is a byte comparison against lowercase), while the uppercase OUTER hex layer slips through, so the same leniency is already rejected one layer down.

## Evidence

The encoder only ever writes lowercase hex — `src/state.rs:2151-2156`: `pub fn encode_page_token(criteria: &str, last_id: &str) -> String { ::hex::encode(format!("{PAGE_TOKEN_LAYOUT}:{}:{last_id}", page_criteria_digest(criteria))) }`.

The decoder accepts any hex case — `src/state.rs:2168`: `let bytes = ::hex::decode(token).map_err(|_| PageTokenRejection::Malformed)?;` (hex 0.4.3, `Cargo.lock:3177-3179`: `name = "hex"` / `version = "0.4.3"`, whose `decode` documents "Both, upper and lower case characters are valid in the input string and can even be mixed").

The rule the codec says it enforces — `src/state.rs:2161-2163`: "[`PageTokenRejection::Malformed`] for a token this service never issues in any request — one that is not hex, or that carries none of a token's fields", and `src/state.rs:2063-2064`: "Not a token this service issues: not hex, or hex that carries none of the fields a token has."

The same rule in the crate's test — `tests/all/pagination_token_binding.rs:470-472`: "A token this service never issued is refused rather than read as a cursor: one that is not a token of this service at all, and one shaped like a token but carrying another request's digest." Its case list (`:487-495`: `"not-hex", "zzzz", "", "30", hex_of_an_id, hex_of_a_digit, truncated`) and its forged-digest list (`:508-518`) contain no case variant of a real token, so nothing pins the lenient behaviour: `real.to_uppercase()` is served as a page. Same codec, same hole, for the two list routes (`src/ops/list_feeds.rs:215`, `src/ops/list_dictionaries.rs:201`) and the search (`src/ops/search_fixtures.rs:355`).

## Steps to Reproduce

1. Create 3 feeds in one Region (POST /v1/feed three times, each with one output)
2. GET /v1/feeds?maxResults=1 and copy the returned nextToken T (lowercase hex, as ::hex::encode wrote it)
3. GET /v1/feeds?maxResults=1&nextToken=<T uppercased> — observe HTTP 200 and the second page, i.e. a byte string this service never issued honoured as a resume point
4. Expected per src/state.rs:2161-2163 and tests/all/pagination_token_binding.rs:470-472: HTTP 400 ValidationException with PAGE_TOKEN_MALFORMED_MESSAGE ("nextToken is not a valid pagination token."), the same answer the non-hex cases already get
5. The same two steps reproduce on GET /v1/dictionaries and on POST /v1/fixtures (SearchFixtures), since all three share crate::state::decode_page_token
