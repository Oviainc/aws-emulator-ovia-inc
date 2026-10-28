Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# The BenefitApplicationDetails resource-guard refusal bypasses the validation envelope every other in-handler refusal of this crate states

_Filed: 2026-10-28T02:39:56Z_
_Operation: CreateBenefitApplication | Class: error-response_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers refuse a `BenefitApplicationDetails` document that breaks this service's own document guards (`state::details_rejection`: depth, member count, byte count) by putting the guard's BARE SENTENCE straight into the modelled `ValidationException$Message`, instead of rendering it through `service::member_clause` + `service::validation_envelope` the way every other in-handler refusal of this crate does.

The result is a `ValidationException` whose `Message` is `"BenefitApplicationDetails nests deeper than the 32 levels this service accepts"` rather than `"1 validation error detected: Value at '<member>' failed to satisfy constraint: <requirement>"`. That message fails the crate's OWN service-wide rule-1 invariant checker (`assert_validation_invariants`, which asserts the envelope prefix and that every clause begins `Value at '`), so a client that parses a refusal clause to learn which member and which requirement failed can read every other refusal of this service but not these three. `state.rs` even already words the SAME depth fault in clause form on the schema path (`"Member must nest no deeper than {MAX_DETAILS_DEPTH} levels"`, state.rs:2208), which goes through the envelope — so the two refusals of one member for one class of fault are worded in two different forms.

All three call sites are the same defect: create (the member's first writer), update and amend.

## Evidence

The rule, stated by the helper the crate routes in-handler refusals through — src/service.rs:267-272:
    /// The AWS validation envelope (`N validation error(s) detected: `) over clauses already
    /// rendered by [`member_clause`]. `None` for no clause at all.
    ///
    /// A handler refusing an input member of its own states it through here, so an
    /// in-handler refusal reads exactly like one the centralized walk produced.
    pub fn validation_envelope(clauses: &[String]) -> Option<String> {

The code that skips it — src/ops/create_benefit_application.rs:226-240:
        if let Some(why) = crate::state::details_rejection(input.benefit_application_details.as_ref())
        {
            return Err(CreateBenefitApplicationErrorModeled::ValidationException(
                ValidationException {
                    message: why.clone(),
                    reason: crate::state::VALIDATION_REASON_FIELD.to_string(),
                    field_list: Some(crate::state::validation_field(
                        "BenefitApplicationDetails",
                        &why,
...and identically in src/ops/update_benefit_application.rs:219-233 and src/ops/amend_benefit_application.rs:602-614 (`message: why.clone(),`).

What `why` is — src/state.rs:2056-2074 (details_rejection):
            return Some(format!(
            "BenefitApplicationDetails nests deeper than the {MAX_DETAILS_DEPTH} levels this service accepts"
        ));
    ...
            "BenefitApplicationDetails holds more than the {MAX_DETAILS_MEMBERS} members this service stores"
    ...
            "BenefitApplicationDetails carries more than the {MAX_DETAILS_BYTES} bytes of text this service stores"

The sibling in-handler refusals that DO use the envelope: state.rs:2165-2175 (`benefit_schema_refusal`), state.rs:2279-2289 (`benefit_not_offered_refusal`), state.rs:2687-2698 (`reserved_tag_key_refusal`), ops/amend_benefit_application.rs:249-262 (`refused`), ops/associate_benefit_application_resource.rs:158-175 (`record_full`) — each `message: crate::service::validation_envelope(std::slice::from_ref(&clause)).unwrap_or_else(...)`.

The crate's own invariant this message breaks — tests/all/service_error_rules.rs:152-172:
    /// The invariants of rule 1, checked on every refusal in every table: the AWS envelope,
    /// no rendering of the rejected value, and no pattern anchors.
    pub(crate) fn assert_validation_invariants(case: &str, message: &str) {
        assert!(
            message.starts_with("1 validation error detected: ") ...
            assert!(
                clause.starts_with("Value at '") || clause.starts_with("Value null at '"),

Test coverage: the amend call site IS exercised (tests/all/amend_benefit_application.rs:846-909 `amend_refuses_a_member_past_the_document_member_bound` and :913-984 `amend_refuses_text_past_the_document_byte_bound`) but those tests assert only the 400 and the `ValidationException` code and never call `assert_validation_invariants` / compare the `Message`; the create and update call sites have no test at all (no test in the crate references `details_rejection`, `MAX_DETAILS_DEPTH` or the guard sentences).

## Steps to Reproduce

1. POST / with X-Amz-Target: PartnerCentralBenefitsService.CreateBenefitApplication and a body whose `BenefitApplicationDetails` is an object nested 33 levels deep (or holding more than MAX_DETAILS_MEMBERS = 256 members, or more than MAX_DETAILS_BYTES = 64 KiB of text).
2. Observe HTTP 400 `ValidationException` whose `Message` is the bare sentence `BenefitApplicationDetails nests deeper than the 32 levels this service accepts`.
3. Compare with any other in-handler refusal of this service, e.g. CreateBenefitApplication carrying a tag key `aws:createdBy`, which answers `1 validation error detected: Value at 'Tags' failed to satisfy constraint: ...` (pinned in tests/all/reserved_tag_keys.rs:162-199).
4. Repeat the first step against UpdateBenefitApplication (same document) and against AmendBenefitApplication (an amendment that nests the stored document past the bound) — both answer the same non-envelope Message.
