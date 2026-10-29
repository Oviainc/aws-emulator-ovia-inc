Issue-Source: trace-sweep-divergence
Issue-Trace: controltower-enabled-baseline-control-detail-correlation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controltower-enabled-baseline-control-detail-correlation controltower-enabled-baseline-discovery-and-tagging controltower-enabled-control-discovery-and-operations controltower-control-operations-pagination-and-filter-boundaries
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower|ListEnabledBaselines|4xx->2xx|$.message
Issue-Key: controltower|ListEnabledBaselines|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: controltower|ListEnabledBaselines|4xx->2xx|status
Issue-Key: controltower|ListEnabledControls|4xx->2xx|$.message
Issue-Key: controltower|ListEnabledControls|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: controltower|ListEnabledControls|4xx->2xx|status
Issue-Key: controltower|GetEnabledBaseline|4xx->4xx|$.message
Issue-Key: controltower|GetEnabledBaseline|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: controltower|GetEnabledBaseline|4xx->4xx|status
Issue-Key: controltower|GetEnabledControl|4xx->4xx|$.message
Issue-Key: controltower|GetControlOperation|4xx->4xx|$.message
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# Control Tower enabled-resource and operation reads ignore the landing-zone/admin-role precondition and answer from empty state with emulator prose

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEnabledBaselines

## Description
In an account with no usable landing zone, AWS Control Tower refuses the enabled-baseline / enabled-control / control-operation reads with a service-level precondition error. The emulator has no such precondition: it answers each call out of its own (empty) store — a success with an empty list where AWS refuses, or a not-found carrying emulator-authored prose instead of AWS's precondition message. Five causes, one shared fault: the precondition gate is missing from these handlers, so the store lookup decides the response.

Cause 1 of 5 — ListEnabledBaselines succeeds where AWS refuses (fixture controltower-enabled-baseline-control-detail-correlation, step task#1).
AWS: Smithy: ListEnabledBaselines -> ListEnabledBaselinesOutput.enabledBaselines required; modelled errors include ValidationException (400). Recorded behavior: HTTP 400, x-amzn-errortype: ValidationException, message "AWS Control Tower could not complete the operation because it could not assume the 'AWSControlTowerAdmin' role. Check the configuration for this role and try again." The message is entirely fixed text; the only variable-looking part is the role name, which is the fixed service role AWSControlTowerAdmin.
Emulator: HTTP 200 with a body that carries no message and no x-amzn-errortype — the listing simply reports nothing enabled.
Why: a client cannot tell 'nothing is enabled' from 'Control Tower cannot answer'. Provisioning code that lists enablements to decide what to create reads the empty list as a clean slate and proceeds, where against AWS it would have stopped and surfaced the role/landing-zone misconfiguration.
Fix: the ListEnabledBaselines handler's precondition check, ahead of projecting the enabled-baseline store.
Traces: controltower-enabled-baseline-control-detail-correlation#task#1 (the cluster also covers controltower-enabled-baseline-discovery-and-tagging#task#1).

Cause 2 of 5 — ListEnabledControls succeeds where AWS refuses (same fixture, step task#2).
AWS: Smithy: ListEnabledControls -> enabledControls required; modelled errors include ResourceNotFoundException (404). Recorded behavior: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.' — all fixed text.
Emulator: HTTP 200, no message, no errortype — an empty enabledControls list.
Why: the same false clean slate as cause 1; additionally the client loses the actionable instruction to create a landing zone.
Fix: the ListEnabledControls handler's landing-zone precondition check, ahead of the store projection.
Traces: controltower-enabled-baseline-control-detail-correlation#task#2 (the cluster also covers controltower-enabled-control-discovery-and-operations#task#1).

Cause 3 of 5 — GetEnabledBaseline reports a lookup miss instead of the precondition refusal (same fixture, step task#3).
AWS: Smithy: GetEnabledBaseline.enabledBaselineIdentifier required (Arn); modelled errors ValidationException (400) and ResourceNotFoundException (404). Recorded behavior: HTTP 400, x-amzn-errortype: ValidationException, message "AWS Control Tower could not complete the operation because it could not assume the 'AWSControlTowerAdmin' role. Check the configuration for this role and try again." (fixed text).
Emulator: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message 'no enabled baseline matches enabledBaselineIdentifier <enabledBaselineIdentifier>' — emulator-authored prose echoing the request identifier.
Why: the wrong error class (400 client-input/validation vs 404 not-found) and non-AWS wording; a caller's error handling branches on x-amzn-errortype and takes the not-found branch for what AWS reports as a service configuration failure.
Fix: the GetEnabledBaseline handler — apply the precondition before resolving the identifier, and emit AWS's message for the refusal.
Traces: controltower-enabled-baseline-control-detail-correlation#task#3.

Cause 4 of 5 — GetEnabledControl's 404 carries emulator prose, not AWS's precondition message (same fixture, step task#4).
AWS: Smithy: GetEnabledControl.enabledControlIdentifier required; ResourceNotFoundException (404). Recorded behavior: HTTP 404 with message 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.' (fixed text).
Emulator: HTTP 404 (status matches) but message 'no enabled control matches enabledControlIdentifier <enabledControlIdentifier>'.
Why: the message is a requirement for this error class; a client surfacing it to an operator reports a missing control rather than the missing landing zone that actually blocks every call.
Fix: the GetEnabledControl handler's not-found path, which must distinguish 'no landing zone' from 'no such enablement' and word each as AWS does.
Traces: controltower-enabled-baseline-control-detail-correlation#task#4.

Cause 5 of 5 — GetControlOperation's 404 carries emulator prose, not AWS's precondition message (fixture controltower-control-operations-pagination-and-filter-boundaries, step task#5).
AWS: Smithy: GetControlOperation.operationIdentifier required @length 36 @pattern ^[a-f0-9]{8}-...; ResourceNotFoundException (404). Recorded behavior: HTTP 404 with message 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.' (fixed text).
Emulator: HTTP 404 with message 'no control operation matches operationIdentifier <operationIdentifier>'.
Why: same as cause 4 — the operation-log read reports an unknown operation id where AWS reports the un-provisioned landing zone, so a poller retries an id that can never resolve.
Fix: the GetControlOperation handler's not-found path, sharing the precondition check with the other control-plane reads.
Traces: controltower-control-operations-pagination-and-filter-boundaries#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (controltower, 5 clusters):

1) ListEnabledBaselines, controltower-enabled-baseline-control-detail-correlation#task#1 (also affects controltower-enabled-baseline-discovery-and-tagging#task#1): status expected 400, got 200; $.message missing (expected "AWS Control Tower could not complete the operation because it could not assume the 'AWSControlTowerAdmin' role. Check the configuration for this role and try again."); $headers.x-amzn-errortype missing (expected 'ValidationException').

2) ListEnabledControls, controltower-enabled-baseline-control-detail-correlation#task#2 (also affects controltower-enabled-control-discovery-and-operations#task#1): status expected 404, got 200; $.message missing (expected 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.'); $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException').

3) GetEnabledBaseline, controltower-enabled-baseline-control-detail-correlation#task#3: status expected 400, got 404; $.message expected "AWS Control Tower could not complete the operation because it could not assume the 'AWSControlTowerAdmin' role. Check the configuration for this role and try again.", got 'no enabled baseline matches enabledBaselineIdentifier arn:aws:controltower:us-west-2:000000000000:enabledbaseline/NONEXISTENT000000000'; $headers.x-amzn-errortype expected 'ValidationException', got 'ResourceNotFoundException'.

4) GetEnabledControl, controltower-enabled-baseline-control-detail-correlation#task#4: status 404 both sides; $.message expected 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.', got 'no enabled control matches enabledControlIdentifier arn:aws:controltower:us-west-2:000000000000:enabledcontrol/NONEXISTENT000000000'.

5) GetControlOperation, controltower-control-operations-pagination-and-filter-boundaries#task#5: status 404 both sides; $.message expected 'AWS Control Tower cannot complete the operation, because you must create a landing zone first. To continue, create your landing zone from the console, or call the CreateLandingZone API.', got 'no control operation matches operationIdentifier 00000000-0000-0000-0000-000000000000'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture controltower-enabled-baseline-control-detail-correlation, step task#1: POST /list-enabled-baselines with no landing zone provisioned; observe 200 with an empty list instead of 400 ValidationException naming the AWSControlTowerAdmin role.
3. Same fixture, step task#2: POST /list-enabled-controls; observe 200 instead of 404 ResourceNotFoundException with the 'create a landing zone first' message.
4. Same fixture, steps task#3 and task#4: POST /get-enabled-baseline and /get-enabled-control with the recorded identifiers; observe the emulator's 'no enabled baseline/control matches ...' prose (and 404 where 400 was recorded for the baseline read).
5. Fixture controltower-control-operations-pagination-and-filter-boundaries, step task#5: POST /get-control-operation with the recorded operationIdentifier; observe 'no control operation matches operationIdentifier ...' instead of AWS's landing-zone precondition message.
