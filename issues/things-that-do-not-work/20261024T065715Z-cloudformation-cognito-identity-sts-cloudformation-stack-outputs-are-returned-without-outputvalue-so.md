Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag
Issue-Key: cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag|run#11|KeyError|outputs = {o["OutputKey"]: o["OutputValue"]
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# CloudFormation stack Outputs are returned without OutputValue, so a CFN-deployed Cognito identity-pool scenario cannot read back any output

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
ONE root cause (cause 1 of 1).

AWS: DescribeStacks returns each stack Output as an Output structure carrying OutputKey AND OutputValue (plus optional Description/ExportName); for a template whose Outputs use Ref/GetAtt, OutputValue is the resolved physical value (identity pool id, role ARN, bucket name). Reading outputs back is the only supported way for a caller to learn what the stack created.
Emulator: the Output entries carry no OutputValue member at all — building a dict of OutputKey -> OutputValue over `Stacks[0]['Outputs']` raises KeyError: 'OutputValue' — so the check cannot obtain the identity pool / role / bucket it deployed and the whole principal-tag-mapping scenario is unreachable.
Why: every consumer of a deployed stack (a test, a pipeline step, a nested stack, a developer) reads values out of Outputs; without OutputValue the stack is write-only and nothing downstream of a CFN deployment can be driven.
Fix: the CloudFormation Output serializer / the stack-output resolution step (resolve Ref/GetAtt to the created resource's physical id and emit OutputValue).
Traces: cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag. Open issue 20261024T055421Z-cloudformation-iam-opensearch-sts-cloudformation-stack-outputs-are-returned-without-outputvalue-so.md records the same defect for service token cloudformation__iam__opensearch__sts and owns that check's replay key; it does not carry this check, so this one is filed here and that issue is named for cross-reference.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1, check cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag:

"File check_cloudformation__cognito_identity__sts_xsvc_cognito_identity_3_cfn_principal_tag_mapping_gates_s3_read_by_session_tag.py, line 281, in run: outputs = {o['OutputKey']: o['OutputValue'] ... KeyError: 'OutputValue'"

Expected: each entry of DescribeStacks Stacks[0].Outputs carries both OutputKey and a resolved OutputValue.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay cloudformation__cognito-identity__sts.xsvc-cognito-identity-3-cfn-principal-tag-mapping-gates-s3-read-by-session-tag.
3. Manual repro: create any stack with an Outputs section using Ref/GetAtt, wait for CREATE_COMPLETE, then cfn.describe_stacks(StackName=...) and inspect Stacks[0]['Outputs'] — the entries have no OutputValue.
