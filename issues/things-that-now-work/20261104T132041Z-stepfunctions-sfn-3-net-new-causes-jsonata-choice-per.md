Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule stepfunctions.choice-jsonata-non-boolean-condition-rejected stepfunctions.deep-sfn-1-sfn-sync-parallel-sibling-fail-aborts-inflight-child
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule|run#5|AssertionError|assert _output(desc) == {"branch": branch, "tag": expected_tag}, (
Issue-Key: stepfunctions.choice-jsonata-non-boolean-condition-rejected|run#8|AssertionError|assert "Choices[0]/Condition" in cause, (
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-parallel-sibling-fail-aborts-inflight-child|_poll#8|AssertionError|raise AssertionError(f"{what}: not reached within {timeout_s}s; last={last!r}")
Issue-Commit: 7cd635da05c853b3aee981acc4bc8b13c0b954d8

# sfn: 3 net-new causes — JSONata Choice per-rule Assign ignored in favour of the state-level Assign, the non-boolean/undefined Condition cause omits the field path and result type, and the in-flight child of a startExecution.sync task in a Parallel branch is never observable RUNNING

> _Note: this issue was identified against the code base at commit `7cd635da05c853b3aee981acc4bc8b13c0b954d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Three independent root causes in the Step Functions interpreter, all surfaced by stepfunctions behavioral checks. Related open issues describe JSONata as never evaluated at all (20261029T183117Z-sfn-interpreter-and-data-fidelity-nine-net-new.md, 20261028T095020Z-sfn-step-functions-jsonata-definitions-are-never-evaluated-at.md); these three are different and net-new — here JSONata IS evaluated and the routing IS correct, but the variable scoping, the diagnostic wording and the .sync child lifecycle diverge. None of these checks failed on a refusal of an operation a fixed issue claimed to fix, so none is a reopen.

=== Cause 1 of 3: a JSONata Choice applies the STATE-level Assign on a matched rule instead of the selected rule's own Assign ===
AWS: recorded behavior only (ASL semantics are not in the Smithy model). In JSONata query language a Choice state may carry an `Assign` block on the state itself and an `Assign` block on each rule in `Choices`. First match wins, and the variables that reach the branch target are the SELECTED rule's `Assign` — not a non-selected rule's, and not the state-level one. The state-level `Assign` applies only when no rule matches and the `Default` transition is taken. Consequently, if the rules carry no `Assign` of their own, a MATCHED rule leaves the variable undefined and the branch target that dereferences it fails the execution with error `States.QueryEvaluationError` (modeled: HistoryEventType includes `EvaluationFailed`; DescribeExecution carries `error`/`cause`), while the `Default` path of the same machine still succeeds because the state-level `Assign` does run there.
Emulator: for a Choice whose state-level `Assign` sets `tag = 'from-state-level'` and whose first and second rules set `tag = 'from-first-rule'` / `'from-second-rule'`, input `{"n": 50}` (satisfying both Conditions) routes correctly to the first rule's target — history entered `["Route", "First"]` — but the target echoes `{"branch": "first", "tag": "from-state-level"}` instead of `{"branch": "first", "tag": "from-first-rule"}`. The emulator runs the state-level Assign on the matched path and never runs the selected rule's Assign.
Why: the execution SUCCEEDS with the wrong variable value, so a client relying on per-rule assignment silently gets the fallback/default value in its output and in every downstream state. The mirror-image defect is just as damaging: a definition with only a state-level Assign will succeed where AWS fails the execution with `States.QueryEvaluationError`, turning a broken workflow into a silent success.
Fix: the Choice-state evaluator in the state-machine interpreter — the variable-assignment step must bind the selected rule's `Assign` (and only it) for a matched rule, and the state's own `Assign` only on the `Default` transition.
Traces: stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule

=== Cause 2 of 3: the States.QueryEvaluationError cause for a mistyped/undefined Choice Condition is emulator-authored — no field path, no result type ===
AWS: recorded behavior only for the exact sentence; the status/code side is modeled (DescribeExecution.cause is a `SensitiveCause`, max 32768; HistoryEventType `EvaluationFailed`). When a JSONata `Condition` evaluates to a non-boolean the execution FAILS with error `States.QueryEvaluationError`, and the cause names (a) the definition field path of the expression that misevaluated — `Choices[0]/Condition`, where the index is the position of the offending rule in that state's `Choices` array, (b) the fixed text `Expected 'boolean'`, and (c) the fixed text `but was '<type>'` where `<type>` is the JSON type actually produced (`string` for `"hello"`, `number` for `1`). The field path, the index and the type name are the variable parts taken from the request's definition and the evaluated value; the rest is fixed text. An expression that resolves to nothing is a DIFFERENT diagnosis under the same error name, whose cause says `returned nothing (undefined)`, and the two causes must not collapse to one byte-identical string.
Emulator: the error name is right but the cause is the emulator's own sentence: `a 'Choice' rule's 'Condition' must produce a boolean, and this one produced "hello"`. It carries no `Choices[0]/Condition` field path, never says `Expected 'boolean'`, and reports the offending VALUE rather than its type (`but was 'string'`).
Why: in a definition with many Choice rules the caller cannot locate the rule that misevaluated, and cannot machine-read the produced type; message-matching clients and runbooks keyed on AWS's wording do not match.
Fix: the JSONata evaluation-failure path of the Choice-state evaluator, where the `States.QueryEvaluationError` cause is composed (and the matching `EvaluationFailed` / `ExecutionFailed` history-event details, which must carry byte-identical cause text to DescribeExecution).
Traces: stepfunctions.choice-jsonata-non-boolean-condition-rejected

