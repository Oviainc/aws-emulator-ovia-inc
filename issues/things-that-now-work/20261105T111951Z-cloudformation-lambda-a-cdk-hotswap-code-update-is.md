Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cdk-hotswap-lambda-code-update-skips-stack-update-timeout-change-falls-back
Issue-Key: cloudformation__lambda.cdk-hotswap-lambda-code-update-skips-stack-update-timeout-change-falls-back|_cdk#12|AssertionError|assert result.returncode == 0, (
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# cloudformation__lambda: a CDK hotswap code update is not applied without a stack update

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionCode

## Description
Cause 1 of 1 — a hotswap-style UpdateFunctionCode outside a stack update does not become the running code, and a change that is not hotswappable does not fall back to a full stack update.
AWS: `cdk deploy --hotswap` calls lambda:UpdateFunctionCode directly, so the next invocation runs the new code while the stack's template and status are untouched; a change CDK cannot hotswap (here a Timeout change) falls back to a CloudFormation stack update that reaches UPDATE_COMPLETE and is visible in DescribeStacks/DescribeStackResources.
Emulator: recorded behavior only — the check fails inside its body at the point where it compares the observed behaviour with those two expectations, so one of the legs does not hold: either the direct code update is not reflected at invoke time, or the non-hotswappable change does not drive a stack update.
Why: a developer using the standard hotswap workflow sees stale code served after a successful UpdateFunctionCode, and a configuration-only change silently does not take effect.
Fix: UpdateFunctionCode's effect on the function's running code in the lambda state, and the CloudFormation stack-update path for a Timeout change.
Traces: cloudformation__lambda.cdk-hotswap-lambda-code-update-skips-stack-update-timeout-change-falls-back.

## Evidence
Error: File "check_cloudformation__lambda_cdk_hotswap_lambda_code_update_skips_stack_update_timeout_change_falls_back.py", line 117, in run — the check's assertion that the hotswapped code is served and that the timeout change falls back to a stack update does not hold. Expected: the new code runs after UpdateFunctionCode with the stack untouched, and the Timeout change produces an UPDATE_COMPLETE stack update.

## Steps to Reproduce
CreateStack with an AWS::Lambda::Function; call UpdateFunctionCode directly with new code and Invoke -> expect the new code; then update the template's Timeout and UpdateStack -> expect UPDATE_COMPLETE and the new timeout in GetFunctionConfiguration.
