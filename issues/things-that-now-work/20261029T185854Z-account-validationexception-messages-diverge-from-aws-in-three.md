Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: account.contact-information-missing-required-field-rejected account.list-regions-invalid-max-results-rejected account.disable-region-enabled-by-default-conflict-rejected
Issue-Shape-Id: com.amazonaws.account#Account
Issue-Key: account.contact-information-missing-required-field-rejected|run#12|AssertionError|assert message == '[object has missing required properties (["PostalCode"])]', message
Issue-Key: account.list-regions-invalid-max-results-rejected|run#8|AssertionError|assert over == ("ValidationException", over_msg), over
Issue-Key: account.disable-region-enabled-by-default-conflict-rejected|run#6|AssertionError|assert message == "us-east-1 is not a valid region for opt-in or opt-out.", message
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# account: ValidationException MESSAGES diverge from AWS in three places — constraint refusals use the generic "1 validation error detected" envelope instead of Account's JSON-schema wording, and the enabled-by-default Region refusal is emulator prose (3 causes)

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutContactInformation

## Description
Three net-new root causes in the AWS Account Management service's refusal wording. In all three the emulator picks the right HTTP status (400) and the right error code (`ValidationException`, Smithy: `com.amazonaws.account#ValidationException`, @error client, httpError 400) — what diverges is the `message` member, which the model marks `required` and which clients, CLIs and assertion suites read verbatim. Causes 1 and 2 share one component (the message rendering of the centralized boundary constraint validator for this service); cause 3 is a hand-written refusal string in this crate's state module. All three must be closed.

