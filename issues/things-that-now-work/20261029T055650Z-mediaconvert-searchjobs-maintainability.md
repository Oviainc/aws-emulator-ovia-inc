Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 12966d24554ed06eeaba7911ae25ff590cd06862

# The job-set queue/status narrowing rule is written out three times (SearchJobs, ListJobs and JobRecord::matches_filter) instead of living in one place

_Filed: 2026-10-29T05:56:50Z_
_Operation: SearchJobs | Class: maintainability_

> _Note: this issue was identified against the code base at commit `12966d24554ed06eeaba7911ae25ff590cd06862`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate deliberately keeps each shared rule in exactly one place and says so: the ordering, cursor and page size of every listing live once in `page_of_records` ("ONE place that owns the ordering, the cursor and the page size, whatever kind of record is being listed, so no paged reader can drift from another"), the set a `listBy` listing reports lives once in `records_listed_by` ("ONE place that owns the SET a `listBy` listing reports"), and the input-file containment test lives once in `JobRecord::matches_input_file` ("Both jobs readers that filter on an input file take this one test ... so ... the two cannot drift apart").

The two OTHER narrowing predicates over the job set — resolve a `queue` reference to a queue name and keep only jobs in that queue, and keep only jobs whose `status` equals the requested one — are not in one place. They are written out as inline closures in `SearchJobs`, written out identically in `ListJobs`, and written a third time inside `JobRecord::matches_filter` for the jobs-query `queue`/`status` filter keys. Three copies of the same two comparisons, over the same record type, reached from three operations.

The copies can drift: `matches_filter` already resolves the queue reference through `queue_name_from_reference` inside the record while the two handlers resolve it in the handler before the closure, so a change to how a queue reference is narrowed (the exact thing the crate centralised for the input-file test right beside it) has to be made in three files and nothing makes the third follow the first two. The fix is the shape `matches_input_file` already has: one predicate on `JobRecord` (or one `jobs_matching(queue, status, input_file)` iterator in `state`) that all three readers call. Every response and every existing assertion stays byte-identical — the predicate is the same conjunction either way.

## Evidence

Copy 1 — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/search_jobs.rs:206-239`:
```rust
        let queue = input
            .queue
            .as_deref()
            .map(crate::state::queue_name_from_reference)
            .map(str::to_string);
        let status = input.status.clone();
...
        let page = crate::state::page_of_jobs(
            state
                .jobs
                .values()
                .filter(|job| queue.as_deref().is_none_or(|name| job.queue == name))
                .filter(|job| status.as_deref().is_none_or(|want| job.status == want))
                .filter(|job| {
                    input_file
                        .as_deref()
                        .is_none_or(|file| job.matches_input_file(file))
                }),
```

Copy 2 — the same two closures and the same reference resolution in `src/ops/list_jobs.rs:191-220`:
```rust
        // `queue` accepts a queue name or a queue ARN; `status` is one `JobStatus`.
        let queue = input
            .queue
            .as_deref()
            .map(crate::state::queue_name_from_reference)
            .map(str::to_string);
        let status = input.status.clone();
...
                .filter(|job| queue.as_deref().is_none_or(|name| job.queue == name))
                .filter(|job| status.as_deref().is_none_or(|want| job.status == want)),
```

Copy 3 — the same two comparisons again, for the jobs-query filter keys, in `src/state.rs:3231-3239`:
```rust
    pub fn matches_filter(&self, key: &str, value: &str) -> bool {
        match key {
            // "Your Queue's name or ARN", so either form resolves to the name of the
            // queue this job was sent to.
            JOBS_QUERY_FILTER_KEY_QUEUE => self.queue == queue_name_from_reference(value),
            JOBS_QUERY_FILTER_KEY_STATUS => self.status == value,
            // "Your input file URL, or PARTIAL input file name" — so a containment test
            // over the inputs' `FileInput`, not an equality one.
            JOBS_QUERY_FILTER_KEY_FILE_INPUT => self.matches_input_file(value),
```

The rule this crate applies to the THIRD predicate of the very same filter set, one function away — `src/state.rs:3263-3275`:
```rust
    /// Whether any input of this job names a file whose `FileInput` CONTAINS `needle`.
    ///
    /// Both jobs readers that filter on an input file take this one test — the jobs
    /// query's `fileInput` filter key ... and `SearchJobs`'s `InputFile` ...
    /// — so a whole URL and any part of one both match, and the two cannot
    /// drift apart.
    pub fn matches_input_file(&self, needle: &str) -> bool {
        self.input_files().any(|file| file.contains(needle))
    }
```

And the crate's own statement of the one-place principle for the other shared listing rules — `src/state.rs:3499-3503` (`page_of_records`: "ONE place that owns the ordering, the cursor and the page size, whatever kind of record is being listed, so no paged reader can drift from another") and `src/state.rs:3656-3658` (`records_listed_by`: "ONE place that owns the SET a `listBy` listing reports").

Test coverage: `tests/all/job_search.rs` (the `SearchJobs` queue/status/inputFile filters), `tests/all/job_lifecycle.rs` and `tests/all/shared_paging.rs` (the `ListJobs` queue/status filters and paging) and `tests/all/jobs_query.rs` (the jobs-query `queue`/`status` filter keys) exercise all three copies end to end and assert only on the response bodies, so collapsing the three into one predicate leaves every existing assertion unchanged.

## Steps to Reproduce

1. Read `src/ops/search_jobs.rs:206-239` and note the `queue`-reference resolution plus the two inline narrowing closures over `state.jobs.values()`.
2. Read `src/ops/list_jobs.rs:191-220` and note the identical resolution and the identical two closures.
3. Read `src/state.rs:3231-3239` and note the same two comparisons a third time, as the `queue` and `status` arms of `JobRecord::matches_filter`.
4. Read `src/state.rs:3263-3275`: the third predicate of that same filter set (`fileInput`) was deliberately centralised on `JobRecord` so "the two cannot drift apart" — the two beside it were not.
5. Compare with the crate's stated one-place principle at `src/state.rs:3499-3503` and `src/state.rs:3656-3658`, and observe that a change to how a job's queue or status is narrowed now has to be made in three files.
