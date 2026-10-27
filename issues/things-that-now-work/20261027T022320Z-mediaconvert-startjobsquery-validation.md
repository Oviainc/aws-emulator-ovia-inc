Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# A jobs-query filter that carries a Key but no Values is silently dropped, so a narrowing query matches every job

_Filed: 2026-10-27T02:23:20Z_
_Operation: StartJobsQuery | Class: validation_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartJobsQuery turns each `JobsQueryFilter` into one `(key, value)` term per entry of that filter's `Values` list. A filter that states a `Key` but carries no `Values` (absent, or `[]`) therefore contributes NO term at all and is silently dropped — even though the handler refuses the mirror-image case (a filter with values and no `Key`) with a `BadRequestException` built from the very same model sentence.

When such a filter is the only one in `filterList`, `terms` ends up empty, and the stored query's match test treats an empty term list as "no filter at all": `JobsQueryRecord::matches` returns true for every job. So `filterList=[{"key":"status"}]` — a request that asks to NARROW the result set — is answered with a query whose page is every stored job, instead of either the modelled 400 the keyless filter gets or a page of nothing. A caller cannot tell the widened query from one they never filtered.

## Evidence

Model (smithy_query shape=JobsQueryFilter): doc = "Provide one or more JobsQueryFilter objects, each containing a Key with an associated Values array. Note that MediaConvert queries jobs using OR logic." Neither `Key` nor `Values` carries @required, so the framework refuses neither.

The handler cites that sentence and enforces only half of it — services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/start_jobs_query.rs:174-198:
```
// "Provide one or more JobsQueryFilter objects, each containing a Key with an
// associated Values array": a filter naming no key describes no job detail to
// match on, so it is a problem in the request, refused naming the member it is in.
...
            let Some(key) = filter.key.as_deref() else {
                return Err(StartJobsQueryErrorModeled::BadRequestException(
                    BadRequestException {
                        message: Some(
                            "You must specify Key for each JobsQueryFilter in filterList."
...
            for value in filter.values.iter().flatten() {
                terms.push(crate::state::JobsQueryTerm { ... });
            }
```
`filter.values.iter().flatten()` yields nothing for an absent/empty `Values`, so the filter leaves `terms` untouched.

The widening is in src/state.rs:3002-3008:
```
    pub fn matches(&self, job: &JobRecord) -> bool {
        self.terms.is_empty()
            || self.terms.iter().any(|term| job.matches_filter(&term.key, &term.value))
    }
```
with its doc "a query carrying no term at all narrows nothing and matches every job".

Test coverage: services/service-com-amazonaws-mediaconvert-mediaconvert/tests/all/jobs_query.rs:715-729 pins the keyless filter at HTTP 400 (`json!({ "filterList": [{ "values": ["alpha"] }] })`). No row in `a_jobs_query_matches_on_any_value_of_any_filter` (jobs_query.rs:249-389) or in `the_modelled_refusals` (jobs_query.rs:679) sends a filter with a key and no values, so nothing pins the current outcome.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_queue(Name='alpha'); create_job(Role=R, Queue='alpha', Settings={Inputs:[{FileInput:'s3://b/in.mp4'}], OutputGroups:[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}) twice, so the account holds two jobs.
3. start_jobs_query(FilterList=[{'Key':'status'}]) — a filter naming a key but no values. Observe HTTP 201 with a query id (the keyless mirror case, FilterList=[{'Values':['alpha']}], is refused with BadRequestException 400).
4. Wait for the query to complete, then get_jobs_query_results(Id=<id>): the page contains EVERY stored job, i.e. the filter narrowed nothing.
5. Compare with start_jobs_query(FilterList=[{'Key':'status','Values':['PROGRESSING']}]), which correctly matches no job.
