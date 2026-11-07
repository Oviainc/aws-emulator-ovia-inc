Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-agreement.describe-agreement-missing-permission-denied
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement.describe-agreement-missing-permission-denied|_poll_until_code#23|AssertionError|raise AssertionError(f"expected {expected_code!r} within {timeout_s}s, last observed {last!r}")
Issue-Commit: 4eae4810055cf0c39415b7b36ad02401c0d0be96

# DescribeAgreement ignores identity-based IAM policy: an explicit Deny on aws-marketplace:DescribeAgreement still answers ResourceNotFoundException instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `4eae4810055cf0c39415b7b36ad02401c0d0be96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAgreement

## Description
One root cause, grounded in one failing behavioral check. Related open issue: 20261022T212948Z-marketplace-agreement-intent-enum-and-proposal-validation-skipped.md describes the same area as its "Cause 3 of 4", but it does not carry this check's current replay key (it recorded the failure as a 60s poll Timeout; the check now fails with an explicit AssertionError naming the observed ResourceNotFoundException), so the suite did not report this check as owned and it is filed here as a surviving failure.

Cause 1 of 1: DescribeAgreement performs no authorization evaluation, so an explicit IAM Deny on the action has no effect.

AWS: Smithy (services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/smithy-model/com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301.json): DescribeAgreement(agreementId: ResourceId @required, @length 1..64, pattern ^[A-Za-z0-9_/-]+$) -> DescribeAgreementOutput, errors AccessDeniedException (HTTP 403), ResourceNotFoundException (HTTP 404), ValidationException (400), ThrottlingException (429), InternalServerException (500); auth scheme sigv4. AWS evaluates the caller's identity-based policy BEFORE resolving the resource: with an explicit Deny on aws-marketplace:DescribeAgreement attached to the calling role, the identical call on an agreementId that does not exist is refused 403 AccessDeniedException, not 404 ResourceNotFoundException; detaching the Deny returns the identical call on the identical session to the 404 not-found outcome. The AccessDeniedException message for this class is AWS's IAM-denial template: fixed text "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " or ", " action, or explicitly denies the actions"; its variable parts come from the request and account state — the caller's assumed-role ARN (arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>), the action name (aws-marketplace:DescribeAgreement), the resource ("*"), and the alternative action AWS names for this API (aws-marketplace:ViewSubscriptions). The ARN, role name and session name in the recorded run are values of that run, not part of the requirement. The not-found refusal is 404 ResourceNotFoundException with message "Provided agreement does not exist." (fixed text; no variable part).

Emulator: after the explicit Deny is attached, the identical DescribeAgreement call on the same assumed-role session keeps returning ('ResourceNotFoundException', 'Provided agreement does not exist.') for the full 60s the check polls — the 403 never arrives. The pre-Deny leg and the post-detach restore leg both match, and the unrestricted caller's control call also matches, so the divergence is isolated to the denied leg: no identity-based policy is consulted for this operation at all, and the state lookup runs unconditionally.

Why: a client or test asserting least-privilege behaviour — that a principal explicitly denied (or never granted) aws-marketplace:DescribeAgreement cannot read agreements — sees the call behave exactly as if it were permitted. Authorization regressions are invisible against the emulator, and AWS's guaranteed 403-before-404 ordering is inverted, so error handling keyed on AccessDeniedException never fires and a caller cannot distinguish "not permitted" from "does not exist".

Fix: authorization for the DescribeAgreement handler — evaluate the caller's identity-based policy for aws-marketplace:DescribeAgreement (honouring an explicit Deny, and the ViewSubscriptions alternative) before the agreement store lookup, and return the modeled AccessDeniedException at HTTP 403 with AWS's IAM-denial message template rendered from the calling principal and the action.

Traces: marketplace-agreement.describe-agreement-missing-permission-denied.

## Evidence
Suite: 15/18 passed; marketplace-agreement 15/18 (83%), 3 failed.

[mismatch] marketplace-agreement.describe-agreement-missing-permission-denied
  Setup: iam.create_role with a trust policy allowing the account root; put_role_policy Allow on aws-marketplace:DescribeAgreement; sts.assume_role into it; marketplace-agreement client in us-east-1 from the session credentials.
  Leg 1 (pre-Deny, passes): as_role.describe_agreement(agreementId=<nonexistent>) -> ('ResourceNotFoundException', 'Provided agreement does not exist.').
  Leg 2 (the divergence): after put_role_policy with {'Effect':'Deny','Action':'aws-marketplace:DescribeAgreement','Resource':'*'},
    Expected: ('AccessDeniedException', 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: aws-marketplace:DescribeAgreement on resource: * because no identity-based policy allows the aws-marketplace:DescribeAgreement or aws-marketplace:ViewSubscriptions action, or explicitly denies the actions') at HTTP 403.
    Actual: ('ResourceNotFoundException', 'Provided agreement does not exist.') on every poll for 60s ->
      AssertionError: expected ('AccessDeniedException', ...) within 60.0s, last observed ('ResourceNotFoundException', 'Provided agreement does not exist.')
  Leg 3 (unrestricted-caller control, passes) and leg 4 (post-detach restore, passes) both give the not-found outcome, isolating the defect to the denied leg.
  Model: DescribeAgreement errors include AccessDeniedException (httpError 403); AccessDeniedException is also a service-common error.

## Steps to Reproduce
All calls against the emulator's marketplace-agreement endpoint in us-east-1 (the service answers only there).

1. iam.create_role(RoleName=<role>, AssumeRolePolicyDocument=<trust policy allowing arn:aws:iam::<account>:root to sts:AssumeRole>).
2. iam.put_role_policy(RoleName=<role>, PolicyName='allow-describe-agreement', PolicyDocument={'Version':'2012-10-17','Statement':[{'Effect':'Allow','Action':'aws-marketplace:DescribeAgreement','Resource':'*'}]}).
3. creds = sts.assume_role(RoleArn=<role arn>, RoleSessionName=<session>)['Credentials']; mpa_role = boto3.client('marketplace-agreement', region_name='us-east-1', **creds).
4. mpa_role.describe_agreement(agreementId='<nonexistent-id>') -> ResourceNotFoundException 'Provided agreement does not exist.' (matches AWS).
5. iam.put_role_policy(RoleName=<role>, PolicyName='deny-describe-agreement', PolicyDocument={'Version':'2012-10-17','Statement':[{'Effect':'Deny','Action':'aws-marketplace:DescribeAgreement','Resource':'*'}]}).
6. Repeat step 4 on the SAME session (polling for IAM eventual consistency): expected AccessDeniedException at HTTP 403 with the IAM-denial message; observed ResourceNotFoundException indefinitely.
7. iam.delete_role_policy(RoleName=<role>, PolicyName='deny-describe-agreement'); repeat step 4: the not-found outcome is expected and observed.
