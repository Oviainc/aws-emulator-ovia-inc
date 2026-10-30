Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# The maxResults page-size rule is spelled out four times, once in each paginated handler, instead of living in the state module that owns this service's paging rules

_Filed: 2026-10-28T05:27:40Z_
_Operation: ListDatastores | Class: maintainability_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four paginated handlers of this crate carry a byte-identical copy of the page-size rule (take `maxResults`, convert to `usize`, clamp into `MIN_LIST_PAGE_SIZE..=MAX_LIST_PAGE_SIZE`, default to `MAX_LIST_PAGE_SIZE` when absent), including the same comment. The state module is where this crate deliberately keeps the paging rules shared by more than one listing — it says so about the token codec, "a token format spelled twice drifts" — and it already owns `page_sort_key`, `encode_page_token`, `MIN_LIST_PAGE_SIZE` and `MAX_LIST_PAGE_SIZE`. Four copies of the computation can drift from each other (one clamp updated, three left behind). Moving the computation into one `state` helper that the four handlers call changes no response: `maxResults` carries `@range(min:1, max:50)` in the model and the two constants are exactly 1 and 50, so the clamp is a pass-through on every value that can reach a handler.

## Evidence

The same computation, four times:
`src/ops/list_datastores.rs:188-199`
```
        let page_size = input
            .max_results
            .and_then(|requested| usize::try_from(requested).ok())
            // Clamped here, not merely at the boundary: one request must never be able
            // to materialise an unbounded page.
            .map(|requested| {
                requested.clamp(
                    crate::state::MIN_LIST_PAGE_SIZE,
                    crate::state::MAX_LIST_PAGE_SIZE,
                )
            })
            .unwrap_or(crate::state::MAX_LIST_PAGE_SIZE);
```
`src/ops/list_dicom_import_jobs.rs:197-208`, `src/ops/list_image_set_versions.rs:163-174` and `src/ops/search_image_sets.rs:216-227` repeat the identical twelve lines (same comment, same constants).
The crate's own stated home for a paging rule used by more than one listing — `src/state.rs:1841-1843`:
```
// The codec lives here because more than one listing uses it, and a token format
// spelled twice drifts. Each collection passes its OWN marker, so a token minted for
// one collection is not a position in another and is refused rather than honoured.
```
The clamp cannot change a response: the model gives `maxResults` `"range": {"min": 1, "max": 50}` on ListDatastores/ListDICOMImportJobs/ListImageSetVersions/SearchImageSets, and `src/state.rs:124-131` sets `MIN_LIST_PAGE_SIZE = 1` / `MAX_LIST_PAGE_SIZE = 50`.
Existing coverage that must stay green unchanged: `tests/all/datastore_lifecycle.rs:600-690` and `:1295-1333` (published bounds accepted, 0 and 51 refused), `tests/all/import_job_listing.rs:186-240` and `:661-735`, `tests/all/image_set_version_listing.rs:185-280` (which imports `MAX_LIST_PAGE_SIZE`/`MIN_LIST_PAGE_SIZE` directly), `tests/all/image_set_search.rs:736-793` and `:991-1002`.

## Steps to Reproduce

1. Open src/ops/list_datastores.rs:188, src/ops/list_dicom_import_jobs.rs:197, src/ops/list_image_set_versions.rs:163 and src/ops/search_image_sets.rs:216 side by side — the same twelve lines, including the comment, four times.
2. Compare with src/state.rs:1831-1844, where this crate says a paging rule used by more than one listing lives in the state module because a rule "spelled twice drifts".
3. Replace the four copies with one call to a single `crate::state` helper and run the crate's tests: every pagination assertion in tests/all/datastore_lifecycle.rs, tests/all/import_job_listing.rs, tests/all/image_set_version_listing.rs and tests/all/image_set_search.rs still passes, and every response byte is unchanged.
