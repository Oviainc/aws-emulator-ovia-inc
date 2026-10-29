Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 355761efac79959eac560a8efce6608069de81f3

# SearchJobs resolves its `queue` filter without the crate's one blank-reference test, so `?queue=` silently reports an empty page

_Filed: 2026-10-28T21:20:07Z_
_Operation: SearchJobs | Class: validation_

> _Note: this issue was identified against the code base at commit `355761efac79959eac560a8efce6608069de81f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SearchJobs` reduces its `queue` query member with `crate::state::queue_name_from_reference` and uses the result as an equality key against each job's stored queue name, with no blank test anywhere in the handler.

A request carrying `?queue=` (or `?queue=%20`) deserializes to `Some("")`/`Some(" ")`. `queue_name_from_reference` hands that value back unchanged (it only strips an `arn:...:queues/` prefix), so the filter becomes `job.queue == ""` — a key no stored job can hold, because `CreateQueue` refuses a `Name` that carries no characters. The caller is answered HTTP 200 with an empty `jobs` list, as if the account simply had no job on that queue, for a value that could never have named one.

That skips the crate's ONE rule for a caller-sent member that references a resource (`crate::state::carries_no_characters`), whose own doc names `Queue` among the references it covers and says such a value must be answered with the bad-request wording "rather than with a lookup of a value nothing could hold" — which is exactly what this path does. `CreateJob` applies that rule to the very same kind of member ten lines into its own handler, and the crate's blank-name test module states the rule as holding for "every door that mints or resolves a named resource".

Secondary symptom of the same skip, in the same handler: `?inputFile=` (a blank value) reaches `JobRecord::matches_input_file`, which is `self.input_files().any(|file| file.contains(needle))` — and `contains("")` is true for every input — so a blank `inputFile` WIDENS the search to every job carrying a `FileInput`, contradicting this handler's own stated rule that "each filter the request carries NARROWS the set".

Note this is a different operation from the already-filed ListJobs/validation issue: there the extra defect is that the ARN form is accepted although `ListJobsRequest$Queue` is name-only, whereas `SearchJobsRequest$Queue` does document both forms, so the only defect here is the missing blank test.

## Evidence

The modelled member, which documents both reference forms but no blank form — `smithy_query(query="doc",shape="SearchJobsRequest",member="Queue")`:
```
"Optional. Provide a queue name, or a queue ARN, to return only jobs from that queue."
```

The handler — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/search_jobs.rs:182-192`:
```rust
        // `queue` is "a queue name, or a queue ARN"; `status` is one `JobStatus`.
        let queue = input
            .queue
            .as_deref()
            .map(crate::state::queue_name_from_reference)
            .map(str::to_string);
        let status = input.status.clone();
        // `InputFile`: "Provide your input file URL or your partial input file name" — so
        // a containment test over this job's inputs, ...
        let input_file = input.input_file.clone();
```
and `src/ops/search_jobs.rs:206-216`:
```rust
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
No `carries_no_characters` call appears anywhere in `search_jobs.rs` (grep for it over `src/ops/` lists 14 call sites; `search_jobs.rs` is not among them).

The crate's ONE rule it skips — `src/state.rs:588-612`:
```rust
/// Whether a string member the caller SENT carries no characters at all.
///
/// Every member in this service that names a resource — the `Name` of a queue, a preset or
/// a job template, and the references (`Queue`, `JobTemplate`, `Role`, the `JobId` a share
/// names, the `Arn` of a certificate or of a tagged resource) that point at one — targets
/// the model's bare `__string`, ... ONE test for all
/// of them, so no door that mints or resolves such a member can disagree about which
/// values are identities, answered with the bad request [`blank_member_message`] words
/// rather than with a lookup of a value nothing could hold.
pub fn carries_no_characters(value: &str) -> bool {
    value.trim().is_empty()
}
```

The sibling door that applies it to the SAME kind of member — `src/ops/create_job.rs:240-254`:
```rust
        // "Optional. When you create a job, you can specify a queue to send it to. If you
        // don't specify, the job will go to the default queue." A `Queue` carrying no
        // characters is neither a queue this account holds nor the member left out, so it
        // is the same problem in the request `CreateJobTemplate` answers for its own
        // `Queue` rather than a lookup of a name nothing could hold.
        if input
            .queue
            .as_deref()
            .is_some_and(crate::state::carries_no_characters)
        {
            return Err(CreateJobErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::blank_member_message("Queue")),
            })
            .into());
        }
```

The crate's own statement of the rule's scope — `tests/all/blank_resource_name.rs:10-13`:
```
//! carries a resource identity — a key with no characters addresses nothing a later read
//! can reach, and a reference with none resolves to nothing — so every door that mints or
//! resolves a named resource answers such a value with `BadRequestException` (400) naming
//! the member, and leaves the store exactly as it was.
```

The widening half — `src/state.rs:3169-3171`:
```rust
    pub fn matches_input_file(&self, needle: &str) -> bool {
        self.input_files().any(|file| file.contains(needle))
    }
```
against this handler's own stated rule, `src/ops/search_jobs.rs:178-180`:
```rust
        // "Optionally filter results further according to input file, queue, or status":
        // each filter the request carries NARROWS the set, so a job is reported only when
        // it satisfies every one of them.
```

Test coverage: `tests/all/job_search.rs:158-224` drives every `inputFile` and `queue` case (whole URL, partial name, name form, ARN form, a queue no job was sent to) — it sends no blank value to either member, so nothing pins the current empty/widened 200 and no existing assertion changes. `tests/all/blank_resource_name.rs` exercises the minting and reference-resolving doors only; it sends nothing to `GET /2017-08-29/search`.

## Steps to Reproduce

1. POST /2017-08-29/jobs to create one job (it lands on the Default queue).
2. GET /2017-08-29/search — the job is reported.
3. GET /2017-08-29/search?queue= (empty value) — observe HTTP 200 with an empty `jobs` array, reported as if the account had no job on that queue.
4. GET /2017-08-29/search?queue=%20 — same empty 200.
5. For comparison, POST /2017-08-29/jobs with "queue": "" — HTTP 400 BadRequestException "Queue must not be empty."
6. GET /2017-08-29/search?inputFile= (empty value) — observe HTTP 200 listing EVERY job, i.e. a filter the caller sent narrowed nothing.