=== Cause 1 of 3: a missing REQUIRED member is reported with the generic "1 validation error detected" envelope instead of Account's JSON-schema wording (check: account.contact-information-missing-required-field-rejected) ===
AWS: `PutContactInformation` takes `ContactInformation` (Smithy: `PutContactInformation.ContactInformation` required; `ContactInformation.PostalCode` required, @length min 1 max 20, @sensitive). The Account Management API front end validates the request body against the shape's JSON schema, so a body omitting a required property is refused 400 `ValidationException` with a message of the fixed form `[object has missing required properties ([<quoted property names>])]` — the recorded response for a body whose `contactInformation` lacks `postalCode` is exactly `[object has missing required properties (["PostalCode"])]`. Fixed text: the brackets, `object has missing required properties (`, `[`, `]`, `)`. Variable parts: the list of property names that the request omitted, each double-quoted and comma-separated, spelled with the member's modelled (Pascal-case) name and with NO member path prefix. (Recorded behavior for the exact spelling; the model states only that the member is required and that `message` is required/@sensitive.)
Emulator: the same request is refused 400 `ValidationException` with `1 validation error detected: Value null at 'contactInformation.postalCode' failed to satisfy constraint: Member must not be null` — the generic cross-service constraint-validator envelope, with a lower-camel member PATH rather than Account's JSON-schema sentence.
Why: a client or test that matches on the refusal message (the overwhelmingly common way to distinguish "you left a field out" from "the field's value is bad" on this API, since `reason`/`fieldList` are not populated for schema failures) sees a message that never occurs on real AWS. Anything parsing the bracketed JSON-schema text — or asserting on it — fails against the emulator while passing against AWS.
Fix: the message rendering of the request-boundary constraint validator as it is applied to this service (the centralized deserialize→validate walk invoked before `handle`; see the handler's own note that "every modelled constraint on this input … was already refused at the boundary"). Account needs the JSON-schema message dialect for required-member/constraint failures, not the shared `N validation error(s) detected: …` dialect.
Traces: account.contact-information-missing-required-field-rejected; services/service-com-amazonaws-account-account/src/ops/put_contact_information.rs:185-196 (the handler documents that the boundary already refused the required members, so no handler-local message exists).

=== Cause 2 of 3: an out-of-@range MaxResults is reported with the same generic envelope instead of Account's JSON-schema numeric wording (check: account.list-regions-invalid-max-results-rejected) ===
AWS: Smithy: `ListRegions.MaxResults` targets `Integer` with @range(min: 1, max: 50); modelled errors include `ValidationException` (400). A value above the maximum is refused 400 `ValidationException` with the fixed form `[numeric instance is greater than the required maximum (maximum: <max>, found: <value>)]`, and a value below the minimum with `[numeric instance is lower than the required minimum (minimum: <min>, found: <value>)]`. Fixed text: everything outside the parentheses plus the `maximum: `/`minimum: `/`found: ` labels. Variable parts: `<max>`/`<min>` taken from the member's modelled range (50 and 1 here) and `<value>` taken from the request. Recorded responses: MaxResults=51 -> `[numeric instance is greater than the required maximum (maximum: 50, found: 51)]`; MaxResults=0 -> `[numeric instance is lower than the required minimum (minimum: 1, found: 0)]`.
Emulator: MaxResults=51 is refused 400 `ValidationException` with `1 validation error detected: Value '51' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50` — again the shared generic dialect. (The boundary-valid MaxResults=50 positive control in the same check passes, so only the refusal wording diverges.)
Why: same client impact as cause 1, on the paging surface: a caller that classifies refusals by message text, or a harness pinned to AWS's wording, cannot recognise an out-of-range page size. It also leaks a different member spelling (`maxResults` with a quoted value) than AWS ever emits for this API.
Fix: same component as cause 1 — the boundary constraint validator's message rendering for this service, for the @range class specifically (greater-than-maximum and lower-than-minimum are two separate templates).
Traces: account.list-regions-invalid-max-results-rejected; services/service-com-amazonaws-account-account/src/ops/list_regions.rs:107-112 (MAX_PAGE_SIZE's doc comment records that "the boundary validator has already refused a page size outside that range by the time this handler runs").

=== Cause 3 of 3: DisableRegion on an ENABLED_BY_DEFAULT Region is refused with emulator prose instead of AWS's "not a valid region for opt-in or opt-out" message (check: account.disable-region-enabled-by-default-conflict-rejected) ===
AWS: Smithy: `DisableRegion.RegionName` required, @length min 1 max 50; modelled errors `ValidationException` (400) and `ConflictException` (409). For a Region that is ENABLED_BY_DEFAULT (us-east-1 in the commercial partition) the call fails INPUT VALIDATION, not a state conflict: 400 `ValidationException` with message of the fixed form `<RegionName> is not a valid region for opt-in or opt-out.` — recorded as `us-east-1 is not a valid region for opt-in or opt-out.` Fixed text: ` is not a valid region for opt-in or opt-out.` including the trailing period. Variable part: the Region code, taken verbatim from the request's `RegionName`. (Recorded behavior for the wording; the model contributes only the error shape and the `invalidRegionOptTarget` reason value.)
Emulator: 400 `ValidationException` with `Region us-east-1 is enabled by default and cannot be enabled or disabled` — a different sentence (leading `Region `, different predicate, no trailing period) that AWS never emits. The status, code and the unchanged opt status are all correct; only the message diverges. Note this is NOT the already-open issue 20261029T151445Z-account-enableregion-error-response.md, which is about the same refusal leaving `fieldList` unset; that issue says nothing about the message text and closing it would not fix this.
Why: a client that distinguishes "this Region cannot be opted out of" from "this Region does not exist" by message text (the two refusals of the same member are otherwise only separable by `reason`, which this payload does populate) reads a sentence that cannot occur on AWS; retry/branching logic and assertions keyed on AWS's wording break.
Fix: the shared refusal constructor both `EnableRegion` and `DisableRegion` decide through, in this crate's state module — `region_not_opt_target`'s `message` must carry AWS's sentence with the request's Region code interpolated.
Traces: account.disable-region-enabled-by-default-conflict-rejected; services/service-com-amazonaws-account-account/src/state.rs:1652-1665 (`region_not_opt_target`), reached from src/ops/disable_region.rs:204-206 and src/ops/enable_region.rs:203-205 via `crate::state::region_opt_target` (state.rs:578-584), so EnableRegion carries the same wrong sentence.

## Evidence
All three checks exercised the live emulator and got HTTP 400 + code `ValidationException` (correct) with a diverging `message`.

Cause 1 — account.contact-information-missing-required-field-rejected
  PutContactInformation, body `contactInformation` complete except `postalCode` omitted (client-side parameter validation disabled so the request reaches the service).
  Expected (AWS): code `ValidationException`, message `[object has missing required properties (["PostalCode"])]`
  Actual (emulator): code `ValidationException`, message `1 validation error detected: Value null at 'contactInformation.postalCode' failed to satisfy constraint: Member must not be null`
  AssertionError raised at check line 46 on the message comparison. The side-effect assertions that follow (baseline FullName/PostalCode survive the rejected Put) and the complete-Put positive control were never reached.

Cause 2 — account.list-regions-invalid-max-results-rejected
  Positive control ListRegions(MaxResults=50) succeeded with 1..=50 Regions.
  ListRegions(MaxResults=51): Expected (AWS) `('ValidationException', '[numeric instance is greater than the required maximum (maximum: 50, found: 51)]')`
  Actual (emulator) `('ValidationException', "1 validation error detected: Value '51' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50")`
  AssertionError at check line 32. The MaxResults=0 assertion (expected `[numeric instance is lower than the required minimum (minimum: 1, found: 0)]`) was not reached, so the minimum template is unverified but produced by the same renderer.

Cause 3 — account.disable-region-enabled-by-default-conflict-rejected
  GetRegionOptStatus(us-east-1) == ENABLED_BY_DEFAULT (correct precondition).
  DisableRegion(RegionName='us-east-1'): Expected (AWS) code `ValidationException`, message `us-east-1 is not a valid region for opt-in or opt-out.`
  Actual (emulator) code `ValidationException`, message `Region us-east-1 is enabled by default and cannot be enabled or disabled`
  AssertionError at check line 30. The follow-on assertions (opt status unchanged; af-south-1 read-through control) were not reached.

Emulator source for cause 3 (constructor of the diverging sentence):
  services/service-com-amazonaws-account-account/src/state.rs:1652-1665 — `region_not_opt_target` builds `format!("Region {region_name} is enabled by default and cannot be enabled or disabled")`.

Model facts used (smithy_query on services/service-com-amazonaws-account-account/smithy-model/com-amazonaws-account-account.json):
  ContactInformation.PostalCode: required, @length(min 1, max 20), @sensitive.
  ListRegions.MaxResults: Integer, @range(min 1, max 50).
  DisableRegion.RegionName: required, @length(min 1, max 50); errors ValidationException(400), ConflictException(409).
  ValidationException: client error, httpError 400, members message (required, @sensitive), reason (invalidRegionOptTarget | fieldValidationFailed), fieldList.

## Steps to Reproduce
Boot the emulator with the account service mounted and point a botocore `account` client at it (region us-east-1). Disable client-side parameter validation (`botocore.config.Config(parameter_validation=False)`) wherever a modelled constraint is deliberately violated, otherwise botocore refuses the call locally and the service is never asked.

Cause 1:
1. PutContactInformation with a complete ContactInformation (FullName, AddressLine1, City, StateOrRegion, PostalCode, CountryCode, PhoneNumber with a leading '+') — succeeds.
2. Re-issue PutContactInformation with the SAME body minus `PostalCode`, using the validation-disabled client.
3. Observe 400 ValidationException with message `1 validation error detected: Value null at 'contactInformation.postalCode' failed to satisfy constraint: Member must not be null`; AWS answers `[object has missing required properties (["PostalCode"])]`.

Cause 2:
1. ListRegions(MaxResults=50) — succeeds (control).
2. With the validation-disabled client, ListRegions(MaxResults=51); observe message `1 validation error detected: Value '51' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50` where AWS answers `[numeric instance is greater than the required maximum (maximum: 50, found: 51)]`.
3. Also ListRegions(MaxResults=0); AWS answers `[numeric instance is lower than the required minimum (minimum: 1, found: 0)]`.

Cause 3:
1. GetRegionOptStatus(RegionName='us-east-1') — expect RegionOptStatus ENABLED_BY_DEFAULT.
2. DisableRegion(RegionName='us-east-1'); observe 400 ValidationException with message `Region us-east-1 is enabled by default and cannot be enabled or disabled` where AWS answers `us-east-1 is not a valid region for opt-in or opt-out.`
3. EnableRegion(RegionName='us-east-1') goes through the same helper and carries the same wrong sentence.

Replay to confirm: account.contact-information-missing-required-field-rejected, account.list-regions-invalid-max-results-rejected, account.disable-region-enabled-by-default-conflict-rejected.
