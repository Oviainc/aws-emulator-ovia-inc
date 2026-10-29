Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# GetQueue/GetPreset/GetJobTemplate probe a blank {Name} as a store key, answering 404 where the crate's one blank-member rule (and GetJob) answers 400

_Filed: 2026-10-29T12:12:01Z_
_Operation: GetQueue | Class: error-response_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::carries_no_characters` is documented as the ONE test every member that names a resource this service holds must take — the doc names "the `Name` of a queue, a preset or a job template" explicitly — so that "no door that mints or resolves such a member can disagree about which values are identities", and such a value is refused with `blank_member_message` "rather than with a lookup of a value nothing could hold".

The three read-by-name doors skip it. `GetQueue`, `GetPreset` and `GetJobTemplate` take the percent-decoded `{Name}` label straight into `state.queues.get` / `state.presets.get` / `state.job_templates.get`, so a label made only of whitespace is probed as a key and answered with `NotFoundException` (404) and `not_found_message("name")`.

That contradicts the crate in two places at once: the sibling DELETE door on the SAME resource with the SAME `{Name}` `@httpLabel` applies the test and answers 400 (`DeleteQueue`, `DeletePreset`, `DeleteJobTemplate`), and the sibling READ door `GetJob` applies it with a comment calling it "the test every resource-naming member in this service takes". So a client branching on the status gets 400 or 404 for the same nameless label depending on which door it used.

No stored state is affected — the request reads nothing either way — hence LOW. This is the same omission already filed for the three `Update*` doors (`20261029T055650Z-mediaconvert-updatejobtemplate-error-response`, which names `UpdateQueue` and `UpdatePreset` as well) and for `GetJobsQueryResults` (`20261029T092305Z-...-error-response`, scoped to "the last reader of a MINTED id that skips the test"). Neither issue covers the three readers of a caller-chosen NAME, which are the remaining doors; a fix should cover all three at once.

## Evidence

The stated rule, naming a queue's/preset's/template's `Name` — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:589-613`:
```rust
/// Whether a string member the caller SENT carries no characters at all.
///
/// Every member in this service that names a resource — the `Name` of a queue, a preset or
/// a job template, and the references (`Queue`, `JobTemplate`, `Role`, the `JobId` a share
/// names, the `Arn` of a certificate or of a tagged resource) that point at one — targets
/// the model's bare `__string`, which states no pattern and no length, so the
/// boundary validator passes the zero-length value and a value made only of whitespace
/// alike. ... ONE test for all
/// of them, so no door that mints or resolves such a member can disagree about which
/// values are identities, answered with the bad request [`blank_member_message`] words
/// rather than with a lookup of a value nothing could hold.
pub fn carries_no_characters(value: &str) -> bool {
    value.trim().is_empty()
}
```

The sibling READ door that applies it, calling it universal — `src/ops/get_job.rs:157-169`:
```rust
        // "the job ID of the job": `{Id}` arrives percent-decoded from the path, so a value
        // made only of whitespace reaches here ... That is
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

The sibling door on the SAME resource and the SAME label applying it — `src/ops/delete_queue.rs:151-161`:
```rust
        // `{Name}` arrives percent-decoded from the path, so a value made only of
        // whitespace reaches here: it names no queue, so the request is refused naming the
        // member it arrived in rather than probed for a key nothing could hold ...
        if crate::state::carries_no_characters(&input.name) {
            return Err(DeleteQueueErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::blank_member_message("Name")),
            })
            .into());
        }
```

The three readers skipping it. `src/ops/get_queue.rs:165-174`:
```rust
        // `GetQueue`'s `{Name}` is a queue name; like `ListJobs`' `queue` and unlike
        // `SearchJobs`' `queue`, it is not documented to accept an ARN.
        let name = input.name;
        let Some(record) = state.queues.get(&name).cloned() else {
            return Err(GetQueueErrorModeled::NotFoundException(NotFoundException {
                // The refusal names the request member the lookup read, `Name`.
                message: Some(crate::state::not_found_message("name")),
            })
            .into());
        };
```
`src/ops/get_preset.rs:162-171`:
```rust
        // `{Name}` is a preset name; the member is not documented to accept an ARN.
        let name = input.name;
        let Some(record) = state.presets.get(&name) else {
            return Err(GetPresetErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::not_found_message("name")),
            })
            .into());
        };
```
`src/ops/get_job_template.rs:157-169`:
```rust
        // The stored key is the name exactly as `CreateJobTemplate` persisted it, so the
        // label is read as sent rather than normalised.
        let name = input.name.as_str();
        ...
        let Some(template) = state.job_templates.get(name) else {
            return Err(GetJobTemplateErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::not_found_message("name")),
            })
            .into());
        };
```

The model confirms there is nothing for the framework to refuse: per `OPERATIONS.md:155-156`, `GetQueue` input is `Name` (string, @httpLabel, required) — a bare `__string` with no pattern or length, as the three delete doors' labels are.

Test coverage: `tests/all/blank_delete_name.rs` pins the 400 for the three DELETE doors (reusing `tests/all/blank_resource_name.rs`'s blank forms and door table), and `tests/all/blank_job_reference.rs` pins it for `GetJob`. `tests/all/blank_resource_name.rs:239-249` only reads back `{collection}/{blank.path}` to assert nothing was stored, asserting no particular status, so no test pins the current 404 on the three GET readers.

## Steps to Reproduce

1. Read `src/state.rs:589-613`: the one rule, whose member list explicitly includes "the `Name` of a queue, a preset or a job template", requires the blank value to be the modelled bad request naming the member rather than a lookup.
2. Read `src/ops/delete_queue.rs:151-161` and `src/ops/get_job.rs:157-169`: the sibling delete door on the same resource and the sibling read door on a job both apply the test.
3. Read `src/ops/get_queue.rs:165-174`, `src/ops/get_preset.rs:162-171`, `src/ops/get_job_template.rs:157-169`: the label goes straight into the map probe with no such test.
4. Send `GET /2017-08-29/queues/%20` and observe 404 `NotFoundException` ("The resource you specified in name was not found.").
5. Send `DELETE /2017-08-29/queues/%20` and observe 400 `BadRequestException` ("Name must not be empty.") — the two doors onto the same resource, carrying the same label, disagree about the same kind of value.
6. Repeat steps 4-5 with `/2017-08-29/presets/%20` and `/2017-08-29/jobTemplates/%20` for the same divergence.
