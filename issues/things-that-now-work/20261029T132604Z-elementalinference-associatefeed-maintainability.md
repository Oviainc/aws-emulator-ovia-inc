Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ef111eb3ea31ef4faca303fd106b7202052fa3ed

# The append path's output-list refusals are hand-rolled in the handler, and state.rs twice documents an `appended_output_list_refusal` helper that does not exist

_Filed: 2026-10-29T13:26:04Z_
_Operation: AssociateFeed | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ef111eb3ea31ef4faca303fd106b7202052fa3ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` states that the rules every carrier of a feed's output list is held to live in ONE place per path: the operations that REPLACE the list go through `output_list_refusal`, and the one that APPENDS to it goes through `appended_output_list_refusal`. The second helper does not exist anywhere in the crate — grep finds the name only in those two doc comments — so the append's rules are instead written out inside the handler: `AssociateFeed` re-implements the output quota rule inline, with its own second sentence for a bound whose single statement (`too_many_outputs_message`) already lives in `crate::state` beside `MAX_OUTPUTS_PER_FEED`.

So the one rule "a feed holds at most MAX_OUTPUTS_PER_FEED outputs" is implemented twice — once in `state::output_list_refusal` (`names.count() > MAX_OUTPUTS_PER_FEED`, worded by `too_many_outputs_message()`) and once in `associate_feed.rs` (`feed.outputs.len() + to_append.len() > MAX_OUTPUTS_PER_FEED`, worded by a format! string in the handler) — and the two copies can drift: they already word the same bound differently, and a change to the bound's wording or to its reporting rule has to be made in two files. The doc links also mean rustdoc resolves `[`appended_output_list_refusal`]` to nothing.

The fix is the one the module already documents: add `appended_output_list_refusal` to `crate::state` as the single statement of the rules an APPENDED list is held to (the duplicate-name scan it already shares, plus the bound measured against the stored list), keep the handler's current sentence verbatim as that helper's message, and have `AssociateFeed` lift only the modelled error class — leaving every response byte and every existing test unchanged.

## Evidence

The documented single statement that is missing — `src/state.rs:397`: "An APPEND is not one of them: it leaves the feed holding what it already holds, so a list that adds nothing takes nothing away either (see [`appended_output_list_refusal`])." and `src/state.rs:410-411`: "so no two operations that carry such a list can come to disagree about what a feed's outputs may look like — the operations that REPLACE the list through [`output_list_refusal`] and the one that APPENDS to it through [`appended_output_list_refusal`]. A handler only lifts the outcome into its own modelled error enum, which is per-operation." `grep -n appended_output_list_refusal` over the crate returns exactly those two lines — no definition.

Copy 1 of the bound, in state — `src/state.rs:466-468`: `if names.count() > MAX_OUTPUTS_PER_FEED { return Some(OutputListRefusal::TooManyOutputs); }`, worded by `src/state.rs:382-387`: `format!("A feed holds at most {MAX_OUTPUTS_PER_FEED} outputs. Specify fewer outputs or request a quota increase.")`.

Copy 2 of the bound, in the handler — `src/ops/associate_feed.rs:424-441`: `if !to_append.is_empty() && feed.outputs.len() + to_append.len() > crate::state::MAX_OUTPUTS_PER_FEED { ... message: format!("You can't add these outputs because a feed holds at most {} outputs. Remove outputs with UpdateFeed or request a quota increase.", crate::state::MAX_OUTPUTS_PER_FEED) ... }`.

Test coverage that must stay green and does: `tests/all/error_message_rule.rs:424-459` (an append past the quota is 402 ServiceQuotaExceededException, the message `contains(&MAX_OUTPUTS_PER_FEED.to_string())` and is ONE sentence for all four (held, added) rows), `tests/all/feed_association.rs:684-747` (the bound itself accepted, one past it refused, a repeat at the bound served), `tests/all/supplied_output_cardinality.rs:435-438` (append rows bounded by `MAX_OUTPUTS_PER_FEED - HELD`), and `tests/all/output_list_rule.rs:155,205-221` for the replacement path through `too_many_outputs_message()`.

## Steps to Reproduce

1. grep -n 'appended_output_list_refusal' services/service-com-amazonaws-elementalinference-elementalinference — only src/state.rs:397 and :411, both doc comments; no definition exists
2. Read src/state.rs:457-470 (output_list_refusal) and src/ops/associate_feed.rs:417-441: the same MAX_OUTPUTS_PER_FEED rule is computed and worded in both places
3. Change the quota sentence in src/state.rs:382-387 and observe that the append path keeps answering its own, unrelated sentence — the drift the module doc says cannot happen
4. Fix: define appended_output_list_refusal in crate::state (duplicate name via first_duplicate_output_name, plus stored-length + added-length against MAX_OUTPUTS_PER_FEED) carrying the handler's current messages verbatim, call it from src/ops/associate_feed.rs, and re-run the tests named above: all responses and assertions unchanged
