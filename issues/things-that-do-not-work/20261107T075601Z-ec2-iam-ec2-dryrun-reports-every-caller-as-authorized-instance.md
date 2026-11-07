Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam.dry-run-answers-unauthorizedoperation-for-a-principal-without-the-permission-rejected ec2__iam.instance-profile-path-preserved-through-launch ec2__iam.instance-profile-role-swap-changes-the-credentials-imds-serves
Issue-Key: ec2__iam.dry-run-answers-unauthorizedoperation-for-a-principal-without-the-permission-rejected|_legs#5|AssertionError|assert denied.get("Code") == _ERR_UNAUTHORIZED, (
Issue-Key: ec2__iam.instance-profile-path-preserved-through-launch|_legs#4|AssertionError|assert described_arn == expected_arn, (
Issue-Key: ec2__iam.instance-profile-role-swap-changes-the-credentials-imds-serves|_poll#13|Timeout|time.sleep(interval_s)
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# EC2 DryRun reports every caller as authorized, instance-profile ARNs lose their path, and IMDS credentials never appear

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopInstances

## Description
Three net-new root causes behind the surviving ec2__iam failures.

Cause 1 of 3: DryRun does not consult the caller's permissions.
AWS: DryRun means "check permissions and parameters, then do nothing": a principal that holds the action is answered DryRunOperation ("Request would have succeeded, but DryRun flag is set."), a principal that does not is answered UnauthorizedOperation — "You are not authorized to perform this operation. User: <principal arn> is not authorized to perform: <action> on resource: <arn> because no identity-based policy allows the <action> action." (fixed text; principal, action and resource come from the request and the caller's identity).
Emulator: the same StopInstances(DryRun=True) request from a principal with no ec2:StopInstances is answered DryRunOperation.
Why: a target that answers DryRunOperation here reports every caller as authorized for everything — the documented pre-flight permission check is useless and least-privilege testing silently passes.
Fix: the DryRun short-circuit in the EC2 request pipeline — run the authorization check before reporting DryRunOperation.
Traces: ec2__iam.dry-run-answers-unauthorizedoperation-for-a-principal-without-the-permission-rejected.

Cause 2 of 3: DescribeInstances reports an instance-profile ARN with the path stripped.
AWS: IamInstanceProfile.Arn is the profile's full ARN including its IAM path — arn:aws:iam::<account>:instance-profile/<path>/<name> — so a profile created under /sim2real-profile-path-test/ reads back with that path.
Emulator: the ARN omits the path: arn:aws:iam::<account>:instance-profile/<name>.
Why: the ARN does not resolve back to the profile; policies and lookups keyed on the pathed ARN fail.
Fix: the instance-profile association state / DescribeInstances IamInstanceProfile serializer (carry the IAM path).
Traces: ec2__iam.instance-profile-path-preserved-through-launch.

Cause 3 of 3: instance-profile credentials are never served, so a role swap can never be observed.
AWS: an instance with an instance profile serves role credentials at the IMDS credentials endpoint; replacing the association changes the role those credentials belong to within minutes (recorded behavior).
Emulator: the scenario never reaches credentials at all and ends "Check timed out after 60.0s".
Why: no instance-identity/credential scenario can be exercised, and a permission difference cannot be told apart from an instance that never served credentials.
Fix: the IMDS credential service backing instance-profile associations (part of the instance runtime).
Traces: ec2__iam.instance-profile-role-swap-changes-the-credentials-imds-serves.

## Evidence
From the behavioral replay:
- dry-run-answers-unauthorizedoperation-for-a-principal-without-the-permission-rejected [mismatch]: AssertionError: the SAME StopInstances(DryRun=True) request from a principal WITHOUT ec2:StopInstances must answer UnauthorizedOperation, not DryRunOperation ... Got 'DryRunOperation' ('Request would have succeeded, but DryRun flag is set.').
- instance-profile-path-preserved-through-launch [mismatch]: AssertionError: DescribeInstances must report the instance profile's full-path Arn 'arn:aws:iam::000000000000:instance-profile/sim2real-profile-path-test/<name>', got 'arn:aws:iam::000000000000:instance-profile/<name>'.
- instance-profile-role-swap-changes-the-credentials-imds-serves [mismatch]: Error: Check timed out after 60.0s.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks in trace_ids: (1) assume a role with no ec2:StopInstances and call StopInstances(DryRun=True) on a real instance; (2) create an instance profile under an IAM path, launch with it and read DescribeInstances IamInstanceProfile.Arn; (3) launch with one profile, replace the association and read the credentials IMDS serves.
