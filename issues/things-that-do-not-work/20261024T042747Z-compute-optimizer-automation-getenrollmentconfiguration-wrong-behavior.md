Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: be3ab9eb2c0ab303301f9b8637a58c2763937db6

# GetEnrollmentConfiguration never reports organizationRuleMode

_Filed: 2026-10-24T04:27:47Z_
_Operation: GetEnrollmentConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `be3ab9eb2c0ab303301f9b8637a58c2763937db6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetEnrollmentConfigurationResponse` declares `organizationRuleMode` (`AnyAllowed | NoneAllowed`) — "Specifies whether the management account can create Automation rules that implement optimization actions for this account." The emulator's `EnrollmentConfig::default()` sets `organization_rule_mode: None` and NOTHING ever writes it (`UpdateEnrollmentConfiguration` only writes `status`, `status_reason` and `last_updated_timestamp`), so the member is absent from every `GetEnrollmentConfiguration` response, before and after enrollment.

Expected: an account whose enrollment the service knows about reports a concrete `organizationRuleMode` (the service-side default for an account that has not restricted organization rules), so a client can read the mode it is documented to read. Actual: the member is always omitted, and a client cannot distinguish "not restricted" from "not reported".

## Evidence

Spec/model: `GetEnrollmentConfiguration` output declares `organizationRuleMode` targeting the `OrganizationRuleMode` enum (`AnyAllowed | NoneAllowed`) with the quoted member doc (smithy_query operation=GetEnrollmentConfiguration, doc shape=GetEnrollmentConfigurationResponse member=organizationRuleMode); OPERATIONS.md, GetEnrollmentConfiguration.

Handler/state: src/ops/get_enrollment_configuration.rs:163-169 copies `stored.organization_rule_mode` straight through; `EnrollmentConfig::default()` (src/state.rs:436-445) initialises it to `None`, and src/ops/update_enrollment_configuration.rs:252-259 — the only writer of an `EnrollmentConfig` — never assigns it, so it is `None` for every account for the life of the process.

Test coverage: tests/all/automation_rule_lifecycle.rs:204 (`enrollment_reports_the_default_until_an_update_persists_one`) asserts `status`/`lastUpdatedTimestamp` only; no test asserts anything about `organizationRuleMode`.

## Steps to Reproduce

1. Call `GetEnrollmentConfiguration` on a fresh emulator — response carries `status=Inactive` and no `organizationRuleMode`.
2. Call `UpdateEnrollmentConfiguration` with `status=Active`.
3. Call `GetEnrollmentConfiguration` again: expected the enrollment's `organizationRuleMode` (e.g. `AnyAllowed`); actual the member is still absent from the JSON body.
