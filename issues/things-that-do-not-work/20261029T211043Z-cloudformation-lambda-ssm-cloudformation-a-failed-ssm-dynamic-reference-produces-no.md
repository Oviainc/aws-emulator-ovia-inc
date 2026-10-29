Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__ssm.cfn-lambda-env-ssm-missing-dynamic-reference-rejected-and-rolled-back cloudformation__lambda__ssm.cfn-macro-transform-injects-working-resource
Issue-Key: cloudformation__lambda__ssm.cfn-lambda-env-ssm-missing-dynamic-reference-rejected-and-rolled-back|run#12|AssertionError|assert stack_reasons == [f"Parameters: [ssm:{missing_source}] cannot be found."], (
Issue-Key: cloudformation__lambda__ssm.cfn-macro-transform-injects-working-resource|_deploy_cfn#10|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# CloudFormation: a failed ssm dynamic reference produces no resource-status reason, and a macro Transform leaves the stack CREATE_FAILED (2 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the cloudformation__lambda__ssm group; neither is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 2 - A stack that fails on an unresolvable `{{resolve:ssm:...}}` dynamic reference records no reason naming the missing parameter.
AWS: when a template's SSM dynamic reference (or an AWS::SSM::Parameter::Value<> parameter) cannot be resolved, the stack fails and the stack's OWN event carries ResourceStatusReason `Parameters: [ssm:<name>] cannot be found.` - fixed text `Parameters: [` ... `] cannot be found.` with the unresolvable reference name taken from the template. Recorded behavior: DescribeStackEvents for the stack resource exposes exactly that reason.
Emulator: the stack's event list carries no reason at all - the collected reasons are `[]` - so a client cannot tell WHY the deployment failed.
Why: the reason string is the only machine-readable explanation of a resolve failure; CI/rollback tooling that greps it sees nothing and reports an unexplained failure.
Fix: the stack-event recording path for template/dynamic-reference resolution failures (populate ResourceStatusReason on the stack-level event), in the CloudFormation stack state/DescribeStackEvents handler.
Traces: cloudformation__lambda__ssm.cfn-lambda-env-ssm-missing-dynamic-reference-rejected-and-rolled-back.

Cause 2 of 2 - A stack whose template declares a macro Transform never reaches CREATE_COMPLETE.
AWS: a template with a `Transform` naming a registered macro is processed (the macro is invoked, its returned fragment is injected) and the stack reaches CREATE_COMPLETE with the injected resource present and working.
Emulator: the consumer stack ends in `CREATE_FAILED`, so the macro-injected resource never exists.
Why: every SAM/macro-based deployment path (a very common pattern) cannot be exercised; a client's stack fails where production succeeds.
Fix: Transform/macro processing in the CreateStack template-processing stage (invoke the macro's provider and splice the returned fragment before resource creation).
Traces: cloudformation__lambda__ssm.cfn-macro-transform-injects-working-resource.

## Evidence
Cause 1: `assert stack_reasons == [f"Parameters: [ssm:{missing_source}] cannot be found."]` -> `AssertionError: the stack's own event must name the missing parameter; reasons were []`.
Cause 2: `AssertionError: Stack sim2real-xsvc-stk-<id> ended in CREATE_FAILED` (expected CREATE_COMPLETE with the macro-injected resource).

## Steps to Reproduce
1. git_clone(); 2. run_behavioral_suite(); 3. replay cloudformation__lambda__ssm.cfn-lambda-env-ssm-missing-dynamic-reference-rejected-and-rolled-back (create a stack whose Lambda Environment uses {{resolve:ssm:<absent-name>}}, then DescribeStackEvents and read ResourceStatusReason) and cloudformation__lambda__ssm.cfn-macro-transform-injects-working-resource (register a macro-backed Transform, deploy a consumer stack, wait for CREATE_COMPLETE).
