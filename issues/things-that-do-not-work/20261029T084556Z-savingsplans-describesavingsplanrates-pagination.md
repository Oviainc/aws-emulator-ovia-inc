Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 14adfff1be9ff0781ca2ef74e7b38762413218b9

# decode_page_token honours cursor spellings encode_page_token never writes, so a token this service never issued is served as a page

_Filed: 2026-10-29T08:45:56Z_
_Operation: DescribeSavingsPlanRates | Class: pagination_

> _Note: this issue was identified against the code base at commit `14adfff1be9ff0781ca2ef74e7b38762413218b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four paginated handlers in this crate resume a walk through `crate::state::decode_page_token`, whose own doc says it answers `None` "when it was not produced by this paginator", and each handler turns that `None` into a 400 `ValidationException` whose message is "nextToken is not a token this operation issued".

The decode does not keep that promise. `encode_page_token` writes the offset with `format!("{kind}:{offset}")`, i.e. plain decimal digits with no sign and no leading zeros, but the decode reads it back with `str::parse::<usize>()`, which also accepts a leading `+` and any number of leading zeros. So `hex::encode("plans:+5")` and `hex::encode("plans:005")` — payloads `encode_page_token` can never produce — decode to offset 5, and `DescribeSavingsPlans` answers 200 with a page that silently starts at the sixth matching plan instead of refusing a cursor it never issued. The same holds for `planRates`, `offerings` and `offeringRates`, since all four decode through the one helper.

The forged token also passes the model's boundary check: `PaginationToken` is `^[A-Za-z0-9/=\+]+$`, and `hex::encode` output is all `[0-9a-f]`, so nothing refuses it before `handle` runs. The result is a silently truncated listing where the handler's own sentence says there must be a 400.

## Evidence

The helper pair — the mint is canonical decimal, the decode is not — services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs:404-418:

```
/// Encode a page offset into an opaque `PaginationToken`.
///
/// Hex keeps the token inside the model's `^[A-Za-z0-9/=\+]+$` pattern, and
/// `kind` keeps a token minted by one paginator from resuming another's page.
pub fn encode_page_token(kind: &str, offset: usize) -> String {
    hex::encode(format!("{kind}:{offset}"))
}

/// Decode a token [`encode_page_token`] produced, or `None` when it was not
/// produced by this paginator.
pub fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let raw = hex::decode(token).ok()?;
    let text = String::from_utf8(raw).ok()?;
    text.strip_prefix(kind)?.strip_prefix(':')?.parse().ok()
}
```

`format!("{kind}:{offset}")` on a `usize` writes only `0-9`; `.parse()` on the way back is `usize::from_str`, which additionally accepts `+N` and leading zeros — so the set of payloads the decode honours is strictly larger than the set the mint writes, which is exactly what the decode's doc line ("or `None` when it was not produced by this paginator") denies.

The rule each handler states in the error it raises off that `None` — src/ops/describe_savings_plan_rates.rs:159-164:

```
        let offset = match input.next_token.as_deref() {
            Some(token) => crate::state::decode_page_token(PAGE_KIND, token).ok_or_else(|| {
                validation("nextToken is not a token this operation issued".to_string())
            })?,
            None => 0,
        };
```

and byte-for-byte the same block in src/ops/describe_savings_plans.rs:150-155 (`PAGE_KIND = "plans"`), src/ops/describe_savings_plans_offerings.rs:164-169 (`"offerings"`) and src/ops/describe_savings_plans_offering_rates.rs:168-173 (`"offeringRates"`).

Where the honoured offset lands — src/ops/describe_savings_plans.rs:184 `let (page, next) = crate::state::paginate(&matched, offset, limit);` with src/state.rs:427 `let start = offset.min(items.len());` — so offset 5 from the forged token simply drops the first five matching plans from the answer.

Test coverage of the intent, which the gap sits just outside: tests/all/plan_rate_card.rs:746-787 `a_cursor_from_another_search_is_refused` drives three rows — "a token from the offering-rate search", "a token from the plan search", "a token this service never mints" (`"ZZZZ"`) — and asserts 400 + `ValidationException` for each. No row covers a well-formed `kind` with a non-canonical offset spelling, so `hex::encode("planRates:+1")` passes today.

## Steps to Reproduce

1. Boot the emulator and purchase a plan, then call DescribeSavingsPlans with maxResults 1 six times (or purchase six plans) so the store holds at least six plans.
2. Compute a forged cursor: hex-encode the ASCII text `plans:+5` → `706c616e733a2b35` (it matches the model's PaginationToken pattern `^[A-Za-z0-9/=\+]+$`, so the boundary validator passes it through).
3. POST /DescribeSavingsPlans with `{"nextToken": "706c616e733a2b35"}`.
4. Observe 200 with a page starting at the sixth plan, instead of the 400 ValidationException "nextToken is not a token this operation issued" that src/ops/describe_savings_plans.rs:150-155 raises for any token this paginator did not mint — `encode_page_token` (src/state.rs:408) can only ever write `plans:5`, never `plans:+5`.
5. Repeat with `hex::encode("plans:005")` for the leading-zero spelling, and with `planRates:`/`offerings:`/`offeringRates:` against the other three paginated operations — all four share the same decode.
