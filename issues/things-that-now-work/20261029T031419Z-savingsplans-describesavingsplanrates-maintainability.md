Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7a484987dc823baaa4ff3cdf8b9628f66a8b0eeb

# The "decode the page cursor or refuse it" rule is written out four times, once per paginated handler

_Filed: 2026-10-29T03:14:19Z_
_Operation: DescribeSavingsPlanRates | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7a484987dc823baaa4ff3cdf8b9628f66a8b0eeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation in this crate re-implements the same cursor-decoding rule inline instead of reaching for a shared helper beside `encode_page_token`/`decode_page_token`:

* an absent `nextToken` means offset 0;
* a `nextToken` that THIS paginator did not mint is the operation's modelled `ValidationException`, carrying the sentence "nextToken is not a token this operation issued".

The block appears byte-for-byte identical in `DescribeSavingsPlanRates`, `DescribeSavingsPlans`, `DescribeSavingsPlansOfferings` and `DescribeSavingsPlansOfferingRates`; the only things that vary are the per-operation `PAGE_KIND` constant and which operation's `validation(..)` wrapper the message is handed to. So the rule — which default, which error, which wording — lives in four places and the copies can drift.

The drift risk is not hypothetical: only ONE of the four copies is pinned by a test (`plan_rate_card.rs:749` `a_cursor_from_another_search_is_refused`, which only drives `DescribeSavingsPlanRates`). Nothing exercises a foreign or malformed cursor against the other three operations, so dropping or rewording the refusal in any of them would pass `cargo test`.

The crate has already established the exact fix shape for this situation. The same refusal was triplicated across the three tagging handlers and was factored into `crate::state`, where one function hands back the MESSAGE and each handler only wraps it in its own error variant (`savings_plan_id_or_refusal`, `resource_not_found_message`). A `state::page_offset(kind, next_token) -> Result<usize, String>` would do the same here: the same offset, the same message, the same `ValidationException` code and 400 status out of every one of the four handlers, so no response and no existing test changes.

## Evidence

The one home of the token format, which the inline copies sit on top of — src/state.rs:404-418:

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
```

Copy 1 — src/ops/describe_savings_plan_rates.rs:157-164:

```
        const PAGE_KIND: &str = "planRates";

        let offset = match input.next_token.as_deref() {
            Some(token) => crate::state::decode_page_token(PAGE_KIND, token).ok_or_else(|| {
                validation("nextToken is not a token this operation issued".to_string())
            })?,
            None => 0,
        };
```

Copy 2 — src/ops/describe_savings_plans.rs:147-155:

```
        const PAGE_KIND: &str = "plans";

        let now = crate::state::now_millis();
        let offset = match input.next_token.as_deref() {
            Some(token) => crate::state::decode_page_token(PAGE_KIND, token).ok_or_else(|| {
                validation("nextToken is not a token this operation issued".to_string())
            })?,
            None => 0,
        };
```

Copy 3 — src/ops/describe_savings_plans_offerings.rs:162-169:

```
        const PAGE_KIND: &str = "offerings";

        let offset = match input.next_token.as_deref() {
            Some(token) => crate::state::decode_page_token(PAGE_KIND, token).ok_or_else(|| {
                validation("nextToken is not a token this operation issued".to_string())
            })?,
            None => 0,
        };
```

Copy 4 — src/ops/describe_savings_plans_offering_rates.rs:166-173:

```
        const PAGE_KIND: &str = "offeringRates";

        let offset = match input.next_token.as_deref() {
            Some(token) => crate::state::decode_page_token(PAGE_KIND, token).ok_or_else(|| {
                validation("nextToken is not a token this operation issued".to_string())
            })?,
            None => 0,
        };
```

The crate's own precedent for collapsing exactly this kind of per-handler repetition — src/state.rs:705-718 (the refusal that used to be written out in all three tagging handlers):

```
/// The Savings Plan id the `resourceArn` of a tagging operation names, or the
/// message with which this service refuses an ARN that is not one of ours.
///
/// ... That refusal is one rule with one message,
/// so — like [`resource_not_found_message`] beside it — it lives in ONE place,
/// shared by every operation that takes a `resourceArn`; each handler only
/// wraps the message in its own `ValidationException` variant.
pub fn savings_plan_id_or_refusal(resource_arn: &str) -> Result<&str, String> {
```

Test coverage — exactly one of the four copies is pinned, tests/all/plan_rate_card.rs:746-770:

```
/// A cursor this search did not issue is refused with a 400
/// `ValidationException`, so a token minted by another paginator cannot resume
/// one caller's walk at another's offset.
#[tokio::test]
async fn a_cursor_from_another_search_is_refused() {
...
        ("a token this service never mints", "ZZZZ".to_string()),
```

A repo-wide grep for that test's shape finds no sibling: `grep -n "ZZZZ|cursor_from_another" tests/` matches only plan_rate_card.rs:750 and :770, so `DescribeSavingsPlans`, `DescribeSavingsPlansOfferings` and `DescribeSavingsPlansOfferingRates` have no foreign-cursor row at all. The happy-path cursor round trip for the two catalog searches is covered by tests/all/shared_catalog_paging.rs:207 (`both_catalog_searches_page_by_the_same_cursor_rule`) and :310 (`the_plan_rate_card_pages_through_the_shared_window`), both of which keep passing under the refactor because the offset and the token spelling are unchanged.

## Steps to Reproduce

1. Run `grep -n "decode_page_token\|nextToken is not a token" services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/` and observe four handlers (describe_savings_plan_rates.rs:160-161, describe_savings_plans.rs:151-152, describe_savings_plans_offerings.rs:165-166, describe_savings_plans_offering_rates.rs:169-170) each carrying the same `match input.next_token.as_deref()` block with the same refusal sentence.
2. Note that `crate::state` already owns the token FORMAT (`encode_page_token`/`decode_page_token`, src/state.rs:404-418) but not the "absent means 0, foreign means ValidationException(<this sentence>)" rule the four handlers apply on top of it.
3. Run `grep -rn "ZZZZ\|cursor_from_another" services/service-com-amazonaws-savingsplans-awssavingsplan/tests/`: only plan_rate_card.rs matches, so three of the four copies are unpinned and can be changed or deleted without any test failing.
4. Compare with src/state.rs:705-718, where the crate already collapsed the triplicated `resourceArn` refusal into one helper that returns the message and lets each handler wrap it in its own `ValidationException` variant — the same move applies here, leaving the 400 status, the `ValidationException` code, the message and every existing test byte-identical.
