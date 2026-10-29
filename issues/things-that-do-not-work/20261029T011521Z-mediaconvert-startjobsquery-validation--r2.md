Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36

# A jobs-query `queue` filter value carrying no characters is stored as a term, so the query silently matches nothing instead of being refused

_Filed: 2026-10-29T01:15:21Z_
_Operation: StartJobsQuery | Class: validation_

> _Note: this issue was identified against the code base at commit `41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartJobsQuery` turns every `(Key, value)` pair of `filterList` into a stored `JobsQueryTerm` after testing only that the `Values` array is present and non-empty. A value that is present but carries no characters (`{"key":"queue","values":[" "]}`) is kept as a term. The `queue` filter key is a QUEUE REFERENCE — the match resolves it with `queue_name_from_reference` and compares it to the job's stored queue name — so a blank value resolves to nothing any job can equal: the query is accepted (201 + id), completes, and `GetJobsQueryResults` reports an empty `jobs` array. The caller is told the query ran and matched no job, for a value that could never have named a queue.

That skips the crate's ONE test for a caller-sent member that names a resource (`crate::state::carries_no_characters`), which every other door that resolves a queue reference applies: `CreateJob$Queue`, `CreateJobTemplate$Queue`, `UpdateJobTemplate$Queue` and the `Queue` of every hop destination (`crate::state::hop_destination_blank_queue_problem`) all answer the modelled 400 `"Queue must not be empty."` rather than resolving a name nothing could hold.

It also contradicts this very handler's stated rule for a filter that cannot narrow: the handler refuses a filter with no key and a filter with no value precisely because a dropped/unsatisfiable filter misreports the query, yet a blank value is kept and narrows the result to the empty set.

NOT the gap the fixed issue `20261027T022320Z-mediaconvert-startjobsquery-validation` closed (a filter carrying a Key but NO Values, which is now refused): this is a filter that DOES carry a value, and the value carries no characters. It is the third door of the same blank-queue-reference family as the two open issues `20261028T210304Z-mediaconvert-listjobs-validation` and `20261028T212007Z-mediaconvert-searchjobs-validation`, which name the identical rule for the `queue` filter of `ListJobs`/`SearchJobs`; neither of those issues mentions `StartJobsQuery` or `filterList`.

## Evidence

The crate's one rule — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:588-599`:
```
/// Whether a string member the caller SENT carries no characters at all.
///
/// Every member in this service that names a resource — the `Name` of a queue, a preset or
/// a job template, and the references (`Queue`, `JobTemplate`, `Role`, the `JobId` a share
/// names, the `Arn` of a certificate or of a tagged resource) that point at one — targets
/// the model's bare `__string`, ... ONE test for all
/// of them, so no door that mints or resolves such a member can disagree about which
/// values are identities, answered with the bad request [`blank_member_message`] words
/// rather than with a lookup of a value nothing could hold.
```

The door that skips it — `src/ops/start_jobs_query.rs:174-214`:
```rust
        // "Provide one or more JobsQueryFilter objects, each containing a Key with an
        // associated Values array": a filter is a key-value pair, so BOTH halves have to
        // be there for it to describe anything to match — one naming no key names no job
        // detail, and one carrying no value (absent or empty) names nothing to compare
        // that detail with. Either way the request asks to narrow by a filter that cannot
        // narrow, which is a problem in the request, refused naming the member it is in
        // rather than silently dropped (a dropped filter would WIDEN the query to every
        // job).
        ...
            let values = filter.values.as_deref().unwrap_or_default();
            if values.is_empty() { /* 400 "You must specify Values ..." */ }
            for value in values {
                terms.push(crate::state::JobsQueryTerm {
                    key: key.to_string(),
                    value: value.clone(),
                });
            }
```
No `carries_no_characters` call appears anywhere in `src/ops/start_jobs_query.rs`.

What the stored blank term then does — `src/state.rs:3182-3185`:
```rust
        match key {
            // "Your Queue's name or ARN", so either form resolves to the name of the
            // queue this job was sent to.
            JOBS_QUERY_FILTER_KEY_QUEUE => self.queue == queue_name_from_reference(value),
```
`queue_name_from_reference(" ")` is `" "` (`src/state.rs:1012-1018`, a value that is not an ARN is the name itself), and no stored job's `queue` can be `" "` because `CreateQueue` refuses such a `Name` (`src/ops/create_queue.rs:188-194`).

The sibling doors that DO apply the test to the same reference — `src/ops/create_job.rs:245-254`:
```rust
        if input
            .queue
            .as_deref()
            .is_some_and(crate::state::carries_no_characters)
        {
            return Err(CreateJobErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::blank_member_message("Queue")),
            })
```
and `src/state.rs:1095-1112`:
```
/// `HopDestination$Queue` is a reference to a queue resource exactly as `CreateJob$Queue`
/// and `CreateJobTemplate$Queue` are ([`hop_destination_queue_name`]), so it takes the same
/// test every resource-naming member in this service takes ([`carries_no_characters`])
```

Test coverage: `tests/all/jobs_query.rs:828-902` (`a_filter_must_carry_both_a_key_and_a_value_to_match_on`) states the rule — "a filter that states only one half of it describes nothing to match and is refused, whichever half is missing: it is never dropped, which would WIDEN the query it was sent to narrow" — and its table covers only an ABSENT key, an absent `values` and an EMPTY `values` list; no row sends a value made of whitespace. `tests/all/blank_resource_name.rs` (whole file) exercises the creates, hop destinations, `CreateResourceShare$JobId` and the three tagging `Arn`s but never `filterList`, so no existing assertion changes.

## Steps to Reproduce

1. Create a queue `alpha` and submit a job to it (POST /2017-08-29/queues, then POST /2017-08-29/jobs).
2. POST /2017-08-29/jobsQueries with body {"filterList":[{"key":"queue","values":[" "]}]} — observe HTTP 201 and an `id`.
3. Poll GET /2017-08-29/jobsQueries/{id} until `status` is COMPLETE — observe `jobs: []`: the query reports that no job matched.
4. For comparison, POST /2017-08-29/jobs with "Queue": " " — observe HTTP 400 BadRequestException "Queue must not be empty.", the refusal the crate's one blank-reference rule requires for the same reference.
