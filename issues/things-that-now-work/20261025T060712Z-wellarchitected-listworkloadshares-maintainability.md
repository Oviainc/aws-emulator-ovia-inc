Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# Two paginated handlers hand-roll the MaxResults page-size clamp that crate::state::page_size already owns, each with its own copy of the 50 ceiling

_Filed: 2026-10-25T06:07:12Z_
_Operation: ListWorkloadShares | Class: maintainability_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service has one place that turns the modelled `MaxResults` into a page size — `crate::state::page_size` over `crate::state::MAX_RESULTS_CEILING`, documented as the single restatement of that bound — and seven paginated handlers call it. ListWorkloadShares and ListShareInvitations instead repeat the clamp inline, each declaring a private `const MAX_RESULTS_CEILING: i32 = 50;` of its own, so the same rule and the same number exist in three places and the copies can drift (the `ListLensReviewImprovements` ceiling already had to differ, which is exactly why the shared helper takes it as a parameter via `page_size_up_to`). Replacing both inline copies with the shared helper leaves every response and every existing test unchanged — the computation is identical (`max_results.unwrap_or(ceiling).clamp(1, ceiling)`).

## Evidence

Shared helper, documented as the one restatement: `services/.../src/state.rs:1149-1157` — "The largest page most of this service's paginated operations serve, and the page size a request that omits `MaxResults` gets. … all but one of them bound it to `@range(min: 1, max: 50)`, so one ceiling covers them" / `pub const MAX_RESULTS_CEILING: i32 = 50;` and `state.rs:1165-1180` `pub fn page_size(max_results: Option<i32>) -> usize { page_size_up_to(max_results, MAX_RESULTS_CEILING) }` with `let clamped = max_results.unwrap_or(ceiling).clamp(1, ceiling);`.

Copy 1: `src/ops/list_workload_shares.rs:183-188` — `let clamped = input.max_results.unwrap_or(MAX_RESULTS_CEILING).clamp(1, MAX_RESULTS_CEILING); let page_size = usize::try_from(clamped).expect("MaxResults is clamped into 1..=50, which always fits in a usize");` plus its own `const MAX_RESULTS_CEILING: i32 = 50;` at `list_workload_shares.rs:269`.

Copy 2: `src/ops/list_share_invitations.rs:228-233` — the same four lines verbatim, plus its own `const MAX_RESULTS_CEILING: i32 = 50;` at `list_share_invitations.rs:321`.

Callers of the shared helper, for contrast: `list_workloads.rs:154`, `list_milestones.rs:159`, `list_answers.rs:177`, `list_lenses.rs:191`, `list_lens_reviews.rs:163`, `list_review_templates.rs:153`, `list_review_template_answers.rs:175`.

Test coverage that must stay green: `tests/all/workload_shares.rs:468-511` (pages enumerate every share exactly once at any MaxResults) and `:699-760` (MaxResults at 1 and 50 served, 0 and 51 refused); `tests/all/share_invitations.rs:513-595` (same two rules for invitations); `tests/all/validation_envelopes.rs:69-127` pins the boundary refusal for values outside @range(1, 50) across the paginated operations.

## Steps to Reproduce

1. Open services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/state.rs:1149-1180 and read the shared page_size / MAX_RESULTS_CEILING definition
2. Open src/ops/list_workload_shares.rs:183-188 and :269 — the clamp and the ceiling are re-declared locally
3. Open src/ops/list_share_invitations.rs:228-233 and :321 — the same clamp and ceiling are re-declared a second time
4. Compare with src/ops/list_workloads.rs:154, which calls crate::state::page_size for the identical rule
