Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-timestamp-comparators-compare-instants-not-lexical-strings
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-timestamp-comparators-compare-instants-not-lexical-strings|run#4|AssertionError|assert _output(desc) == expected_output, (
Issue-Commit: ac324ffb1b52e9d6af970e8a700749caf205348a

# stepfunctions: Choice Timestamp* comparators do not compare instants — an RFC3339 value with a numeric UTC offset matches no comparator and silently falls through to Default

> _Note: this issue was identified against the code base at commit `ac324ffb1b52e9d6af970e8a700749caf205348a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One root cause behind 1 failing check.

Cause 1 of 1: the ASL Choice state's TimestampEquals / TimestampLessThan / TimestampGreaterThan family does not evaluate an RFC3339 timestamp that carries a numeric UTC offset as the instant it denotes. Such a value satisfies no Timestamp comparator at all, so the rule list falls through to the state's Default branch.

AWS: recorded behavior only (no Smithy member models ASL comparator semantics; the Choice state is a definition document, not a modeled input member). The documented rule (Step Functions Developer Guide, Choice state, Timestamp comparators) is that a Timestamp* comparator compares RFC3339/ISO-8601 *instants*, not lexical strings: against the cutoff `2024-03-01T12:00:00Z`, the value `2024-03-01T13:00:00+01:00` denotes the SAME instant and therefore matches `TimestampEquals` (and must NOT match `TimestampGreaterThan`), and `2024-03-01T08:00:00-05:00` denotes 13:00Z and therefore matches `TimestampGreaterThan` even though it sorts before the cutoff as a string. Only a value that is not a full RFC3339 timestamp at all (e.g. `not-a-timestamp`, or a bare date `2024-03-01`) satisfies nothing and takes `Default`. Every branch and the Default in the replayed definition are Pass states, so the execution ends SUCCEEDED with the branch's result as its output in all cases.

Emulator: the execution SUCCEEDS but routes by the wrong rule. For input `{"when": "<cutoff instant expressed with a +HH:MM offset>"}` the emulator entered the Default state and DescribeExecution returned the Default branch's output (`"unparsed"` in the replayed definition) where AWS enters the TimestampEquals branch (`"same"`). The execution history shows `["Route", "Unparsed"]` instead of `["Route", "Same"]` — i.e. the offset form is treated as an unparseable value by all three Timestamp comparators rather than normalised to an instant. The symmetric `-HH:MM` case in the same check is unreachable behind the first assertion but has the same cause.

Note on related work: the open issue 20261027T000406Z-sfn-step-functions-choice-and-wait-evaluation-diverges-numericequals.md lists this check id, but it carries a different replay key for it — there the offset value FAILED the execution with `States.Runtime` / `"... is not an ISO 8601 instant: ... Smithy does not support timezone offsets in RFC-3339 date times"`. The execution no longer fails; it now succeeds down the wrong branch, so that issue's key no longer selects this failure and the offset form is still not compared as an instant.

Why: any workflow that branches on timestamps produced by a non-UTC emitter (the common case for ISO-8601 producers and for any client that serialises local time) takes the Default/fallback branch instead of the matching one. The divergence is silent — the execution SUCCEEDS — so a client sees wrong routing, wrong output and wrong downstream work with no error to key on, which is strictly worse than the previous hard failure.

Fix: the Choice state comparator evaluator in the sfn crate's ASL interpreter — parse the candidate value and the comparator operand as full RFC3339/ISO-8601 instants including a numeric UTC offset, normalise both to an instant, and compare instants for the whole Timestamp* family; reserve the no-match/Default path for values that are genuinely not full RFC3339 timestamps (non-timestamp strings and bare dates must still match nothing, and must not be date-padded into a comparable instant).

Traces: stepfunctions.choice-timestamp-comparators-compare-instants-not-lexical-strings.

## Evidence
Definition (JSONPath, STANDARD): Choice `Route` with rules [{Variable:$.when, TimestampGreaterThan:<cutoff>} -> After, {Variable:$.when, TimestampEquals:<cutoff>} -> Same, {Variable:$.when, TimestampLessThan:<cutoff>} -> Before], Default -> Unparsed; After/Same/Before/Unparsed are Pass states returning "after"/"same"/"before"/"unparsed". Cutoff = 2024-03-01T12:00:00Z.

Expected (AWS):
- when = 2024-03-01T13:00:00Z        -> output "after",     history [Route, After]
- when = 2024-03-01T12:00:00Z        -> output "same",      history [Route, Same]
- when = 2024-03-01T11:00:00Z        -> output "before",    history [Route, Before]
- when = 2024-03-01T13:00:00+01:00   -> output "same",      history [Route, Same]   (same instant as the cutoff)
- when = 2024-03-01T08:00:00-05:00   -> output "after",     history [Route, After]  (13:00Z)
- when = not-a-timestamp / 2024-03-01 -> output "unparsed", history [Route, Unparsed]

Actual (emulator), from the replayed check:
AssertionError: when='2024-03-01T13:00:00+01:00' routed to output 'unparsed', expected 'same' -- THE SAME INSTANT as the 12:00Z cutoff expressed at +01:00; it sorts AFTER the cutoff as a string, so a lexical comparison wrongly takes TimestampGreaterThan

Status was SUCCEEDED with no error field, so the comparators evaluated and simply matched nothing for the offset form. (The three bare-Z cases passed, which isolates the offset handling.)

## Steps to Reproduce
1. Create an IAM role Step Functions may assume (CreateStateMachine requires a passable roleArn; Choice/Pass states never assume it).
2. CreateStateMachine (type STANDARD, JSONPath) with the Choice definition above: three Timestamp* rules against the cutoff 2024-03-01T12:00:00Z plus a Default, each branch a Pass state returning a distinct string.
3. StartExecution with input {"when": "2024-03-01T13:00:00+01:00"}, wait for a terminal status.
4. DescribeExecution: status SUCCEEDED, output "unparsed" (expected "same"); GetExecutionHistory shows the entered states ["Route", "Unparsed"] (expected ["Route", "Same"]).
5. Repeat with {"when": "2024-03-01T08:00:00-05:00"} — expected output "after" (13:00Z). Control inputs 2024-03-01T13:00:00Z / 12:00:00Z / 11:00:00Z route correctly, and "not-a-timestamp" / "2024-03-01" must still route to Unparsed.
