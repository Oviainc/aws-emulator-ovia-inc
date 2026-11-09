Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__iam__sqs.cdk-bootstrap-custom-permissions-boundary-blocks-a-deploy-outside-it
Issue-Key: cloudformation__dynamodb__iam__sqs.cdk-bootstrap-custom-permissions-boundary-blocks-a-deploy-outside-it|run#24|AssertionError|assert boundary is not None, (
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# cdk bootstrap --custom-permissions-boundary leaves the synthesized cfn-exec role with no PermissionsBoundary

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRole

## Description
Cause 1 of 1: the roles the CDK bootstrap stack creates are provisioned without the PermissionsBoundary the bootstrap template sets, so the boundary never constrains a later deploy.

AWS: `cdk bootstrap --custom-permissions-boundary <policy-name>` renders the bootstrap template with a PermissionsBoundary property on the bootstrap roles (notably `cdk-<qualifier>-cfn-exec-role-<account>-<region>`). After the bootstrap stack completes, GetRole on that role returns Role.PermissionsBoundary = {PermissionsBoundaryType: 'Policy', PermissionsBoundaryArn: 'arn:aws:iam::<account>:policy/<policy-name>'}, and any action outside the boundary is denied for a deploy executed by that role. Smithy: iam#Role carries the optional member PermissionsBoundary (AttachedPermissionsBoundary{PermissionsBoundaryType, PermissionsBoundaryArn}) and CreateRole/PutRolePermissionsBoundary set it — recorded behavior only for the bootstrap template itself, which the recording shows producing a boundary-bearing exec role.

Emulator: GetRole on the bootstrapped exec role returns no PermissionsBoundary at all, so the role is unbounded and the deploy the check expects to be blocked outside the boundary is not blocked.

Why: a client that bootstraps with a custom permissions boundary believes every CDK deploy in that environment is confined to it. Here the guardrail silently does not exist: deploys that AWS would refuse succeed, and the account's blast radius is unbounded. A boundary that is reported absent is also indistinguishable from one that was never requested.

Fix: the AWS::IAM::Role provisioner in CloudFormation must carry the template's PermissionsBoundary through to CreateRole, and IAM's role state/GetRole serializer must round-trip the attached boundary; the boundary must then be evaluated by the authorization gate.

Traces: cloudformation__dynamodb__iam__sqs.cdk-bootstrap-custom-permissions-boundary-blocks-a-deploy-outside-it.

## Evidence
Expected: GetRole on 'cdk-<qualifier>-cfn-exec-role-<account>-<region>' returns Role.PermissionsBoundary naming the policy passed to --custom-permissions-boundary.
Actual: AssertionError: GetRole on 'cdk-qd604d5cb7-cfn-exec-role-000000000000-us-west-2' carries no PermissionsBoundary after bootstrapping with --custom-permissions-boundary 'sim2real-cdkboundary-807991900cad'

## Steps to Reproduce
1. Create a managed policy to act as the boundary.
2. Run `cdk bootstrap --custom-permissions-boundary <policy-name>` against the emulator.
3. Call GetRole on the synthesized cdk-<qualifier>-cfn-exec-role-<account>-<region>.
4. Observe no PermissionsBoundary on the returned Role.
