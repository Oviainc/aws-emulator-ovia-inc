Issue-Source: trace-sweep-divergence
Issue-Trace: account-contact-information-validation-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: account-contact-information-validation-errors account-region-transition-conflict-errors
Issue-Shape-Id: com.amazonaws.account#Account
Issue-Key: account|PutContactInformation|4xx->4xx|$.message
Issue-Key: account|EnableRegion|4xx->4xx|$.fieldList
Issue-Key: account|EnableRegion|4xx->4xx|$.message
Issue-Key: account|EnableRegion|4xx->4xx|$.reason
Issue-Key: account|DisableRegion|4xx->4xx|$.fieldList
Issue-Key: account|DisableRegion|4xx->4xx|$.message
Issue-Key: account|DisableRegion|4xx->4xx|$.reason
Issue-Commit: 54073c01592ce4e56a3cca58cb5387afed010108

# account ValidationException refusal payloads diverge from AWS: wrong message wording, wrong reason value and missing fieldList (3 causes)

> _Note: this issue was identified against the code base at commit `54073c01592ce4e56a3cca58cb5387afed010108`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutContactInformation

## Description
Three net-new wire divergences in the AWS Account Management (sdkId `account`) service, all of them in the BODY of a 400 `ValidationException`. In all three the emulator picks the right HTTP status (400) and the right error code (`com.amazonaws.account#ValidationException`, @error client, httpError 400); what diverges is the payload the model declares for that shape — `message` (required, @sensitive), `reason` (`ValidationExceptionReason`: `invalidRegionOptTarget | fieldValidationFailed`) and `fieldList` (`ValidationExceptionFieldList`, "The field where the invalid entry was detected"). Causes 2 and 3 are the same refusal constructor reached from two operations. All three must be closed before this issue is submitted.

