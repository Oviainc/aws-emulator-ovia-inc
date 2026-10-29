Issue-Source: trace-sweep-divergence
Issue-Trace: taxsettings-tax-inheritance-toggle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: taxsettings-tax-inheritance-toggle taxsettings-tax-registration-lifecycle
Issue-Shape-Id: com.amazonaws.taxsettings#TaxSettings
Issue-Key: tax|PutTaxInheritance|4xx->2xx|$
Issue-Key: tax|PutTaxInheritance|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: tax|PutTaxInheritance|4xx->2xx|status
Issue-Key: tax|GetTaxRegistration|2xx->2xx|$.taxRegistration.status
Issue-Commit: d220b5129ac92467b2bf094da95d24c404aff436

# tax: two AWS outcomes the emulator can never produce — the PutTaxInheritance management-account refusal and the Rejected TRN validation status

> _Note: this issue was identified against the code base at commit `d220b5129ac92467b2bf094da95d24c404aff436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTaxInheritance,GetTaxRegistration

## Description
Two net-new root causes in the implemented taxsettings paths. They share one fault: an outcome the real service produces is structurally unreachable in the emulator — one handler has no gate that can ever refuse, and one state machine has only one terminal value — so a client that branches on either outcome never sees it. Both must be closed before this issue is submitted.

Cause 1 of 2 — PutTaxInheritance has no AWS Organizations management-account gate (fixture taxsettings-tax-inheritance-toggle, step task#2; same cluster also covers task#3).
AWS: setting the tax-inheritance election is reserved to the organization's management account. A caller that is not that account is refused with the operation's modeled ValidationException — HTTP 400, header x-amzn-errortype: ValidationException, body errorCode 'FieldValidationFailed' and message 'Only AWS orgs management account is allowed to set tax inheritance.', with fieldList absent. That message is entirely fixed text for this error class: it names no request value, no account id and no limit, so there is no variable part to substitute. Smithy: PutTaxInheritance input is heritageStatus (HeritageStatus: OptIn | OptOut) only, so the standing being judged is the CALLER's, not a request member; the operation models ValidationException (HTTP 400) whose errorCode member targets ValidationExceptionErrorCode, which publishes FieldValidationFailed. The recorded response supplies the status, the errorCode and the message.
Emulator: every PutTaxInheritance call is accepted. services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_tax_inheritance.rs validates only that heritageStatus is present and then replaces state.tax_inheritance unconditionally, answering 200 with the empty modeled output; the replay shows status expected 400, got 200, an empty body where the ValidationException document was expected, and no x-amzn-errortype header at all. The state doc comment (src/state.rs, the tax_inheritance field) states the design explicitly: the single presented account is assumed to BE the organization's management account and 'there is nothing here to tell one caller's standing from another's', so the refusal can never be produced. Hypothesis: the gate was dropped wholesale when the earlier over-refusal was fixed (resolved issue 20261024T151653Z-taxsettings-puttaxinheritance-wrong-behavior, 'PutTaxInheritance can never succeed'); the correct shape is to refuse unless the caller is the management account, not to accept unconditionally.
Why: a client that must handle 'you are not the management account' — a console flow, an onboarding check, or any code that catches ValidationException/FieldValidationFailed to tell the operator which account to use — sees a 200 instead, concludes the election was recorded, and the emulator then reports that election back from GetTaxInheritance. The divergence is worse than a missing error: a write AWS refused is persisted, so subsequent reads disagree with AWS as well.
Fix: PutTaxInheritance's handler/validation in services/service-com-amazonaws-taxsettings-taxsettings/src/ops/put_tax_inheritance.rs, plus the state's notion of whether the presented account is the organization's management account (src/state.rs, the tax_inheritance field and the shared validation() helper with ERROR_CODE_FIELD_VALIDATION_FAILED). The refusal must be decided BEFORE anything is stored, so a refused call leaves the previously recorded election untouched.
Traces: taxsettings-tax-inheritance-toggle#task#2 (representative) and taxsettings-tax-inheritance-toggle#task#3.

Cause 2 of 2 — a stored tax registration can never settle to Rejected; TRN validation always succeeds (fixture taxsettings-tax-registration-lifecycle, step task#2).
AWS: PutTaxRegistration accepts an entry and reports it Pending, and the TRN validation that follows is asynchronous and can FAIL: the registration then reads back with status Rejected. Smithy: TaxRegistrationStatus is an enum with values Verified | Pending | Deleted | Rejected, and PutTaxRegistration's output status member is documented as the status after processing, 'Based on the validation occurring on the TRN, the status can be Verified, Pending or Rejected' — so Rejected is a terminal outcome of validating a registration the write already accepted with 200, not an alternative to the 200. GetTaxRegistration returns that registration (taxRegistration.status) and is the read a client polls. The recording polls GetTaxRegistration for taxRegistration.status == 'Rejected' with 'Verified' as a fail-on value, i.e. it records a registration whose TRN validation failed after an accepted write.
Emulator: the settled status is a compile-time constant. services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs defines SETTLED_REGISTRATION_STATUS = STATUS_VERIFIED and documents STATUS_REJECTED as 'Never reached' on the premise that anything the write accepted must verify; settle_due()/StoredTaxRegistration::settle() therefore move every Pending record to Verified after TRN_VALIDATION_DELAY, and GetTaxRegistration (src/ops/get_tax_registration.rs) reports that. The replay shows $.taxRegistration.status expected 'Rejected', got 'Verified', and the poll terminating because 'Verified' is in fail_on after 2 attempts. Note this is the mirror image of the resolved issue 20261024T170536Z-taxsettings-gettaxregistration-wrong-behavior ('Every stored TRN reads back status Rejected ... Verified is unreachable'): that fix made Verified the only reachable terminal status instead of making both reachable, so the state machine still has exactly one outcome.
Why: the Pending -> Rejected path is the whole reason a client polls GetTaxRegistration. A client that polls until a terminal status and then branches (re-submit a corrected TRN on Rejected, proceed on Verified) takes the success branch for a registration AWS rejected, and any test that asserts the rejection path cannot be written against the emulator at all. A poll loop using Verified as a failure sentinel, as the recording does, terminates on the wrong value.
Fix: the registration state machine in services/service-com-amazonaws-taxsettings-taxsettings/src/state.rs — the settled-status decision (SETTLED_REGISTRATION_STATUS / StoredTaxRegistration::settle / ComAmazonawsTaxsettingsTaxsettingsState::settle_due) must be able to resolve an accepted, stored registration to Rejected as well as Verified, with the outcome fixed by the write (so every reader settles one record identically) rather than hard-coded to one value. The 200/Pending answer of PutTaxRegistration is unchanged: acceptance by the write and the outcome of validation are separate facts.
Traces: taxsettings-tax-registration-lifecycle#task#2.

## Evidence
Replay of trace set simparity-wire-ga-prod-v1 (4 fixtures, 9 steps, 6 passed, 3 mismatched, score 66.7%); emulator up (/__admin/health 200).

Cause 1 — fixture taxsettings-tax-inheritance-toggle, step task#2, operation PutTaxInheritance, verdict mismatch, affected_count 2 (task#2, task#3):
  status: expected 400, got 200
  $: empty_body (expected {'errorCode': 'FieldValidationFailed', 'fieldList': None, 'message': 'Only AWS orgs management account is allowed to set tax inheritance.'}, got {})
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)
Expected: HTTP 400 + x-amzn-errortype: ValidationException + errorCode 'FieldValidationFailed' + the fixed message 'Only AWS orgs management account is allowed to set tax inheritance.' (no variable parts) + fieldList absent.
Actual: HTTP 200 with the empty PutTaxInheritanceOutput document, no error header, and the submitted heritageStatus persisted into state.tax_inheritance.

Cause 2 — fixture taxsettings-tax-registration-lifecycle, step task#2, operation GetTaxRegistration, verdict mismatch, affected_count 1:
  $.taxRegistration.status: value_mismatch (expected 'Rejected', got 'Verified')
  $.taxRegistration.status: poll: 'Verified' is in fail_on after 2 attempt(s) (expected 'Rejected', got 'Verified')
Expected: the polled registration reaches terminal status 'Rejected' (TaxRegistrationStatus enum: Verified | Pending | Deleted | Rejected).
Actual: HTTP 200 with taxRegistration.status 'Verified' on every attempt; src/state.rs pins SETTLED_REGISTRATION_STATUS = STATUS_VERIFIED and declares STATUS_REJECTED unreachable, so no stored record can ever report it.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-ga-prod-v1 wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture taxsettings-tax-inheritance-toggle and inspect step task#2 (and task#3) — PutTaxInheritance with a heritageStatus from a caller that is not the organization's management account. The emulator answers 200 with an empty body instead of 400 ValidationException / x-amzn-errortype: ValidationException / errorCode FieldValidationFailed / message 'Only AWS orgs management account is allowed to set tax inheritance.'; a following GetTaxInheritance then reports the election that AWS never recorded.
3. Cause 2: replay fixture taxsettings-tax-registration-lifecycle and inspect step task#2 — PutTaxRegistration is accepted (200, Pending) and GetTaxRegistration is polled for taxRegistration.status. After the settle delay the emulator reports 'Verified' on every attempt, so the poll's fail_on value is hit and the recorded terminal status 'Rejected' is never reached.
