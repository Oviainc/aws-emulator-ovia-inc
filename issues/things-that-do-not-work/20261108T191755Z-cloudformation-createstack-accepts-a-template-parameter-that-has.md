Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.parameter-constraint-violations-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.parameter-constraint-violations-rejected|_error#23|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Commit: b668b970d0d9c7dee5ca992802cd99dd932aec82

# cloudformation: CreateStack accepts a template parameter that has no Default and no supplied value, deploying the stack instead of refusing ValidationError "Parameters: [<Name>] must have values"

> _Note: this issue was identified against the code base at commit `b668b970d0d9c7dee5ca992802cd99dd932aec82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause in CloudFormation's stack-parameter validation. The neighbouring resolved issue 20261102T101202Z-cloudformation-10-net-new-root-causes-stack-level.md covered AllowedPattern/MinLength/MaxLength/MinValue/MaxValue/Number-typing enforcement (those cases now pass in this check) and carries none of the replay keys below; the missing-required-value rule is a separate, still-unimplemented gate.

Cause 1 of 1: a required template parameter (declared with no Default) that the request does not supply is silently treated as satisfied, and the stack is created.
AWS: Smithy: CreateStack takes `Parameters` (list of Parameter) and models no dedicated error for this case — the refusal is the awsQuery common error ValidationError at HTTP 400 (Smithy: CreateStack errors AlreadyExistsException / InsufficientCapabilitiesException / LimitExceededException / TokenAlreadyExistsException, none of which applies). Recorded behavior: AWS refuses the call before any stack exists, with Error.Code `ValidationError` and the message `Parameters: [<ParameterKey>] must have values`, where the fixed text is `Parameters: [` … `] must have values` and the variable part is the comma-separated list of the template's declared parameter names that have no Default and no supplied ParameterValue (here the single name `Secret`). After the refusal DescribeStacks on that name answers ValidationError `Stack with id <StackName> does not exist` — no stack is left behind.
Emulator: the call is accepted with HTTP 200 and returns a StackId/OperationId (e.g. `{'StackId': 'arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>', 'OperationId': '<uuid>'}`), so a stack is created whose required parameter was never given a value. No ValidationError is raised at all.
Why: a client's own input contract is unenforceable — a deploy that omits a required parameter (a typo in a --parameters list, a dropped key in CI) proceeds instead of failing fast, and every resource whose property interpolates that parameter is provisioned from an unset/empty value. Callers that branch on ValidationError to prompt for the missing input never see it, and the "rejected call leaves no stack" invariant is broken, so cleanup logic sees an unexpected stack.
Fix: the parameter-resolution/validation stage of the CreateStack handler (the same stage that already enforces AllowedPattern, MinLength/MaxLength and Number MinValue/MaxValue), extended to refuse when a template parameter has neither a Default nor a supplied ParameterValue, with AWS's `Parameters: [...] must have values` wording; the same gate belongs on UpdateStack.
Traces: cloudformation.parameter-constraint-violations-rejected (the per-constraint cases earlier in the same check pass; the check stops at the missing-required-value case, so the undeclared-ParameterKey case that follows it is unexercised).

## Evidence
Check cloudformation.parameter-constraint-violations-rejected, at the missing-required-parameter step:
AssertionError: expected the call to be rejected, but it succeeded and returned {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-behav-pconstraint-missing-02e7cc0bb881/92e06e4c-d1ca-5281-aecf-8febba05f55b', 'OperationId': '29dad456-cdd6-b815-22f8-e5e4c749b1be', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Request: CreateStack with a template declaring Parameters Slug (String, AllowedPattern/MinLength/MaxLength/ConstraintDescription), Retention (Number, MinValue 1, MaxValue 30) and Secret (String, NoEcho, NO Default), and Parameters=[Slug=valid-slug, Retention=7] — Secret omitted.
Expected: HTTP 400, Error.Code 'ValidationError', Error.Message 'Parameters: [Secret] must have values', and DescribeStacks on the stack name answering ValidationError 'Stack with id <name> does not exist'.
Actual: HTTP 200 with a StackId.
The stack name, account, region, StackId uuid, OperationId and RequestId above are per-run fillers, not requirements; the requirement is the 400/ValidationError/'Parameters: [<missing names>] must have values' triple and that no stack is created.

## Steps to Reproduce
Replay cloudformation.parameter-constraint-violations-rejected. Minimal repro: 1) Build a template whose Parameters block declares a parameter with no Default (e.g. `Secret: {Type: String, NoEcho: true}`) alongside parameters that are supplied. 2) Call CreateStack with TemplateBody set to that template and a Parameters list that omits the no-Default parameter. 3) Observe HTTP 200 and a returned StackId instead of a 400 ValidationError whose message is 'Parameters: [Secret] must have values'. 4) Call DescribeStacks on that stack name: on AWS it does not exist; on the emulator it does.
