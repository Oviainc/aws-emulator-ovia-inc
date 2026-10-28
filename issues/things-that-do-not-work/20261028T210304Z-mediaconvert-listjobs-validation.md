Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f6d3b7b16cb7e1ce86179d792dd5264644759e0e

# The ListJobs `queue` filter is resolved as an ARN reference and never takes the crate's one blank-reference test, so `?queue=` silently reports an empty page

_Filed: 2026-10-28T21:03:04Z_
_Operation: ListJobs | Class: validation_

> _Note: this issue was identified against the code base at commit `f6d3b7b16cb7e1ce86179d792dd5264644759e0e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListJobs` reduces its `queue` query member with `crate::state::queue_name_from_reference` and then uses the result as an equality key against each job's stored queue name. Two problems follow from that one line:

1. The member takes NO blank test. A request carrying `?queue=` (or `?queue=%20`) deserializes to `Some("")`/`Some(" ")`, which `queue_name_from_reference` hands back unchanged, and the filter becomes `job.queue == ""` — a key no stored job can hold, because `CreateQueue` refuses a `Name` that carries no characters. The caller gets HTTP 200 with an empty `jobs` list for a value that could never have named a queue, instead of the refusal the crate states ONE rule for. `CreateJob` applies that rule to the very same kind of member (`Queue`) ten lines of its own handler, and the crate's rule says explicitly that such a value must be "answered with the bad request [`blank_member_message`] words rather than with a lookup of a value nothing could hold" — this path is exactly that lookup.

2. The ARN form is accepted although this operation's modelled member does not offer it. The model distinguishes the two jobs readers: `SearchJobsRequest$Queue` is "Optional. Provide a queue name, or a queue ARN, to return only jobs from that queue", while `ListJobsRequest$Queue` is "Optional. Provide a queue name to get back only jobs from that queue" — name only. The crate asserts the opposite in two places (`queue_name_from_reference`'s doc and `GetQueue`'s comment), and `GetQueue` cites "it is not documented to accept an ARN" as its own reason for NOT resolving a reference — the test this handler fails. The consequence is not cosmetic: a queue whose `Name` is itself an ARN-shaped string (names are stored byte for byte) can never be selected by this filter, because its own name is reduced to the suffix after `:queues/`.

## Evidence

The modelled member, name only — `smithy_query(query="doc",shape="ListJobsRequest",member="Queue")`:
```
"Optional. Provide a queue name to get back only jobs from that queue."
```
and the sibling reader that DOES offer the ARN form — `smithy_query(query="doc",shape="SearchJobsRequest",member="Queue")`:
```
"Optional. Provide a queue name, or a queue ARN, to return only jobs from that queue."
```

The handler — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/list_jobs.rs:176-181` and `:196-205`:
```rust
        // `queue` accepts a queue name or a queue ARN; `status` is one `JobStatus`.
        let queue = input
            .queue
            .as_deref()
            .map(crate::state::queue_name_from_reference)
            .map(str::to_string);
...
        let page = crate::state::page_of_jobs(
            state
                .jobs
                .values()
                .filter(|job| queue.as_deref().is_none_or(|name| job.queue == name))
```
No `carries_no_characters` call appears anywhere in the file (grep for it in `src/ops/` lists every caller; `list_jobs.rs` is not among them).

The crate's ONE rule it skips — `src/state.rs:588-599` and `:610-612`:
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

The sibling door that applies it to the SAME member — `src/ops/create_job.rs:240-254`:
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

The crate's own (incorrect) claim about this member — `src/state.rs:934-940`:
```rust
/// The queue name a caller named, whether they gave the name or the queue's ARN.
///
/// `CreateJob`'s `Queue` and `ListJobs`' `queue` both accept either form; the resource
/// id in a MediaConvert queue ARN is `queues/<name>`.
pub fn queue_name_from_reference(reference: &str) -> &str {
```
and the handler that reasons the other way from the same distinction — `src/ops/get_queue.rs:165-167`:
```rust
        // `GetQueue`'s `{Name}` is a queue name; unlike `CreateJob`'s `Queue` and
        // `ListJobs`' `queue`, it is not documented to accept an ARN.
        let name = input.name;
```

Test coverage: `tests/all/blank_resource_name.rs` (whole file) exercises the named creates, `HopDestination$Queue`, `CreateResourceShare$JobId` and the three tagging `Arn`s — it sends no `?queue=` to a listing, so nothing pins the current empty-200 and no existing assertion changes.

## Steps to Reproduce

1. POST /2017-08-29/jobs to create one job on the Default queue.
2. GET /2017-08-29/jobs — the job is listed.
3. GET /2017-08-29/jobs?queue= (empty value) — observe HTTP 200 with an empty `jobs` array, reported as if the account simply has no job on that queue.
4. GET /2017-08-29/jobs?queue=%20 — same empty 200.
5. For comparison, POST /2017-08-29/jobs with "queue": "" — HTTP 400 BadRequestException "Queue must not be empty."
6. GET /2017-08-29/jobs?queue=arn:aws:mediaconvert:us-west-2:000000000000:queues/Default — the jobs are listed, although the modelled member offers only a queue name (SearchJobs is the reader the model gives the ARN form to).
