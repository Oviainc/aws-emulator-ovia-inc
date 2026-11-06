Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: controltower.disable-control-missing-identifiers-rejected controltower.enable-baseline-malformed-target-arn-rejected controltower.enabled-baselines-list-get-consistency controltower.enabled-controls-list-get-consistency controltower.list-enabled-controls-target-filter-validation controltower.get-control-operation-malformed-id-validation controltower.get-landing-zone-nonexistent-not-found
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower.disable-control-missing-identifiers-rejected|run#3|AssertionError|assert missing_code == "ResourceNotFoundException", (
Issue-Key: controltower.enable-baseline-malformed-target-arn-rejected|_error#8|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: controltower.enabled-baselines-list-get-consistency|_error#3|AssertionError|raise AssertionError(
Issue-Key: controltower.enabled-controls-list-get-consistency|_error#3|AssertionError|raise AssertionError(
Issue-Key: controltower.list-enabled-controls-target-filter-validation|_error#6|AssertionError|raise AssertionError(
Issue-Key: controltower.get-control-operation-malformed-id-validation|run#9|AssertionError|assert wellformed_missing_message == NO_LANDING_ZONE_MESSAGE, (
Issue-Key: controltower.get-landing-zone-nonexistent-not-found|run#7|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# Control Tower control-plane calls ignore the landing-zone / AWSControlTowerAdmin precondition and answer from empty state with emulator-authored prose

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEnabledControls

