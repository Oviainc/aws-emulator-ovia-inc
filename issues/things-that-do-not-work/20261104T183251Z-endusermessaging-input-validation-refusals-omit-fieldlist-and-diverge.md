Issue-Source: trace-sweep-divergence
Issue-Trace: endusermessaging-create-brand-profile-from-registration-rejections
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: endusermessaging-create-brand-profile-from-registration-rejections endusermessaging-notify-code-configuration-lifecycle endusermessaging-brand-profile-lifecycle endusermessaging-brand-profile-idempotent-create
Issue-Shape-Id: com.amazonaws.endusermessaging#EndUserMessaging
Issue-Key: endusermessaging|CreateBrandProfileFromRegistration|4xx->4xx|$.fieldList
Issue-Key: endusermessaging|CreateBrandProfileFromRegistration|4xx->4xx|$.message
Issue-Key: endusermessaging|CreateNotifyCodeConfiguration|4xx->4xx|$.fieldList
Issue-Key: endusermessaging|CreateNotifyCodeConfiguration|4xx->4xx|$.message
Issue-Key: endusermessaging|GetBrandProfile|4xx->4xx|$.fieldList
Issue-Key: endusermessaging|GetBrandProfile|4xx->4xx|$.message
Issue-Key: endusermessaging|GetBrandProfile|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: endusermessaging|GetBrandProfile|4xx->4xx|status
Issue-Key: endusermessaging|CreateBrandProfile|4xx->2xx|$.message
Issue-Key: endusermessaging|CreateBrandProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: endusermessaging|CreateBrandProfile|4xx->2xx|status
Issue-Commit: e5b2e36fc94a41937ab22fe33196324420972c76

# endusermessaging input-validation refusals omit fieldList and diverge in template, status and idempotency checks

