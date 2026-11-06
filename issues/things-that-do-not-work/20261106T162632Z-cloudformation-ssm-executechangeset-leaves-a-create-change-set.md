Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.cdk-deploy-parameters-are-reused-until-no-previous-parameters
Issue-Key: cloudformation__ssm.cdk-deploy-parameters-are-reused-until-no-previous-parameters|run#11|AssertionError|assert rc == 0, f"deploy {variant} {flags!r} exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 3e7452904cdb4574f2c938b153c1d775b5f33518

# cloudformation__ssm: ExecuteChangeSet leaves a CREATE change-set stack in REVIEW_IN_PROGRESS, so `cdk deploy --parameters` fails and the parameter reuse / --no-previous-parameters contract is unreachable

> _Note: this issue was identified against the code base at commit `3e7452904cdb4574f2c938b153c1d775b5f33518`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: a stack created through CreateChangeSet(ChangeSetType=CREATE) is never taken out of REVIEW_IN_PROGRESS by ExecuteChangeSet, so the stack has no deployed template and every `cdk deploy` leg of the parameter-reuse contract aborts on the first deploy. Failing check: cloudformation__ssm.cdk-deploy-parameters-are-reused-until-no-previous-parameters.

AWS: a new stack whose first change set is created with ChangeSetType=CREATE sits in REVIEW_IN_PROGRESS with no template; ExecuteChangeSet then adopts the change set's template, moves the stack to CREATE_IN_PROGRESS and on to CREATE_COMPLETE, after which GetTemplate serves TemplateBody and DescribeStacks reports the stack's Parameters. Smithy (cloudformation): ExecuteChangeSet models only ChangeSetNotFoundException (404, ChangeSetNotFound), InsufficientCapabilitiesException (400), InvalidChangeSetStatusException (400) and TokenAlreadyExistsException (400) — no modelled outcome leaves the stack in review; GetTemplate models only ChangeSetNotFoundException (404) and returns TemplateBody (@length min 1) plus StagesAvailable. The refusal text `Template does not exist for stack [<stack name>] in REVIEW_IN_PROGRESS state` is AWS's wording for GetTemplate against a stack that is still in review and has no executed template — its fixed text is `Template does not exist for stack [` … `] in REVIEW_IN_PROGRESS state`, with the stack name taken from the request; it is correct only while the stack genuinely has no executed template, which after ExecuteChangeSet it must not. That a real-AWS run of this check completes all three deploys (parameter given, parameter reused, --no-previous-parameters falling back to the template default) is recorded behavior only.

Emulator: the first `cdk deploy <stack> --require-approval never --parameters <stack>:Greeting=first` of a brand-new stack synthesizes, creates the change set, prints `deploying... [1/1]` and then fails: `❌  <stack> failed to deploy` / `‣ ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state`, and the CDK CLI exits 1. The stack is still in REVIEW_IN_PROGRESS when the CLI reads its template, i.e. the executed change set was never adopted as the stack's template and the stack never advanced to a create status, so no AWS::SSM::Parameter copy is provisioned and DescribeStacks never reports Parameters={'Greeting': 'first'}.

Why: a client using the change-set deploy path (every `cdk deploy`, and `aws cloudformation create-change-set` + `execute-change-set`) can never provision a new stack — the stack is parked in a state that is neither in-progress nor terminal, GetTemplate stays refused, and the downstream parameter semantics (a value given with --parameters reaching the resources, a later deploy with no --parameters reusing the previous value, --no-previous-parameters falling back to the template default) are unobservable.

Fix: the ExecuteChangeSet handler and the stack state machine/provisioner it drives in the CloudFormation service — the executed change set must become the stack's template (so GetTemplate serves it) and advance the stack out of REVIEW_IN_PROGRESS through CREATE_IN_PROGRESS to a terminal create status with the change set's Parameters recorded on the stack; GetTemplate's REVIEW_IN_PROGRESS refusal must then no longer apply.

The open issue 20261106T153504Z-cloudformation-sqs-ssm-a-change-set-created-stack.md describes the same ExecuteChangeSet/REVIEW_IN_PROGRESS cause for the cloudformation__sqs__ssm checks, and 20261103T061702Z-cloudformation-ssm-8-net-new-root-causes-in.md covers the prepare-change-set check; neither carries this check's replay key, so this failing check is filed here and must be re-replayed to confirm the fix.

Traces: cloudformation__ssm.cdk-deploy-parameters-are-reused-until-no-previous-parameters.

## Evidence
Expected (AWS): `cdk deploy <stack> --require-approval never --parameters <stack>:Greeting=first` exits 0; DescribeStacks(StackName=<stack>) then reports Parameters == {'Greeting': 'first'} and the stack's AWS::SSM::Parameter copy <prefix>-1 holds 'first'. A second deploy with no --parameters keeps 'first' (previous value reused) and a third with --no-previous-parameters falls back to the template default 'default-value'.
Actual (emulator): the first deploy exits 1. Output tail: '✨  Synthesis time: 5s / <stack>: creating CloudFormation changeset... / <stack>: deploying... [1/1] / ❌  <stack> failed to deploy / ‣ ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state'. The check fails at `assert rc == 0, f"deploy {variant} {flags!r} exited {rc}"` for variant 1; the stack never leaves REVIEW_IN_PROGRESS, so the later reuse and --no-previous-parameters legs are never reached.

## Steps to Reproduce
1. Synthesize a minimal CDK app (LegacyStackSynthesizer, one CfnParameter 'Greeting' with default 'default-value', one AWS::SSM::Parameter whose value is that parameter).
2. Run `cdk deploy <stack> --require-approval never --parameters <stack>:Greeting=first` against the emulator — equivalently: CreateChangeSet(StackName=<new stack>, ChangeSetType=CREATE, Parameters=[{ParameterKey:Greeting,ParameterValue:first}]) then ExecuteChangeSet(ChangeSetName=<id>).
3. Poll DescribeStacks(StackName=<stack>): StackStatus stays 'REVIEW_IN_PROGRESS' instead of advancing to CREATE_IN_PROGRESS/CREATE_COMPLETE, and GetTemplate(StackName=<stack>) is refused with ValidationError 'Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state'; the CDK CLI surfaces that refusal and exits 1.
4. GetParameter(Name=<prefix>-1) shows the copy was never created and DescribeStacks reports no Parameters.
Replay check cloudformation__ssm.cdk-deploy-parameters-are-reused-until-no-previous-parameters.