## Description
In an account with no landing zone (which is the emulator's state: ListLandingZones returns an empty list), AWS Control Tower refuses its enabled-resource, control-operation and baseline-enablement calls with a service-level precondition error, and words the refusal in its own fixed text. The emulator has no such gate: each handler goes straight to its own (empty) store, so the lookup decides the response — a 200 success where AWS refuses, or a not-found/validation error carrying emulator-authored prose that echoes the request identifier instead of AWS's message. SIX distinct causes below, all in this one service.

An existing OPEN issue, 20261029T183207Z-controltower-control-tower-enabled-resource-and-operation-reads-ignore, describes the same missing precondition from the WIRE corpus for ListEnabledBaselines / ListEnabledControls / GetEnabledBaseline / GetEnabledControl / GetControlOperation. It does not carry these behavioral check keys, and causes 1, 2 and 6 here (DisableControl, EnableBaseline, GetLandingZone) are not in it at all. The replay marked none of these checks as owned.

Cause 1 of 6 — DisableControl with NO identifiers is refused with the wrong error class.
AWS: Smithy: DisableControl input members controlIdentifier, targetIdentifier, enabledControlIdentifier are all OPTIONAL in the model (no @required); modelled errors include ResourceNotFoundException (404) and ValidationException (400). Recorded behavior: a call with no identifiers at all reaches the lookup and is refused with ResourceNotFoundException and the message 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.' — entirely fixed text, with no request- or state-derived values. The same no-identifier/unknown-identifier path is distinct from a MALFORMED enabledControlIdentifier, which AWS refuses earlier with BadRequestException 'Invalid request body' (that part of the check passes today).
Emulator: ValidationException (400) for the no-identifier call.
Why: a caller branching on the error type sees an input-shape complaint where AWS reports an un-provisioned landing zone, so it 'fixes' its request instead of creating the landing zone that actually blocks the call; the retry/abort decision for 404 vs 400 also differs in SDK error handling.
Fix: the DisableControl handler — apply the landing-zone precondition before (and in place of) the arity complaint, and emit AWS's 404 message.
Traces: controltower.disable-control-missing-identifiers-rejected.

Cause 2 of 6 — EnableBaseline SUCCEEDS on a well-formed but nonexistent target, where AWS refuses.
AWS: Smithy: EnableBaseline.targetIdentifier required, Arn @length 20..2048 @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$; modelled errors ValidationException (400) and ResourceNotFoundException (404). Recorded behavior: a well-formed organizational-unit ARN that does not exist passes the format gate and is then refused — with NO landing zone present, ValidationException and the message "AWS Control Tower could not complete the operation because it could not assume the 'AWSControlTowerAdmin' role. Check the configuration for this role and try again." (all fixed text; the only name in it is the fixed service role AWSControlTowerAdmin); with a landing zone present, ResourceNotFoundException for the unknown OU. Either way the call is never a success.
Emulator: HTTP 200 with a freshly minted operationIdentifier and enabledbaseline ARN — it enables a baseline on an organizational unit that does not exist, in an account with no landing zone. (The malformed-ARN half of the same check passes: that is refused with BadRequestException 'Invalid request body'.)
Why: the emulator fabricates an enablement and an operation id for a target AWS never accepted. Provisioning code then polls that operation id and records a baseline as enabled against a nonexistent OU, state real AWS can never reach.
Fix: the EnableBaseline handler — the landing-zone/admin-role precondition and the target-existence lookup, after the ARN format gate and before anything is persisted or an operation minted.
Traces: controltower.enable-baseline-malformed-target-arn-rejected.

Cause 3 of 6 — ListEnabledBaselines succeeds with an empty list where AWS refuses.
AWS: Smithy: ListEnabledBaselines output.enabledBaselines @required; modelled errors ValidationException (400), AccessDeniedException, ThrottlingException, InternalServerException. Recorded behavior with no landing zone: ValidationException (400) with the message "AWS Control Tower could not complete the operation because it could not assume the 'AWSControlTowerAdmin' role. Check the configuration for this role and try again." (fixed text).
Emulator: HTTP 200, body {'enabledBaselines': []}.
Why: a client cannot distinguish 'nothing is enabled' from 'Control Tower cannot answer'. Code that lists enablements to decide what to create reads the empty list as a clean slate and proceeds, where against AWS it would stop and surface the role/landing-zone misconfiguration.
Fix: the ListEnabledBaselines handler's precondition check, ahead of projecting the enabled-baseline store.
Traces: controltower.enabled-baselines-list-get-consistency.

Cause 4 of 6 — ListEnabledControls succeeds with an empty list where AWS refuses, both unfiltered and with a targetIdentifier filter.
AWS: Smithy: ListEnabledControls output.enabledControls @required; targetIdentifier optional, @length 20..2048 @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$; modelled errors include ResourceNotFoundException (404). Recorded behavior with no landing zone: ResourceNotFoundException (404) with 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.' (fixed text) — for the plain call AND for a call carrying a well-formed but nonexistent organizational-unit ARN as targetIdentifier. (The malformed-targetIdentifier half is refused correctly today with BadRequestException 'Invalid request body'.)
Emulator: HTTP 200, body {'enabledControls': []}, in both cases.
Why: the same false clean slate as cause 3, plus the client loses the actionable instruction to create a landing zone; a filter on a nonexistent target silently reports 'no controls on that target' rather than 'that target cannot be resolved'.
Fix: the ListEnabledControls handler's landing-zone precondition check, ahead of the store projection and the target filter.
Traces: controltower.enabled-controls-list-get-consistency, controltower.list-enabled-controls-target-filter-validation.

Cause 5 of 6 — GetControlOperation's 404 carries emulator prose, not AWS's precondition message.
AWS: Smithy: GetControlOperation.operationIdentifier @required, @length 36..36, @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$; modelled errors ResourceNotFoundException (404), ValidationException (400). Recorded behavior: a well-formed but unknown UUID is refused with ResourceNotFoundException and the message 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.' (entirely fixed text — the requested operation id is NOT interpolated). The malformed-pattern half of the same check passes today (BadRequestException 'Invalid request body').
Emulator: ResourceNotFoundException with the message 'no control operation matches operationIdentifier <operationIdentifier>', i.e. emulator-authored wording whose variable part is the request's operationIdentifier.
Why: the message is a requirement of this error class. A poller surfaces 'unknown operation id' and retries an id that can never resolve, instead of reporting the un-provisioned landing zone that blocks every control operation.
Fix: the GetControlOperation handler's not-found path, sharing the precondition check with the other control-plane reads, and emitting AWS's fixed text.
Traces: controltower.get-control-operation-malformed-id-validation.

Cause 6 of 6 — GetLandingZone's not-found message is emulator prose, not AWS's.
AWS: Smithy: GetLandingZone.landingZoneIdentifier @required (String); modelled errors ResourceNotFoundException (404), ValidationException (400). Recorded behavior: a well-formed but nonexistent landing-zone ARN is refused with ResourceNotFoundException (the emulator's code matches) and the message 'AWS Control Tower could not find a landing zone identifier value for the management account: <accountId>, in Region <region>. To continue, provide a valid resource ARN or landingZoneIdentifier string for your landing zone.' The fixed text is everything outside the two substitutions; the variable parts are the CALLER's management account id and the request's Region — notably NOT the landingZoneIdentifier that was passed.
Emulator: ResourceNotFoundException with 'no landing zone matches landingZoneIdentifier <landingZoneIdentifier>' — different wording, and it echoes the requested ARN where AWS names the account and Region instead.
Why: operators and support tooling match on this message to tell 'wrong identifier' from 'this account/Region has no landing zone at all'; the emulator's wording asserts the former for what AWS reports as the latter.
Fix: the GetLandingZone handler's not-found path (the message construction), which must name the management account and Region as AWS does. An existing OPEN issue, 20261029T043829Z-controltower-getlandingzone-wrong-behavior, concerns ARN addressability under a non-canonical signing region and does not cover this message.
Traces: controltower.get-landing-zone-nonexistent-not-found.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator booted from this checkout (controltower 6/15 passed; all 9 failures are controltower, none marked ALREADY FILED or OWNED).

1) controltower.disable-control-missing-identifiers-rejected [mismatch]: AssertionError: expected ResourceNotFoundException when no identifiers are supplied, got ValidationException. (Asserted message, never reached: 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.')

2) controltower.enable-baseline-malformed-target-arn-rejected [mismatch]: AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'operationIdentifier': '<uuid>', 'arn': 'arn:aws:controltower:<region>:<account>:enabledbaseline/<id>'} — for enable_baseline(baselineIdentifier=<AWSControlTowerBaseline arn>, baselineVersion='4.0', targetIdentifier='arn:aws:organizations::<account>:ou/o-aaaaaaaaaa/ou-aaaa-aaaaaaaa'). Expected ValidationException with "AWS Control Tower could not complete the operation because it could not assume the 'AWSControlTowerAdmin' role. Check the configuration for this role and try again." (no landing zone branch).

