Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-error-getcontrol-invalid-arn-format
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-error-getcontrol-invalid-arn-format controlcatalog-error-invalid-filter-and-pagination-constraints
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|GetControl|4xx->4xx|$.message
Issue-Key: controlcatalog|ListControlMappings|4xx->4xx|$.message
Issue-Commit: 837306e76dca4544e8b76426022c8d4a2dc48510

# Validation error messages inject a rejected-value rendering AWS does not emit

> _Note: this issue was identified against the code base at commit `837306e76dca4544e8b76426022c8d4a2dc48510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetControl

## Description
Constraint violations are reported with the right status and the right error shape, but the message the emulator builds does not follow the request-derived format the recordings show: it inserts a rendering of the rejected value and echoes the modeled pattern with its anchors. Because this message is built from the request (it names the member and its constraint), its format is a requirement, not an incidental string. 2 causes, both on service controlcatalog.

Cause 1 of 2 - GetControl with a well-formed ARN that is not a control ARN (fixture controlcatalog-error-getcontrol-invalid-arn-format, step task#1).
AWS: Smithy: GetControl input GetControlRequest.ControlArn is required with @pattern ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$; modeled error ValidationException (httpError 400). Recorded behavior: the member-constraint message names the member and the violated constraint only - "1 validation error detected: Value at 'controlArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern with the leading ^ and trailing $ stripped>" - with no rendering of the rejected value.
Emulator: same status (400) and same field ($.message), but the text adds the rejected value ("Value 'arn:aws:controlcatalog:::domain/...' at 'controlArn'") and echoes the pattern anchored as ^...$.
Why: the message is deterministic and request-derived, so clients and test suites assert on it and operators grep it; a message that embeds the caller's input where AWS does not also leaks the submitted ARN into logs that the real service keeps value-free.
Fix: the shared constraint-violation message renderer used by this operation's input validation (strip the pattern anchors, omit the value rendering).

Cause 2 of 2 - ListControlMappings with two ARNs in Filter.ControlArns (fixture controlcatalog-error-invalid-filter-and-pagination-constraints, step task#3; cluster also covers task#4).
AWS: Smithy: ControlMappingFilter.ControlArns is @length min 1 max 1; ValidationException (400). Recorded behavior: "1 validation error detected: Value at 'filter.controlArns' failed to satisfy constraint: Member must have length less than or equal to 1" - again naming member and constraint with no value rendering.
Emulator: same status (400) and field, but inserts a synthesized value rendering: "Value '2 items' at 'filter.controlArns' failed to satisfy constraint: Member must have length less than or equal to 1".
Why: same as Cause 1 - the format is the contract for this class of error, and "'2 items'" is a rendering AWS never produces, so message-sensitive clients diverge even though the constraint was detected correctly.
Fix: the same constraint-violation message renderer, for the list-length violation path.

Traces: controlcatalog-error-getcontrol-invalid-arn-format#task#1 and controlcatalog-error-invalid-filter-and-pagination-constraints#task#3.

## Evidence
Cause 1: fixture controlcatalog-error-getcontrol-invalid-arn-format step task#1 - expected_status 400, actual_status 400; $.message value_mismatch: expected "1 validation error detected: Value at 'controlArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+", got "1 validation error detected: Value 'arn:aws:controlcatalog:::domain/d4msesd9vvmzmmuvlv06m92uq' at 'controlArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$".
Cause 2: fixture controlcatalog-error-invalid-filter-and-pagination-constraints step task#3 - expected_status 400, actual_status 400; $.message value_mismatch: expected "1 validation error detected: Value at 'filter.controlArns' failed to satisfy constraint: Member must have length less than or equal to 1", got "1 validation error detected: Value '2 items' at 'filter.controlArns' failed to satisfy constraint: Member must have length less than or equal to 1".

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Replay fixture controlcatalog-error-getcontrol-invalid-arn-format, step task#1 (POST /get-control with a domain ARN in ControlArn): 400 ValidationException whose message embeds the rejected ARN and the anchored pattern.
3. Replay fixture controlcatalog-error-invalid-filter-and-pagination-constraints, step task#3 (POST /list-control-mappings with two entries in Filter.ControlArns): 400 ValidationException whose message embeds "'2 items'" as the rejected value.