> _Note: this issue was identified against the code base at commit `e5b2e36fc94a41937ab22fe33196324420972c76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBrandProfileFromRegistration

## Description
Four root causes in how endusermessaging refuses bad input, all observed replaying the wire corpus simparity-wire-aws-v1 against the running (bootstrapped) emulator. No open endusermessaging issue covers any of them: the two open stub issues cover unimplemented operations (CreateBrandProfileAttributes, CreateNotifyCodeConfiguration) and the open behavioral issue '20261104T173700Z-endusermessaging-brand-profiles-2-net-new-causes-createbrandprofile.md' is about attributesCreated and the conflict/not-found message templates, not about the validation envelope, the id-format gate or the idempotency check; it also carries no wire replay key, and the replay report shows all four steps below unowned.

=== Cause 1 of 4: the modelled-constraint ValidationException omits fieldList and re-words AWS's message envelope (pattern constraint) ===
AWS: Smithy model ValidationException (HTTP 400, @error client; members message @required and fieldList -> ValidationExceptionFieldList of ValidationExceptionField{path @required, message @required}, documented as 'A list of specific failures encountered while validating the input'). CreateBrandProfileFromRegistration input member registrationId targets RegistrationIdOrArn with @pattern ^[A-Za-z0-9_:/-]+$ and @length 1..256. For a member that violates a modelled constraint AWS answers 400 ValidationException with message '<N> validation error detected. Value at '<json-pointer>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>' and populates fieldList with one entry per failure: path is the JSON pointer to the member ('/registrationId') and message is that same per-field sentence. Fixed text of the template: '<N> validation error detected. Value at ', "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; the variable parts are the failure count, the member's JSON pointer and the pattern from the model. The offending value is NOT echoed.
Emulator: answers 400 (correct status) but with message "1 validation error detected: Value 'not a valid registration id!!' at 'registrationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_:/-]+$" — a colon instead of a period after 'detected', the rejected value interpolated, the path rendered as a bare dotted member name instead of a JSON pointer — and omits fieldList entirely.
Why: a client or console that reads fieldList to point a user at the offending field gets nothing and must fall back to parsing prose; a client matching AWS's envelope (period, no echoed value, '/member' pointer) never matches, and the echoed value leaks the submitted (potentially sensitive) input into the summary message where AWS does not put it.
Fix: the service's shared input-constraint validation / ValidationException renderer — it must build AWS's message template and emit a ValidationExceptionField per failed constraint with a JSON-pointer path.
Traces: endusermessaging-create-brand-profile-from-registration-rejections#task#1.

=== Cause 2 of 4: the same envelope defect for @range constraints, with the wrong constraint sentence ===
AWS: Smithy model CodeConfigurationParameters.codeLength targets CodeLength with @range min 4 max 8 ('The number of characters in the one-time passcode. Valid values range from 4 through 8'); CreateNotifyCodeConfiguration declares ValidationException (400). For a range violation AWS's per-field sentence is 'Value at '/codeConfigurationParameters/codeLength' failed to satisfy constraint: Member must be between 4 and 8, inclusive' — the two-sided 'Member must be between <min> and <max>, inclusive' wording, a nested JSON-pointer path, and the same '1 validation error detected. ' prefix plus a fieldList entry.
Emulator: answers 400 with "1 validation error detected: Value '9' at 'codeConfigurationParameters.codeLength' failed to satisfy constraint: Member must have value less than or equal to 8" and no fieldList: a one-sided constraint sentence AWS does not use for a two-sided @range, a dotted path instead of a JSON pointer, and the value echoed.
Why: a client that classifies the refusal by constraint sentence, or shows the valid range to a user, gets neither the documented 4..8 range wording nor a machine-readable field entry.
Fix: the same shared validation renderer — range constraints must render AWS's inclusive two-sided sentence and the nested member's JSON pointer, in both message and fieldList.
Traces: endusermessaging-notify-code-configuration-lifecycle#task#4.

=== Cause 3 of 4: a syntactically invalid brandProfileId is looked up instead of validated, so the refusal is 404 ResourceNotFoundException instead of 400 ValidationException ===
AWS: GetBrandProfile input brandProfileId is an @httpLabel targeting BrandProfileIdOrArn (@pattern ^[A-Za-z0-9_:/-]+$, @length 1..256) and the operation declares both ValidationException (400) and ResourceNotFoundException (404). A value that satisfies the member pattern but is not a well-formed brand-profile identifier is rejected by the service's own id-format gate BEFORE any store lookup: 400 ValidationException with message 'invalid brand profile ID format: <value>' and fieldList [{path: 'brandProfileId', message: 'invalid brand profile ID format: <value>'}] — fixed text 'invalid brand profile ID format: ' plus the identifier taken verbatim from the request path; note this service-level gate reports the bare member name as path (recorded behavior), unlike the modelled-constraint envelope in causes 1-2.
Emulator: skips the format gate and treats the value as a possible key, so it answers 404 ResourceNotFoundException with x-amzn-errortype ResourceNotFoundException and message 'Brand profile bp-sim2real-probe-eum-missing-2fa644d9 was not found.'
Why: a client cannot distinguish 'you sent a malformed id' (non-retryable, fix the request) from 'the resource is gone' (retry/recreate); SDK retry and error-class branching differ between 400 ValidationException and 404 ResourceNotFoundException, so a caller that treats 404 as a deletion race loops on a request AWS would reject outright.
Fix: GetBrandProfile's validation, ahead of the state lookup — validate the brandProfileId's format (and apply the same gate wherever BrandProfileIdOrArn is accepted) and raise ValidationException with the id-format message and fieldList entry before resolving it against the store.
Traces: endusermessaging-brand-profile-lifecycle#task#5.

=== Cause 4 of 4: a reused clientToken with different parameters is served the original profile (200) instead of the idempotency-mismatch refusal (400) ===
AWS: CreateBrandProfile input clientToken carries @idempotencyToken; the operation's modelled client errors include ValidationException (400) and ServiceQuotaExceededException (400). Replaying the SAME clientToken with parameters that differ from the request it first served is refused with HTTP 400 and the fixed message 'The idempotency token and parameters presented do not match' (the recording asserts status 400 and that exact message; x-amzn-errortype is recorded but not asserted, so the error code is whichever modelled 400 client error the service uses). Only a replay whose parameters match is served the original resource.
Emulator: the handler's idempotency path looks the token up and, if a profile is recorded for it, returns that profile verbatim without comparing the request parameters — so the mismatched replay answers 200 with the first profile's body and no message/x-amzn-errortype at all.
Why: the whole point of the token is to detect that a caller reused it for a different request; a client that mutates its request but reuses the token silently receives a profile with the OLD name/settings and believes its new parameters were applied, instead of being told to mint a fresh token.
Fix: CreateBrandProfile's handler/idempotency bookkeeping — record the request fingerprint alongside the token and refuse a replay whose parameters differ with the 400 idempotency-mismatch error, serving the stored profile only on an exact match.
Traces: endusermessaging-brand-profile-idempotent-create#task#2.

## Evidence
Cause 1 — fixture endusermessaging-create-brand-profile-from-registration-rejections step task#1 (CreateBrandProfileFromRegistration, 400 vs 400):
  $.message expected: "1 validation error detected. Value at '/registrationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_:/-]+$"
  $.message actual:   "1 validation error detected: Value 'not a valid registration id!!' at 'registrationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_:/-]+$"
  $.fieldList expected: [{'path': '/registrationId', 'message': "Value at '/registrationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_:/-]+$"}]; actual: missing.
  Fixed text vs values: 'N validation error detected. Value at ' / "' failed to satisfy constraint: Member must satisfy regular expression pattern: " are fixed; '/registrationId' is the member's JSON pointer and ^[A-Za-z0-9_:/-]+$ is the model's pattern. The registrationId string shown is this recording's request value.

Cause 2 — fixture endusermessaging-notify-code-configuration-lifecycle step task#4 (CreateNotifyCodeConfiguration, 400 vs 400):
  $.message expected: "1 validation error detected. Value at '/codeConfigurationParameters/codeLength' failed to satisfy constraint: Member must be between 4 and 8, inclusive"
  $.message actual:   "1 validation error detected: Value '9' at 'codeConfigurationParameters.codeLength' failed to satisfy constraint: Member must have value less than or equal to 8"
  $.fieldList expected: [{'path': '/codeConfigurationParameters/codeLength', 'message': "Value at '/codeConfigurationParameters/codeLength' failed to satisfy constraint: Member must be between 4 and 8, inclusive"}]; actual: missing.
  4 and 8 are the model's @range bounds on CodeLength (not recording-specific); '9' is this request's submitted value, which AWS does not echo.

Cause 3 — fixture endusermessaging-brand-profile-lifecycle step task#5 (GetBrandProfile):
  status expected 400, got 404.
  $.message expected: 'invalid brand profile ID format: bp-sim2real-probe-eum-missing-2fa644d9'; actual: 'Brand profile bp-sim2real-probe-eum-missing-2fa644d9 was not found.'
  $.fieldList expected: [{'path': 'brandProfileId', 'message': 'invalid brand profile ID format: bp-sim2real-probe-eum-missing-2fa644d9'}]; actual: missing.
  $headers.x-amzn-errortype: shape_mismatch — emulator sent 'ResourceNotFoundException'.
  Fixed text: 'invalid brand profile ID format: '; the id after it comes from the request path.

Cause 4 — fixture endusermessaging-brand-profile-idempotent-create step task#2 (CreateBrandProfile):
  status expected 400, got 200.
  $.message expected: 'The idempotency token and parameters presented do not match' (entirely fixed text); actual: absent.
  $headers.x-amzn-errortype expected present (shape not asserted); actual: absent.
  Source confirming the cause: services/service-com-amazonaws-endusermessaging-endusermessaging/src/ops/create_brand_profile.rs:188-199 — the token lookup returns output_for(existing) with no comparison of the incoming parameters.

## Steps to Reproduce
Cause 1:
1. POST /v1/brand-profiles/create-from-registration with {"registrationId": "not a valid registration id!!", "brandProfileName": "<any valid name>"}.
2. The 400 body's message uses ':' after 'detected', echoes the value and reports 'registrationId'; fieldList is absent. AWS: '1 validation error detected. Value at '/registrationId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_:/-]+$' plus the matching fieldList entry.

Cause 2:
1. POST /v1/notify-code-configurations with {"notifyCodeConfigurationName": "<any valid name>", "codeConfigurationParameters": {"codeLength": 9}}.
2. The 400 message says 'Member must have value less than or equal to 8' with a dotted path and no fieldList; AWS says 'Member must be between 4 and 8, inclusive' at path '/codeConfigurationParameters/codeLength' and populates fieldList.

Cause 3:
1. GET /v1/brand-profiles/bp-sim2real-probe-eum-missing-2fa644d9 (pattern-legal but not a well-formed brand-profile id).
2. The emulator answers 404 ResourceNotFoundException 'Brand profile <id> was not found.'; AWS answers 400 ValidationException 'invalid brand profile ID format: <id>' with fieldList [{path: 'brandProfileId', ...}].

Cause 4:
1. POST /v1/brand-profiles with {"brandProfileName": "<name-A>", "clientToken": "<token>"} -> 200.
2. POST /v1/brand-profiles again with the SAME clientToken but different parameters (e.g. "<name-B>" and/or deletionProtectionEnabled flipped).
3. The emulator answers 200 with the first profile; AWS answers 400 with message 'The idempotency token and parameters presented do not match'.

Replay: wire corpus simparity-wire-aws-v1, fixtures endusermessaging-create-brand-profile-from-registration-rejections#task#1, endusermessaging-notify-code-configuration-lifecycle#task#4, endusermessaging-brand-profile-lifecycle#task#5, endusermessaging-brand-profile-idempotent-create#task#2.
