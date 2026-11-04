Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule stepfunctions.deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output stepfunctions.intrinsic-function-library-evaluates-exactly stepfunctions.deep-sfn-language-hash-unsupported-algorithm-rejected stepfunctions.deep-sfn-language-choice-nested-and-or-not-path-comparators stepfunctions.execution-output-numeric-precision-preserved stepfunctions.sfn-retry-with-backoff stepfunctions.sfn-task-timeout
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule|run#5|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output|run#8|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.intrinsic-function-library-evaluates-exactly|run#4|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-language-hash-unsupported-algorithm-rejected|run#4|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.deep-sfn-language-choice-nested-and-or-not-path-comparators|run#5|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.execution-output-numeric-precision-preserved|run#14|AssertionError|assert output["bigInt"] == big_int, f"{output['bigInt']} != {big_int}"
Issue-Key: stepfunctions.sfn-retry-with-backoff|run#7|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.sfn-task-timeout|run#8|AssertionError|assert desc.get("error") == "States.Timeout", (
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sfn interpreter and data fidelity: five causes still open — a Choice rule's own Assign, a Catch reading $states.errorOutput, States.Hash, Choice *Path type mismatches, large-integer precision, and the optimized lambda:invoke (5 causes)

> _These are the causes of the nine-cause bundle
> `20261029T183117Z-sfn-interpreter-and-data-fidelity-nine-net-new.md` that are STILL
> OPEN: their eight checks still diverge. The four causes of that bundle that are
> resolved, with their nine checks, are recorded under
> `issues/things-that-now-work/`. Each cause below names the file the symptom shows in
> and the prerequisite it is blocked behind._

## Operation
StartExecution

## Description

Cause 1 of 5: a `Choice` rule's OWN `Assign` is not what the selected rule assigns.
  AWS: recorded behavior only (the ASL query language is not in the Smithy model).
  "You can use Assign in a Choice rule": the rule the Choice SELECTED assigns its own
  variables, and those — not the other rules' and not the state-level `Assign` — are
  what the states after the Choice read. The first rule whose `Condition` holds wins,
  even when a later rule's condition holds too.
  Emulator: with an input satisfying BOTH rules' conditions the right branch is taken
  but the state-level `Assign` is what the output carries: `{"branch": "first",
  "tag": "from-state-level"}` where AWS answers `{"branch": "first",
  "tag": "from-first-rule"}`.
  Fix: the `Choice` state's own evaluation in
  `services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs`
  (`Interpreter::choose`, and the `Assign`/`Output` order in
  `Interpreter::jsonata_output`).
  Check: `.choice-jsonata-assign-is-scoped-to-the-selected-rule`.

Cause 2 of 5: a `Catch` whose `Output` reads `$states.errorOutput` does not recover
the execution.
  AWS: recorded behavior only. A catcher that matches binds `$states.errorOutput` for
  its own `Output` and `Assign`, and the execution carries on at the catcher's `Next`.
  Emulator: the execution ends `FAILED` with the task's own error
  (`error='DynamoDB.ResourceNotFoundException' cause='Requested resource not found'`)
  as though no catcher had matched.
  Fix: the catcher half of the walk in `src/state/asl.rs` — which members of a
  `Catch` a JSONata state's catcher may carry, and the `$states.errorOutput` binding
  it is evaluated with.
  Check: `.deep-sfn-2-sfn-jsonata-catch-output-from-states-error-output`.

Cause 3 of 5: the `States.Hash` intrinsic is not evaluated — any state using it fails
with `States.Runtime` and the cause `'States.Hash' is an intrinsic function this
emulator does not evaluate, so nothing was run for it`.
  AWS: recorded behavior only. AWS evaluates `States.Hash(data, algorithm)` for MD5,
  SHA-1, SHA-256, SHA-384 and SHA-512 and returns the lowercase hex digest, and
  refuses an algorithm outside that set with `States.IntrinsicFailure` naming the
  algorithm the request asked for.
  Why: hashing-dependent workflows cannot run, and the unsupported-algorithm negative
  is unreachable because the supported control already fails. The digests belong to a
  maintained crate (`md-5`, `sha1`, `sha2`, all already in `Cargo.lock`) and the hex
  coding to `hex`, not to a hand-written implementation; the current refusal is a
  deliberate decline recorded in `apply_intrinsic`'s own doc comment, so that comment
  has to go with the fix.
  Fix: the intrinsic-function library in `src/state/asl.rs` (`apply_intrinsic`).
  Checks: `.intrinsic-function-library-evaluates-exactly`,
  `.deep-sfn-language-hash-unsupported-algorithm-rejected`.

Cause 4 of 5: a `Choice` comparator whose two operand VALUES are not of the type the
comparator compares fails the execution instead of not matching.
  AWS: recorded behavior only. A comparator it cannot apply to the data simply does
  not match, so the enclosing `And`/`Or`/`Not` evaluates normally and the next rule or
  the `Default` is taken; the execution succeeds.
  Emulator: `status='FAILED'`, `error='States.Runtime'`, `cause="the choice rule on
  '$.n' compares 3 with \"zeta\" through 'NumericEqualsPath', which compares numbers"`.
  Why: a `Choice` whose arms deliberately test for absence or mismatch — the normal
  way to write a defensive router — aborts the execution instead of routing. The
  structural defects (an operand that is not a path string for a `*Path` comparator,
  a comparator name the language does not define) must stay failures: it is the
  judgement about the DATA that is a non-match.
  Fix: `compare` in `src/state/asl.rs`.
  Check: `.deep-sfn-language-choice-nested-and-or-not-path-comparators`.

