Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__stepfunctions.confused-deputy-source-arn-mismatch-denied iam__stepfunctions.distributed-map-csv-item-reader-invalid-delimiter-rejected-at-create iam__stepfunctions.retry-maxattempts-negative-literal-rejected-at-create-state-machine iam__stepfunctions.pass-state-result-builder-reads-variable-before-its-own-assign-commits iam__stepfunctions.task-retry-attempt-budget-resets-on-redrive iam__stepfunctions.test-state-denied-without-permission iam__stepfunctions.test-state-reveal-secrets-requires-permission
Issue-Key: iam__stepfunctions.confused-deputy-source-arn-mismatch-denied|run#16|AssertionError|assert bad["status"] == "FAILED" and bad["error"] == "States.TaskFailed", (
Issue-Key: iam__stepfunctions.distributed-map-csv-item-reader-invalid-delimiter-rejected-at-create|_refusal#9|AssertionError|raise AssertionError(f"expected the call to be refused, it succeeded with {result!r}")
Issue-Key: iam__stepfunctions.retry-maxattempts-negative-literal-rejected-at-create-state-machine|_error#4|AssertionError|raise AssertionError(f"expected CreateStateMachine to be refused, but it succeeded: {result!r}")
Issue-Key: iam__stepfunctions.pass-state-result-builder-reads-variable-before-its-own-assign-commits|run#4|AssertionError|assert nata_out == {"seen": 1, "nextSeen": 2}, (
Issue-Key: iam__stepfunctions.task-retry-attempt-budget-resets-on-redrive|run#5|AssertionError|assert desc["error"] == "Ec2.Ec2Exception", f"execution error was {desc['error']!r}"
Issue-Key: iam__stepfunctions.test-state-denied-without-permission|run#14|AssertionError|assert refused == expected_denied, f"expected {expected_denied!r}, got {refused!r}"
Issue-Key: iam__stepfunctions.test-state-reveal-secrets-requires-permission|_settle#7|AssertionError|raise AssertionError(
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# iam__stepfunctions: 7 net-new causes — trust/permission gates for Task execution and TestState are inert, two invalid definitions are accepted at CreateStateMachine, JSONata Assign is visible too early, task failure errors are mislabelled, and a denial names the wrong resource

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
Seven distinct root causes in the Step Functions authorization and definition-validation surface.

Cause 1 of 7: a Task whose execution-role trust can never be satisfied still succeeds.
AWS: when the execution role's trust policy carries an aws:SourceArn condition that can never equal the calling state machine's own ARN, Step Functions cannot assume the role, so every attempt fails: DescribeExecution reports status FAILED with error `States.TaskFailed`. Smithy: sfn#DescribeExecution returns status and error; the confused-deputy trust evaluation is a service fact — recorded behavior only, and the recording shows every attempt FAILED.
Emulator: the execution reports status 'SUCCEEDED' with error None — the role was never actually assumed.
Why: the confused-deputy guard is the control that stops one state machine from borrowing another's role. Here it is inert, and a client that tests it sees a pass where AWS fails the execution.
Fix: the Task execution path must assume the execution role through STS with the full request context (SourceArn/SourceAccount) and fail the Task on refusal.
Traces: iam__stepfunctions.confused-deputy-source-arn-mismatch-denied.

Cause 2 of 7: a Distributed Map CSV ItemReader with an invalid delimiter is accepted at create time.
AWS: CreateStateMachine validates the definition and refuses a CSV ItemReader whose delimiter is not a supported value with an InvalidDefinition-class client error; no state machine is created. Smithy: sfn#CreateStateMachine declares InvalidDefinition — the refusal is modelled.
Emulator: CreateStateMachine returns 200 with a stateMachineArn.
Why: the authoring error is accepted and surfaces only at run time (if at all), so a client's CI-time definition validation passes on a definition AWS rejects.
Fix: ItemReader/ReaderConfig validation in the definition validator used by CreateStateMachine/UpdateStateMachine.
Traces: iam__stepfunctions.distributed-map-csv-item-reader-invalid-delimiter-rejected-at-create.

Cause 3 of 7: a Retry MaxAttempts negative literal is accepted at create time.
AWS: CreateStateMachine refuses a Retrier whose MaxAttempts is a negative literal with an InvalidDefinition-class client error; no state machine is created. Smithy: sfn#CreateStateMachine declares InvalidDefinition — the refusal is modelled.
Emulator: CreateStateMachine returns 200 with a stateMachineArn.
Why: same as above — an invalid retry policy is deployed and the client's definition gate is useless.
Fix: Retrier field validation in the definition validator.
Traces: iam__stepfunctions.retry-maxattempts-negative-literal-rejected-at-create-state-machine.