3) controltower.enabled-baselines-list-get-consistency [mismatch]: AssertionError: expected ListEnabledBaselines to reject with no landing zone present, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'enabledBaselines': []}. Expected ValidationException + the AWSControlTowerAdmin-role message.

4) controltower.enabled-controls-list-get-consistency [mismatch]: AssertionError: expected ListEnabledControls to reject with no landing zone present, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'enabledControls': []}. Expected ResourceNotFoundException + the 'create a landing zone first' message.

5) controltower.list-enabled-controls-target-filter-validation [mismatch]: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'enabledControls': []} — for list_enabled_controls(targetIdentifier='arn:aws:organizations::<account>:ou/o-aaaaaaaaaa/ou-aaaa-aaaaaaaa'). Expected ResourceNotFoundException + the 'create a landing zone first' message. The malformed-targetIdentifier assertion earlier in the same check passed (BadRequestException / 'Invalid request body').

6) controltower.get-control-operation-malformed-id-validation [mismatch]: AssertionError: expected the no-landing-zone message for a well-formed but unknown operationIdentifier, got 'no control operation matches operationIdentifier 342f2a8a-cce6-492d-8ee2-2607478357ed'. Error code ResourceNotFoundException matched; only the message diverges.

7) controltower.get-landing-zone-nonexistent-not-found [mismatch]: AssertionError: expected 'AWS Control Tower could not find a landing zone identifier value for the management account: <account>, in Region <region>. To continue, provide a valid resource ARN or landingZoneIdentifier string for your landing zone.', got 'no landing zone matches landingZoneIdentifier arn:aws:controltower:<region>:<account>:landingzone/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'. Error code ResourceNotFoundException matched.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1 for service controltower (checks listed in trace_ids).
2. Confirm the precondition state: POST /list-landingzones -> {'landingZones': []} (no landing zone provisioned).
3. POST /disable-control with an empty body; observe ValidationException instead of 404 ResourceNotFoundException with the 'create a landing zone first' message.
4. POST /enable-baseline with the AWSControlTowerBaseline ARN from /list-baselines, baselineVersion '4.0' and targetIdentifier 'arn:aws:organizations::<account>:ou/o-aaaaaaaaaa/ou-aaaa-aaaaaaaa'; observe HTTP 200 with a minted operationIdentifier and enabledbaseline ARN instead of 400 ValidationException naming the AWSControlTowerAdmin role.
5. POST /list-enabled-baselines (maxResults 20); observe 200 with an empty list instead of 400 ValidationException.
6. POST /list-enabled-controls (maxResults 20), then again with targetIdentifier set to the nonexistent OU ARN above; observe 200 with an empty list in both cases instead of 404 ResourceNotFoundException.
7. POST /get-control-operation with a well-formed random UUID; observe the message 'no control operation matches operationIdentifier ...' instead of AWS's landing-zone precondition text.
8. POST /get-landingzone with 'arn:aws:controltower:<region>:<account>:landingzone/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'; observe 'no landing zone matches landingZoneIdentifier ...' instead of AWS's 'could not find a landing zone identifier value for the management account: <account>, in Region <region>...' text.
