Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources
Issue-Key: cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: c0b127c099d8543a6fa74c4b10d6d49cf92e859e

# cloudformation__lambda: UpdateStack on a CREATE_FAILED stack without DisableRollback is accepted instead of refused with ValidationError (1 cause)

> _Note: this issue was identified against the code base at commit `c0b127c099d8543a6fa74c4b10d6d49cf92e859e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: UpdateStack against a stack in CREATE_FAILED, submitted WITHOUT DisableRollback, succeeds — AWS refuses it and tells the caller which two APIs can move the stack out of that state.
Failing check ids for this cause: cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources.

AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): UpdateStack(StackName required, DisableRollback boolean) models InsufficientCapabilitiesException and TokenAlreadyExistsException; a state refusal is CloudFormation's common ValidationError (HTTP 400). Recorded behavior: a stack left in CREATE_FAILED by a create submitted with DisableRollback=true is a NON-TERMINAL state for update purposes — UpdateStack is refused with Error.Code `ValidationError` and Error.Message `This stack is currently in a non-terminal [CREATE_FAILED] state. To update the stack from this state, please use the disable-rollback parameter with update-stack API. To rollback to the last known good state, use the rollback-stack API`. The text is fixed apart from the status inside the brackets, which is the stack's current StackStatus; there is no trailing period. Only two things are then accepted: UpdateStack WITH DisableRollback=true, or RollbackStack. After RollbackStack has taken the stack to ROLLBACK_COMPLETE, even UpdateStack with DisableRollback=true is refused, with `Stack:<StackId> is in ROLLBACK_COMPLETE state and can not be updated.`

Emulator: the call is ACCEPTED — the check's helper reports "the call succeeded; real AWS rejects it, so there is no error code" — so no ValidationError is raised, no message is produced, and the stack is updated from a state AWS treats as non-terminal. Everything before this point in the check now passes: the create does land CREATE_FAILED, the partially created estate (Role, Fn0, Fn0Version CREATE_COMPLETE; Fn0Alias CREATE_FAILED) is retained and reported, and lambda GetFunction still resolves the kept function.

Why: the refusal is the guard-rail that forces a caller to choose explicitly between "fix forward with rollback disabled" and "roll back to the last known good state". With the guard missing, a client that relies on CloudFormation to refuse an unqualified update of a half-created stack instead mutates it silently, the two-API instruction in the message is never delivered, and the downstream RollbackStack semantics (and the ROLLBACK_COMPLETE refusal after it) can diverge unnoticed.

Fix: the state guard of the UpdateStack handler (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/update_stack.rs) — treat CREATE_FAILED (and the other non-terminal states) as update-blocking unless DisableRollback=true is set, and refuse with ValidationError carrying AWS's `This stack is currently in a non-terminal [<StackStatus>] state. ...` text.

Related open issue, named for the fixer but NOT covering this (it does not carry this check's current replay key): 20261107T153025Z-cloudformation-lambda-7-net-new-causes-a-failed.md lists this same check id, but its Cause 6 is that DescribeStackResources returned NO resource rows for the rollback-disabled CREATE_FAILED stack. That retention now works and the run proceeds to the UpdateStack guard described here, which that issue does not mention.

Traces: cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources.

## Evidence
Check cloudformation__lambda.cfnuf-create-with-missing-alias-version-and-rollback-disabled-keeps-its-resources:
`AssertionError: the call succeeded; real AWS rejects it, so there is no error code` (raised by the check's `_error` helper wrapping `cfn.update_stack(StackName=<stack in CREATE_FAILED>, TemplateBody=<fixed template>, Capabilities=['CAPABILITY_IAM'])`).

Expected (AWS): HTTP 400, Error.Code 'ValidationError', Error.Message
`This stack is currently in a non-terminal [CREATE_FAILED] state. To update the stack from this state, please use the disable-rollback parameter with update-stack API. To rollback to the last known good state, use the rollback-stack API`
Actual (emulator): the UpdateStack call returns successfully (no error at all).

Message anatomy: fixed text is everything except the status inside the brackets, which is the stack's current StackStatus ('CREATE_FAILED' here); no trailing period.

Passing context that must stay passing: the create with DisableRollback=True lands CREATE_FAILED; DescribeStackResources reports {'Role': 'CREATE_COMPLETE', 'Fn0': 'CREATE_COMPLETE', 'Fn0Version': 'CREATE_COMPLETE', 'Fn0Alias': 'CREATE_FAILED'}; lambda GetFunction resolves the kept function.
Not yet reached because of this failure (same check, after the refusal): RollbackStack -> ROLLBACK_COMPLETE, lambda GetFunction -> ResourceNotFoundException / 'Function not found: arn:aws:lambda:<region>:<account>:function:<fn>', and UpdateStack with DisableRollback=True -> ValidationError / 'Stack:<StackId> is in ROLLBACK_COMPLETE state and can not be updated.'

## Steps to Reproduce
1. cloudformation CreateStack with Capabilities=['CAPABILITY_IAM'] and DisableRollback=True on a template holding an AWS::IAM::Role (lambda.amazonaws.com trust, no policy), an AWS::Lambda::Function, an AWS::Lambda::Version {FunctionName: !Ref Fn0} and an AWS::Lambda::Alias {Name: 'LATEST', FunctionVersion: '7'} — a version number that does not exist, so Lambda's CreateAlias refuses it.
2. Poll DescribeStacks to CREATE_FAILED; DescribeStackResources must show Role/Fn0/Fn0Version CREATE_COMPLETE and Fn0Alias CREATE_FAILED, and lambda GetFunction must still resolve the function (already correct).
3. cloudformation UpdateStack(StackName, TemplateBody=<same template with the alias pointing at !GetAtt Fn0Version.Version>, Capabilities=['CAPABILITY_IAM']) — WITHOUT DisableRollback.
   Expected: HTTP 400 ValidationError with Message 'This stack is currently in a non-terminal [CREATE_FAILED] state. To update the stack from this state, please use the disable-rollback parameter with update-stack API. To rollback to the last known good state, use the rollback-stack API'.
   Observed: the call succeeds.
4. Then confirm the two accepted paths still behave: the same UpdateStack WITH DisableRollback=True, and RollbackStack -> ROLLBACK_COMPLETE followed by UpdateStack(DisableRollback=True) -> ValidationError 'Stack:<StackId> is in ROLLBACK_COMPLETE state and can not be updated.'
5. Or replay the check id in trace_ids.
