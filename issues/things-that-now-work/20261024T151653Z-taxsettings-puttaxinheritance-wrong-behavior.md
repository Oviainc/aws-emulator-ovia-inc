Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# PutTaxInheritance can never succeed: every call is refused and heritageStatus is never persisted

_Filed: 2026-10-24T15:16:53Z_
_Operation: PutTaxInheritance | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: PutTaxInheritance records the account's tax-inheritance election ("The updated tax inheritance status.") and answers 200 with an empty body; a following GetTaxInheritance then reports the election that was just set (OptIn after an OptIn write, OptOut after an OptOut write). The pair Put -> Get is the only workflow this pair of operations has.

Actual: the handler never touches state at all (`let _ = (ctx, &self.engine);`) and unconditionally returns 400 ValidationException with errorCode FieldValidationFailed and the fixed message "Only AWS orgs management account is allowed to set tax inheritance." for BOTH modeled HeritageStatus values. The state field `ComAmazonawsTaxsettingsTaxsettingsState::tax_inheritance` therefore has no writer anywhere in the crate, so GetTaxInheritance can only ever report the hard-coded default `OptOut` (`DEFAULT_HERITAGE_STATUS`). The modeled success path of the operation, and the whole OptIn branch of GetTaxInheritance, are unreachable in this emulator.

The handler justifies this by asserting that no caller it serves is an Organizations management account. Whatever the justification, the observable result is an operation with a 100% failure rate and a read that can report only one of its two modeled values; an emulator that presents its single account as the (management) account of its own organization would accept the election and persist it.

## Evidence

Spec: the model binds PutTaxInheritance with input `heritageStatus` (HeritageStatus: OptIn | OptOut) and an empty output, declaring only ConflictException / InternalServerException / ResourceNotFoundException / ValidationException; the operation doc (`smithy_query(query="doc", shape="com.amazonaws.taxsettings#PutTaxInheritance")`) is "The updated tax inheritance status." and states no management-account restriction. GetTaxInheritance is modeled to report `heritageStatus`, i.e. the value the writer stores.

Handler: `src/ops/put_tax_inheritance.rs` `handle()` lines ~144-179 — after the missing-input check it unconditionally returns `ValidationException(validation_of_the_call(ERROR_CODE_FIELD_VALIDATION_FAILED, MANAGEMENT_ACCOUNT_ONLY))`; no branch writes state. `src/state.rs` `tax_inheritance` has no assignment anywhere (grep shows only the read in `src/ops/get_tax_inheritance.rs:148`).

Tests: `tests/all/fidelity_rules.rs::tax_inheritance_is_reported_as_an_enum_and_set_only_by_a_management_account` and `tests/all/tax_registration_lifecycle.rs` (~lines 1053-1072) assert the permanent 400 for both enum values, so the gap is pinned by tests rather than untested — a fixer must update those assertions.

## Steps to Reproduce

1. POST /GetTaxInheritance with {} — observe 200 {"heritageStatus":"OptOut"}.
2. POST /PutTaxInheritance with {"heritageStatus":"OptIn"}.
3. Observe 400 ValidationException (errorCode FieldValidationFailed, message "Only AWS orgs management account is allowed to set tax inheritance.") instead of a 200 empty body.
4. POST /GetTaxInheritance again — still {"heritageStatus":"OptOut"}: the election was never stored, and no request sequence can make this read report OptIn.
