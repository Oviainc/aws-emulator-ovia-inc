Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: synthetics.tag-resource-invalid-arn-bad-request-exception
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics.tag-resource-invalid-arn-bad-request-exception|run#5|AssertionError|assert error["Message"] == expected_message, f"expected message {expected_message!r}, got {error['Message']!r}"
Issue-Commit: b76a0d3e2714c851cb33db867db4b883b5e1bfcc

# synthetics: ValidationException pattern message echoes the Smithy regex anchors (^...$) that AWS strips

> _Note: this issue was identified against the code base at commit `b76a0d3e2714c851cb33db867db4b883b5e1bfcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new root cause.

Cause 1 of 1: the constraint-violation message the emulator serves for a @pattern failure reproduces the modeled regular expression verbatim, including the anchoring `^` and `$`, whereas AWS reports the pattern with the anchors removed. Failing check: synthetics.tag-resource-invalid-arn-bad-request-exception.

AWS: TagResource's input member `resourceArn` is `com.amazonaws.synthetics#ResourceArn`, a string with `@length(min:1, max:2048)` and `@pattern("^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\\d{1}:\\d{12}:(canary|group):[0-9a-z_\\-]+$")` (Smithy: TagResourceRequest.ResourceArn / ResourceArn shape, httpLabel, required). For a malformed value real AWS answers HTTP 400 with error code `ValidationException` — a code that is NOT in the operation's modeled error set (BadRequestException/ConflictException/NotFoundException/TooManyRequestsException) — and the standard AWS constraint-violation message:
  `1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern-without-anchors>`
Fixed text: `N validation error(s) detected: Value '` … `' at '` … `' failed to satisfy constraint: Member must satisfy regular expression pattern: `. Variable parts taken from the request and the model: the rejected value (here the supplied `resourceArn`), the wire member name (`resourceArn`, lowercase-first as serialized, not the Smithy member `ResourceArn`), and the regular expression from the member's @pattern trait with its leading `^` and trailing `$` STRIPPED. The recorded AWS message for this input is therefore `... Member must satisfy regular expression pattern: arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:(canary|group):[0-9a-z_\-]+` — no `^`, no `$`.

Emulator: status 400 and code `ValidationException` are correct, but the message is emitted with the raw modeled pattern including anchors: `1 validation error detected: Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:(canary|group):[0-9a-z_\-]+$`. The only difference is the extra `^` prefix and `$` suffix around the pattern, i.e. the emulator copies the @pattern trait string straight through instead of unanchoring it.

Why: clients and test suites that assert on, log, or surface AWS's validation text see a message AWS never produces. Tooling that re-compiles the reported pattern, or that diffs expected-vs-actual error text in contract tests, fails against the emulator while passing against AWS. Because this is the shared constraint-violation formatter, every @pattern-constrained member of this service (and any other service using the same formatter) reports a pattern AWS would report unanchored, so the divergence is systemic rather than specific to this one ARN.

Fix: belongs in the shared input-validation/constraint-violation message formatter that renders @pattern failures for synthetics operations (the validation layer invoked for TagResource's `resourceArn`, ahead of the operation handler) — strip a leading `^` and a trailing `$` from the modeled pattern when composing the `Member must satisfy regular expression pattern:` clause, keeping the rest of the pattern and the rest of the message as-is. Note this failure is NOT the already-open stub issue 20261029T184613Z-synthetics-tagresource-stub.md: validation rejects the request before the unimplemented TagResource handler is reached, so that stub issue does not cover this message defect.

Traces: synthetics.tag-resource-invalid-arn-bad-request-exception.

## Evidence
Check synthetics.tag-resource-invalid-arn-bad-request-exception, [mismatch]:

Request: TagResource(ResourceArn="not-an-arn", Tags={"k":"v"})

Expected (AWS): Code `ValidationException`, Message
  1 validation error detected: Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:(canary|group):[0-9a-z_\-]+

Actual (emulator): Code `ValidationException` (correct), Message
  1 validation error detected: Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:(canary|group):[0-9a-z_\-]+$

AssertionError raised at check line 28:
  assert error["Message"] == expected_message

Smithy model consulted: services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json — shape com.amazonaws.synthetics#ResourceArn: length {min:1,max:2048}, pattern "^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\\d{1}:\\d{12}:(canary|group):[0-9a-z_\\-]+$"; TagResourceRequest.ResourceArn is required + httpLabel. The anchors are present in the model and must be removed from the rendered message.

## Steps to Reproduce
1. Boot the emulator and point a botocore synthetics client at it.
2. Call TagResource with a ResourceArn that does not match the modeled ARN pattern, e.g.
     synth.tag_resource(ResourceArn="not-an-arn", Tags={"k": "v"})
3. Catch the ClientError and inspect response["Error"]["Code"] and ["Message"].
4. Observe Code is ValidationException (correct) but the Message's `Member must satisfy regular expression pattern:` clause is wrapped in `^` ... `$`, which AWS's message does not contain.
5. Equivalent repro with any other @pattern-constrained member of the service confirms the formatter, not this one ARN, is the cause.
