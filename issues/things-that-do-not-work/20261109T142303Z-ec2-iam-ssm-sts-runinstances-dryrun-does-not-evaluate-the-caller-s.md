Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__ssm__sts.run-instances-subnet-scoped-policy-refuses-other-subnet
Issue-Key: ec2__iam__ssm__sts.run-instances-subnet-scoped-policy-refuses-other-subnet|_legs#6|AssertionError|assert (denied.get("Code"), message) == (_ERR_UNAUTHORIZED, (
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# RunInstances DryRun does not evaluate the caller's IAM policy — a subnet-scoped Deny answers DryRunOperation instead of UnauthorizedOperation

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Cause 1 of 1: RunInstances with DryRun=true short-circuits to the dry-run success response without authorizing the request, so a launch into a subnet the caller's policy does not permit is reported as "would have succeeded".

AWS: DryRun means "perform the full authorization and parameter validation, then do not act". EC2 evaluates the caller's identity policy first: if the caller is not authorized, the response is 403 `UnauthorizedOperation` with the message `You are not authorized to perform this operation.` (optionally followed by the encoded authorization failure message); only an authorized request returns 412 `DryRunOperation` with `Request would have succeeded, but DryRun flag is set.` Smithy (ec2): RunInstances input DryRun -> Boolean; the service's common errors include UnauthorizedOperation and DryRunOperation. Fixed text of each message as quoted; there are no variable parts in either.

Emulator: assuming a role whose policy allows RunInstances only for subnet A, a DryRun RunInstances into subnet B answers `{'Code': 'DryRunOperation', 'Message': 'Request would have succeeded, but DryRun flag is set.'}` — the authorization step did not run, so the unauthorized subnet is reported as permitted.

Why: DryRun is the standard pre-flight permission probe (used by CI policy checks, CDK/Terraform plan-time validation and least-privilege tooling). A client using it to prove a role cannot launch outside its subnet gets the opposite answer, so a policy regression passes review; and because the two outcomes carry different codes and HTTP statuses (403 vs 412), code branching on them takes the wrong path.

Fix: the RunInstances handler's DryRun path in the EC2 service — identity-policy authorization (including resource-level conditions such as the subnet ARN) must be evaluated before the DryRunOperation response is produced.

Traces: check ec2__iam__ssm__sts.run-instances-subnet-scoped-policy-refuses-other-subnet.

## Evidence
Expected (subnet B leg): 403 `UnauthorizedOperation` with message `You are not authorized to perform this operation.`

Actual: `AssertionError: subnet B: unexpected refusal {'Code': 'DryRunOperation', 'Message': 'Request would have succeeded, but DryRun flag is set.'}` — i.e. 412 DryRunOperation, the authorized-dry-run answer.
Both messages are entirely fixed text; the divergence is the error code/status and therefore the message, not any request-derived value. The subnet-A positive control is expected to return DryRunOperation and does.

## Steps to Reproduce
1. Create a VPC with two subnets A and B; create a role whose policy allows ec2:RunInstances only when ec2:Subnet equals subnet A's ARN, and assume it via sts.
2. As that role, call ec2 RunInstances(DryRun=True, SubnetId=<subnet A>) — DryRunOperation (correct).
3. Call ec2 RunInstances(DryRun=True, SubnetId=<subnet B>) — it answers DryRunOperation "Request would have succeeded, but DryRun flag is set." instead of UnauthorizedOperation "You are not authorized to perform this operation.".