=== Cause 3 of 3: the child execution of a states:startExecution.sync task inside a Parallel branch is already ABORTED when the parent records TaskSubmitted — it is never observable RUNNING ===
AWS: recorded behavior only. A `arn:aws:states:::states:startExecution.sync` Task starts a real child execution and blocks on it; from the moment the parent emits `TaskSubmitted` (whose `taskSubmittedEventDetails.output` carries the child `ExecutionArn`) the child is a normal execution in status `RUNNING` (modeled ExecutionStatus) and stays RUNNING until its own workflow finishes or it is aborted. Only when a SIBLING Parallel branch reaches its `Fail` state does the parent fail and abort the still-in-flight child, which then ends `ABORTED` with `ExecutionAborted` last in its history and no `WaitStateExited`. With a sibling branch that waits 12 s before failing and a child that waits 60 s, the child is therefore RUNNING for the first several seconds after TaskSubmitted.
Emulator: polling `DescribeExecution` on the child for 30 s immediately after the parent's `TaskSubmitted` event never once observes `RUNNING` — every observation, including the first, is `ABORTED`. The child is torn down before it is ever reachable in a running state, so the sibling-abort ordering the check exists to prove cannot be observed at all. Hypothesis: either the sibling branch's `Wait` is not honoured (so the Parallel fails and cancels the child immediately) or the .sync task aborts its child right after submitting it.
Why: a client that starts a long child via `.sync` and polls it — a standard pattern for progress reporting and for cancelling a child out of band — sees a child that is already dead, so every in-flight observation, heartbeat and cancellation window is lost; and a workflow whose correctness depends on a sibling branch's delay ordering behaves as if the delay were zero.
Fix: the `states:startExecution.sync` task integration in the interpreter together with the Parallel-state branch scheduler and the Wait-state timer — the child must be left RUNNING and only aborted when a sibling branch actually reaches its terminal failure.
Traces: stepfunctions.deep-sfn-1-sfn-sync-parallel-sibling-fail-aborts-inflight-child

## Evidence
Cause 1 — stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule:
  AssertionError: input {'n': 50} produced {'branch': 'first', 'tag': 'from-state-level'}, expected {'branch': 'first', 'tag': 'from-first-rule'} -- n=50 satisfies BOTH Conditions; the first rule must win, and its own Assign -- not the second rule's and not the state-level one -- must be the value that reaches the branch target
  Definition (JSONata): Route = Choice with state-level Assign {tag: 'from-state-level'}; Choices[0] = {Condition: $states.input.n > 10, Assign: {tag: 'from-first-rule'}, Next: First}; Choices[1] = {Condition: $states.input.n > 0, Assign: {tag: 'from-second-rule'}, Next: Second}; Default: Fallback. Targets are Pass states whose Output echoes {% $tag %}.
  Expected per input: {n:50} -> SUCCEEDED, output {branch:'first', tag:'from-first-rule'}, entered ['Route','First']; {n:5} -> {branch:'second', tag:'from-second-rule'}; {n:-1} -> {branch:'fallback', tag:'from-state-level'}.
  Expected on the state-level-only twin (per-rule Assign removed): {n:50} and {n:5} FAIL with error 'States.QueryEvaluationError', cause containing "returned nothing (undefined)" and "executing the state 'First'"/"'Second'", output absent, while {n:-1} still SUCCEEDS with tag 'from-state-level'.
  Actual: routing correct (entered ['Route','First']) but tag = 'from-state-level' on the matched path.

