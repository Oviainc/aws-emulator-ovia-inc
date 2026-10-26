Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 55c71d6cc75db3eca6552dffd5fa68c4cb6ec346

# nextToken selection fingerprint collides when a filter value contains the canonical-string delimiters

_Filed: 2026-10-26T03:36:56Z_
_Operation: ListRecommendedActions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `55c71d6cc75db3eca6552dffd5fa68c4cb6ec346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The pagination token carries a 128-bit fingerprint of the SELECTION it was issued over, and the handler refuses a token whose fingerprint does not match the current request's filter. That fingerprint is a SHA-256 of a hand-built canonical string in which the filter's parts are concatenated with un-escaped delimiters: each filter becomes "<key>:<matchOption>:<value>,<value>..." and several filters are joined with "&". `ActionFilter$values` elements are plain strings the model allows to contain any character (pattern `[\S\s]*`, length 1..1024), so a caller-supplied value may itself contain `,`, `:` or `&` and produce the same canonical string — and therefore the same fingerprint — as a DIFFERENT selection.

When that happens the token check passes and the handler resumes a listing of a different selection at an offset that was computed over another record set: records are silently skipped (or the answer is an empty page) instead of the `ValidationException` the handler's own documentation promises for a token "issued for a different filter". The resulting response is a 200 whose page content is wrong, which is exactly the failure the fingerprint exists to prevent.

## Evidence

The rule the code states for itself — `src/ops/list_recommended_actions.rs:429-431`:
```
/// The fingerprint of a listing's SELECTION: the filters (order-independent, since
/// they are ANDed). A `nextToken` carries it so a token cannot resume a listing of a
/// different selection, where the offset it names would land on an unrelated record.
```
and the module doc, `src/ops/list_recommended_actions.rs:56-58`: "a token that does not parse, or that was issued over a different filter, is refused with `ValidationException` rather than silently restarting the listing."

The code that breaks it — `src/ops/list_recommended_actions.rs:339-348` (`CompiledFilter::canonical`):
```
        format!(
            "{}:{}:{}",
            self.key.as_str(),
            self.match_option.as_str(),
            values.join(",")
        )
```
and `src/ops/list_recommended_actions.rs:435-441` (`query_fingerprint`):
```
    let mut parts: Vec<String> = filters.iter().map(CompiledFilter::canonical).collect();
    parts.sort();
    let canonical = format!("filters={}", parts.join("&"));
    let digest = hex::encode(sha2::Sha256::digest(canonical.as_bytes()));
```
Nothing escapes a value, and the handler explicitly declines to constrain values — `src/ops/list_recommended_actions.rs:410-414`: "The ELEMENTS of `values` are NOT validated here: the model closes no value set over them".

The model permits the colliding values: `smithy_query(query="shape", shape="FilterValues")` → `"member": {"target": "FilterValue", "length": {"min": 1, "max": 1024}, "pattern": "[\\S\\s]*"}` — so `"BUDGETS,PAYMENTS"` and `"X&SEVERITY:NOT_EQUALS:INFO"` are legal single values.

Consequence at the accept path, `src/ops/list_recommended_actions.rs:466-474`: `if token_fingerprint != fingerprint { return Err(cannot_parse("The nextToken was issued for a different request. ...")) }` — with a collision the comparison succeeds, and `src/ops/list_recommended_actions.rs:600-601` then applies the foreign offset to the current selection: `let start = resume_at.min(matching.len()); let end = start.saturating_add(page_size).min(matching.len());`.

Test coverage: `tests/all/list_recommended_actions.rs:1591` (`a_token_still_names_the_selection_it_was_issued_over`) pins the refusal only for delimiter-free values — line 1620 `Resumption::refused("a different filter value", &budgets, 1, &payments, Some(1))`, where the selections are `values: ["BUDGETS"]` vs `values: ["PAYMENTS"]` (lines 1596-1601). No case uses a value containing `,`, `:` or `&`, so the collision is unpinned.

## Steps to Reproduce

1. Start the emulator with an empty store and seed three records: r1 (feature BUDGETS, severity INFO), r2 (feature PAYMENTS, severity INFO), r3 (feature IAM, severity WARNING).
2. POST / with X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions and body {"filter":{"actions":[{"key":"FEATURE","matchOption":"NOT_EQUALS","values":["X&SEVERITY:NOT_EQUALS:INFO"]}]},"maxResults":2} — selection B keeps all three records (no record's feature equals that literal), so the response is [r1, r2] plus a nextToken T minted at offset 2 over the canonical string "filters=FEATURE:NOT_EQUALS:X&SEVERITY:NOT_EQUALS:INFO".
3. POST again with the DIFFERENT selection A and that token: {"filter":{"actions":[{"key":"FEATURE","matchOption":"NOT_EQUALS","values":["X"]},{"key":"SEVERITY","matchOption":"NOT_EQUALS","values":["INFO"]}]},"nextToken":"<T>"} — selection A keeps only r3, and its canonical string sorts/joins to the same "filters=FEATURE:NOT_EQUALS:X&SEVERITY:NOT_EQUALS:INFO", so the fingerprint matches and the token is accepted.
4. Observe HTTP 200 with recommendedActions: [] and no nextToken: the offset 2 issued over selection B was applied to selection A (length 1), so r3 is never reported and no ValidationException is raised, contradicting the handler's documented rule that a token issued over a different filter is refused.
5. Simpler variant of the same collision: mint a token with {"filter":{"actions":[{"key":"FEATURE","matchOption":"EQUALS","values":["BUDGETS","PAYMENTS"]}]},"maxResults":1}, then present it with {"filter":{"actions":[{"key":"FEATURE","matchOption":"EQUALS","values":["BUDGETS,PAYMENTS"]}]},"nextToken":"<T>"} — both canonicalize to "filters=FEATURE:EQUALS:BUDGETS,PAYMENTS", so the foreign token is accepted instead of refused.
