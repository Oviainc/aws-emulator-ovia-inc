Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.op-contract-listGroups-missing-permission-denied
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.op-contract-listGroups-missing-permission-denied|_poll#10|Timeout|time.sleep(interval_s)
Issue-Commit: 8b134aa8d1570c43dbe434e21aa8020747d2a419

# iam: an explicit Deny naming iam:ListGroups is never enforced on a real signed call, so the refusal never arrives and the check polls to the 60 s cap

> _Note: this issue was identified against the code base at commit `8b134aa8d1570c43dbe434e21aa8020747d2a419`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListGroups

## Description
One net-new root cause in the bootstrapped `iam` service, grounded in one failing behavioral check of this suite. The check is not marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 1: IAM does not evaluate the signing principal's identity-based policies when serving its own ListGroups read, so an explicit Deny naming iam:ListGroups has no effect and the operation keeps answering 200.

AWS: Smithy (com.amazonaws.iam#AWSIdentityManagementV20100508) binds ListGroups with input ListGroupsRequest{PathPrefix, Marker, MaxItems range 1-1000} and output ListGroupsResponse{Groups required, IsTruncated default false, Marker}; the only modeled operation error is ServiceFailureException (awsQuery code ServiceFailure, 500). Authorization is not expressed in the model, so the authorization contract here is AWS's documented IAM policy-evaluation rule: every IAM API call is evaluated against the caller's identity-based policies, and an explicit Deny statement matching the action always wins over any Allow. A principal carrying both an Allow for iam:ListGroups/iam:ListRoles and a second inline policy with Effect=Deny on iam:ListGroups is therefore refused AccessDenied at HTTP 403 for ListGroups, while the sibling action the Deny does not name (iam:ListRoles) keeps succeeding, and deleting the Deny restores the Allow. The refusal message AWS builds from the request (principal ARN, action, resource ARN, and the phrase 'with an explicit deny in an identity-based policy') is therefore also a requirement, because it is derived from the request rather than being a fixed service string.

Emulator: the real signed ListGroups succeeds with HTTP 200 both before and after the Deny is attached. The check polls the signed call for up to 90 s waiting for the error code to become AccessDenied; because the decision never changes, the harness kills the check first — `Error: Check timed out after 60.0s`. No AccessDenied is ever produced, so neither the error code, the 403 status, nor the request-derived denial message can be observed, and the downstream legs (the sibling action staying allowed, SimulatePrincipalPolicy reporting explicitDeny, and the Deny's deletion restoring access) are never reached.

Why: a client validating least privilege against this emulator gets a vacuous pass. A policy written to deny iam:ListGroups appears to be in force while the call it is supposed to block still returns the group list, so a configuration validated here leaks group inventory in production. The 60 s hang also makes the divergence look like an emulator availability problem rather than a missing authorization decision.

Fix: the authorization gate ahead of the IAM operation handlers — services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/service.rs plus the ListGroups handler in src/ops/list_groups.rs — must resolve the signing principal from the SigV4 access key id, evaluate its attached and inline identity policies (explicit Deny over Allow) for action iam:ListGroups, and refuse with AccessDenied at HTTP 403 carrying the message built from the caller ARN, action and resource ARN. The open issue 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four.md describes the same missing gate for other IAM reads (its Cause 10 names iam.op-contract-listRoles-missing-permission-denied and the condition-key checks), but this check's replay key is not carried by that issue and the replay report did not attribute it there, so it is filed separately; ListGroups must be covered by whatever gate closes that issue.

Traces: iam.op-contract-listGroups-missing-permission-denied.

## Evidence
Failing check: [mismatch] iam.op-contract-listGroups-missing-permission-denied
Reported detail: `Error: Check timed out after 60.0s`

Expected (AWS): after PutUserPolicy attaches {"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListGroups","Resource":"*"}]} to the same user that already holds an Allow for iam:ListGroups and iam:ListRoles, the signed ListGroups(MaxItems=5) is refused with error code AccessDenied at HTTP 403, message of the form 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: iam:ListGroups on resource: arn:aws:iam::<account>:group/ with an explicit deny in an identity-based policy'; the signed ListRoles(MaxItems=5) continues to return 200; SimulatePrincipalPolicy for iam:ListGroups reports EvalDecision 'explicitDeny'; after DeleteUserPolicy of the Deny, the signed ListGroups returns 200 again with IsTruncated boolean and each Groups entry carrying GroupName and an 'arn:aws:iam::'-prefixed Arn.

Actual (emulator): the signed ListGroups keeps returning HTTP 200 with the group collection after the Deny is attached. The check's bounded poll (`_poll(..., lambda c: c == "AccessDenied", timeout_s=90, interval_s=3, description="the explicit Deny takes effect on the real signed ListGroups")`) never observes AccessDenied, so the check runs past the 60 s harness cap and is reported as a timeout. The pre-Deny positive controls for both ListGroups and ListRoles succeed immediately, confirming the emulator serves the operation and simply never applies the identity policy.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateUser(UserName=<u>, Path='/').
3. PutUserPolicy(UserName=<u>, PolicyName='allow-both', PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:ListGroups","iam:ListRoles"],"Resource":"*"}]}).
4. CreateAccessKey(UserName=<u>) and build an iam client that signs with that key id/secret against the emulator endpoint.
5. As that principal, call ListGroups(MaxItems=5) and ListRoles(MaxItems=5) — both return 200 (positive control).
6. PutUserPolicy(UserName=<u>, PolicyName='deny-listgroups', PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListGroups","Resource":"*"}]}).
7. As that principal, call ListGroups(MaxItems=5) repeatedly — it keeps returning 200 instead of AccessDenied/403, so the poll never converges and the check is killed at the 60 s cap.
8. Re-replay iam.op-contract-listGroups-missing-permission-denied to confirm the fix.
