Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.deep-sfn-language-format-placeholder-count-mismatch-rejected stepfunctions.deep-sfn-language-intrinsic-type-mismatch-rejected stepfunctions.deep-sfn-language-json-merge-deep-mode-rejected stepfunctions.deep-sfn-language-hash-every-algorithm-matches-hashlib stepfunctions.deep-sfn-language-array-and-string-intrinsics-edge-semantics stepfunctions.deep-sfn-language-array-range-over-1000-items-rejected stepfunctions.deep-sfn-language-uuid-distinct-per-call-and-math-random-bounds
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.deep-sfn-language-format-placeholder-count-mismatch-rejected|run#9|AssertionError|assert bad.get("error") == "States.Runtime", (
Issue-Key: stepfunctions.deep-sfn-language-intrinsic-type-mismatch-rejected|run#8|AssertionError|assert bad.get("error") == "States.Runtime", (
Issue-Key: stepfunctions.deep-sfn-language-json-merge-deep-mode-rejected|run#10|AssertionError|assert bad.get("error") == "States.Runtime", (
Issue-Key: stepfunctions.deep-sfn-language-hash-every-algorithm-matches-hashlib|run#7|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-language-array-and-string-intrinsics-edge-semantics|run#5|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-language-array-range-over-1000-items-rejected|run#8|AssertionError|assert bad["status"] == "FAILED", (
Issue-Key: stepfunctions.deep-sfn-language-uuid-distinct-per-call-and-math-random-bounds|run#15|AssertionError|assert set(draws) <= {1, 2}, (
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# Step Functions intrinsic functions diverge: wrong error name, States.Hash unimplemented, Format escapes miscounted, ArrayRange limit unenforced, MathRandom end bound inclusive (5 causes)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
This issue bundles FIVE distinct root causes in the `States.*` intrinsic-function evaluator. All five must be closed before submitting. Re-replay every check id in trace_ids.

Cause 1 of 5: a failing intrinsic surfaces the wrong execution error name.
AWS: recorded behavior only — when an intrinsic cannot be evaluated (wrong placeholder count for `States.Format`, a non-numeric argument to `States.MathAdd`, an unsupported third argument to `States.JsonMerge`) the execution terminates with `error='States.Runtime'`.
Emulator: the execution terminates with `error='States.IntrinsicFailure'` instead, e.g. cause="'States.MathAdd($.v, $.d)': argument 1 of 'States.MathAdd' must be a finite number".
Why: a Catch/Retry block whose `ErrorEquals` lists `States.Runtime` (the name AWS produces) never matches, so error handling that works on AWS is dead in the emulator. The cause text itself is only an example; the error NAME is the requirement.
Fix: the intrinsic-evaluation error mapping in the sfn execution interpreter's intrinsic component — classify evaluation failures as `States.Runtime`.
Traces: sfn.deep-sfn-language-format-placeholder-count-mismatch-rejected, sfn.deep-sfn-language-intrinsic-type-mismatch-rejected, sfn.deep-sfn-language-json-merge-deep-mode-rejected.

Cause 2 of 5: `States.Hash` is not implemented.
AWS: `States.Hash(data, algorithm)` is supported for MD5, SHA-1, SHA-256, SHA-384 and SHA-512 and returns the hex digest; recorded behavior: the check compares the result against hashlib for every algorithm and expects SUCCEEDED.
Emulator: the execution fails with cause="'States.Hash($.s, 'MD5')': 'States.Hash' is an intrinsic function this emulator does not evaluate, so nothing was run for it".
Why: any definition using States.Hash fails outright instead of returning a digest.
Fix: add States.Hash to the intrinsic-function registry/evaluator in the sfn crate.
Traces: sfn.deep-sfn-language-hash-every-algorithm-matches-hashlib.

Cause 3 of 5: `States.Format` counts ESCAPED braces as placeholders.
AWS: in a `States.Format` template, `\{` and `\}` are literal characters and are NOT placeholders; only bare `{}` consume an argument. The recorded template `States.Format('x\{\} {} \'q\' {}', $.s, $.n)` has two placeholders and two arguments and evaluates successfully.
Emulator: the execution fails with cause="'States.Format('x\{\} {} \'q\' {}', $.s, $.n)': 'States.Format' needs at least 4 arguments" — the escaped `\{\}` was counted as a third placeholder.
Why: templates containing literal braces or escaped quotes are rejected, so string formatting that works on AWS fails the execution.
Fix: the `States.Format` placeholder scanner in the sfn intrinsic evaluator must honour backslash escaping of `{`, `}`, `'` and `\`.
Traces: sfn.deep-sfn-language-array-and-string-intrinsics-edge-semantics.

Cause 4 of 5: `States.ArrayRange` does not enforce its 1,000-item result limit.
AWS: recorded behavior only — `States.ArrayRange` producing more than 1,000 items fails the execution.
Emulator: a 1,001-item ArrayRange evaluates and the execution ends `SUCCEEDED`.
Why: a definition AWS refuses at runtime silently produces an oversized array, so a client's quota-exceeded path is never exercised and downstream payload limits shift.
Fix: a result-size check in the `States.ArrayRange` implementation of the sfn intrinsic evaluator.
Traces: sfn.deep-sfn-language-array-range-over-1000-items-rejected.

Cause 5 of 5: `States.MathRandom` treats the end bound as inclusive.
AWS: recorded behavior only — `States.MathRandom(start, end)` yields values in `[start, end)`; the recording of `States.MathRandom(1, 3)` only ever produced 1 and 2.
Emulator: repeated evaluation produced [1, 2, 3] — the end bound is included.
Why: a workflow using MathRandom to index into an array of length `end - start` can select an out-of-range index, producing failures that cannot happen on AWS.
Fix: the `States.MathRandom` implementation in the sfn intrinsic evaluator (exclusive upper bound).
Traces: sfn.deep-sfn-language-uuid-distinct-per-call-and-math-random-bounds.

## Evidence
Cause 1: expected error 'States.Runtime'; got 'States.IntrinsicFailure' for all three of States.Format arity, States.MathAdd type mismatch and States.JsonMerge deep mode.
Cause 2: expected SUCCEEDED with digests matching hashlib; got FAILED error='States.IntrinsicFailure' cause="'States.Hash($.s, 'MD5')': 'States.Hash' is an intrinsic function this emulator does not evaluate, so nothing was run for it".
Cause 3: expected SUCCEEDED; got FAILED error='States.IntrinsicFailure' cause="'States.Format('x\{\} {} \'q\' {}', $.s, $.n)': 'States.Format' needs at least 4 arguments".
Cause 4: expected the execution to FAIL for a 1001-item ArrayRange; got 'SUCCEEDED'.
Cause 5: expected MathRandom(1, 3) values within [1, 3); got the observed set [1, 2, 3].

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: CreateStateMachine with a Pass state whose Parameters contain "x.$": "States.MathAdd($.v, $.d)" with a non-numeric $.v; StartExecution and read DescribeExecution.error.
3. Cause 2: same shape using "States.Hash($.s, 'MD5')".
4. Cause 3: same shape using "States.Format('x\\{\\} {} \\'q\\' {}', $.s, $.n)" with exactly two arguments.
5. Cause 4: same shape using "States.ArrayRange(1, 1001, 1)".
6. Cause 5: same shape using "States.MathRandom(1, 3)" executed repeatedly; collect the values.
