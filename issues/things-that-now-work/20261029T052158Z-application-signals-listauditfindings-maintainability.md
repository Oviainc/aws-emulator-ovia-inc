Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1920b48c1ae57650846bfce45e9fdc1dadf1068d

# The "an absent MaxResults takes this read's own default page" rule is written out three times, once per read whose member declares no @default

_Filed: 2026-10-29T05:21:58Z_
_Operation: ListAuditFindings | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1920b48c1ae57650846bfce45e9fdc1dadf1068d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three paged reads of this crate take a `MaxResults` whose own model declares the whole range but NO usable `@default` (`ListAuditFindings` `range 1..10`, `ListServiceLevelObjectiveExclusionWindows` `range 1..10, default null`, `ListServiceLevelObjectives` `range 1..50, default null`), so each receives an `Option<i32>` and each decides its page size with its OWN inline copy of one rule: "the centralized walk already refused anything outside the modelled range, so an absent member takes this operation's documented default page and a stated bound is read straight off the input". Two copies spell it `input.max_results.unwrap_or(<DEFAULT>).max(0) as usize`; the third spells the identical computation as a `match`. All three default constants already live in `state.rs` beside the two page-bound helpers the crate centralises there.

`crate::state` states that this is exactly what must not happen: `defaulted_page_bound`'s doc says of itself and `page_bound` that "Both live here so every read of this service that holds to one of them holds to ONE copy of it, and the page sizes they default to are the constants beside them." The crate already paid this debt once for the two instrumentation reads whose `MaxResults` carries a real `@default` — that duplication was filed and fixed by hoisting `defaulted_page_bound` into `state.rs` (issue `20261028T145307Z-application-signals-listinstrumentationconfigurations-maintainability`). The third variant of the rule, for the `Option<i32>` members, was left in the handlers in three copies, so a later change to the reasoning (refusing a non-positive bound instead of defaulting it, as `page_bound` does, or changing a default page) can be applied to one read and missed on the other two, leaving three lists disagreeing about one rule.

The fix is a single `crate::state` helper taking the member's documented default — e.g. `fn optional_page_bound(max_results: Option<i32>, default: i32) -> usize`, beside `page_bound` and `defaulted_page_bound` — called from all three handlers. Every response and every existing assertion stays byte-identical: for each of the three defaults (10, 10, 50) the helper computes exactly what the three copies compute today (`None` -> the default; `Some(n)` -> `n.max(0)`).

## Evidence

THE RULE THE CRATE STATES, beside the two helpers it already single-homes — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:1419-1439:
```
/// The page size a read serves for a `MaxResults` whose own model declares the WHOLE range it
/// admits together with the `@default` its documentation names, where `default` is the page
/// that operation documents for a caller who states no bound.
...
/// [`page_bound`] is the same rule for a member whose model states a maximum ONLY: there the
/// bound is refused rather than defaulted, because such a member documents no default to fall
/// back on. Both live here so every read of this service that holds to one of them holds to
/// ONE copy of it, and the page sizes they default to are the constants beside them.
pub fn defaulted_page_bound(max_results: i32, default: i32) -> usize {
    if max_results < MIN_PAGE_BOUND {
        default as usize
    } else {
        max_results as usize
    }
}
```

COPY 1 — src/ops/list_audit_findings.rs:211-217:
```
        // `MaxResults` declares no `@default`, so an absent one means the default the
        // member documents. A value outside the modelled `range 1..10` is already refused
        // before this handler runs.
        let page_size = input
            .max_results
            .unwrap_or(crate::state::DEFAULT_AUDIT_FINDING_PAGE)
            .max(0) as usize;
```

COPY 2 — src/ops/list_service_level_objective_exclusion_windows.rs:185-192, the same sentence and the same expression:
```
        // `MaxResults` declares no `@default`, so an omitted bound takes this service's own
        // default page (`crate::state::DEFAULT_EXCLUSION_WINDOW_PAGE`); its modelled
        // `range 1..10` is refused before this handler runs, so a stated bound is read
        // straight off the input.
        let page_size = input
            .max_results
            .unwrap_or(crate::state::DEFAULT_EXCLUSION_WINDOW_PAGE)
            .max(0) as usize;
```

