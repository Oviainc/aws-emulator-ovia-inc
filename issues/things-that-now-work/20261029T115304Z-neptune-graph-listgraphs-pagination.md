Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: da4330c5500325c8e5ff90f3211b2833b72817eb

# decode_page_token honours uppercase/mixed-case hex spellings encode_page_token never mints, so a nextToken this service never issued is served as a page instead of refused

_Filed: 2026-10-29T11:53:04Z_
_Operation: ListGraphs | Class: pagination_

> _Note: this issue was identified against the code base at commit `da4330c5500325c8e5ff90f3211b2833b72817eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service has ONE pagination-token codec in `crate::state`, shared by all five listings (`ListGraphs`, `ListGraphSnapshots`, `ListImportTasks`, `ListExportTasks`, `ListPrivateGraphEndpoints`). The minting half is `hex::encode`, which emits LOWERCASE hex digits only; the reading half is `hex::decode`, which (hex 0.4.3) accepts `A`..`F` as well as `a`..`f`. So every listing accepts a token spelling it can never have handed out.

Concretely, the token for the position `g-0123456789` is only ever minted as `672d30313233343536373839`; the handler nevertheless accepts `672D30313233343536373839` (and any mixed-case variant such as `672d3031323334353637_3839` uppercased in part) and serves a page positioned at `g-0123456789`. There are 2^k such accepted spellings for a token containing k hex letters, exactly one of which the service issues.

That contradicts the rule the codec and every handler that calls it state in their own words: the decode half is documented to answer `None` for anything that is not a token `encode_page_token` minted, and each listing's gate says a token this service did not issue "names no page, so it is refused rather than treated as the start of the listing". The refusal exists so a client cannot assemble a resume point the service never cut (the same reason one listing's token is barred from paging another's id space, which IS enforced via the `is_*_id` predicate). Because the decoded value of an uppercase spelling is byte-identical to the issued one, the id-space predicate cannot catch it: the case check has to happen on the TOKEN, before decoding.

The gap is in the shared helper `crate::state::decode_page_token`; `ListGraphs` is named here as the first listing whose handler calls it (via `list_graphs::decode_page_token`), and the same single fix covers the other four call sites.

## Evidence

The minting half writes lowercase hex only — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/state.rs:2405-2413:
```
/// The pagination token that resumes a listing after `last_returned_key` — the id, or
/// other stable key, of the last entry the page just returned.
///
/// ONE token format for every listing of this service, so a client that pages one
/// listing and then another sees one token grammar: hex keeps the token opaque to a
/// client and within `PaginationToken`'s 1..8192 length.
pub fn encode_page_token(last_returned_key: &str) -> String {
    hex::encode(last_returned_key)
}
```

The reading half claims to accept only what that minted, but delegates the spelling to `hex::decode` — src/state.rs:2415-2429:
```
/// The position a token resumes after, or `None` when it is not a token
/// [`encode_page_token`] minted for the id space `in_id_space` accepts.
...
pub fn decode_page_token(token: &str, in_id_space: impl Fn(&str) -> bool) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    in_id_space(&decoded).then_some(decoded)
}
```
and `hex` is version 0.4.3 — Cargo.lock:3153-3157:
```
[[package]]
name = "hex"
version = "0.4.3"
```
whose `FromHex` accepts `b'A'..=b'F'` as well as `b'a'..=b'f'`, while `hex::encode` emits lowercase only — so the accepted set is strictly larger than the minted set.

The rule the handler states, which this breaks — src/ops/list_graphs.rs:179-187:
```
        // A token this service did not issue names no page, so it is refused rather
        // than treated as the start of the listing. The refusal is the service's own
        // modelled 400, worded and classified once in `crate::state`.
        let start_after = match input.next_token.as_deref() {
            Some(token) => Some(decode_page_token(token).ok_or_else(|| {
                ListGraphsErrorModeled::ValidationException(unissued_page_token(token))
            })?),
            None => None,
        };
```
(the identical gate and comment sit at src/ops/list_graph_snapshots.rs:170-178, src/ops/list_import_tasks.rs:169-177, src/ops/list_export_tasks.rs:158-166 and src/ops/list_private_graph_endpoints.rs:174-184).

Test coverage, and the hole in it: the unit test's own doc states the rule — src/ops/list_graphs.rs:224-226:
```
    /// A token round-trips to the id it was minted from, and a token this service
    /// never issued is rejected rather than silently treated as the first page.
```
but its rejection rows are only non-hex, odd-length, and hex of values outside the id space (src/ops/list_graphs.rs:237-245: `"not-a-token"`, `"zz"`, `"abc"`, `hex::encode("not-a-graph-id")`, `hex::encode("g-short")`, `hex::encode("gs-0123456789")`). The wire suite is the same: tests/all/unissued_page_token_refusal.rs:183-203 enumerates "the FORM of the unissued token" as `"not-a-token"`, `"abc"`, `hex::encode("not-a-position-at-all")`, `hex::encode(listing.prefixed_but_invalid)`, `hex::encode("z".repeat(1500))` and each other id space — no case variant. tests/all/pagination_token_codec.rs:371-375 pins the ISSUED spelling as lowercase:
```
                assert_eq!(
                    issued,
                    hex::encode(last),
                    "case {case}: expected the token to be the hex encoding of {last:?}, got {issued:?}"
                );
```
and `grep -n "to_uppercase|to_ascii_uppercase|UPPER" tests/` over the crate matches nothing, so no existing test or comment pins — or justifies — accepting the uppercase spelling.

## Steps to Reproduce

1. Boot the emulator from this checkout and point a botocore `neptune-graph` client (or plain HTTP) at it.
2. Call `CreateGraph(graphName='token-case', provisionedMemory=16)` so the graph listing has at least one row.
3. `GET /graphs?nextToken=672d30313233343536373839` — the lowercase hex of the position `g-0123456789`, the only spelling `encode_page_token` ever mints: answers 200 and resumes after that position.
4. `GET /graphs?nextToken=672D30313233343536373839` — the same bytes in UPPERCASE hex, a token this service never issued: expected 400 `ValidationException` with `reason=ILLEGAL_ARGUMENT` and message "nextToken '672D30313233343536373839' is not a pagination token this service issued" (src/state.rs:2443-2448), as the handler comment at src/ops/list_graphs.rs:179-181 promises; observed 200 with the resumed page.
5. Repeat step 4 against `/snapshots`, `/importtasks`, `/exporttasks` and `/graphs/{graphId}/endpoints/` with the uppercase hex of that listing's own id space (`gs-0123456789`, `t-0123456789`, `vpc-0a1b2c3d4e5f00000`): each of the five listings accepts the unissued spelling through the same shared `crate::state::decode_page_token`.