Cause 4 of 7: a JSONata state's own Assign is visible to its own output builder.
AWS: in a JSONata Pass state, the state's Output expression is evaluated against the variable values committed BEFORE its own Assign, so the state reads the earlier value ($x == 1) and only the NEXT state sees the value its Assign committed ($x == 2). Smithy: the evaluation order is interpreter behaviour — recorded behavior only; the recording pins {'seen': 1, 'nextSeen': 2}.
Emulator: the state reads its own assignment: {'seen': 2, 'nextSeen': 2}.
Why: a workflow that reads a variable while updating it computes different values here than on AWS — a silent data-correctness divergence in state output, not an error the client can detect.
Fix: the variable-commit ordering in the JSONata Assign/Output evaluation of the state interpreter.
Traces: iam__stepfunctions.pass-state-result-builder-reads-variable-before-its-own-assign-commits.

Cause 5 of 7: a service-integration task failure is reported as States.Runtime rather than the service's own error name.
AWS: when an AWS-SDK/optimized integration Task fails because the called service refused, the execution's error is the service-qualified error name (here `Ec2.Ec2Exception`), which is what Retry/Catch match on; States.Runtime is reserved for interpreter-level failures. Smithy: sfn#DescribeExecution.error carries that name — recorded behavior only for the mapping, pinned by the recording.
Emulator: the execution error is 'States.Runtime'.
Why: Retry and Catch rules written against the service error name never match, so error handling and the redrive attempt-budget semantics the check exercises cannot work; the client also loses the identity of the failing service.
Fix: the service-integration error mapping in the Task executor, which must surface `<Service>.<ErrorName>` for service refusals.
Traces: iam__stepfunctions.task-retry-attempt-budget-resets-on-redrive.

Cause 6 of 7: the states:TestState denial names the wrong resource.
AWS: a principal without states:TestState is refused with `AccessDeniedException` and the message `User: <principal arn> is not authorized to perform: states:TestState on resource: arn:aws:states:<region>:<account>:*:* because no identity-based policy allows the states:TestState action` — TestState is authorized against the wildcard state-machine/activity resource ARN, not a bare `*`. Fixed text is the standard no-policy wording; the principal ARN, region and account come from the request.
Emulator: the same message but with `on resource: *`.
Why: a client that parses the denial to learn which resource was refused (or that compares messages) gets a different resource than AWS reports, so least-privilege diagnosis points at the wrong ARN shape.
Fix: the resource-ARN construction for TestState in the Step Functions authorization gate's denial message.
Traces: iam__stepfunctions.test-state-denied-without-permission.

Cause 7 of 7: TestState's revealSecrets=true is served without states:RevealSecrets.
AWS: TestState with revealSecrets=true additionally requires states:RevealSecrets; a caller without it is refused with `AccessDeniedException`. Smithy: sfn#TestState takes revealSecrets and declares AccessDeniedException — the refusal is modelled.
Emulator: every attempt succeeds (twelve consecutive 'OK' outcomes over 120 s), so secret-revealing test runs are unguarded.
Why: the permission exists so that secret material in a state's input/output is only exposed to principals explicitly trusted with it. Unenforced, any caller that may test a state can read those secrets.
Fix: the TestState handler's authorization step must charge states:RevealSecrets when revealSecrets is true.
Traces: iam__stepfunctions.test-state-reveal-secrets-requires-permission.

## Evidence
Cause 1 — Expected FAILED/States.TaskFailed; Actual 'SUCCEEDED' error=None.
Cause 2 — Expected refusal; Actual 200 {'stateMachineArn': 'arn:aws:states:…:stateMachine:…'}.
Cause 3 — Expected refusal; Actual 200 {'stateMachineArn': 'arn:aws:states:…:stateMachine:…'}.
Cause 4 — Expected {'seen': 1, 'nextSeen': 2}; Actual {'seen': 2, 'nextSeen': 2}.
Cause 5 — Expected error 'Ec2.Ec2Exception'; Actual 'States.Runtime'.
Cause 6 — Expected '… on resource: arn:aws:states:us-west-2:000000000000:*:* because no identity-based policy allows the states:TestState action'; Actual '… on resource: * because …'.
Cause 7 — Expected AccessDeniedException three times running; Actual twelve consecutive 'OK'.

## Steps to Reproduce
Cause 1: create an execution role whose trust has an aws:SourceArn that cannot match the machine's ARN; run a Task that calls out through the role; DescribeExecution.
Cause 2/3: CreateStateMachine with a Distributed Map CSV ItemReader using an unsupported delimiter, and separately with a Retrier MaxAttempts of -1; observe 200.
Cause 4: run a JSONata Pass state that both reads $x in Output and sets $x in Assign, followed by a state that reads $x.
Cause 5: run a Task whose EC2 service integration is refused; read DescribeExecution.error.
Cause 6: call TestState as a principal with no states: grant and compare the denial's resource.
Cause 7: call TestState with revealSecrets=true as a principal without states:RevealSecrets.
