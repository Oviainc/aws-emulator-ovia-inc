Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c03b32b2dcc5227aba431e790e0b1e2397cf2279

# Tagging operations hand-roll the not-found message instead of the crate's single template

_Filed: 2026-10-25T07:05:11Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `c03b32b2dcc5227aba431e790e0b1e2397cf2279`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate declares ONE not-found message template for the whole service in `crate::state::not_found_message` and documents that it is the only definition — "One definition for the whole service, so every lookup that cannot find what it was asked for words it the same way, whichever error shape the operation models for it" (state.rs:631-633). Every suite-definition and suite-run lookup honours it (GetSuiteDefinition, GetSuiteRun, StopSuiteRun, StartSuiteRun, UpdateSuiteDefinition all call `crate::state::not_found_message(<member>, <value>)`).

The three tagging handlers — ListTagsForResource, TagResource, UntagResource — are the exception: each builds its own `ResourceNotFoundException` text, `"No IoT Device Advisor resource with resourceArn {}"`, which neither carries the documented `Unable to find resource with ` prefix nor the `<member>: ` colon form, and each of the three repeats that literal independently so the copies can drift further. The resourceArn lookup IS a lookup of "the resource the request NAMED", so the rule the comment states applies to it and the code under it breaks the rule. The integration tests for the tagging refusals assert only the status and the error code, never the message, so aligning the three sites on `not_found_message("resourceArn", &input.resource_arn)` leaves every existing assertion passing.

## Evidence

Rule the crate states once, in its own state module — `services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state.rs:627-636`:

    /// The message a refusal carries when the resource the request NAMED does not exist.
    ///
    /// Device Advisor builds this text from the request: the template is
    /// `Unable to find resource with <member>: <value>`, where `<member>` is the input member
    /// that identified the missing resource and `<value>` is the value the caller sent. One
    /// definition for the whole service, so every lookup that cannot find what it was asked
    /// for words it the same way, whichever error shape the operation models for it.
    pub fn not_found_message(member: &str, value: &str) -> String {
        format!("Unable to find resource with {member}: {value}")
    }

Siblings that honour it: `src/ops/get_suite_definition.rs:162-169` (`message: Some(crate::state::not_found_message("suiteDefinitionId", &input.suite_definition_id))`), `src/ops/get_suite_run.rs:167-174` (`not_found_message("suiteRunId", ...)`), `src/ops/stop_suite_run.rs:147-154`, `src/ops/start_suite_run.rs:174-194`, `src/ops/update_suite_definition.rs:204-212`.

The three sites that break it:
- `src/ops/list_tags_for_resource.rs:157-167`:
        if !store.holds_tagged_resource(&input.resource_arn, &resource) {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(format!(
                        "No IoT Device Advisor resource with resourceArn {}",
                        input.resource_arn
                    )),
- `src/ops/tag_resource.rs:157-167`: same `format!("No IoT Device Advisor resource with resourceArn {}", input.resource_arn)`
- `src/ops/untag_resource.rs:165-175`: same literal a third time

The crate's own test for the rule states it as universal but only exercises the id-keyed lookups — `tests/all/suite_runs_listing_and_update.rs:776-777`:

    /// Every lookup of something the store does not hold is refused with AWS's template,
    /// `Unable to find resource with <member>: <value>`, built from the request.

and its rows cover only `suiteDefinitionId` / `suiteDefinitionVersion` / `suiteRunId` (lines 808-874). Test coverage of the tagging refusals, `tests/all/resource_tags.rs:515-612` (`an_unknown_or_foreign_resource_arn_is_refused_by_every_tagging_operation`), asserts `expected_status` and `expected_code` only — no message assertion — so no existing test pins the hand-rolled wording.

## Steps to Reproduce

1. Boot the emulator and CreateSuiteDefinition to obtain a suiteDefinitionId (and its ARN).
2. GET /suiteDefinitions/1e8542c9-cb2a-4172-ae95-04aeb8a757da — 404 ResourceNotFoundException with message "Unable to find resource with suiteDefinitionId: 1e8542c9-cb2a-4172-ae95-04aeb8a757da", i.e. the template state.rs:629-633 declares.
3. GET /tags/arn%3Aaws%3Aiotdeviceadvisor%3Aus-east-1%3A000000000000%3Asuitedefinition%2F11111111-2222-3333-4444-555555555555 — also 404 ResourceNotFoundException, but the message is "No IoT Device Advisor resource with resourceArn arn:aws:iotdeviceadvisor:...", not the documented single template.
4. Repeat with POST /tags/{resourceArn} (TagResource) and DELETE /tags/{resourceArn}?tagKeys=k (UntagResource) on the same unknown ARN: both answer the same hand-rolled wording from their own copy of the literal.
