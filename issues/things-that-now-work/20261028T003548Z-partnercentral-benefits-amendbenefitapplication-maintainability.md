Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2f325610dba287ab6eb0f722c6fb72ebe3ad3d74

# Extract the one member-refusal ValidationException construction this crate writes four times

_Filed: 2026-10-28T00:35:48Z_
_Operation: AmendBenefitApplication | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2f325610dba287ab6eb0f722c6fb72ebe3ad3d74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The composition that turns a (member path, requirement clause) pair into this service's modelled `ValidationException` — `member_clause` -> `validation_envelope` with the `unwrap_or(clause)` fallback branch -> `validation_field(member, requirement, INVALID_VALUE)` — is written out four separate times in this crate: twice in `src/state.rs` and once each in two handler files. The four copies are identical apart from which `ValidationExceptionReason` constant they pass, so the fallback branch and the field-list wiring can drift independently (a fifth refusal added in a new handler has nothing to call).

The fix is to add one constructor beside the existing `validation_field` helper in `src/state.rs` taking `(member, requirement, reason)` and returning `crate::ops::types::ValidationException`, and have `benefit_schema_refusal`, `benefit_not_offered_refusal`, `amend_benefit_application::refused` and `associate_benefit_application_resource::record_full` call it. Every response body and every existing test stays byte-identical: the inputs to the three helpers are unchanged on all four paths.

## Evidence

Copy 1 — src/state.rs:2165-2175 (benefit_schema_refusal):
    let clause = crate::service::member_clause(&violation.member, &violation.requirement, false);
    Some(crate::ops::types::ValidationException {
        message: crate::service::validation_envelope(std::slice::from_ref(&clause))
            .unwrap_or_else(|| clause.clone()),
        reason: VALIDATION_REASON_FIELD.to_string(),
        field_list: Some(validation_field(
            &violation.member,
            &violation.requirement,
            VALIDATION_CODE_INVALID_VALUE,
        )),
    })

Copy 2 — src/state.rs:2279-2289 (benefit_not_offered_refusal):
    let clause = crate::service::member_clause(member, &requirement, false);
    Some(crate::ops::types::ValidationException {
        message: crate::service::validation_envelope(std::slice::from_ref(&clause))
            .unwrap_or_else(|| clause.clone()),
        reason: VALIDATION_REASON_BUSINESS.to_string(),
        field_list: Some(validation_field(
            member,
            &requirement,
            VALIDATION_CODE_INVALID_VALUE,
        )),
    })

Copy 3 — src/ops/amend_benefit_application.rs:250-259 (refused):
    let rendered = crate::service::member_clause(member, clause, false);
    AmendBenefitApplicationErrorModeled::ValidationException(ValidationException {
        message: crate::service::validation_envelope(std::slice::from_ref(&rendered))
            .unwrap_or(rendered),
        reason: crate::state::VALIDATION_REASON_FIELD.to_string(),
        field_list: Some(crate::state::validation_field(
            member,
            clause,
            crate::state::VALIDATION_CODE_INVALID_VALUE,
        )),
    })

Copy 4 — src/ops/associate_benefit_application_resource.rs:163-173 (record_full):
    let clause = crate::service::member_clause(ASSOCIATED_RESOURCES_MEMBER, &requirement, false);
    AssociateBenefitApplicationResourceErrorModeled::ValidationException(ValidationException {
        message: crate::service::validation_envelope(std::slice::from_ref(&clause))
            .unwrap_or_else(|| clause.clone()),
        reason: crate::state::VALIDATION_REASON_FIELD.to_string(),
        field_list: Some(crate::state::validation_field(
            ASSOCIATED_RESOURCES_MEMBER,
            &requirement,
            crate::state::VALIDATION_CODE_INVALID_VALUE,
        )),
    })

Existing tests that pin the rendered bodies on these four paths (and must stay green unchanged): tests/all/validation_error_body.rs, tests/all/validation_exception_message.rs, tests/all/catalog_refusal_one_construction.rs, tests/all/reserved_tag_keys.rs, tests/all/entitlement_denial.rs, tests/all/amend_benefit_application.rs, tests/all/associate_benefit_application_resource.rs, tests/all/benefit_catalog_record.rs.

## Steps to Reproduce

1. Open src/state.rs and read `benefit_schema_refusal` (lines 2160-2176) and `benefit_not_offered_refusal` (lines 2268-2290).
2. Open src/ops/amend_benefit_application.rs and read `refused` (lines 249-262).
3. Open src/ops/associate_benefit_application_resource.rs and read `record_full` (lines 158-175).
4. Observe the same member_clause -> validation_envelope(+fallback) -> validation_field composition in all four, differing only in the VALIDATION_REASON_* constant passed.
