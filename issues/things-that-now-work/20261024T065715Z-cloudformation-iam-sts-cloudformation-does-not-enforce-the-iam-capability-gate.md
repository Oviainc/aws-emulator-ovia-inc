Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied
Issue-Key: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied|_error#21|AssertionError|raise AssertionError(f"expected this call to fail, but it succeeded and returned {ok!r}")
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# CloudFormation does not enforce the IAM capability gate: CreateStack with a named IAM role and only CAPABILITY_IAM returns a StackId instead of InsufficientCapabilitiesException

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE root cause (cause 1 of 1).

AWS: CreateStack for a template that creates a NAMED IAM resource (here AWS::IAM::Role with an explicit RoleName) requires Capabilities to include CAPABILITY_NAMED_IAM. Submitting only CAPABILITY_IAM is refused with error code InsufficientCapabilitiesException, HTTP 400, and the refusal is total: no stack is created (a following DescribeStacks on the name answers ValidationError "Stack with id <name> does not exist") and no IAM role is created. The same template with CAPABILITY_NAMED_IAM then deploys to CREATE_COMPLETE (the check's positive control). Recorded behavior for the gate itself; the error is CloudFormation's modeled InsufficientCapabilitiesException.
Emulator: the call is accepted with HTTP 200 and returns a StackId (e.g. {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-xsvc-ditr-.../...', 'OperationId': ...}), so the capability gate does not exist and a stack/role AWS refuses to create is created.
Why: the capability gate is the guard that stops a template from silently minting privileged, name-pinned IAM identities. A pipeline that passes against the emulator is rejected by AWS at deploy time, and conversely an emulator-validated template creates resources the operator never acknowledged.
Fix: the CreateStack handler's capability validation (classify template resources that require CAPABILITY_IAM vs CAPABILITY_NAMED_IAM and refuse with InsufficientCapabilitiesException/400 before any resource is provisioned).
Traces: cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied (the check fails at its first negative leg, so the later rollback/trust-flip legs of the same check are still unverified).

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1, check cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied:

"File check_cloudformation__iam__sts_deploy_iam_1_cfn_rollback_and_trust_flip_assume_denied.py, line 86, in run: code, msg = _error(lambda: cfn.create_stack(...)) ... AssertionError: expected this call to fail, but it succeeded and returned {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-xsvc-ditr-72da44da419d/641da161-6115-aacb-4ac0-e08184a57feb', 'OperationId': '3bbf94d1-5c00-0b4f-b862-33b4ca1c5eac', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"

The check asserts code == "InsufficientCapabilitiesException" for a template whose only resource is an AWS::IAM::Role with an explicit RoleName, submitted with Capabilities=['CAPABILITY_IAM'].

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay cloudformation__iam__sts.deploy-iam-1-cfn-rollback-and-trust-flip-assume-denied.
3. Manual repro: cfn.create_stack(StackName='probe', TemplateBody=<one AWS::IAM::Role with RoleName set>, Capabilities=['CAPABILITY_IAM']) — the emulator returns 200 + StackId where AWS returns InsufficientCapabilitiesException/400, and DescribeStacks then finds the stack instead of answering ValidationError.
