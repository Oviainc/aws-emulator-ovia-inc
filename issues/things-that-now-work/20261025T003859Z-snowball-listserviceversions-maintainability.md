Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# ListServiceVersions re-implements the shared MaxResults page-size rule instead of calling state::page_size

_Filed: 2026-10-25T00:38:59Z_
_Operation: ListServiceVersions | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has one helper for turning the modelled `MaxResults` into a page size — `crate::state::page_size` — and `ListJobs` and `ListClusterJobs` both call it, each with a comment saying the shared helper is what keeps the job listings from drifting apart. `ListServiceVersions` instead inlines the same three-step rule (try_from, discard 0, fall back to `DEFAULT_MAX_RESULTS`) with its own copy of the justification comment. Two copies of one rule can drift: a change to how 0 or an out-of-range value is read (or to the default) in the helper would silently leave `ListServiceVersions` on the old reading, and nothing in the crate links the two copies.

The fix is to call the existing helper from the handler. Every response and every existing test stays identical: the inlined expression is character-for-character the helper's body.

## Evidence

The shared helper — services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/state.rs:1165-1178:

    /// How many entries a list operation puts on one page, given the `MaxResults` the request
    /// carried.
    /// ... the only reading of 0 under which
    /// the paginated contract terminates is "unspecified" — the same as omitting the member,
    /// which gets [`DEFAULT_MAX_RESULTS`], the largest page the member admits.
    pub fn page_size(max_results: Option<i32>) -> usize {
        max_results
            .and_then(|max| usize::try_from(max).ok())
            .filter(|max| *max > 0)
            .unwrap_or(DEFAULT_MAX_RESULTS)
    }

The second copy — src/ops/list_service_versions.rs:360-369:

    // `MaxResults` is modelled `range 0..100` and is "the maximum number of
    // ListServiceVersions objects to return". A page of zero entries with a
    // continuation token would never advance the walk, so the only reading of 0 under
    // which the walk terminates is "unspecified" ...
    let page_size = input
        .max_results
        .and_then(|max| usize::try_from(max).ok())
        .filter(|max| *max > 0)
        .unwrap_or(crate::state::DEFAULT_MAX_RESULTS);

The sibling handlers call the helper and say why — src/ops/list_jobs.rs:153-156: "The page size, the cursor codec and the entry conversion are the ones every job-listing operation of this service shares (`state::page_size`, `state::page_of_jobs`), so `ListJobs` and `ListClusterJobs` cannot drift apart." followed by `let page_size = crate::state::page_size(input.max_results);` — and src/ops/list_cluster_jobs.rs:158-161 repeats it.

Tests covering both copies (so the change is observable if it alters anything): tests/all/service_versions.rs:311 `the_catalog_pages_at_every_page_size` (rows include "MaxResults at the published minimum", i.e. 0, and "no MaxResults"), tests/all/job_lifecycle.rs:956 `list_jobs_pages_over_every_stored_job_in_creation_order`, tests/all/cluster_job_listing.rs:259 `list_cluster_jobs_pages_over_exactly_that_cluster_s_jobs_in_creation_order`.

## Steps to Reproduce

1. Read src/state.rs:1173-1178 — `pub fn page_size(max_results: Option<i32>) -> usize` with the try_from / `> 0` / `DEFAULT_MAX_RESULTS` chain.
2. Read src/ops/list_jobs.rs:156 and src/ops/list_cluster_jobs.rs:161 — both call `crate::state::page_size(input.max_results)`.
3. Read src/ops/list_service_versions.rs:365-369 — the same chain written out again against `input.max_results`, with `crate::state::DEFAULT_MAX_RESULTS` as the fallback.
4. Observe that a change to the helper's reading of `MaxResults` (0, or the default page) would not reach ListServiceVersions, which is pinned by its own test `the_catalog_pages_at_every_page_size` at tests/all/service_versions.rs:311.
