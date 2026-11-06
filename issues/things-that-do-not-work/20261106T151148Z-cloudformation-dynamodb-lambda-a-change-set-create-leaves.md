Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.cdk-nested-stack-output-feeds-a-parent-function-that-writes-the-nested-table
Issue-Key: cloudformation__dynamodb__lambda.cdk-nested-stack-output-feeds-a-parent-function-that-writes-the-nested-table|run#16|AssertionError|assert rc == 0, f"`cdk deploy` exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# cloudformation__dynamodb__lambda: a change-set create leaves the stack in REVIEW_IN_PROGRESS with no template, so `cdk deploy` of a nested stack fails with ValidationError

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
One root cause behind 1 failing check.

Cause 1 of 1: a stack created by CreateChangeSet(ChangeSetType=CREATE) is left with no retrievable template while in REVIEW_IN_PROGRESS, so the next call of the deploy flow is refused and the deployment aborts.
AWS: CreateChangeSet with ChangeSetType CREATE creates the stack record in status `REVIEW_IN_PROGRESS` and stores the submitted template against it; GetTemplate / DescribeChangeSet / ExecuteChangeSet all work from that point on, and ExecuteChangeSet moves the stack to CREATE_IN_PROGRESS and then CREATE_COMPLETE. A stack in REVIEW_IN_PROGRESS does have a template — it is the change set's template. Model: CreateChangeSet/ExecuteChangeSet/GetTemplate are modelled in services/service-com-amazonaws-cloudformation-cloudformation; the exact refusal text below is recorded behavior only.
Emulator: `cdk deploy` of a parent stack with a nested stack fails with `ValidationError: Template does not exist for stack [<stack-name>] in REVIEW_IN_PROGRESS state` — the stack exists in REVIEW_IN_PROGRESS but the emulator holds no template for it, so the deploy cannot proceed. The fixed text here is `Template does not exist for stack [...] in REVIEW_IN_PROGRESS state`; the bracketed name is the stack name from the request. On AWS this call succeeds, so a 400 ValidationError at this point is itself the divergence.
Why: the change-set deploy path used by both the AWS CLI (`aws cloudformation deploy`) and the CDK is unusable: no CDK app with a nested stack can be deployed, and the parent's output cannot be fed to a function. This is the standard deployment route, so most infrastructure-as-code work is blocked.
Fix: the CreateChangeSet handler and the stack/template store — a CREATE change set must persist its template with the REVIEW_IN_PROGRESS stack so GetTemplate and ExecuteChangeSet can read it.
Traces: cloudformation__dynamodb__lambda.cdk-nested-stack-output-feeds-a-parent-function-that-writes-the-nested-table.

## Evidence
Expected (AWS): `cdk deploy` of a parent + nested stack exits 0; the stack goes REVIEW_IN_PROGRESS -> CREATE_IN_PROGRESS -> CREATE_COMPLETE and the parent function writes the nested stack's table.

Actual (emulator): `cdk deploy` exits 1; output tail: `... creating CloudFormation changeset... deploying... [1/1]  failed to deploy  ‣ ValidationError: Template does not exist for stack [sim2real-cdk-nest-<id>] in REVIEW_IN_PROGRESS state`. (The stack name is a per-run value; the fixed text is `Template does not exist for stack [...] in REVIEW_IN_PROGRESS state`.)

## Steps to Reproduce
1. Synthesize a CDK app with a parent stack, a nested stack exporting a DynamoDB table name, and a Lambda in the parent that writes to it.
2. Run `cdk deploy <stack> --require-approval never`.
3. Observe the deploy abort with ValidationError `Template does not exist for stack [...] in REVIEW_IN_PROGRESS state` immediately after the change set is created.