Cause 2 — stepfunctions.choice-jsonata-non-boolean-condition-rejected:
  AssertionError: the cause must name the field whose expression misevaluated, so a caller can find it in a definition with many Choices; got 'a \'Choice\' rule\'s \'Condition\' must produce a boolean, and this one produced "hello"'
  Expected cause substrings, for a Condition evaluating to a string: "Choices[0]/Condition", "Expected 'boolean'", "but was 'string'"; for a number Condition: "but was 'number'". Expected error name: States.QueryEvaluationError (this part matched). Expected history: an EvaluationFailed event plus ExecutionFailed whose executionFailedEventDetails.error is States.QueryEvaluationError and whose cause is byte-identical to DescribeExecution.cause; entered states exactly ['Route'] (neither branch target entered); DescribeExecution.output absent.
  Expected for an undefined result ({"other": ...}, no $.label at all): same error name, but a DISTINCT cause containing "returned nothing (undefined)".
  Actual: emulator-authored sentence quoting the value instead of the type, with no definition field path and no "Expected 'boolean'" text.

Cause 3 — stepfunctions.deep-sfn-1-sfn-sync-parallel-sibling-fail-aborts-inflight-child:
  AssertionError: child arn:aws:states:<region>:<account>:execution:<child-machine-name>:<exec-id> RUNNING: not reached within 30s; last='ABORTED'
  Setup: child machine = single Wait of 60 s. Parent = Parallel with branch A = Task arn:aws:states:::states:startExecution.sync on the child, branch B = Wait 12 s then Fail(Error 'SiblingBoom', Cause <generated>).
  Expected: right after the parent's TaskSubmitted event the child DescribeExecution.status == 'RUNNING' (within 30 s); then the parent ends FAILED with exactly one ExecutionFailed carrying branch B's error/cause, exactly one FailStateEntered, and NO TaskSucceeded; then the child leaves RUNNING for 'ABORTED', stops after branch B's FailStateEntered timestamp and within 45 s of it, runs less than its full 60 s Wait, its history ends with ExecutionAborted and contains no WaitStateExited.
  Actual: the child is 'ABORTED' on the very first and every subsequent poll in the 30 s window — RUNNING is never observed, so the whole sibling-abort ordering is unobservable.

## Steps to Reproduce
Cause 1:
1. Create an IAM role trusted by states.amazonaws.com.
2. CreateStateMachine (STANDARD) with a JSONata definition: StartAt 'Route'; 'Route' is a Choice with state-level Assign {"tag": "{% 'from-state-level' %}"}, Choices[0] = {Condition "{% $states.input.n > 10 %}", Assign {"tag": "{% 'from-first-rule' %}"}, Next 'First'}, Choices[1] = {Condition "{% $states.input.n > 0 %}", Assign {"tag": "{% 'from-second-rule' %}"}, Next 'Second'}, Default 'Fallback'; First/Second/Fallback are Pass states with Output {"branch": "<name>", "tag": "{% $tag %}"} and End true.
3. StartExecution with input {"n": 50}; poll DescribeExecution to a terminal status.
4. Observe output tag == 'from-state-level'; AWS yields 'from-first-rule'. Repeat with {"n": 5} (expect 'from-second-rule') and {"n": -1} (expect 'from-state-level').
5. Create the same machine with the two per-rule Assign blocks REMOVED and run {"n": 50} / {"n": 5}: AWS FAILS these with States.QueryEvaluationError (cause says the expression "returned nothing (undefined)" while "executing the state 'First'"/'Second'), and still SUCCEEDS on {"n": -1}.

Cause 2:
1. CreateStateMachine (STANDARD, JSONata) with StartAt 'Route'; 'Route' is a Choice whose Choices[0].Condition is an expression over $states.input.label that returns the label verbatim, Next 'Matched', Default 'Fallback'.
2. StartExecution with input {"label": "hello"}; poll DescribeExecution to terminal.
3. DescribeExecution: status FAILED and error States.QueryEvaluationError (both correct), but inspect `cause` — it lacks "Choices[0]/Condition", "Expected 'boolean'" and "but was 'string'".
4. Repeat with {"label": 1} (expect "but was 'number'") and with {"other": "no label at all"} (expect a DISTINCT cause containing "returned nothing (undefined)").
5. GetExecutionHistory: confirm an EvaluationFailed event exists and ExecutionFailed's cause equals DescribeExecution's cause.

Cause 3:
1. Create a role trusted by states.amazonaws.com allowing states:StartExecution on the child machine and states:DescribeExecution/StopExecution on its executions.
2. CreateStateMachine 'child' = {StartAt 'Hold', States {Hold: {Type Wait, Seconds 60, End true}}}.
3. CreateStateMachine 'parent' = Parallel 'Fan' with branch A = {CallChild: Task, Resource arn:aws:states:::states:startExecution.sync, Parameters {StateMachineArn <child arn>, Input {...}}, End true} and branch B = {Delay: Wait 12 s -> Boom: Fail(Error 'SiblingBoom', Cause <any>)}.
4. StartExecution on the parent; poll GetExecutionHistory until a TaskSubmitted event appears and read the child ExecutionArn from taskSubmittedEventDetails.output.
5. Immediately poll DescribeExecution on that child ARN: AWS reports RUNNING; the emulator reports ABORTED on the first poll and never RUNNING within 30 s.
