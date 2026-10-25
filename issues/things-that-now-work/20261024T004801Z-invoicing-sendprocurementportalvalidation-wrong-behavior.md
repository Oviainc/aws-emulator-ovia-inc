Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# A refused SendProcurementPortalValidation still advances the preference's Version and LastUpdateDate

_Filed: 2026-10-24T00:48:01Z_
_Operation: SendProcurementPortalValidation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

SendProcurementPortalValidation always refuses with 400 ValidationException (delivery of the test e-invoice cannot be confirmed), but before returning that refusal it WRITES the preference: it increments `unconfirmed_validation_attempts`, does `preference.version = version + 1` and sets `preference.last_update_date = now()`. The write persists, so a later GetProcurementPortalPreference / ListProcurementPortalPreferences reports a Version (and LastUpdateDate) that moved even though no configuration of the resource changed and the call answered 4xx.

Expected: the refused call changes nothing a reader can observe — `Version` is documented as "the version number of the procurement portal preference configuration", so a call that wrote no configuration and answered 400 must leave Version and LastUpdateDate exactly as the last successful write left them (a create leaves 1; a create followed by one Put leaves 2, and any number of refused sends keeps it at 2).

Actual: every send attempt bumps Version by one and rewrites LastUpdateDate, so N refused sends leave the resource reporting Version = last_successful_writes + N. Because the operation can only refuse today, a client that polls Version to detect whether its own write landed sees the counter drift on calls that did nothing.

## Evidence

Handler: services/service-com-amazonaws-invoicing-invoicing/src/ops/send_procurement_portal_validation.rs — in `handle`, lines ~250-271: `stored.unconfirmed_validation_attempts = ...saturating_add(1); stored.preference.version = stored.preference.version.saturating_add(1); stored.preference.last_update_date = attempted_at;` and then `Err(...ValidationException(test_einvoice_delivery_not_confirmed(..)))`. The design is restated in services/service-com-amazonaws-invoicing-invoicing/src/state.rs module docs ("Every call that WRITES a preference advances its `Version` and `LastUpdateDate`, including the two validation calls").

Spec/recording grounding: the model declares `ProcurementPortalPreference$Version` (required Long, "the version number of the procurement portal preference configuration") and `LastUpdateDate` (required) — smithy_query(query="shape", shape="ProcurementPortalPreference"). The already-fixed issue 20261023T095223Z-invoicing-procurement-portal-preference-lifecycle-updates-and-deletes quotes the recorded wire fixture `invoicing-procurement-portal-preference-lifecycle`: the Get at task#5 (right after the Put) and the Get at task#9 (AFTER the refused SendProcurementPortalValidation at task#6 and the refused VerifyProcurementPortalValidation at task#7) are one cluster with the SAME expected detail, `$.ProcurementPortalPreference.Version: expected 2` — i.e. real AWS did not advance Version across the two refused validation calls.

Test coverage: services/service-com-amazonaws-invoicing-invoicing/tests/all/procurement_portal_preference_lifecycle.rs:767-782 asserts the OPPOSITE ("expected Version {before+2} after two attempts"), so this emulator-authored expectation has to change with the handler; no test asserts AWS's behaviour.

## Steps to Reproduce

1. CreateProcurementPortalPreference for a fresh portal/buyer/supplier identity; note the returned ARN.
2. PutProcurementPortalPreference on that ARN (any legal configuration) and GetProcurementPortalPreference — Version is 2, LastUpdateDate is the Put's instant.
3. Call SendProcurementPortalValidation on the ARN twice; both answer 400 ValidationException.
4. Call GetProcurementPortalPreference again. Observed: Version 4 and a LastUpdateDate from the refused sends. Expected (per the recorded lifecycle fixture): Version still 2 and LastUpdateDate unchanged, because neither refused call wrote any configuration.
