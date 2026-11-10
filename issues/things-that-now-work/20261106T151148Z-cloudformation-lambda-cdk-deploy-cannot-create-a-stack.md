Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cdk-deploy-hotswap-updates-lambda-code-without-a-stack-update
Issue-Key: cloudformation__lambda.cdk-deploy-hotswap-updates-lambda-code-without-a-stack-update|run#10|AssertionError|assert rc == 0, f"the first deploy exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# cloudformation__lambda: `cdk deploy` cannot create a stack — the change-set path leaves it in REVIEW_IN_PROGRESS with no template

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
One root cause behind 1 failing check.

Cause 1 of 1: the first `cdk deploy` of a stack fails because CloudFormation holds no template for the stack the CREATE change set just created.
AWS: CreateChangeSet with ChangeSetType=CREATE registers the stack in status `REVIEW_IN_PROGRESS` together with the submitted template; the subsequent DescribeChangeSet/ExecuteChangeSet/GetTemplate calls the CDK makes all succeed, and the deploy exits 0 with the stack CREATE_COMPLETE. Model: CreateChangeSet / ExecuteChangeSet / GetTemplate are modelled in services/service-com-amazonaws-cloudformation-cloudformation; the refusal text quoted below is recorded behavior only.
Emulator: the deploy exits 1 with `ValidationError: Template does not exist for stack [<stack-name>] in REVIEW_IN_PROGRESS state`. Fixed text: `Template does not exist for stack [...] in REVIEW_IN_PROGRESS state`; the bracketed value is the stack name from the request. AWS answers this call successfully, so any 400 ValidationError here is a divergence.
Why: no CDK (or `aws cloudformation deploy`) workflow can create a stack at all, so hotswap, code updates and every downstream behaviour that depends on a deployed stack are unreachable.
Fix: the CreateChangeSet handler and the stack/template store — a CREATE change set must persist its template alongside the REVIEW_IN_PROGRESS stack. The open issue 20261105T111951Z-cloudformation-lambda-a-cdk-hotswap-code-update-is.md owns the sibling hotswap check; it does not carry this check's replay key.
Traces: cloudformation__lambda.cdk-deploy-hotswap-updates-lambda-code-without-a-stack-update.

## Evidence
Expected (AWS): the first `cdk deploy` exits 0 and the stack reaches CREATE_COMPLETE.

Actual (emulator): AssertionError: the first deploy exited 1; output tail `... creating CloudFormation changeset... deploying... [1/1]  failed to deploy  ‣ ValidationError: Template does not exist for stack [sim2real-cdk-hot-<id>] in REVIEW_IN_PROGRESS state`.

## Steps to Reproduce
1. Synthesize a CDK app containing one Lambda function.
2. Run `cdk deploy <stack> --require-approval never`.
3. Observe exit code 1 and `ValidationError: Template does not exist for stack [...] in REVIEW_IN_PROGRESS state` right after the change set is created.
