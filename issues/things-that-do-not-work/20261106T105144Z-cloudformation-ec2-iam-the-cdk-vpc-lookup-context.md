Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role
Issue-Key: cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role|_wait_for_describe_vpcs#41|Timeout|creds = sts.assume_role(
Issue-Commit: 9b69e978f6d681ad8f3aa9948779773328c03c6b

# cloudformation + ec2 + iam: the CDK VPC-lookup context-cache scenario never settles — the lookup role's explicit deny on ec2:DescribeVpcs is never observable, so the check is killed with no verdict

> _Note: this issue was identified against the code base at commit `9b69e978f6d681ad8f3aa9948779773328c03c6b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
ONE root cause. (Note: two open issues describe the same non-termination — 20261104T045949Z-cloudformation-ec2-iam-the-cdk-vpc-context-provider.md, which carries this exact check in Issue-Trace-Ids but a different Issue-Key frame (`_wait_for_describe_vpcs#41`), and 20261104T083619Z-cloudformation-ec2-iam-ssm-the-cdk-ec2-vpc.md, which covers the SSM-parameter variant. Neither was recognised as owning this replay key in this run, so the failing check is re-filed here; a fixer closing those should re-replay this check id.)

Cause 1 of 1: the EC2 read path the CDK VPC context provider drives never makes an identity-based explicit deny on ec2:DescribeVpcs observable to an assumed role, so the scenario never reaches a terminal result and the harness kills it.
Failing check id: cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role

AWS: recorded behavior only for the end-to-end timing — the Smithy model (services/service-com-amazonaws-ec2-amazonec2) declares DescribeVpcs as a plain paginated read, input {Filters, VpcIds, NextToken, MaxResults @range 5..1000, DryRun} -> DescribeVpcsResult.Vpcs, with errors: [] (likewise DescribeAvailabilityZones -> AvailabilityZones, errors: []), so no modelled error carries authorization; EC2's authorization refusal is the service-common client error. Against AWS the sequence terminates at every step: a `cdk bootstrap`ed, qualifier-scoped lookup role resolves the VPC on a cache-cold `cdk synth`; after `iam:PutRolePolicy` attaches an explicit Deny on ec2:DescribeVpcs to that lookup role, a call to DescribeVpcs made with credentials from `sts:AssumeRole` of that role is REFUSED with HTTP 403 and error code UnauthorizedOperation, whose message's fixed text is "You are not authorized to perform this operation. User: <assumed-role session ARN> is not authorized to perform: ec2:DescribeVpcs on resource: <resource> with an explicit deny in an identity-based policy" — the fixed parts are "is not authorized to perform: ec2:DescribeVpcs" and "with an explicit deny in an identity-based policy"; the session ARN, role name, resource and encoded-authorization-failure blob are values taken from the request and from IAM state, never constants. Removing the deny makes the same call succeed again. Because both outcomes are reached promptly, `cdk synth` either completes or exits non-zero relaying that refusal.

Emulator: no outcome at all is produced. The replay detail is verbatim `Error: Check timed out after 60.0s`; no HTTP status, no error code and no synth exit status were observed, and no assertion of the check was evaluated. Hypothesis (the only point not stated by the model or the recording): the check's bounded propagation probe re-assumes the lookup role and polls DescribeVpcs until the deny is observable (budget 300s, 10s interval); the previously filed Issue-Key for this same check pins the hang in that loop (`_wait_for_describe_vpcs#41|Timeout|time.sleep(interval_s)`), i.e. the emulator keeps answering DescribeVpcs 200 OK for a role carrying an explicit Deny, so the loop can never terminate and the whole scenario is killed.

Why: a client cannot use CDK environment-aware constructs (ec2.Vpc.from_lookup and the context cache that is meant to insulate a later synth from a revoked lookup role) against this emulator — the workflow stalls instead of finishing. Worse, the REJECTION path is unobservable: a caller whose lookup role genuinely lacks ec2:DescribeVpcs gets a hang rather than AWS's deterministic 403 UnauthorizedOperation, so a permissions problem is indistinguishable from an unavailable service, and any tooling that retries on an authorization refusal never sees one.

Fix: EC2's authorization gate for the context-provider read path — DescribeVpcs, and with it DescribeSubnets, DescribeRouteTables and DescribeAvailabilityZones — must evaluate the caller's identity-based policies (including an explicit Deny reaching an assumed-role session) and answer 403 UnauthorizedOperation instead of serving the read; and the CloudFormation bootstrap/deploy path these synths depend on must return an answer or a modelled refusal promptly rather than leaving the caller waiting. This belongs in those operations' handlers and the shared request-authorization component they call, not in the check.

Traces: cloudformation__ec2__iam.cdk-vpc-lookup-context-cache-outlives-a-denied-lookup-role.

## Evidence
Expected (AWS): the scenario terminates at every step. Cache-cold `cdk synth` resolves the real VPC id, CIDR 10.93.0.0/16, owner account and exactly one Isolated subnet group through the assumed `cdk-<qualifier>-lookup-role-<account>-<region>` role; after an explicit Deny on ec2:DescribeVpcs is attached to that role, DescribeVpcs called with that role's STS credentials is refused with HTTP 403, error code UnauthorizedOperation and a message containing the fixed text "is not authorized to perform: ec2:DescribeVpcs" and "with an explicit deny in an identity-based policy" (session ARN and resource are request/state values); a synth with the cached context key still exits 0, a synth after the cached key is dropped exits non-zero relaying that refusal and writes no vpc-provider cache entry; removing the deny restores the identical resolution.

Actual (emulator): no response was observed at all. The suite reports, verbatim, `Error: Check timed out after 60.0s` — no HTTP status, no error code, no synth exit code. The check's own budgets (_BOOTSTRAP_TIMEOUT_S = 900.0, _SYNTH_TIMEOUT_S = 300.0, _PERMISSION_PROPAGATION_BUDGET_S = 300.0) were never consumed because the sequence never progressed to a terminal result; the previously recorded Issue-Key for this same check locates the stall inside `_wait_for_describe_vpcs` (frame `_wait_for_describe_vpcs#41`, `time.sleep(interval_s)`), the loop that waits for the explicit deny to become observable.

## Steps to Reproduce
1. `cdk bootstrap` a qualifier-scoped toolkit against the emulator (qualifier Q; toolkit stack plus `cdk-<Q>-assets-<account>-<region>` staging bucket and the `cdk-<Q>-lookup-role-<account>-<region>` role).
2. With boto3 against the emulator, create a VPC (CidrBlock 10.93.0.0/16) tagged sim2real=<marker> and one subnet 10.93.0.0/24 in an available AZ.
3. `cdk synth` a stack using `ec2.Vpc.from_lookup(...)` with DefaultStackSynthesizer(qualifier=Q) so the VPC context provider runs through the assumed lookup role; confirm cdk.context.json holds exactly one `vpc-provider:` key resolving the real vpc id.
4. `iam:PutRolePolicy` on the lookup role with {"Effect":"Deny","Action":"ec2:DescribeVpcs","Resource":"*"}.
5. `sts:AssumeRole` that lookup role and call `ec2:DescribeVpcs` with the returned credentials, polling until the change is observable.
6. Expected: that call is refused with HTTP 403 / UnauthorizedOperation naming ec2:DescribeVpcs and the explicit deny in an identity-based policy. Observed: the call keeps succeeding, the poll loop never terminates, and the check is killed with `Error: Check timed out after 60.0s` having reached no assertion.
