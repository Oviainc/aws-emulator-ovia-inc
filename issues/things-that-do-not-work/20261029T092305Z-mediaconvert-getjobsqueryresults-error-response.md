Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 511f9d11774345a530a86fa6799cabc26a1e1916

# GetJobsQueryResults probes a blank {Id} as a key, answering 404 where the sibling GetJob answers the crate's blank-member 400

_Filed: 2026-10-29T09:23:05Z_
_Operation: GetJobsQueryResults | Class: error-response_

> _Note: this issue was identified against the code base at commit `511f9d11774345a530a86fa6799cabc26a1e1916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states ONE rule for a caller-sent member that names a resource it holds: the value takes `crate::state::carries_no_characters`, and a value made only of whitespace is the modelled `BadRequestException` naming the member, "rather than with a lookup of a value nothing could hold". `GetJob` — the other read door addressed by an `{Id}` `@httpLabel` — applies it, and its own comment calls it "the test every resource-naming member in this service takes". `GetJobsQueryResults` does not: it takes `input.id` straight to `state.jobs_queries.get(&input.id)` and answers a whitespace-only label with `NotFoundException` (404) and `not_found_message("id")`.

So the two `{Id}`-addressed readers of this service disagree about what a nameless id is: `GET /2017-08-29/jobs/%20` is 400 `BadRequestException` naming `Id`, while `GET /2017-08-29/jobsQueries/%20` is 404 `NotFoundException`. A jobs query is a resource this store holds under a minted id exactly as a job is (`state.jobs_queries` keyed by the id `accept_jobs_query` minted), so nothing about the resource makes it the exception.

No stored state is affected — the request reads nothing either way — hence LOW; but the status and error code a client branches on depend on which `{Id}` door it used. This is the same omission that was already filed and fixed for `CancelJob` (issue `20261029T011521Z-mediaconvert-canceljob-error-response`: "CancelJob answers a job `{Id}` carrying no characters with NotFoundException, where the crate's one blank-reference rule ... makes it a BadRequest") and is still open for the three `Update*` doors (`20261029T055650Z-mediaconvert-updatejobtemplate-error-response`); `GetJobsQueryResults` is not covered by either, and is the last reader of a minted id that skips the test.

## Evidence

The stated rule — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:588-599`:
```rust
/// Whether a string member the caller SENT carries no characters at all.
///
/// Every member in this service that names a resource — the `Name` of a queue, a preset or
/// a job template, and the references (`Queue`, `JobTemplate`, `Role`, the `JobId` a share
/// names, the `Arn` of a certificate or of a tagged resource) that point at one — targets
/// the model's bare `__string`, which states no pattern and no length, so the
/// boundary validator passes the zero-length value and a value made only of whitespace
/// alike. Neither carries a resource identity: a key with no characters addresses nothing
/// a later read can reach ... ONE test for all
/// of them, so no door that mints or resolves such a member can disagree about which
/// values are identities, answered with the bad request [`blank_member_message`] words
/// rather than with a lookup of a value nothing could hold.
```

The sibling `{Id}` reader applying it — `src/ops/get_job.rs:157-169`:
```rust
        // "the job ID of the job": `{Id}` arrives percent-decoded from the path, so a value
        // made only of whitespace reaches here, and it is no more a job id than the blank
        // `JobId` the other door on a job reference (`CreateResourceShare`) refuses. That is
        // the test every resource-naming member in this service takes
        // (`crate::state::carries_no_characters`), so it is refused as a problem in the
        // request naming the member it arrived in, never as a lookup of an id nothing could
        // hold.
        if crate::state::carries_no_characters(&input.id) {
            return Err(GetJobErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::blank_member_message("Id")),
            })
            .into());
        }
```

The same kind of label taken straight to the lookup, with no such test — `src/ops/get_jobs_query_results.rs:174-182`:
```rust
        let Some(query) = state.jobs_queries.get(&input.id) else {
            return Err(GetJobsQueryResultsErrorModeled::NotFoundException(
                NotFoundException {
                    // The refusal names the request member the lookup read, `Id`.
                    message: Some(crate::state::not_found_message("id")),
                },
            )
            .into());
        };
```

The model confirms both members are the same thing — `smithy_query(query="operation",operation="GetJob,GetJobsQueryResults")`: `GetJob` input `Id` = `{"target": "__string", "httpLabel": true, "required": true}`; `GetJobsQueryResults` input `Id` = `{"target": "__string", "httpLabel": true, "required": true}` — a bare `__string`, so the boundary validator passes a whitespace-only label to both handlers.

Test coverage: `tests/all/blank_resource_name.rs` and `tests/all/blank_delete_name.rs` pin the rule for the minting, reference-resolving and delete doors; `tests/all/jobs_query.rs:92` and `tests/all/error_envelope.rs:193` exercise `GetJobsQueryResults` with a real id and with an id nothing is stored under (`1700000000000-nosuch`, which keeps its 404 because that value carries characters). No existing assertion sends a blank `{Id}` to `/2017-08-29/jobsQueries/`, so no test pins the current 404 for it.

## Steps to Reproduce

1. Read `src/state.rs:588-599` and note the one stated rule: a caller-sent member that names a resource this service holds takes `carries_no_characters`, and a blank value is the modelled bad request naming the member rather than a lookup of a value nothing could hold.
2. Read `src/ops/get_job.rs:157-169`: the other reader addressed by an `{Id}` `@httpLabel` applies that test and answers 400 with `blank_member_message("Id")`.
3. Read `src/ops/get_jobs_query_results.rs:174-182`: `input.id` goes straight into `state.jobs_queries.get(&input.id)`, so a whitespace-only label is probed as a key.
4. Send `GET /2017-08-29/jobs/%20` and observe 400 `BadRequestException` with message `Id must not be empty.`
5. Send `GET /2017-08-29/jobsQueries/%20` and observe 404 `NotFoundException` with message `The resource you specified in id was not found.` — the two `{Id}` readers of the same service disagree about the same kind of value.
