Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-pricing-calculator.create-workload-estimate-invalid-name-pattern-rejected
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator.create-workload-estimate-invalid-name-pattern-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# CreateWorkloadEstimate pattern-violation ValidationException message is framed differently from AWS (echoes the rejected value, keeps regex anchors)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkloadEstimate

## Description
One root cause, in the constraint-violation message builder used by bcm-pricing-calculator's request validation.

Cause 1 of 1: the `name` @pattern violation on CreateWorkloadEstimate is reported with the right status and error code but the wrong message construction.

AWS: Smithy model services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json — CreateWorkloadEstimate.name targets WorkloadEstimateName, @required, @length max 64, @pattern "^[a-zA-Z0-9-]+$"; modeled error ValidationException (errorKind client, httpError 400). WorkloadEstimateName is NOT @sensitive, and the model does not state the message text, so the message FRAMING below is recorded behavior only: the recorded AWS response rejects a pattern-violating name with HTTP 400 / error code `ValidationException` and a message built from the request in the standard constraint-validation form that (a) names the member path and the violated constraint WITHOUT echoing the submitted value, and (b) renders the pattern with the leading `^` and trailing `$` anchors stripped. Recorded example message (an example of that rule, not a required literal): "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+".

Emulator: returns the correct HTTP 400 and the correct error code `ValidationException`, and correctly refuses the name (no resource is leaked), but builds the message with the other framing on both points — it interpolates the rejected value into the member clause and emits the raw anchored pattern from the model. Observed: "1 validation error detected: Value 'bad name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-]+$".

Why: the rejection itself is right, so clients that branch only on code/status are unaffected. What breaks is anything that parses the validation message, which is the only machine-readable place the violated member and constraint appear for this service (the emulator's response carries no fieldList either): message-scraping assertions, parity tests and support tooling that extract the member path or recompile the quoted pattern get a different string shape than AWS, and a pattern recompiled with `^`/`$` versus without is not the same regex object. Echoing the submitted value also makes the emulator leak request content into an error string where AWS does not, which diverges for any caller that logs these messages under a redaction policy calibrated on AWS.

Fix: the shared constraint-violation message formatter behind CreateWorkloadEstimate's input validation in services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator (the validation layer feeding ValidationException, not the operation's state or serializer): emit the no-value member clause and strip the anchors when quoting a @pattern. Since the same formatter serves every @pattern/@length member in this service, apply it there rather than at the single call site.

Traces: bcm-pricing-calculator.create-workload-estimate-invalid-name-pattern-rejected.

## Evidence
Check: bcm-pricing-calculator.create-workload-estimate-invalid-name-pattern-rejected [mismatch]

Request: CreateWorkloadEstimate(name="bad name!")

Agreed (not the defect): HTTP 400, Error.Code == "ValidationException", the create is refused, and the rejected name never appears in list_workload_estimates.

Diverging field — Error.Message:
  expected (recorded AWS framing): "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+"
  actual   (emulator):             "1 validation error detected: Value 'bad name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-]+$"

Two construction differences in one message: the submitted value is interpolated into the member clause, and the regex is quoted with its `^`/`$` anchors intact.

Assertion from the check:
  AssertionError: expected message "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+", got "1 validation error detected: Value 'bad name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-]+$"

Model grounding (smithy_query, operation=CreateWorkloadEstimate): name -> WorkloadEstimateName, required, length max 64, pattern "^[a-zA-Z0-9-]+$"; errors include ValidationException (client, 400). smithy_query shape=WorkloadEstimateName shows no @sensitive trait; shape=ValidationException has members message (required), reason, fieldList.

## Steps to Reproduce
1. Boot the emulator and point a botocore client for `bcm-pricing-calculator` at it.
2. Call CreateWorkloadEstimate with a name that violates ^[a-zA-Z0-9-]+$, e.g. name="bad name!".
3. Catch the ClientError and read response["Error"]["Code"] and response["Error"]["Message"].
4. Observe Code == "ValidationException" with HTTP 400 (correct), but the message is "1 validation error detected: Value 'bad name!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9-]+$" instead of AWS's framing, which omits the submitted value and quotes the pattern without the ^/$ anchors.
5. Replay to confirm: bcm-pricing-calculator.create-workload-estimate-invalid-name-pattern-rejected (the check also exercises a valid-name positive control plus a GetWorkloadEstimate read-back, both of which already pass).
