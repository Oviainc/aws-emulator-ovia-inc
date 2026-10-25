Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# ListHarvestJobs re-implements the shared MaxResults page-size resolution with its own copy of the three bounds constants

_Filed: 2026-10-25T04:07:54Z_
_Operation: ListHarvestJobs | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::page_size` is documented as the one place every paged list resolves its page size, and `ListChannelGroups`, `ListChannels` and `ListOriginEndpoints` all call it. `ListHarvestJobs` instead declares its own `MAX_RESULTS_MIN` / `MAX_RESULTS_MAX` / `MAX_RESULTS_DEFAULT` constants and repeats the `usize::try_from(...) + range check + default` computation verbatim in the handler, so the same rule (and the same three model-derived numbers) exist twice in this crate and can drift: changing the shared resolver would silently leave the harvest-job list on the old rule, which is exactly the outcome the shared helper's doc says it prevents.

The fix is a pure substitution — replace lines 233-236 with `crate::state::page_size(input.max_results)` and delete the three local constants. The resolved value is identical for every input, so every response and every existing test stays unchanged.

## Evidence

The shared resolver and its stated scope — src/state.rs:3099-3118:
```
/// The smallest `MaxResults` the model's `ListResourceMaxResults` range allows.
const MAX_RESULTS_MIN: usize = 1;
/// The largest `MaxResults` the model's `ListResourceMaxResults` range allows.
const MAX_RESULTS_MAX: usize = 100;
/// The page size the model declares as `MaxResults`'s default.
const MAX_RESULTS_DEFAULT: usize = 10;
...
/// means the member was absent: fall back to the modelled default. Every paged list in
/// this service resolves its page size here, so a caller cannot force a larger response
/// from one list than from another.
pub fn page_size(max_results: i32) -> usize {
    match usize::try_from(max_results) {
        Ok(size) if (MAX_RESULTS_MIN..=MAX_RESULTS_MAX).contains(&size) => size,
        _ => MAX_RESULTS_DEFAULT,
    }
}
```

The second copy — src/ops/list_harvest_jobs.rs:155-160:
```
/// The smallest `MaxResults` the model's `ListResourceMaxResults` range admits.
const MAX_RESULTS_MIN: usize = 1;
/// The largest `MaxResults` that range admits.
const MAX_RESULTS_MAX: usize = 100;
/// The `MaxResults` the model defaults the member to.
const MAX_RESULTS_DEFAULT: usize = 10;
```
and src/ops/list_harvest_jobs.rs:233-236:
```
        let page_size = match usize::try_from(input.max_results) {
            Ok(size) if (MAX_RESULTS_MIN..=MAX_RESULTS_MAX).contains(&size) => size,
            _ => MAX_RESULTS_DEFAULT,
        };
```

The siblings that call the shared one: src/ops/list_channel_groups.rs:182 `let page_size = crate::state::page_size(input.max_results);`, src/ops/list_channels.rs:205, src/ops/list_origin_endpoints.rs:253.

Existing tests that cover the behaviour and must stay green unchanged: tests/all/control_plane_validation.rs:683-718 (paging `ListChannelGroups` at `maxResults` 1/5/100 and asserting no page exceeds the requested size) and tests/all/rejection_rule_and_negative_paths.rs:1354-1359 onwards (the ListHarvestJobs status-filter walk).

## Steps to Reproduce

1. Open src/ops/list_harvest_jobs.rs and read lines 155-160 and 233-236.
2. Open src/state.rs and read lines 3099-3118 — the same three constants and the same resolution expression, documented as the single place every paged list resolves its page size.
3. Note that the other three list handlers (list_channel_groups.rs:182, list_channels.rs:205, list_origin_endpoints.rs:253) call `crate::state::page_size`, so only this handler holds a second copy of the rule.
