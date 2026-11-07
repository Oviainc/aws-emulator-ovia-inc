Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth
Issue-Key: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth|run#21|AssertionError|assert rc != 0, (
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# EC2 DescribeVpcs ignores an identity Deny on the assumed CDK lookup role, so a denied `cdk synth` succeeds with the real VPC id

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
Cause 1 of 1: an inline IAM policy denying ec2:DescribeVpcs on the role a CDK context lookup assumes has no effect — the lookup still resolves.
AWS: when the principal performing DescribeVpcs is denied `ec2:DescribeVpcs` by an identity policy, EC2 refuses the call with HTTP 403 and error code `UnauthorizedOperation`; its message is EC2's fixed authorization text ("You are not authorized to perform this operation.", optionally followed by the encoded authorization failure message), with no variable parts taken from the request. The Smithy model declares no per-operation error for DescribeVpcs — authorization refusals are the service-common ones — so the status/code above are the recorded behavior of a denied EC2 read. CDK's vpc-provider then cannot resolve the context: `cdk synth` exits non-zero, names ec2:DescribeVpcs and the lookup role in its output, writes no vpc-provider context entry, and the template it still emits carries CDK's dummy id `vpc-12345` rather than a real one.
Emulator: with `{"Effect":"Deny","Action":"ec2:DescribeVpcs","Resource":"*"}` attached to the lookup role and the deny re-polled for its full propagation budget, DescribeVpcs keeps answering 200 and the lookup resolves the real VPC: `cdk synth` exits 0 and the written template's output carries the real `vpc-<id>` plus a resolved SSM marker. The refusal, and therefore the whole denied-lookup behavior, is unreachable.
Why: a client relying on EC2 authorization sees a privilege boundary that does not exist — least-privilege lookup roles, deny-based guardrails and any negative test of a denied context provider all pass when real AWS refuses them, so IAM regressions are invisible and the emulator cannot be used to validate permissions.
Fix: EC2 read operations must evaluate the caller's identity policies (explicit Deny wins) before serving, and answer 403 UnauthorizedOperation when denied — EC2's request authorization path, shared by DescribeVpcs and its sibling reads.
Traces: cloudformation__ec2__iam__ssm.cdk-vpc-context-lookup-denied-lookup-role-fails-synth.

## Evidence
Expected: `cdk synth` exits non-zero after the deny is attached, its output names ec2:DescribeVpcs and the lookup role, and the emitted template carries the dummy `vpc-12345`.
Actual: `AssertionError: 'cdk synth' exited 0 while the lookup role was denied ec2:DescribeVpcs; output tail: '...VpcId:\n    Value: vpc-<id>\n  Marker:\n    Value: marker-<id>...'` — i.e. the denied principal's DescribeVpcs succeeded and the real VPC id was resolved (the concrete ids/markers are per-run values; the requirement is the 403/UnauthorizedOperation refusal).

## Steps to Reproduce
1. Replay the behavioral suite and look under `### cloudformation__ec2__iam__ssm` for cdk-vpc-context-lookup-denied-lookup-role-fails-synth.
2. Bootstrap a CDK environment, create a tagged VPC with public and isolated subnets, and synth an app whose stack does `ec2.Vpc.from_lookup(tags={'Name': <vpc-name>})` — it resolves.
3. iam put_role_policy on the `cdk-<qualifier>-lookup-role-<account>-<region>` role with Effect Deny / Action ec2:DescribeVpcs / Resource * and re-run `cdk synth`.
4. Observe rc=0 and the real VPC id in cdk.out instead of a non-zero exit and the dummy `vpc-12345`; calling ec2 DescribeVpcs directly as that role also returns 200 instead of 403 UnauthorizedOperation.
