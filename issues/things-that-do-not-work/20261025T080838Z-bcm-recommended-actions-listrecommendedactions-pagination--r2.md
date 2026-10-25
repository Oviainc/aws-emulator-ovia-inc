Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 521a9a87ca33ad668d554c20273a56e6e89cc063

# nextToken fingerprint concatenates caller-controlled filter values with unescaped delimiters, so a token is accepted for a different selection

_Filed: 2026-10-25T08:08:38Z_
_Operation: ListRecommendedActions | Class: pagination_

> _Note: this issue was identified against the code base at commit `521a9a87ca33ad668d554c20273a56e6e89cc063`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The pagination token's selection fingerprint is built by string-concatenating each filter as `key:matchOption:value,value` and joining the filters with `&`, then hashing the result. The delimiters `:`, `,` and `&` are NOT escaped and the `values` elements are arbitrary caller-supplied strings (`FilterValue` is a plain string, `length 1..1024`, `pattern [\S\s]*`), so two DIFFERENT selections can canonicalise to the same string and therefore to the same fingerprint. `parse_token` compares only that fingerprint, so a token minted over one selection is happily accepted when it is replayed with a different selection — exactly the case the handler documents as a refusal, and exactly the case the handler says would make "the offset it names ... land on an unrelated record". The result is a silent HTTP 200 reporting a page taken at an offset of an unrelated record set (an empty final page, or a page that skips records), instead of the documented `ValidationException`.

Concretely, one filter `{FEATURE, NOT_EQUALS, ["COST_OPTIMIZATION_HUB,RESERVATIONS"]}` (one value containing a comma — selects every record, since no record's feature equals that literal) canonicalises to `FEATURE:NOT_EQUALS:COST_OPTIMIZATION_HUB,RESERVATIONS`, byte-for-byte the same string as the genuinely different filter `{FEATURE, NOT_EQUALS, ["COST_OPTIMIZATION_HUB","RESERVATIONS"]}` (two values — excludes two features). The same hole exists across filters: `[{FEATURE,EQUALS,["ACCOUNT"]},{SEVERITY,EQUALS,["INFO"]}]` collides with the single filter `[{FEATURE,EQUALS,["ACCOUNT&SEVERITY:EQUALS:INFO"]}]`.

A secondary symptom of the same class, from the same canonicalisation: `query_fingerprint` sorts but does not de-duplicate the per-filter parts, so the list `[A, A]` — the same ANDed selection as `[A]`, since `keeps` is evaluated for every filter with `all` — produces a different fingerprint and a token minted over `[A]` is refused when replayed with `[A, A]`, although the comment calls the fingerprint "the fingerprint of a listing's SELECTION".

## Evidence

Handler builds the fingerprint from unescaped concatenation — `src/ops/list_recommended_actions.rs:339-349`:
```
    fn canonical(&self) -> String {
        let mut values: Vec<&str> = self.values.iter().map(String::as_str).collect();
        values.sort_unstable();
        values.dedup();
        format!(
            "{}:{}:{}",
            self.key.as_str(),
            self.match_option.as_str(),
            values.join(",")
        )
```
and `src/ops/list_recommended_actions.rs:435-441`:
```
fn query_fingerprint(filters: &[CompiledFilter]) -> String {
    let mut parts: Vec<String> = filters.iter().map(CompiledFilter::canonical).collect();
    parts.sort();
    let canonical = format!("filters={}", parts.join("&"));
    let digest = hex::encode(sha2::Sha256::digest(canonical.as_bytes()));
    digest[..TOKEN_FINGERPRINT_LEN].to_string()
```
The rule the code is supposed to implement, `src/ops/list_recommended_actions.rs:429-431`: "The fingerprint of a listing's SELECTION: the filters (order-independent, since they are ANDed). A `nextToken` carries it so a token cannot resume a listing of a different selection, where the offset it names would land on an unrelated record."
And the module doc, `src/ops/list_recommended_actions.rs:55-58`: "a token that does not parse, or that was issued over a different filter, is refused with `ValidationException` rather than silently restarting the listing."
The values that break it are values the model permits — `smithy_query(query="closure", operation="ListRecommendedActions")`: `"FilterValue": {"type": "string", "length": {"min": 1, "max": 1024}, "pattern": "[\\S\\s]*"}` — i.e. any characters, including `:`, `,` and `&`, so the constraint boundary cannot reject them, and `compile_filters` deliberately does not: `src/ops/list_recommended_actions.rs:410-414` "The ELEMENTS of `values` are NOT validated here: the model closes no value set over them".
Only `parse_token` guards the resume, and it compares the fingerprint alone — `src/ops/list_recommended_actions.rs:466-467`: `let (token_fingerprint, offset) = token.split_at(TOKEN_FINGERPRINT_LEN); if token_fingerprint != fingerprint {`.
Test coverage: `tests/all/list_recommended_actions.rs:1590-1628` (`a_token_still_names_the_selection_it_was_issued_over`) pins the rule only for selections whose values are bare enum spellings (`["BUDGETS"]` vs `["PAYMENTS"]`, filter added, filter dropped — `Resumption::refused(...)` at lines 1618-1627); no case uses a value containing `,`, `:` or `&`, and no case replays a token with a duplicated filter, so both symptoms are unasserted.

## Steps to Reproduce

1. Boot the emulator with this service mounted (the store boots with three computed records whose features are COST_OPTIMIZATION_HUB, RESERVATIONS and SAVINGS_PLANS — `src/state.rs:228-256`).
2. POST `/` with `X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions` and body `{"maxResults": 1, "filter": {"actions": [{"key": "FEATURE", "matchOption": "NOT_EQUALS", "values": ["COST_OPTIMIZATION_HUB,RESERVATIONS"]}]}}`. This selection matches all three records (no record's feature equals that one comma-containing literal); note the returned `nextToken` (offset 1).
3. POST `/` again with body `{"nextToken": "<token from step 2>", "filter": {"actions": [{"key": "FEATURE", "matchOption": "NOT_EQUALS", "values": ["COST_OPTIMIZATION_HUB", "RESERVATIONS"]}]}}` — a DIFFERENT selection (it matches only the SAVINGS_PLANS record).
4. Observed: HTTP 200. The token is accepted because both selections canonicalise to `filters=FEATURE:NOT_EQUALS:COST_OPTIMIZATION_HUB,RESERVATIONS`, and the offset is applied to the unrelated one-record selection, so the response is an empty `recommendedActions` page with no `nextToken`. Expected per the handler's own rule: HTTP 400 `ValidationException` (reason `cannotParse`, "issued for a different filter"), as the sibling cases at tests/all/list_recommended_actions.rs:1618-1627 get.
5. Secondary symptom: POST `{"maxResults": 1, "filter": {"actions": [{"key": "FEATURE", "matchOption": "NOT_EQUALS", "values": ["ACCOUNT"]}]}}`, then replay the returned token with the same filter listed TWICE in `filter.actions` (the same ANDed selection). Observed: HTTP 400; expected: the listing resumes, since the selection is unchanged.
