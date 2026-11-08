Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-comparators-are-type-strict-and-never-coerce-rejected stepfunctions.choice-timestamp-comparators-compare-instants-not-lexical-strings stepfunctions.choice-path-comparators-dereference-the-referenced-field stepfunctions.deep-sfn-language-wait-seconds-path-invalid-value-rejected
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-comparators-are-type-strict-and-never-coerce-rejected|run#4|AssertionError|assert _output(desc) == "fell-back", (
Issue-Key: stepfunctions.choice-timestamp-comparators-compare-instants-not-lexical-strings|run#4|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.choice-path-comparators-dereference-the-referenced-field|run#4|AssertionError|assert _INVALID_PATH_CAUSE in cause, (
Issue-Key: stepfunctions.deep-sfn-language-wait-seconds-path-invalid-value-rejected|run#7|AssertionError|assert "SecondsPath" in (bad.get("cause") or ""), (
Issue-Commit: 6402e991ee1ffb1f37749ece0148ecf3009a760a

# Step Functions Choice and Wait evaluation diverges: NumericEquals coerces across JSON types, timestamp comparators reject RFC-3339 offsets via a Smithy timestamp parser, and the States.Runtime causes do not name the offending ASL field (4 causes)

> _Note: this issue was identified against the code base at commit `6402e991ee1ffb1f37749ece0148ecf3009a760a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Four net-new root causes in the ASL expression evaluator: two are wrong evaluation semantics for Choice state comparators, two are States.Runtime cause messages that do not name the ASL field at fault. These states DO run in the emulator (the executions reach terminal state and the Choice/Wait events appear in the history), so the open issue 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md does not cover them; neither does 20261024T114502Z-sfn-step-functions-asl-definition-validator-accepts-invalid-definitions.md, which is about definition-time validation, while these four are run-time evaluation.

Cause 1 of 4: Choice comparators coerce across JSON types instead of being type-strict.
AWS: recorded behavior only (the ASL Choice state's documented comparator rule: a typed comparator matches only a value of that JSON type). `NumericEquals: 10` tested against the string value `"10"` does NOT match on AWS; evaluation falls through to the next Choice rule and finally to `Default`.
Emulator: the typed comparator matched the differently-typed value, so the `Default` branch was never taken — the recorded execution produced no `fell-back` output at all (output was absent where the Default branch's output was required). A match across JSON types means the comparator coerced string to number.
Why: a client's Choice routing silently takes the wrong branch whenever an upstream producer emits a numeric-looking string (a very common shape for ids, versions and amounts), so workflows branch into the wrong state and the Default/fallback path is dead code.
Fix: the Choice state comparator evaluator in the sfn crate's ASL interpreter — require the candidate value's JSON type to equal the comparator's type (NumericEquals/NumericLessThan... numbers only, StringEquals... strings only, BooleanEquals booleans only) and treat a type mismatch as not-matched rather than coercing.
Traces: sfn.choice-comparators-are-type-strict-and-never-coerce-rejected.

Cause 2 of 4: timestamp comparators are parsed with a Smithy RFC-3339 parser that refuses a UTC offset, so a valid ISO-8601 instant fails the execution.
AWS: recorded behavior only (the ASL TimestampEquals/TimestampLessThan/TimestampGreaterThan family compares instants, not lexical strings). `2024-03-01T13:00:00+01:00` is a valid ISO-8601 instant and AWS compares it as the instant it denotes; the recorded execution for this input must reach a Pass state and end SUCCEEDED, because every branch and the Default are Pass states.
Emulator: the execution ends `status=FAILED` with `error='States.Runtime'` and `cause="'<the input timestamp>' is not an ISO 8601 instant: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times"`. The fixed text here is the emulator's own; the timestamp in it is the request's value. Two defects are visible in one message: a valid instant is rejected, and the diagnosis leaks the emulator's Smithy timestamp-parsing layer into an ASL run-time cause (AWS never mentions Smithy).
Why: any workflow that compares timestamps produced by a non-UTC client (the overwhelming majority of ISO-8601 emitters) fails the whole execution instead of branching. The comparator is unusable outside bare-`Z` input.
Fix: the timestamp comparator in the Choice state evaluator in the sfn crate — parse the value as a full ISO-8601/RFC-3339 instant including a numeric UTC offset, normalise to an instant and compare instants; do not route ASL timestamp parsing through the Smithy wire-timestamp parser.
Traces: sfn.choice-timestamp-comparators-compare-instants-not-lexical-strings.

Cause 3 of 4: the States.Runtime cause for an unresolvable Choice condition path does not identify the condition path.
AWS: recorded behavior only. When a Choice rule's `*Path` operand cannot be dereferenced in the state input, the execution fails `States.Runtime` with a cause that names the unresolvable condition path, so a caller can tell this apart from any other States.Runtime failure.
Emulator: the cause is `"the path '$.limit' selected no value"` — emulator-authored wording. Its fixed text is the emulator's own sentence; `$.limit` is the JSONPath taken from the request's definition. It does not identify which Choice condition/operand owned the path, so the failure is indistinguishable from any other path-resolution failure in the execution.
Why: a caller debugging a failed execution cannot tell which Choice rule's operand was unresolvable, and any client that string-matches the cause to classify the failure mis-classifies it.
Fix: the Choice state evaluator's error path in the sfn crate — build the States.Runtime cause so it names the condition path/operand that failed to resolve, in AWS's wording for that error class, with the path itself taken from the definition.
Traces: sfn.choice-path-comparators-dereference-the-referenced-field.

Cause 4 of 4: the Wait state's invalid-SecondsPath cause names `Seconds` instead of `SecondsPath`.
AWS: recorded behavior only. A Wait state using `SecondsPath` whose resolved value is not a non-negative whole number (negative, non-numeric string, fractional, or absent) fails with a cause that names `SecondsPath` — the ASL field the caller actually wrote.
Emulator: the cause is `"state 'W': 'Seconds' must be a non-negative whole number"`. The fixed text is emulator-authored and names the wrong ASL member: the definition contains `SecondsPath`, not `Seconds`. The state name (`W` in the recording) is the variable part taken from the request's definition; the member name is not variable — it must be the member the definition used.
Why: the message points the caller at an ASL field their definition does not contain, so the diagnosis is actively misleading; a client that keys on the member name to locate the fault looks in the wrong place.
Fix: the Wait state evaluator in the sfn crate — carry which of `Seconds`/`SecondsPath`/`Timestamp`/`TimestampPath` the definition used into the failure cause and name that member, in AWS's wording for the error class.
Traces: sfn.deep-sfn-language-wait-seconds-path-invalid-value-rejected.

## Evidence
Cause 1 — sfn.choice-comparators-are-type-strict-and-never-coerce-rejected:
  AssertionError: NumericEquals:10 must NOT match the differently typed value '10' (type str); expected the Default output 'fell-back', got None -- a match here means the comparator coerced across JSON types

Cause 2 — sfn.choice-timestamp-comparators-compare-instants-not-lexical-strings:
  expected: input when='2024-03-01T13:00:00+01:00' -> status SUCCEEDED (every branch and the Default are Pass states)
  actual:   status=FAILED error='States.Runtime' cause="'2024-03-01T13:00:00+01:00' is not an ISO 8601 instant: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times"
  (the trailing clause names the emulator's own Smithy timestamp parser; AWS never names Smithy in an ASL run-time cause)

Cause 3 — sfn.choice-path-comparators-dereference-the-referenced-field:
  expected: a States.Runtime cause identifying the unresolvable condition path, so a caller can tell this apart from any other States.Runtime failure
  actual:   cause = "the path '$.limit' selected no value"  (fixed text emulator-authored; '$.limit' is the definition's path)

Cause 4 — sfn.deep-sfn-language-wait-seconds-path-invalid-value-rejected:
  expected: the cause names SecondsPath (the member the definition used)
  actual:   cause = "state 'W': 'Seconds' must be a non-negative whole number"  (names 'Seconds'; 'W' is the definition's state name)
  Negative/non-numeric/fractional/absent resolved values all take this same wrong-member path.

## Steps to Reproduce
1. Create an execution role Step Functions may assume (Choice/Pass/Wait states never assume it, but CreateStateMachine demands a passable role ARN).
2. Cause 1: CreateStateMachine (STANDARD, JSONPath) with a Choice state whose first rule is {"Variable": "$.n", "NumericEquals": 10, "Next": "Matched"} and a Default of a Pass state returning "fell-back". StartExecution with input {"n": "10"} (the STRING "10"). DescribeExecution: the Default output "fell-back" is absent — the typed comparator matched the string.
3. Cause 2: a Choice state using TimestampGreaterThan / TimestampLessThan against {"when": "$.when"}, with every branch and the Default a Pass. StartExecution with input {"when": "2024-03-01T13:00:00+01:00"}. DescribeExecution returns FAILED / States.Runtime with the "Smithy does not support timezone offsets in RFC-3339 date times" cause instead of SUCCEEDED.
4. Cause 3: a Choice rule whose operand is a *Path (e.g. {"Variable": "$.value", "NumericLessThanPath": "$.limit"}) and start an execution whose input omits `limit`. DescribeExecution: cause is "the path '$.limit' selected no value" and never names the condition/operand that owned it.
5. Cause 4: a Wait state {"Type":"Wait","SecondsPath":"$.d","End":true} named "W". StartExecution with each of input {"d": -1}, {"d": "abc"}, {"d": 1.5} and {} . DescribeExecution: every cause reads "state 'W': 'Seconds' must be a non-negative whole number" — naming `Seconds`, a member the definition does not contain.
