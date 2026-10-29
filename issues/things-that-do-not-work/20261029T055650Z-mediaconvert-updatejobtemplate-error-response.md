Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 12966d24554ed06eeaba7911ae25ff590cd06862

# UpdateJobTemplate resolves its addressed {Name} without the crate's one blank-reference test, answering 404 where the sibling DeleteJobTemplate answers 400

_Filed: 2026-10-29T05:56:50Z_
_Operation: UpdateJobTemplate | Class: error-response_

> _Note: this issue was identified against the code base at commit `12966d24554ed06eeaba7911ae25ff590cd06862`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states one rule for every member that names a resource it holds: such a value takes `crate::state::carries_no_characters` and a value made only of whitespace is the modelled `BadRequestException` naming the member, "rather than ... a lookup of a value nothing could hold". `DeleteJobTemplate` — the sibling door on the SAME resource, carrying the SAME `{Name}` `@httpLabel` — applies it. `UpdateJobTemplate` does not: it takes `input.name` straight to `state.job_templates.get_mut(&name)` and answers a whitespace-only label with `NotFoundException` (404) and `not_found_message("name")`.

The inconsistency is visible inside the single handler: `UpdateJobTemplate` DOES apply the blank test to the other resource-naming member it carries, `Queue` (refused with `blank_member_message("Queue")` before the store is touched), and the module doc of `carries_no_characters` names "the `Name` of a queue, a preset or a job template" in exactly the same list as `Queue`. So one of the two resource-naming members of one request is held to the rule and the other is not, and the two mutating doors onto the same stored template disagree about what a nameless label is.

No caller-visible data is corrupted — the request changes nothing either way — hence LOW; but the status and error code a client branches on differ by which door it used. The same omission is present on the other two `Update*` doors (`UpdateQueue`'s `{Name}` at `src/ops/update_queue.rs:195` and `UpdatePreset`'s at `src/ops/update_preset.rs:169`), whose own sibling deletes (`DeleteQueue`, `DeletePreset`) do apply the test, so a fix should cover all three mutating-by-name doors at once.

## Evidence

The rule, with the member list that includes a job template's `Name` — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:588-599`:
```rust
/// Whether a string member the caller SENT carries no characters at all.
///
/// Every member in this service that names a resource — the `Name` of a queue, a preset or
/// a job template, and the references (`Queue`, `JobTemplate`, `Role`, the `JobId` a share
/// names, the `Arn` of a certificate or of a tagged resource) that point at one — targets
/// the model's bare `__string` ...
/// ... ONE test for all
/// of them, so no door that mints or resolves such a member can disagree about which
/// values are identities, answered with the bad request [`blank_member_message`] words
/// rather than with a lookup of a value nothing could hold.
```

The sibling door on the same resource applying it — `src/ops/delete_job_template.rs:151-162`:
```rust
        // `{Name}` arrives percent-decoded from the path, so a value made only of
        // whitespace reaches here: it names no job template, so it is refused naming the
        // member it arrived in rather than probed for a key nothing could hold — exactly as
        // `DeleteQueue` and `DeletePreset` refuse one.
        if crate::state::carries_no_characters(&input.name) {
            return Err(DeleteJobTemplateErrorModeled::BadRequestException(
                BadRequestException {
                    message: Some(crate::state::blank_member_message("Name")),
                },
            )
            .into());
        }
```

The same handler applying the test to its OTHER resource-naming member — `src/ops/update_job_template.rs:179-191`:
```rust
        // "The new queue for the job template, if you are changing it" — a queue the
        // caller named is a reference to a queue resource ... A member the caller SENT is a value they meant, so an empty
        // one is refused rather than quietly dropped.
        let queue = match input.queue.as_deref() {
            Some(reference) if crate::state::carries_no_characters(reference) => {
                return Err(UpdateJobTemplateErrorModeled::BadRequestException(
                    BadRequestException {
                        message: Some(crate::state::blank_member_message("Queue")),
                    },
                )
                .into())
            }
```

And the addressed `{Name}` taken straight to the lookup, with no such test — `src/ops/update_job_template.rs:201-204` and `:248-253`:
```rust
        // The stored key is the name exactly as `CreateJobTemplate` persisted it, so the
        // label is read as sent rather than normalised.
        let name = input.name.clone();
        let mut state = state.lock();
```
```rust
        let Some(template) = state.job_templates.get_mut(&name) else {
            return Err(UpdateJobTemplateErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::not_found_message("name")),
            })
            .into());
        };
```

The crate's own test suite states the rule for the delete doors it covers — `tests/all/blank_delete_name.rs:9-14`:
```
//! only of whitespace percent-decodes to a value that reaches the handler and carries no
//! resource identity at all. The rule is the one `tests/all/blank_resource_name.rs` pins
//! for the minting and reference-resolving doors, and the certificate suite pins for
//! `DisassociateCertificate`: such a value is the modelled `BadRequestException` (400)
//! naming the member it arrived in, never a probe of a key nothing could hold
```

Test coverage: `tests/all/blank_delete_name.rs` and `tests/all/blank_resource_name.rs` pin the rule for the create/delete/reference doors (including `UpdateJobTemplate`'s blank `Queue`, `blank_resource_name.rs:425-431`); `tests/all/job_template_crud.rs` and `tests/all/job_template_queue_reference.rs` exercise `UpdateJobTemplate` end to end. No existing assertion covers a whitespace-only `{Name}` on an `Update*` door.

## Steps to Reproduce

1. Read `src/state.rs:588-599` and note the stated rule: every member naming a resource this service holds — explicitly including "the `Name` of ... a job template" — takes the one `carries_no_characters` test and a blank value is the modelled bad request naming the member.
2. Read `src/ops/delete_job_template.rs:151-162`: the sibling door on the same resource, carrying the same `{Name}` `@httpLabel`, applies that test and answers 400 `blank_member_message("Name")`.
3. Read `src/ops/update_job_template.rs:179-191`: the same handler applies the test to its `Queue` member.
4. Read `src/ops/update_job_template.rs:201-204` and `:248-253`: the addressed `{Name}` skips the test and a whitespace-only label is probed as a key, answering 404 `not_found_message("name")`.
5. Send `PUT /2017-08-29/jobTemplates/%20` with body `{"category":"x"}` and observe `404 NotFoundException`, then `DELETE /2017-08-29/jobTemplates/%20` and observe `400 BadRequestException` naming `Name` — the two mutating doors onto the same resource disagree.
6. Observe the same omission at `src/ops/update_queue.rs:195` and `src/ops/update_preset.rs:169`, whose sibling deletes (`src/ops/delete_queue.rs:156-161`, `src/ops/delete_preset.rs:167-172`) do apply the test.
