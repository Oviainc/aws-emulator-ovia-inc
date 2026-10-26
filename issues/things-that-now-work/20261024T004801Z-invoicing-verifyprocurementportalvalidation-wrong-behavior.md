Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# A rejected VerifyProcurementPortalValidation code still advances the preference's Version and LastUpdateDate

_Filed: 2026-10-24T00:48:01Z_
_Operation: VerifyProcurementPortalValidation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the submitted `Code` is not the outstanding issued code (today: always, since no code is ever issued), the handler refuses with 400 ValidationException — but first it writes the preference: `failed_verification_attempts += 1`, `preference.version += 1`, `preference.last_update_date = now()`. That write persists and is observable through GetProcurementPortalPreference / ListProcurementPortalPreferences.

Expected: a rejected code leaves the resource's reported configuration version untouched; only a configuration write (create / put / status update) advances `Version` and `LastUpdateDate`. A failed-attempt counter may be tracked internally (AWS reports it in the refusal text, "Attempt 1 of 3 failed"), but it is not a configuration version bump.

Actual: each wrong-code submission bumps Version by one and rewrites LastUpdateDate, so a client retrying an OTP three times sees Version move by three with no configuration change.

## Evidence

Handler: services/service-com-amazonaws-invoicing-invoicing/src/ops/verify_procurement_portal_validation.rs, `handle` lines ~237-259: on `outstanding != Some(input.code)` it does `stored.failed_verification_attempts = ...saturating_add(1); stored.preference.version = stored.preference.version.saturating_add(1); stored.preference.last_update_date = crate::state::now();` before returning `Err(...ValidationException { message: INVALID_CODE_MESSAGE, reason: "invalidInput" })`.

Spec/recording grounding: `ProcurementPortalPreference$Version` is documented as the version of the preference CONFIGURATION (smithy_query(query="shape", shape="ProcurementPortalPreference")). The recorded wire fixture quoted in the fixed issue 20261023T095223Z-invoicing-procurement-portal-preference-lifecycle-updates-and-deletes shows the Get at task#9 — after the refused send (task#6) and the wrong-code verify (task#7) — still expecting `$.ProcurementPortalPreference.Version: 2`, the value the preceding Put left.

Test coverage: tests/all/procurement_portal_preference_lifecycle.rs:802-819 exercises wrong-code verifies but asserts only status/code, so no test pins Version across a rejected verify. (The missing `Message`/attempt-counter text of this same refusal is already tracked by the open issue 20261023T095223Z-invoicing-procurement-portal-error-bodies-omit-the-message; this issue is about the state mutation, not the message.)

## Steps to Reproduce

1. CreateProcurementPortalPreference for a fresh identity; note the ARN. GetProcurementPortalPreference shows Version 1.
2. Call VerifyProcurementPortalValidation on that ARN with Code="000000" three times; each answers 400 ValidationException.
3. Call GetProcurementPortalPreference. Observed: Version 4 and a LastUpdateDate from the last rejected attempt. Expected: Version still 1 and LastUpdateDate unchanged — a rejected code writes no configuration.
