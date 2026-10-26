Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# The "first answer always carries a cursor, a resumed one only while results remain" rule is implemented twice in this crate's two catalog searches

_Filed: 2026-10-26T08:22:06Z_
_Operation: DescribeSavingsPlansOfferingRates | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two catalog searches each spell out the same cursor rule in their own handler, in two different shapes, so the copies can drift: `DescribeSavingsPlansOfferings` derives it from `paginate`'s `next` plus `matched.len()`, while `DescribeSavingsPlansOfferingRates` derives it from `more_remain` plus `offset + window.len()`. The accompanying comment block stating the rule is itself duplicated almost verbatim in both files, and the surrounding lazy page-window idiom ("take one item past the page, note whether more remain, truncate") is duplicated a third time in `DescribeSavingsPlanRates`.

Nothing about the responses needs to change: the rule could live once beside `crate::state::paginate` in state.rs (which already owns `encode_page_token`/`decode_page_token`/`paginate`) and be called from all three handlers, leaving every current answer and every existing test untouched.

## Evidence

Copy 1 — services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/describe_savings_plans_offerings.rs:206-207:

            let next = next
                .or_else(|| (input.next_token.is_none()).then_some(matched.len()));

under the comment at describe_savings_plans_offerings.rs:195-205 ("The FIRST answer of a search always carries the cursor to resume at … a RESUMED answer carries a cursor only while results remain past its page").

Copy 2 — services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/describe_savings_plans_offering_rates.rs:226-231:

            let reached = offset.saturating_add(window.len());
            let next = if more_remain {
                Some(reached)
            } else {
                input.next_token.is_none().then_some(reached)
            };

under the near-identical comment at describe_savings_plans_offering_rates.rs:217-225 ("The FIRST answer of a search always carries the cursor to resume at … a RESUMED answer carries a cursor only while results remain past its page").

The page-window idiom duplicated a third time — describe_savings_plan_rates.rs:203-210:

                .take(page_size.saturating_add(1))
                …
            let more_remain = window.len() > page_size;
            window.truncate(page_size);

versus describe_savings_plans_offering_rates.rs:213-216:

                    .take(page_size.saturating_add(1))
                    .collect();
            let more_remain = window.len() > page_size;
            window.truncate(page_size);

The single-home that already exists for the rest of paging — services/service-com-amazonaws-savingsplans-awssavingsplan/src/state.rs:393-406 (`pub fn paginate<T: Clone>(items, offset, limit) -> (Vec<T>, Option<usize>)`).

Existing tests that pin the behaviour and must stay green unchanged: tests/all/offering_catalog.rs:337 `the_offerings_search_answers_with_a_resumable_cursor` (and :430-439, "the first answer of a search must carry the cursor to resume at"), tests/all/offering_catalog.rs:448 `a_search_whose_filters_admit_nothing_still_answers_with_a_cursor`, tests/all/offering_rate_catalog.rs:697 `the_rate_search_answers_with_a_resumable_cursor`, and tests/all/plan_rate_card.rs:571 `a_rate_card_is_paged_once_in_a_stable_order`.

## Steps to Reproduce

1. Open src/ops/describe_savings_plans_offerings.rs:195-207 and src/ops/describe_savings_plans_offering_rates.rs:217-231 side by side: the same cursor rule and nearly the same comment appear twice, computed two different ways.
2. Open src/ops/describe_savings_plan_rates.rs:199-210 and src/ops/describe_savings_plans_offering_rates.rs:202-216: the lazy "take page_size + 1, note more_remain, truncate" window is a third copy.
3. Note that src/state.rs:393-406 already holds the shared `paginate` helper beside `encode_page_token`/`decode_page_token`, so the rule has an existing home; changing only one of the copies would silently change one search's cursor behaviour relative to the other.
