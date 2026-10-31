Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: db5b8cf4b40010fa5fa0137914026d79724270f1

# The three tagging doors answer a missing resource with a hand-rolled sentence that names the caller's value, breaking the crate's one not-found rule

_Filed: 2026-10-28T20:11:59Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `db5b8cf4b40010fa5fa0137914026d79724270f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::not_found_message` is the crate's single statement of how a lookup that resolves nothing is worded, and its doc states the rule twice: the sentence "names the REQUEST MEMBER that pointed at the resource, not the value the caller supplied", and it is "one sentence for a reference that resolves to nothing, whichever modelled error carries it". Every lookup refusal in the crate goes through it — `GetQueue`, `GetPreset`, `GetJobTemplate`, `GetJob`, `CancelJob`, `GetJobsQueryResults`, `UpdateQueue`, `UpdatePreset`, `UpdateJobTemplate`, `DisassociateCertificate`, `CreateResourceShare`, `CreateJob`'s queue/template/hop references, and even the settings-graph refusal `SettingsProblem::PresetNotFound`.

The three tagging handlers are the exception: each builds its own sentence `format!("Resource {} not found.", input.arn)`, which does the one thing the rule forbids — it echoes the value the caller supplied and names no request member. The three copies are written out independently, so they can also drift from each other. The same three handlers also hand-roll the "not a MediaConvert resource ARN" sentence three times, while the exactly parallel certificate pair keeps its ARN-form refusal in one place (`crate::state::certificate_arn_message`) precisely so "the two refusals cannot drift".

Nothing pins the current wording: `tests/all/resource_tagging.rs` asserts only the status and `error_type` for these cases, never `/message`.

## Evidence

The crate's stated rule — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:620-635`:
```
/// How this service words a reference that resolves to nothing: it names the REQUEST
/// MEMBER that pointed at the resource, not the value the caller supplied.
///
/// MediaConvert answers a lookup that resolves nothing with "The resource you specified
/// in `<member>` was not found.", where `<member>` is the input member the handler read
/// the reference from (`name` for a queue, `id` for a job, `queue` for a job's queue) —
/// so the wording is built from the request, exactly as the other member-naming refusals
/// in this crate are. It is the message of the `NotFoundException` a request that
/// ADDRESSES a missing resource gets, and the same sentence names the member inside the
/// settings-graph refusal ([`SettingsProblem::PresetNotFound`]) ... one sentence for a reference that resolves to nothing,
/// whichever modelled error carries it.
pub fn not_found_message(member: &str) -> String {
    format!("The resource you specified in {member} was not found.")
}
```

The three doors that break it:
* `src/ops/list_tags_for_resource.rs:197-204`:
```rust
        let Some(tags) = state.tags(&resource) else {
            return Err(ListTagsForResourceErrorModeled::NotFoundException(
                NotFoundException {
                    message: Some(format!("Resource {} not found.", input.arn)),
                },
            )
            .into());
        };
```
* `src/ops/tag_resource.rs:207-212`: `message: Some(format!("Resource {} not found.", input.arn)),`
* `src/ops/untag_resource.rs:193-198`: `message: Some(format!("Resource {} not found.", input.arn)),`

The sibling that keeps its refusal sentence in one place — `src/state.rs:1653-1660`:
```
/// How this service words the refusal of an `Arn` that is not an ACM certificate ARN.
///
/// Built from the value the request carried, as [`not_found_message`] is built from the
/// member it arrived in, and worded like the ARN refusal `TagResource` already answers: it
/// names the member, the value, and the kind of ARN the operation accepts.
pub fn certificate_arn_message(arn: &str) -> String {
    format!("Arn {arn} is not the ARN of an ACM certificate.")
}
```
while `DisassociateCertificate`'s own not-found refusal does go through the rule — `src/ops/disassociate_certificate.rs:201-208`: `message: Some(crate::state::not_found_message("arn")),`.

Test coverage: `tests/all/resource_tagging.rs:915-951` (`an_arn_naming_no_stored_resource_is_not_found`) asserts only `reply.status == 404` and `reply.error_type == Some("NotFoundException")` for all three doors — no `/message` assertion anywhere in the file (grep for `not_found|message` in it matches only the test name), so moving these three to `not_found_message("arn")` changes no existing assertion.

## Steps to Reproduce

1. GET /2017-08-29/queues/absent — observe 404 with message "The resource you specified in name was not found." (the crate's one sentence, naming the request member).
2. GET /2017-08-29/tags/arn%3Aaws%3Amediaconvert%3Aus-east-1%3A000000000000%3Aqueues%2Fabsent — observe 404 with message "Resource arn:aws:mediaconvert:us-east-1:000000000000:queues/absent not found." instead.
3. Repeat with POST /2017-08-29/tags ({"arn":<same>,"tags":{"a":"b"}}) and PUT /2017-08-29/tags/<same encoded ARN> ({"tagKeys":["a"]}) — both answer the same hand-rolled sentence, which echoes the caller's value and names no member.
