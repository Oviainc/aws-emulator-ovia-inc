Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle
Issue-Key: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle|phase#5|_PhaseError|raise _PhaseError(
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# DescribeStacks Outputs entries omit OutputValue, so a stack output cannot be read (1 cause)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1 — a stack output served by DescribeStacks has no OutputValue member.
AWS: Smithy (com.amazonaws.cloudformation) DescribeStacks returns Stacks[].Outputs[] of shape Output, whose OutputKey and OutputValue are both populated for every output of a stack in a terminal CREATE/UPDATE_COMPLETE state — OutputValue is the resolved value of the template's Outputs entry, which is the entire purpose of a stack output.
Emulator: the Outputs entries lack OutputValue, so a caller indexing the output by that key raises KeyError: 'OutputValue' — the check fails in its deploy phase before it can measure anything.
Why: stack outputs are how every deployment hands physical resource ids (table names, function names, queue URLs) to the next step. An output entry with a key but no value makes the stack's contract unreadable: the caller cannot resolve any resource the template exported, and there is no error from CloudFormation to explain it.
Fix: the Outputs projection in the cloudformation crate's DescribeStacks serializer and the output-resolution step that records each output's resolved value when the stack settles.
Traces: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle

One cause must be closed before submitting.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

e2e-diagnose-onyx-lambda-processing-failure-lifecycle (verdict: error):
  _PhaseError: [phase 1/6 deploy] KeyError: 'OutputValue'

The check reads a declared stack output from DescribeStacks immediately after the stack settles; the Output entry carries no OutputValue member. AWS populates OutputKey and OutputValue for every output of a settled stack.

## Steps to Reproduce
Boot the emulator from this checkout.

1. aws cloudformation create-stack --stack-name t --template-body '{"Resources":{"T":{"Type":"AWS::DynamoDB::Table","Properties":{"KeySchema":[{"AttributeName":"id","KeyType":"HASH"}],"AttributeDefinitions":[{"AttributeName":"id","AttributeType":"S"}],"BillingMode":"PAY_PER_REQUEST"}}},"Outputs":{"TableName":{"Value":{"Ref":"T"}}}}'
2. aws cloudformation wait stack-create-complete --stack-name t
3. aws cloudformation describe-stacks --stack-name t --query 'Stacks[0].Outputs'
-> the entry for TableName has no OutputValue. AWS returns {"OutputKey":"TableName","OutputValue":"<physical table name>"}.

Then re-replay: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle
