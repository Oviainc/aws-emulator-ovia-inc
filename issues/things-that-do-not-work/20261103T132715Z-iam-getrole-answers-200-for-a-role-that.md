Issue-Source: trace-sweep-divergence
Issue-Trace: iam-ekscc-traj-attach-cluster-policy-mistakes
Issue-Step: teardown#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-ekscc-traj-attach-cluster-policy-mistakes
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetRole|4xx->2xx|$.ErrorResponse
Issue-Key: iam|GetRole|4xx->2xx|status
Issue-Commit: f9571cbe65aa0d276fa5ca35992cbed3efbd51b6

# IAM GetRole answers 200 for a role that has been deleted

> _Note: this issue was identified against the code base at commit `f9571cbe65aa0d276fa5ca35992cbed3efbd51b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRole

## Description
Cause 1 of 1: after the fixture's teardown deletes the role, GetRole still returns it.
AWS: Smithy GetRole.RoleName is @required and the operation models NoSuchEntityException with awsQueryError code NoSuchEntity and httpError 404. Recording: 404 with Error.Type Sender, Error.Code NoSuchEntity and the message "The role with name <roleName> cannot be found." — fixed text "The role with name " + the requested RoleName + " cannot be found."
Emulator: 200 with a GetRoleResponse carrying the deleted role.
Why: a client that polls GetRole to confirm a delete completed never sees the role disappear, so cleanup loops never terminate and a re-create of the same name will collide. It also means role state is not actually removed, so any permission the role carried remains evaluable.
Fix: the role state store — DeleteRole must remove the role from the lookup used by GetRole (and GetRole must raise NoSuchEntity for an absent name).
Traces: iam-ekscc-traj-attach-cluster-policy-mistakes#teardown#13.

The open issue 20261029T184359Z-iam-nosuchentity-messages-use-emulator-prose-instead-of.md concerns the wording of NoSuchEntity on other operations, not this missing refusal.

## Evidence
Replay of trace set simparity-wire-aws-v1: iam-ekscc-traj-attach-cluster-policy-mistakes#teardown#13, GetRole — status expected 404, got 200; $.ErrorResponse expected {'Error': {'Type': 'Sender', 'Code': 'NoSuchEntity', 'Message': 'The role with name sim2real-probe-ekscc-iam-role-8283844b cannot be found.'}, 'RequestId': '<shaped-not-asserted>'}, got None. The same fixture's earlier teardown steps delete the role.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay fixture iam-ekscc-traj-attach-cluster-policy-mistakes through its teardown phase.
3. At step teardown#13 call GetRole for the role the teardown deleted and observe 200 with the role body instead of 404 NoSuchEntity.
