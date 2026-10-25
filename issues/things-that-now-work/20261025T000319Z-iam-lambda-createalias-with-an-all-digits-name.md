Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.cfnuf-alias-named-like-a-version-or-dollar-latest-rejected
Issue-Key: iam__lambda.cfnuf-alias-named-like-a-version-or-dollar-latest-rejected|run#3|AssertionError|assert err == expected, f"CreateAlias named {bad!r}: expected {expected!r}, got {err!r}"
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# iam__lambda: CreateAlias with an all-digits name is refused InvalidParameterValueException instead of ValidationException

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
Cause 1 of 1: the alias-name pattern violation is classified under the wrong error code.
AWS: Smithy CreateAlias.Name targets Alias with @pattern ^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$, and an all-digits name (one that looks like a version) is refused with error code ValidationException and HTTP 400, message built from the request: "1 validation error detected: Value '593' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!^[0-9]+$)([a-zA-Z0-9-_]+)". ValidationException is Lambda's generic constraint-violation code; it is not in CreateAlias's modeled error list (which carries InvalidParameterValueException for semantic rejections), so the emulator is reusing the semantic code for a constraint failure.
Emulator: refuses with error code InvalidParameterValueException (status 400) and message "1 validation error detected: Value '593' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$". The message rule (member + constraint) is right; the code is wrong, and the echoed pattern is the anchored model form rather than the unanchored one AWS reports.
Why: clients branch on the error code — ValidationException means 'the request shape is invalid, do not retry and fix the input', InvalidParameterValueException is a semantic refusal of an otherwise-valid value. Mapping one to the other breaks retry/classification logic and any test that asserts Lambda's constraint-violation code.
Fix: the input-constraint validation path for CreateAlias in the lambda service crate (raise ValidationException 400 for @pattern/@length violations, reserving InvalidParameterValueException for semantic rejections) and report the pattern as AWS does.
Traces: iam__lambda.cfnuf-alias-named-like-a-version-or-dollar-latest-rejected.

## Evidence
AssertionError: CreateAlias named '593': expected ('ValidationException', "1 validation error detected: Value '593' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!^[0-9]+$)([a-zA-Z0-9-_]+)"), got ('InvalidParameterValueException', "1 validation error detected: Value '593' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$").

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Create a Lambda function and publish a version.
3. CreateAlias(FunctionName=fn, Name='593', FunctionVersion='1') — observe error code InvalidParameterValueException instead of ValidationException (both 400).
4. Re-replay iam__lambda.cfnuf-alias-named-like-a-version-or-dollar-latest-rejected to confirm.
