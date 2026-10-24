Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role
Issue-Key: ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role|run#17|AssertionError|assert err == expected, (
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# EC2 CreateNetworkInterface with DryRun=true answers DryRunOperation for a principal that is not authorized, instead of UnauthorizedOperation

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetworkInterface

## Description
One root cause (1 of 1).

AWS: Smithy: CreateNetworkInterfaceRequest.DryRun is documented as "Checks whether you have the required permissions for the action, without actually making the request, and provides an error response. If you have the required permissions, the error response is DryRunOperation. Otherwise, it is UnauthorizedOperation." So DryRun is an AUTHORIZATION probe: the two outcomes are the two sides of an IAM decision, and for a principal whose identity policy does not allow ec2:CreateNetworkInterface the error code is UnauthorizedOperation (awsQuery error, HTTP 403), with a message of the documented form "You are not authorized to perform this operation. User: <assumed-role arn> is not authorized to perform: ec2:CreateNetworkInterface on resource: <subnet arn> because no identity-based policy allows the ec2:CreateNetworkInterface action." (recorded behavior for the message's exact wording; the code and the allow/deny rule are the documented requirement).
Emulator: the dry run is answered DryRunOperation with "Request would have succeeded, but DryRun flag is set." for a role whose identity policy grants only secretsmanager actions and no ec2:CreateNetworkInterface — i.e. the DryRun branch is taken before, and independently of, any IAM evaluation, so it reports "you are authorized" unconditionally.
Why: DryRun exists so that a caller can test an IAM grant without creating anything. An emulator that always answers DryRunOperation inverts the answer for every unauthorized principal: preflight checks, policy-tightening CI gates and least-privilege reviews all report success and then fail in AWS. It is also the single worst case for this flag, because a client reads DryRunOperation as positive confirmation.
Fix: the CreateNetworkInterface handler's DryRun path must run the normal IAM authorization for ec2:CreateNetworkInterface on the target subnet resource FIRST and answer UnauthorizedOperation / HTTP 403 when it does not allow, answering DryRunOperation only when it does.
Traces: ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role. Note: the open issue 20261024T065715Z-ec2-iam-sts-ec2-dryrun-authorization-never-refuses-an-unauthorized-principal.md describes this same check failing as a non-converging 60s timeout; this replay produces a concrete, immediate mismatch instead (the emulator now answers DryRunOperation at once), so the failure is recorded here with its new evidence and the two should be closed together.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1. Check ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role, verdict mismatch:

`AssertionError: DryRun CreateNetworkInterface as the secrets-only role: expected ('UnauthorizedOperation', 'You are not authorized to perform this operation. User: arn:aws:sts::000000000000:assumed-role/sim2real-asgeni-role-<id>/sim2real-asgeni is not authorized to perform: ec2:CreateNetworkInterface on resource: arn:aws:ec2:us-west-2:000000000000:subnet/subnet-<id> because no identity-based policy allows the ec2:CreateNetworkInterface action.'), got ('DryRunOperation', 'Request would have succeeded, but DryRun flag is set.')`

The role under test carries an inline grant of secretsmanager:DescribeSecret, secretsmanager:GetSecretValue and ec2:CreateNetworkInterface only in the positive-control leg; the failing leg uses the secrets-only variant with no ec2 action at all.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay ec2__iam__sts.asgeni-eni-create-dry-run-unauthorized-for-secrets-only-role.
3. Minimal repro: iam.create_role with a trust policy allowing the ambient caller, attach an inline policy granting ONLY secretsmanager:DescribeSecret and secretsmanager:GetSecretValue on "*", sts.assume_role it, then with those credentials call ec2.create_network_interface(SubnetId=<a subnet>, DryRun=True).
4. Observed: error code DryRunOperation. Expected: error code UnauthorizedOperation, HTTP 403, naming the assumed-role principal, the ec2:CreateNetworkInterface action and the subnet resource.
