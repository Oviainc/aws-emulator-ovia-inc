Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehubv2-system-validation-and-conflict-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehubv2-system-validation-and-conflict-errors
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2|CreateSystem|4xx->4xx|$.message
Issue-Commit: 00190d8ff93cbdcec3daea62daacf6064f196fd5

# resiliencehubv2 ValidationException message uses legacy validation wording instead of the Smithy constraint-violation text

> _Note: this issue was identified against the code base at commit `00190d8ff93cbdcec3daea62daacf6064f196fd5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSystem

## Description
Cause 1 of 1: the ValidationException message emitted when CreateSystem's `name` violates its @pattern constraint is worded in the legacy AWS query/EC2 'N validation error(s) detected' style instead of the Smithy constraint-violation style resiliencehubv2 actually serves.

AWS: CreateSystem.name targets EntityName (Smithy: @required, @length min 2 max 60, @pattern "^[A-Za-z0-9][A-Za-z0-9_\\-]{1,59}$"); the modeled refusal is ValidationException (errorKind client, httpError 400, required member `message`, plus optional `reason` and `fieldList`). The model does not state the message text, so the message requirement is recorded behavior only: resiliencehubv2 is served by the Smithy server runtime, whose constraint-violation message is a bracketed list of violations, one entry per failed constraint, each reading `ECMA 262 regex "<the member's pattern>" does not match input string "<the rejected value>"`. Fixed text: the surrounding brackets, `ECMA 262 regex `, ` does not match input string `, and the quoting of both operands. Variable text: the pattern (taken from the violated member's @pattern in the model) and the rejected input string (taken verbatim from the request member). Status 400 and the ValidationException error code are correct today.

Emulator: for the same request it answers 400 ValidationException but with `1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` — i.e. the legacy wording, which also introduces a leading violation count, the `at '<member>'` locator and the `Member must satisfy ...` clause that this service never emits.

Why: clients and test suites that assert on, parse or snapshot the refusal text for resiliencehubv2 (and anything that distinguishes a Smithy constraint violation from a legacy query-protocol validation error, e.g. to extract the offending pattern or to count violations) see a message shape AWS never returns, so the assertion or parse fails even though the status and error code are right. Operators reading emulator logs are also told the error came from a different validation layer than the real service's.

Fix: the shared request-validation/error-serialization path for this service — the component that renders constraint violations into ValidationException.message for resiliencehubv2's operations (it must emit the Smithy constraint-violation list form, built from the violated member's own @pattern/@length and the request's value, and apply to every constrained member, not just CreateSystem.name).

Related open issues: `20261029T191749Z-resiliencehubv2-createsystem-stub.md` (CreateSystem scaffold stub) exists but does not cover this message-wording divergence — the replay reports this step as an unblocked, unowned mismatch, i.e. CreateSystem does answer and answers with the wrong refusal text.

Traces: resiliencehubv2-system-validation-and-conflict-errors#task#1 (CreateSystem with name="bad name has spaces!").

## Evidence
Fixture resiliencehubv2-system-validation-and-conflict-errors, step task#1, operation CreateSystem, verdict mismatch.
Expected status 400, actual status 400 (status and error code agree).
Diverging key $.message:
  expected (recorded AWS): [ECMA 262 regex "^[A-Za-z0-9][A-Za-z0-9_\\-]{1,59}$" does not match input string "bad name has spaces!"]
  actual (emulator):       1 validation error detected: Value 'bad name has spaces!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9][A-Za-z0-9_\\-]{1,59}$
In the expected text, the fixed parts are the brackets, 'ECMA 262 regex ' and ' does not match input string '; the pattern and the input string are values taken from the model's @pattern and from the request's `name` member respectively. The emulator's entire message template is wrong, not just its values.
Model: CreateSystem input member `name` -> EntityName (@required, @length 2..60, @pattern ^[A-Za-z0-9][A-Za-z0-9_\\-]{1,59}$); modeled error ValidationException (client, 400) with required `message`.
Divergence key: service=resiliencehubv2|CreateSystem|status=400->400|keys=$.message

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture resiliencehubv2-system-validation-and-conflict-errors.
2. At step task#1, POST /v2/create-system (resiliencehubv2 CreateSystem) with a `name` that violates EntityName's @pattern, e.g. a value containing spaces and punctuation such as "bad name has spaces!".
3. Observe the 400 ValidationException body: the emulator returns the legacy "1 validation error detected: Value '<value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" message, where AWS returns the bracketed Smithy constraint-violation message [ECMA 262 regex "<pattern>" does not match input string "<value>"].
