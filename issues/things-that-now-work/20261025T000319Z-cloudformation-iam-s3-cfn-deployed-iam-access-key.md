Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__s3.deploy-iam-2-cfn-access-key-deactivate-and-serial-rotation-old-key-rejected cloudformation__iam__s3.deploy-iam-2-cfn-group-explicit-deny-overrides-allow-for-deployed-access-key-denied
Issue-Key: cloudformation__iam__s3.deploy-iam-2-cfn-access-key-deactivate-and-serial-rotation-old-key-rejected|run#20|KeyError|out1 = {o["OutputKey"]: o["OutputValue"]
Issue-Key: cloudformation__iam__s3.deploy-iam-2-cfn-group-explicit-deny-overrides-allow-for-deployed-access-key-denied|run#20|KeyError|outputs = {o["OutputKey"]: o["OutputValue"]
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# cloudformation__iam__s3: CFN-deployed IAM access-key stacks are unusable — stack Outputs carry no OutputValue, so the deployed key cannot be read back

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: a settled stack's Outputs entries have no OutputValue member.
AWS: DescribeStacks returns, for each declared Output, an Output structure whose OutputKey and OutputValue are both populated once the stack reaches CREATE_COMPLETE; !Ref/!GetAtt in an Outputs block are resolved to concrete values (here the deployed AWS::IAM::AccessKey id and its secret, and the user name).
Emulator: the Output entries come back without the 'OutputValue' key at all, so the check dies with `KeyError: 'OutputValue'` while reading the deployed access key out of the stack. The stack itself reports a terminal success state.
Why: every cross-resource handoff a template performs through Outputs is lost — a client that deploys an identity with CloudFormation and then signs requests with the deployed credential, or feeds an output into a second stack, gets a KeyError/None instead of a value, and nothing downstream of the deploy can run.
Fix: the Outputs resolution/serialization path of the cloudformation service crate (resolve !Ref/!GetAtt at stack settle and populate Output.OutputValue in the DescribeStacks response).
Note: the same root cause is already filed for other service tokens as 20261024T055421Z-cloudformation-iam-opensearch-sts-cloudformation-stack-outputs-are-returned-without-outputvalue-so.md and 20261024T055421Z-cloudwatch-iam-lambda-logs-opensearch-s3-cfn-deployed-search-indexing-pipeline-fails-immediately-stack.md; the replay report did not attribute these two checks to either, so they are filed here under their own service token.
Traces: cloudformation__iam__s3.deploy-iam-2-cfn-access-key-deactivate-and-serial-rotation-old-key-rejected, cloudformation__iam__s3.deploy-iam-2-cfn-group-explicit-deny-overrides-allow-for-deployed-access-key-denied.

## Evidence
Both checks: KeyError: 'OutputValue' raised while reading the settled stack's Outputs. Expected: DescribeStacks Outputs = [{'OutputKey': ..., 'OutputValue': '<resolved value>'}, ...] with the deployed access-key id/secret and user name readable.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Deploy a stack declaring AWS::IAM::User + AWS::IAM::AccessKey with an Outputs block exporting !Ref/!GetAtt of those resources.
3. Wait for CREATE_COMPLETE and call DescribeStacks; inspect Stacks[0].Outputs — the entries have no OutputValue.
4. Re-replay the two check ids in trace_ids to confirm.
