Issue-Source: trace-sweep-divergence
Issue-Trace: controltower-control-operations-pagination-and-filter-boundaries
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controltower-control-operations-pagination-and-filter-boundaries controltower-get-enabled-resource-malformed-and-not-found
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower|ListControlOperations|4xx->4xx|$.message
Issue-Key: controltower|ListControlOperations|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: controltower|GetEnabledBaseline|4xx->4xx|$.message
Issue-Key: controltower|GetEnabledBaseline|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: controltower|GetEnabledControl|4xx->4xx|$.message
Issue-Key: controltower|GetEnabledControl|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: f81229b87ab2500fc3bfee437baf8654766d98fe

# ControlTower input-validation rejections answer ValidationException instead of recorded BadRequestException front-door error

> _Note: this issue was identified against the code base at commit `f81229b87ab2500fc3bfee437baf8654766d98fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListControlOperations

## Description
Three causes, all in the AWS Control Tower APIs service, all one shared fault: when a request fails input validation the emulator synthesizes a Smithy-style constraint-detail ValidationException, while the recorded AWS responses reject the request at the REST front door with a generic bad-request error carrying the constant body message 'Invalid request body'. The HTTP status (400) agrees in every cause; the error code carried in the x-amzn-errortype header, and the body message, do not. This is one validation/error-rendering decision shared by every operation of the service, not three independent operation bugs. No existing open issue mentions controltower (list_existing_issues query='controltower' returns none), so nothing already covers this.

Cause 1 of 3: ListControlOperations, maxResults above its modeled ceiling.
AWS: Smithy (services/service-com-amazonaws-controltower-awscontroltowerapis) models ListControlOperations.maxResults -> ListControlOperationsMaxResults with @range min 1, max 100, and binds errors AccessDeniedException(403), InternalServerException(500), ThrottlingException(429), ValidationException(400) over POST /list-control-operations. The model does NOT bind any BadRequestException, yet the recording for an out-of-range maxResults is HTTP 400 with x-amzn-errortype: BadRequestException and body message 'Invalid request body' — i.e. recorded behavior only: the request is refused by the front door before the modeled service error is reached. Requirement taken from the recording: status 400 and error code BadRequestException.
Emulator: HTTP 400 (status agrees) but x-amzn-errortype: ValidationException, with message "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Why: a client that branches on the error code sees a modeled, service-level ValidationException where AWS gives an unmodeled generic bad-request refusal. SDK error unmarshalling maps the two differently (typed ValidationException vs. generic service error), so retry/catch logic written against AWS takes the wrong branch, and any assertion on the error code fails.
Fix: the error rendering used by the service's request-validation path — the shared input-validation/error-serializer layer for the controltower crate, not the ListControlOperations handler alone, since the same decision produces causes 2 and 3.
Traces: controltower-control-operations-pagination-and-filter-boundaries#task#2 (cluster also covers task#3 of the same fixture).

Cause 2 of 3: GetEnabledBaseline with a non-ARN identifier.
AWS: Smithy models GetEnabledBaseline.enabledBaselineIdentifier as required, target Arn with @length min 20 max 2048 and @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$, errors ...ValidationException(400), ResourceNotFoundException(404). Recorded behavior only: for the malformed identifier 'totally-not-an-arn' AWS answers HTTP 400 with body message 'Invalid request body' and does not carry the error code the emulator sends.
Emulator: HTTP 400 with x-amzn-errortype: ValidationException (reported by replay as shape_mismatch against the recorded header, expected '<shaped-not-asserted>') and message "2 validation errors detected: Value 'totally-not-an-arn' at 'enabledBaselineIdentifier' failed to satisfy constraint: Member must have length greater than or equal to 20; ... must satisfy regular expression pattern: ^arn:aws[0-9a-zA-Z_\-:\/]+$".
Why: same as cause 1 — the error code a client dispatches on differs from AWS's for a malformed-identifier request. (The recorded 'Invalid request body' text itself is quoted only as an example: no model or AWS doc states a rule producing it, so the message difference alone is not the defect; the error code is.)
Fix: same shared validation/error-serializer layer, reached from the GetEnabledBaseline input validation.
Traces: controltower-get-enabled-resource-malformed-and-not-found#task#1.

Cause 3 of 3: GetEnabledControl with a non-ARN identifier.
AWS: Smithy models GetEnabledControl.enabledControlIdentifier as required, target Arn with @length min 20 max 2048 and @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$, errors ...ValidationException(400), ResourceNotFoundException(404). Recorded behavior only: for 'totally-not-an-arn' AWS answers HTTP 400 with body message 'Invalid request body' and not the emulator's error code.
Emulator: HTTP 400 with x-amzn-errortype: ValidationException (shape_mismatch vs the recorded header) and message "2 validation errors detected: Value 'totally-not-an-arn' at 'enabledControlIdentifier' failed to satisfy constraint: Member must have length greater than or equal to 20; ... must satisfy regular expression pattern: ^arn:aws[0-9a-zA-Z_\-:\/]+$".
Why: same as above; identical client-visible consequence on a second operation, confirming the fault is service-wide rather than per-handler.
Fix: same shared validation/error-serializer layer, reached from the GetEnabledControl input validation.
Traces: controltower-get-enabled-resource-malformed-and-not-found#task#2.

All three causes must be closed before submitting: fixing only the pagination bound (cause 1) leaves the two malformed-identifier causes diverging.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0) produced 3 divergence clusters, all service=controltower, all verdict=mismatch, none carrying stub_path or owned_by.

Cause 1 — fixture controltower-control-operations-pagination-and-filter-boundaries, step task#2 (cluster also affects task#3), operation ListControlOperations:
  expected_status 400, actual_status 400
  $.message: value_mismatch (expected 'Invalid request body', got "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100")
  $headers.x-amzn-errortype: value_mismatch (expected 'BadRequestException', got 'ValidationException')

Cause 2 — fixture controltower-get-enabled-resource-malformed-and-not-found, step task#1, operation GetEnabledBaseline:
  expected_status 400, actual_status 400
  $.message: value_mismatch (expected 'Invalid request body', got "2 validation errors detected: Value 'totally-not-an-arn' at 'enabledBaselineIdentifier' failed to satisfy constraint: Member must have length greater than or equal to 20; Value 'totally-not-an-arn' at 'enabledBaselineIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[0-9a-zA-Z_\-:\/]+$")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')

Cause 3 — fixture controltower-get-enabled-resource-malformed-and-not-found, step task#2, operation GetEnabledControl:
  expected_status 400, actual_status 400
  $.message: value_mismatch (expected 'Invalid request body', got "2 validation errors detected: Value 'totally-not-an-arn' at 'enabledControlIdentifier' failed to satisfy constraint: Member must have length greater than or equal to 20; Value 'totally-not-an-arn' at 'enabledControlIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[0-9a-zA-Z_\-:\/]+$")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')

Model evidence (smithy_query, services/service-com-amazonaws-controltower-awscontroltowerapis/smithy-model/com-amazonaws-controltower-awscontroltowerapis.json): ListControlOperations maxResults @range 1..100, POST /list-control-operations; GetEnabledBaseline/GetEnabledControl identifiers target Arn @length 20..2048 @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$; all three bind ValidationException(400) and none binds BadRequestException — so the recorded generic bad-request refusal is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture controltower-control-operations-pagination-and-filter-boundaries, step task#2 — POST /list-control-operations with maxResults above the modeled ceiling (recorded value 101). Observe HTTP 400 with x-amzn-errortype: ValidationException and a constraint-detail message, where the recording has error code BadRequestException and body message 'Invalid request body'. Step task#3 of the same fixture reproduces identically.
3. Cause 2: replay fixture controltower-get-enabled-resource-malformed-and-not-found, step task#1 — POST /get-enabled-baseline with enabledBaselineIdentifier 'totally-not-an-arn' (violates both @length min 20 and the ^arn:aws... @pattern). Observe HTTP 400 with x-amzn-errortype: ValidationException and a 2-constraint detail message instead of the recorded generic bad-request body.
4. Cause 3: replay fixture controltower-get-enabled-resource-malformed-and-not-found, step task#2 — POST /get-enabled-control with enabledControlIdentifier 'totally-not-an-arn'. Same divergence as step 3.
5. After the fix, re-replay both fixtures (all four steps) — the status must stay 400 while the error code and body match the recordings.
