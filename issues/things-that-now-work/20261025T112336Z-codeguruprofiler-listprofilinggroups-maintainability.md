Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 949cf2f2a2f42313b725a3f96914da0fa4cf14da

# Three list handlers carry byte-identical copies of the cursor/page/next-token computation (and a fourth copy has already drifted)

_Filed: 2026-10-25T11:23:36Z_
_Operation: ListProfilingGroups | Class: maintainability_

> _Note: this issue was identified against the code base at commit `949cf2f2a2f42313b725a3f96914da0fa4cf14da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListProfilingGroups`, `ListFindingsReports` and `GetFindingsReportAccountSummary` each spell out the SAME pagination computation over an already-ordered `Vec`: refuse a cursor past the end of the list, default the page size to `crate::state::DEFAULT_LIST_PAGE_SIZE`, convert `maxResults` with `usize::try_from(max).unwrap_or(0)`, take `start.saturating_add(limit).min(len)`, slice, and emit `(end < len).then(|| end.to_string())`. The three copies are identical line for line; only the error constructor they lift the refusal into differs.

That the copies drift is already visible: the fourth list handler, `ListProfileTimes`, writes the same computation with two different decisions — it clamps the page size with `.unwrap_or(1).max(1)` instead of `unwrap_or(0)`, and refuses a cursor with `input.next_token.is_some() && first >= starts.len()` instead of `start > len`, so a hand-built token pointing exactly at the end of the list is a 400 there and a 200 empty page in the other three.

One helper in `crate::state` (taking the ordered slice, the `nextToken`, the `maxResults` and returning the page plus the next token, or the 'token this service never issued' signal) would hold this rule once, the way the crate already holds its interval rule in `validate_interval` and its tag rules in `validate_tags`. Scope the fix to the three IDENTICAL copies so every response and every existing test stays exactly as it is; `ListProfileTimes`' divergent copy is pinned by its own test and is not part of this change.

## Evidence

Copy 1 — src/ops/list_profiling_groups.rs:181-219:
```
        if start > names.len() {
            return Err(crate::scaffolded::Scaffolded::validation(
                "nextToken is not a token this service issued",
            ));
        }
        let limit = input.max_results.map_or(
            crate::state::DEFAULT_LIST_PAGE_SIZE,
            |max| usize::try_from(max).unwrap_or(0),
        );
        let end = start.saturating_add(limit).min(names.len());
        ...
            next_token: (end < names.len()).then(|| end.to_string()),
```

Copy 2 — src/ops/list_findings_reports.rs:228-253:
```
        if start > reports.len() { ... "nextToken is not a token this service issued" ... }
        let limit = input
            .max_results
            .map_or(crate::state::DEFAULT_LIST_PAGE_SIZE, |max| {
                usize::try_from(max).unwrap_or(0)
            });
        let end = start.saturating_add(limit).min(reports.len());
        ...
            next_token: (end < reports.len()).then(|| end.to_string()),
```

Copy 3 — src/ops/get_findings_report_account_summary.rs:203-225:
```
        if start > summaries.len() { ... "nextToken is not a token this service issued" ... }
        let limit = input
            .max_results
            .map_or(crate::state::DEFAULT_LIST_PAGE_SIZE, |max| {
                usize::try_from(max).unwrap_or(0)
            });
        let end = start.saturating_add(limit).min(summaries.len());
        ...
            next_token: (end < summaries.len()).then(|| end.to_string()),
```

The drifted fourth copy — src/ops/list_profile_times.rs:271-290:
```
        if input.next_token.is_some() && first >= starts.len() { ... }
        let limit = input.max_results.map_or(
            crate::state::DEFAULT_LIST_PAGE_SIZE,
            |max| usize::try_from(max).unwrap_or(1).max(1),
        );
```
and the test that pins its stricter cursor rule — tests/all/profile_times.rs:685-690:
```
            // The group holds two five-minute windows, so a cursor of 2 points exactly
            // at the end — a position this service never issues a token for.
            "a nextToken exactly at the end of the list",
```

Tests that cover the three copies and must stay green unchanged: tests/all/profiling_group_lifecycle.rs::listing_pages_over_every_stored_group_once, tests/all/findings_reports_listing.rs::paging_visits_every_report_exactly_once and ::a_listing_refuses_what_it_cannot_answer, tests/all/findings_report_account_summary.rs::paging_visits_every_report_exactly_once and ::the_summary_refuses_what_it_cannot_answer.

## Steps to Reproduce

1. Open src/ops/list_profiling_groups.rs:181-219, src/ops/list_findings_reports.rs:228-253 and src/ops/get_findings_report_account_summary.rs:203-225 side by side — the cursor refusal, the page-size default, the `unwrap_or(0)` conversion, the `saturating_add(..).min(len)` slice and the `(end < len).then(|| end.to_string())` token are identical in all three.
2. Open src/ops/list_profile_times.rs:271-290 — the same computation written with `.unwrap_or(1).max(1)` and with `next_token.is_some() && first >= len`, i.e. the copies have already diverged.
3. Confirm the divergence is observable: with a group holding N reports, GET /internal/profilingGroups/N/findingsReports?...&nextToken=<N> answers 200 with an empty page, while ListProfileTimes refuses the equivalent cursor with 400 (tests/all/profile_times.rs:685-690).
4. Consolidate the three identical copies into one helper in src/state.rs beside `validate_interval`, leaving ListProfileTimes' copy untouched, and re-run the five tests named in the evidence.