=== Cause 1 of 3: the "this country requires a StateOrRegion" refusal message is emulator prose instead of AWS's sentence (fixture account-contact-information-validation-errors, step task#1, PutContactInformation) ===
AWS: Smithy: `PutContactInformation.ContactInformation` required; `ContactInformation.StateOrRegion` targets `StateOrRegion` @length(min 1, max 50) @sensitive and is OPTIONAL in the model, its own doc stating "This field is required in the following countries: US, CA, GB, DE, JP, IN, and BR"; modelled errors include `ValidationException` (400). A `ContactInformation` whose `CountryCode` is one of those countries and which carries no `StateOrRegion` is refused 400 `ValidationException` with a message of the fixed form `'<MemberName>' field is required for country '<CountryCode>'` — fixed text: the single quotes, ` field is required for country `, and the trailing quote; variable parts: the modelled (Pascal-case) member name that is missing, and the country code taken verbatim from the request's `ContactInformation.CountryCode`. The recorded response for a US address with no state is `'StateOrRegion' field is required for country 'US'`. (Recorded behavior for the exact spelling; the model contributes only the member doc and the error shape.)
Emulator: the same request is refused 400 `ValidationException` with message `StateOrRegion is required in US` — a different sentence (no quoting of the member name, no `field`/`for country` wording) that AWS never emits.
Why: the member name and the country are the only machine-readable parts of this refusal for a client that reads the message (AWS's `fieldList`/`reason` are not a substitute for callers and CLIs that surface the sentence). A caller or test that classifies "you must supply a state for this country" by AWS's wording, or that parses the quoted member name out of it, cannot recognise the emulator's refusal, while the same code passes against AWS.
Fix: the refusal constructor for this rule in this crate's state module — `state_or_region_required`'s `message` (and the `fieldList` entry's `message`, which it reuses) must carry AWS's sentence with the missing member name and the request's country code interpolated. It is reached from the operation's handler before the store is touched, so no other component is involved.
Traces: account-contact-information-validation-errors#task#1; services/service-com-amazonaws-account-account/src/state.rs:320-330 (`state_or_region_required`, which builds `format!("{STATE_OR_REGION_MEMBER} is required in {country_code}")`), reached from src/ops/put_contact_information.rs:192-196 via `crate::state::validated_contact_information` (state.rs:281-293).

=== Cause 2 of 3: EnableRegion on an ENABLED_BY_DEFAULT Region reports the wrong message, the wrong `reason` value and no `fieldList` (fixture account-region-transition-conflict-errors, step task#1, EnableRegion) ===
AWS: Smithy: `EnableRegion.RegionName` required, @length(min 1, max 50); modelled errors `ValidationException` (400) and `ConflictException` (409). A Region that is enabled by default in the partition (us-east-1 in the commercial partition) is not an opt target, and AWS refuses it as a FIELD VALIDATION failure on that member: 400 `ValidationException` with `reason` = `fieldValidationFailed`, `message` of the fixed form `<RegionName> is not a valid region for opt-in or opt-out.` (fixed text ` is not a valid region for opt-in or opt-out.` including the trailing period; variable part the Region code taken verbatim from the request's `RegionName`), and `fieldList` carrying ONE entry whose `name` is the offending member (`RegionName`) and whose `message` is that same sentence. Recorded response for `RegionName=us-east-1`: message `us-east-1 is not a valid region for opt-in or opt-out.`, reason `fieldValidationFailed`, fieldList `[{"name": "RegionName", "message": "us-east-1 is not a valid region for opt-in or opt-out."}]`. (Recorded behavior for the wording; the model contributes the error shape, the reason enum and `fieldList`'s meaning.)
Emulator: 400 `ValidationException` with message `Region us-east-1 is enabled by default and cannot be enabled or disabled`, `reason` = `invalidRegionOptTarget`, and `fieldList` ABSENT — three diverging members in one body.
Why: a client cannot tell this refusal apart from the other refusal of the same member the way AWS intends. `reason` is the enum clients branch on, and AWS reserves `invalidRegionOptTarget` for a different condition than the one it reports here; `fieldList` is how a caller learns WHICH input was rejected, and it is missing entirely; and the sentence a CLI or log surfaces is one AWS never emits. Retry/branching logic and assertions keyed on any of the three break.
Fix: the shared refusal constructor both `EnableRegion` and `DisableRegion` decide through in this crate's state module — `region_not_opt_target` must emit AWS's sentence, `reason` = `fieldValidationFailed`, and a `fieldList` naming `RegionName` with the same sentence.
Traces: account-region-transition-conflict-errors#task#1; services/service-com-amazonaws-account-account/src/state.rs:1652-1665 (`region_not_opt_target`), reached from src/ops/enable_region.rs:203-205 via `crate::state::region_opt_target` (state.rs:578-584).

=== Cause 3 of 3: DisableRegion carries the identical wrong payload for the same Region class (fixture account-region-transition-conflict-errors, step task#2, DisableRegion) ===
AWS: Smithy: `DisableRegion.RegionName` required, @length(min 1, max 50); errors `ValidationException` (400), `ConflictException` (409). The opt-out path refuses an enabled-by-default Region exactly as the opt-in path does: 400 `ValidationException`, `reason` = `fieldValidationFailed`, message `<RegionName> is not a valid region for opt-in or opt-out.`, `fieldList` = one entry `name` `RegionName` with that sentence. Recorded response for `RegionName=us-east-1` is the same body as cause 2's.
Emulator: 400 `ValidationException` with message `Region us-east-1 is enabled by default and cannot be enabled or disabled`, `reason` = `invalidRegionOptTarget`, `fieldList` absent.
Why: same client impact as cause 2, on the opt-OUT surface; a caller that handles both directions sees the wrong reason enum and no field list on either.
Fix: same component as cause 2 (`region_not_opt_target`); listed as its own cause because it is a distinct replayed step and must be re-replayed to confirm the fix reaches the DisableRegion handler too (src/ops/disable_region.rs:204-206).
Traces: account-region-transition-conflict-errors#task#2.

Relation to already-open issues (neither covers these, and neither carries a wire replay key): 20261029T151445Z-account-enableregion-error-response.md reports only that this refusal leaves `fieldList` unset and says nothing about the message or the `reason` value; 20261029T185854Z-account-validationexception-messages-diverge-from-aws-in-three.md reports the region sentence from the BEHAVIORAL corpus plus two boundary-validator message dialects, and nothing about the `reason` value, the EnableRegion path, or the PutContactInformation state-or-region sentence in cause 1 (its PutContactInformation cause is the missing-required-property JSON-schema envelope, a different refusal). Closing either leaves the divergences above failing.

## Evidence
Replayed corpus simparity-wire-aws-v1 against the booted emulator: 34 steps, 31 passed, 3 mismatched (score 91.2%). All three mismatches are service `account`, verdict `mismatch`, expected status 400 == actual status 400, with diverging response bodies. No step was blocked by an unimplemented operation (stub_path empty) and none is owned by an open issue (owned_by empty).

Cause 1 — fixture `account-contact-information-validation-errors`, step `task#1`, PutContactInformation
  divergence_key: service=account|PutContactInformation|status=400->400|keys=$.message
  $.message: expected "'StateOrRegion' field is required for country 'US'"
              actual   'StateOrRegion is required in US'
  Status 400 and the ValidationException code are correct; only the message diverges.

Cause 2 — fixture `account-region-transition-conflict-errors`, step `task#1`, EnableRegion
  divergence_key: service=account|EnableRegion|status=400->400|keys=$.fieldList,$.message,$.reason
  $.fieldList: expected [{'name': 'RegionName', 'message': 'us-east-1 is not a valid region for opt-in or opt-out.'}], actual None (member absent)
  $.message:   expected 'us-east-1 is not a valid region for opt-in or opt-out.'
               actual   'Region us-east-1 is enabled by default and cannot be enabled or disabled'
  $.reason:    expected 'fieldValidationFailed', actual 'invalidRegionOptTarget'

Cause 3 — fixture `account-region-transition-conflict-errors`, step `task#2`, DisableRegion
  divergence_key: service=account|DisableRegion|status=400->400|keys=$.fieldList,$.message,$.reason
  Identical three-member divergence to cause 2 (same expected body, same actual body), reached through the same helper from the DisableRegion handler.

Emulator sources of the diverging bodies:
  services/service-com-amazonaws-account-account/src/state.rs:320-330 — `state_or_region_required` builds `format!("{STATE_OR_REGION_MEMBER} is required in {country_code}")` for both `message` and the `fieldList` entry's `message`.
  services/service-com-amazonaws-account-account/src/state.rs:1652-1665 — `region_not_opt_target` builds `format!("Region {region_name} is enabled by default and cannot be enabled or disabled")` with `reason: Some(INVALID_REGION_OPT_TARGET)` and `field_list: None`.
  services/service-com-amazonaws-account-account/src/state.rs:578-584 — `region_opt_target`, the single decision both opt operations make, selects that constructor for a `RegionKind::EnabledByDefault` Region.

Model facts used (smithy_query on services/service-com-amazonaws-account-account/smithy-model/com-amazonaws-account-account.json):
  PutContactInformation: POST /putContactInformation, input `ContactInformation` required; errors AccessDenied(403), InternalServer(500), TooManyRequests(429), ValidationException(400).
  ContactInformation.StateOrRegion: optional, @length(min 1, max 50), @sensitive, doc "This field is required in the following countries: US, CA, GB, DE, JP, IN, and BR".
  EnableRegion / DisableRegion: POST /enableRegion, /disableRegion; `RegionName` required @length(min 1, max 50); errors include ValidationException(400) and ConflictException(409).
  ValidationException: client error, httpError 400; members `message` (required, @sensitive), `reason` (ValidationExceptionReason: invalidRegionOptTarget | fieldValidationFailed), `fieldList` (list of ValidationExceptionField{name required, message required}) — "The field where the invalid entry was detected."

## Steps to Reproduce
Boot the emulator with the account service mounted and replay the two fixtures below (or issue the equivalent requests against the data listener with an `account` client for region us-east-1; disable client-side parameter validation where a modelled constraint is deliberately violated).

Cause 1 — fixture account-contact-information-validation-errors, step task#1:
1. POST /putContactInformation with a ContactInformation that satisfies every modelled constraint (FullName, AddressLine1, City, PostalCode, CountryCode='US', PhoneNumber with a leading '+') but OMITS StateOrRegion.
2. Observe 400 ValidationException with message `StateOrRegion is required in US`; AWS answers `'StateOrRegion' field is required for country 'US'`.
3. Repeat with CountryCode in {CA, GB, DE, JP, IN, BR} — the same constructor, so the same wrong sentence.

Causes 2 and 3 — fixture account-region-transition-conflict-errors, steps task#1 and task#2:
1. GET/POST /getRegionOptStatus with RegionName='us-east-1' to confirm the precondition ENABLED_BY_DEFAULT.
2. POST /enableRegion with {"RegionName": "us-east-1"} — observe 400 ValidationException with message `Region us-east-1 is enabled by default and cannot be enabled or disabled`, reason `invalidRegionOptTarget` and NO fieldList, where AWS answers message `us-east-1 is not a valid region for opt-in or opt-out.`, reason `fieldValidationFailed` and fieldList [{name: 'RegionName', message: same sentence}].
3. POST /disableRegion with {"RegionName": "us-east-1"} — the identical wrong body, since both handlers decide through crate::state::region_opt_target.
4. Controls that must keep passing: /enableRegion with an opt-in Region (e.g. af-south-1) still succeeds, and /enableRegion with a nonexistent Region (e.g. zz-nowhere-1) still answers reason `fieldValidationFailed` with a fieldList naming RegionName.

Replay to confirm: account-contact-information-validation-errors#task#1, account-region-transition-conflict-errors#task#1, account-region-transition-conflict-errors#task#2.
