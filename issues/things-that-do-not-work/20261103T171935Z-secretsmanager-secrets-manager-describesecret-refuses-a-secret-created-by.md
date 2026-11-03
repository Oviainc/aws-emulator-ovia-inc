Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: secretsmanager|DescribeSecret|2xx->4xx|$.ARN
Issue-Key: secretsmanager|DescribeSecret|2xx->4xx|$.CreatedDate
Issue-Key: secretsmanager|DescribeSecret|2xx->4xx|$.Description
Issue-Key: secretsmanager|DescribeSecret|2xx->4xx|$.LastChangedDate
Issue-Key: secretsmanager|DescribeSecret|2xx->4xx|$.Name
Issue-Key: secretsmanager|DescribeSecret|2xx->4xx|$.Tags
Issue-Key: secretsmanager|DescribeSecret|2xx->4xx|status
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# Secrets Manager DescribeSecret refuses a secret created by a CloudFormation stack

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
Cause 1 of 1 - DescribeSecret answers 400 for a secret that a CloudFormation stack created.
AWS: a secret created by an AWS::SecretsManager::Secret resource is an ordinary secret, so DescribeSecret answers 200 with ARN (arn:<partition>:secretsmanager:<region>:<account>:secret:<name>-<6 random chars>), Name, Description, CreatedDate, LastChangedDate and Tags - including the reserved aws:cloudformation:stack-id, aws:cloudformation:stack-name and aws:cloudformation:logical-id tags CloudFormation stamps on the resources it creates.
Emulator: 400 with no secret body, so the stack-created secret cannot be read at all.
Why: every consumer of a template-provisioned secret (a database's master credentials here) starts with DescribeSecret; refusing it makes the stack's secret unusable and hides the stack attribution tags that route a secret back to its stack and DeletionPolicy.
Fix: the AWS::SecretsManager::Secret provisioning path must register the secret in Secrets Manager state (with the reserved CloudFormation tags) so DescribeSecret's handler resolves it.
Traces: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy#task#7.

## Evidence
Replay of simparity-wire-aws-v1, fixture cloudformation-refarch-aur-secret-stack-route-to-deletion-policy step task#7 (stub_path='', owned_by=[]). Expected status 200, got 400. All result members missing: $.ARN (expected the secret ARN), $.Name, $.Description, $.CreatedDate, $.LastChangedDate and $.Tags (expected aws:cloudformation:stack-id = the stack ARN, aws:cloudformation:stack-name = the stack name, aws:cloudformation:logical-id = the resource's logical id).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Fixture cloudformation-refarch-aur-secret-stack-route-to-deletion-policy, step task#7: deploy a stack declaring an AWS::SecretsManager::Secret, then call Secrets Manager DescribeSecret on the secret the stack reports.
3. Check the call answers 200 with ARN, Name, Description, CreatedDate, LastChangedDate and the aws:cloudformation:* tags.
