Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-list-pagination
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-list-pagination artifact-report-term-and-download
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|ListReports|4xx->4xx|$.message
Issue-Key: artifact|GetReport|4xx->4xx|$.message
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Artifact error message bodies do not follow the recorded AWS message rules

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListReports

## Description
Two causes, one fault: on error paths where the emulator already returns the right HTTP status and error code, the body's message is rendered by a different rule than the one the recordings show, so clients and fixtures that assert on the documented message rule diverge. The same message-rendering fault has already been filed for another service as 20261021T234107Z-controlcatalog-validation-error-messages-inject-a-rejected-value-rendering.md; that issue carries no artifact replay key, so this one is filed separately and must be re-replayed against artifact.

Cause 1 of 2 - the validation-error renderer injects the rejected value for a numeric range constraint, which AWS does not.
AWS: Smithy: ListReportsRequest.maxResults : MaxResultsAttribute @httpQuery maxResults @range min 1 max 300; a violation is refused with HTTP 400 ValidationException whose message is built from the request and names the member and the constraint. The recorded rule for a numeric range violation names the member and the bound but does NOT echo the submitted value: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300".
Emulator: same status (400) and same error type, but renders the rejected value into the message: "1 validation error detected: Value '301' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300".
Why: the message is request-derived and therefore part of the contract; callers (and compliance tooling) that parse or match AWS's validation text get a message AWS never emits, and echoing the rejected value back is a rendering AWS deliberately omits for range constraints.
Fix: the shared ValidationException message renderer used by the artifact input-validation layer (range-constraint branch).
Traces: artifact-list-pagination#task#3.

Cause 2 of 2 - GetReport reports an internal reason instead of AWS's invalid-token message.
AWS: Smithy: GetReportRequest.termToken @required @httpQuery termToken @length 1..256 - 'Unique download token provided by GetTermForReport API'. Recorded behavior: a termToken that was not issued for the requested report is refused with HTTP 400 and the message 'The provided token is invalid.' - a fixed, non-request-derived refusal text.
Emulator: same status (400) and error type, but answers with the internal explanation 'termToken was not issued by GetTermForReport'.
Why: the body leaks the emulator's internal check instead of AWS's stable refusal text, so clients keyed on AWS's message (and recorded download flows) do not recognise the failure.
Fix: the GetReport handler's term-token validation error construction.
Traces: artifact-report-term-and-download#task#4.

## Evidence
Cause 1: fixture artifact-list-pagination step task#3, ListReports, status expected 400 / actual 400 (error type matches); detail "$.message: value_mismatch (expected \"1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300\", got \"1 validation error detected: Value '301' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 300\")". Model: ListReportsRequest.maxResults @range min 1 max 300; ValidationException httpError 400.
Cause 2: fixture artifact-report-term-and-download step task#4, GetReport, status expected 400 / actual 400 (error type matches); detail "$.message: value_mismatch (expected 'The provided token is invalid.', got 'termToken was not issued by GetTermForReport')". Model: GetReportRequest.termToken @required @length 1..256, documented as the token provided by GetTermForReport.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture artifact-list-pagination step task#3 (GET /v1/report/list?maxResults=301) and compare the 400 body's message with the recorded rendering that omits the rejected value.
3. Cause 2: replay fixture artifact-report-term-and-download step task#4 (GET /v1/report/get with a termToken not issued for that report) and compare the 400 body's message with the recorded 'The provided token is invalid.'