COPY 3 — src/ops/list_service_level_objectives.rs:180-188, the same rule written as a `match`:
```
        // `MaxResults` declares no default, so an absent one means "as many as a page can
        // hold": the modelled maximum of `ListServiceLevelObjectivesMaxResults`. A supplied
        // value is read straight off the input, because its modelled `range 1..50` is
        // enforced and worded once for every operation of this service — by the centralized
        // walk, before any handler runs — exactly as every sibling list here relies on.
        let page_size = match input.max_results {
            None => crate::state::MAX_SLO_PAGE as usize,
            Some(max_results) => max_results.max(0) as usize,
        };
```

THE DEFAULTS ARE ALREADY IN THE SAME MODULE AS THE HELPERS — src/state.rs:78 `pub const MAX_SLO_PAGE: i32 = 50;`, src/state.rs:130 `pub const DEFAULT_AUDIT_FINDING_PAGE: i32 = 10;`, src/state.rs:1830 `pub const DEFAULT_EXCLUSION_WINDOW_PAGE: i32 = MAX_EXCLUSION_WINDOWS_PER_SLO as i32;`.

MODEL confirming the three members really are one case (whole range declared, no usable default, so the handler receives an `Option`) — `smithy_query(query="operation", operation="ListAuditFindings,ListServiceLevelObjectiveExclusionWindows,ListServiceLevelObjectives")`:
```
"MaxResults": {"target": "ListAuditFindingMaxResults", "range": {"min": 1, "max": 10}, "type": "integer"}
"MaxResults": {"target": "ListServiceLevelObjectiveExclusionWindowsMaxResults", "httpQuery": "MaxResults", "default": null, "range": {"min": 1, "max": 10}, "type": "integer"}
"MaxResults": {"target": "ListServiceLevelObjectivesMaxResults", "httpQuery": "MaxResults", "default": null, "range": {"min": 1, "max": 50}, "type": "integer"}
```

PRECEDENT IN THIS CRATE — the same duplication for the OTHER page-bound variant was filed and fixed: issue `20261028T145307Z-application-signals-listinstrumentationconfigurations-maintainability`, "The 'a non-positive MaxResults can only mean the member was absent' branch is written out twice, once per instrumentation read", whose fix created `defaulted_page_bound` in `state.rs`.

TEST COVERAGE THE FIX MUST LEAVE UNCHANGED (all three reads are exercised at their published bounds and with the member absent):
* tests/all/max_results_range.rs:3-15 — "One rule, one implementation: a page size outside the `@range` its own `MaxResults` member declares is refused before the operation's handler runs … the suite drives the SAME shared path through four of them", listing `ListServiceLevelObjectives,ListServices,ListServiceOperations,ListAuditFindings`;
* tests/all/page_bounds.rs:3-13 — "The page bound every paginated list of this service holds to … whether the member's own model states the minimum (`ListServices`, `ListServiceLevelObjectives`) or states a maximum only";
* tests/all/exclusion_window_reads.rs and tests/all/audit_findings.rs for the two other reads' own pages;
* tests/all/defaulted_page_bound.rs:12-14, which covers only the two `@default` reads (`ListInstrumentationConfigurations`, `GetInstrumentationConfigurationStatus`) and so pins nothing these three copies decide.

## Steps to Reproduce

1. Open services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/list_audit_findings.rs at lines 211-217 and src/ops/list_service_level_objective_exclusion_windows.rs at lines 185-192: the comment states the same rule and the expression `input.max_results.unwrap_or(<DEFAULT>).max(0) as usize` is identical apart from the constant it names.
2. Open src/ops/list_service_level_objectives.rs at lines 180-188: the same rule a third time, written as a `match` on the same `Option<i32>`, defaulting to `crate::state::MAX_SLO_PAGE`.
3. Read src/state.rs:1419-1439 (`defaulted_page_bound`), whose doc says of it and `page_bound`: "Both live here so every read of this service that holds to one of them holds to ONE copy of it, and the page sizes they default to are the constants beside them" — and note that all three constants the copies name are already in that module (state.rs:78, 130, 1830).
4. Add one `crate::state` helper beside them, e.g. `pub fn optional_page_bound(max_results: Option<i32>, default: i32) -> usize`, and call it from all three handlers.
5. Run the suites that cover the three reads — tests/all/max_results_range.rs, tests/all/page_bounds.rs, tests/all/audit_findings.rs, tests/all/exclusion_window_reads.rs, tests/all/slo_lifecycle.rs — and confirm every assertion passes unchanged: the helper computes exactly what the three copies compute for each default (10, 10, 50).