Cause 5 of 5: integers in execution data lose precision — they are round-tripped
through a double.
  AWS: recorded behavior only for the JSON representation. AWS carries a JSON number
  through unchanged, so a 30-digit integer written in a `Pass` `Result` comes back
  from `DescribeExecution` with the digits it was written with.
  Emulator: `1.2345678901234568e+29 != 123456789012345678901234567890` — the value is
  re-encoded in exponential float form.
  Why: any workflow carrying ids, amounts or counters larger than 2^53 silently
  corrupts them, which no client can detect from the response.
  Fix: the JSON number representation of the sfn crate's execution data
  (`src/state/asl.rs` and `src/state.rs`). `serde_json` without `arbitrary_precision`
  cannot hold such a number at all, and that feature is not additive — enabling it
  unifies across the whole workspace and changes how every `serde_json::Value` under
  a `#[serde(flatten)]`/`#[serde(untagged)]` seam deserializes (for example
  `services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/state.rs`),
  so the representation has to be decided for this crate rather than switched on
  globally.
  Check: `.execution-output-numeric-precision-preserved`.

Cause 6 of 5 (the fifth distinct cause, two checks): the optimized
`arn:aws:states:::lambda:invoke` integration reaches Lambda and Lambda runs nothing,
so Retry/backoff and `TimeoutSeconds` contracts cannot be observed.
  AWS: recorded behavior only. AWS invokes the function under the execution role;
  repeated handler failures drive the Retry policy's backoff and then the `Catch`, and
  a `Task` exceeding `TimeoutSeconds` fails with `States.Timeout`.
  Emulator: the Step Functions side DOES dispatch the integration now; the task fails
  with `States.Runtime` and the cause `the 'Invoke' operation of Lambda is not
  implemented by this emulator, so nothing was run for this task`, because
  `Invoke`'s `RequestResponse` path has no in-process runtime for a deployment
  package
  (`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs`, the
  `NotImplemented` at the end of `handle`). Reporting a retry sequence, a backoff or a
  `States.Timeout` from the Step Functions side would be an outcome no engine
  produced.
  Fix: `Invoke`'s synchronous path in the Lambda service crate — a contained, bounded
  in-process engine for the function's deployment package — after which the Step
  Functions Retry and `TimeoutSeconds` halves can be observed and asserted.
  Checks: `.sfn-retry-with-backoff`, `.sfn-task-timeout`.

## Evidence
Cause 1: `AssertionError: input {'n': 50} produced {'branch': 'first', 'tag': 'from-state-level'}, expected {'branch': 'first', 'tag': 'from-first-rule'}`.
Cause 2: `AssertionError: the Catch did not recover the execution: 'FAILED' error='DynamoDB.ResourceNotFoundException' cause='Requested resource not found'`.
Cause 3: `AssertionError: intrinsic evaluation must succeed, got FAILED error=States.Runtime cause='States.Hash($.a, 'SHA-256')': 'States.Hash' is an intrinsic function this emulator does not evaluate, so nothing was run for it`.
Cause 4: `AssertionError: [R1 blocked: both Or arms false] execution must succeed, got 'FAILED' error='States.Runtime' cause='the choice rule on \'$.n\' compares 3 with "zeta" through \'NumericEqualsPath\', which compares numbers'`.
Cause 5: `AssertionError: 1.2345678901234568e+29 != 123456789012345678901234567890`.
Cause 6: `AssertionError: Expected SUCCEEDED via catch-after-retries, got FAILED: the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task`; `AssertionError: Expected error States.Timeout, got error=States.Runtime ...`.

## Steps to Reproduce
Cause 1: CreateStateMachine with `QueryLanguage=JSONata` and a `Choice` whose first
and second rules both match the input and each carry their own `Assign`, beside a
state-level `Assign`; StartExecution and read `DescribeExecution.output`.
Cause 2: a JSONata `Task` whose integration call fails, with a `Catch` whose `Output`
reads `$states.errorOutput`; StartExecution.
Cause 3: StartExecution on a `Pass` whose `Parameters` use
`States.Hash($.s, 'SHA-256')`, and once with an algorithm outside the documented set.
Cause 4: StartExecution on a `Choice` whose `Or` arms use `NumericEqualsPath` against
a string-valued path, with a `Default` `Pass`.
Cause 5: StartExecution on a `Pass` whose `Result` is the integer
`123456789012345678901234567890`, and read `DescribeExecution.output`.
Cause 6: StartExecution on a `Task` whose `Resource` is
`arn:aws:states:::lambda:invoke` with `Retry` and `TimeoutSeconds`.

## Notes
The remaining interpreter and data-representation causes all show in
`services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs` (8862 lines)
and `src/state.rs` (7538 lines), both past the 5000-line limit a change may touch, so
a behaviour-preserving module split of each — the validation/diagnostics half, the
Interpreter's walk, the path/payload-template/choice/intrinsic half, and the
single-state `TestState` half — is the prerequisite and belongs in its own change
ahead of them.
