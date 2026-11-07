Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks-auth.assume-role-access-denied-explicit-deny
Issue-Shape-Id: com.amazonaws.eksauth#EKSAuthFrontend
Issue-Key: eks-auth.assume-role-access-denied-explicit-deny|_poll_until#8|AssertionError|raise AssertionError(f"expected {expect!r} within {attempts * delay}s of polling, last={last!r}")
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# eks-auth AssumeRoleForPodIdentity never evaluates the caller's IAM authorization, so an explicitly-denied principal is answered InvalidParameterException instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleForPodIdentity

## Description
One net-new root cause in service eks-auth, on the only operation this service binds (AssumeRoleForPodIdentity, POST /clusters/{clusterName}/assume-role-for-pod-identity, restJson1).

RELATION TO OPEN ISSUES: the open issue 20261023T153145Z-eks-auth-assumeroleforpodidentity-rejects-in-the-wrong-order.md describes this same surface as its "Cause 3 of 3", but the behavioral replay did NOT mark this check as OWNED by it — that issue's replay key for this check records a harness Timeout signature (`eks-auth.assume-role-access-denied-explicit-deny|_poll_until#8|Timeout|time.sleep(delay)`), while the check now fails with a different, concrete signature: the poll completes and reports last='InvalidParameterException'. The emulator's behaviour on this path has therefore CHANGED (the constraint-rejection code is now the modeled InvalidParameterException rather than ValidationException/ResourceNotFoundException), but the authorization gap itself is still present and is re-grounded here with the current evidence so a fixer can re-replay it. It is filed, not skipped, because no open issue carries this check's current replay key.

Cause 1 of 1: the caller's IAM permissions are never evaluated for eks-auth:AssumeRoleForPodIdentity, so a principal carrying an explicit Deny is admitted into request validation instead of being refused.
AWS: Smithy (services/service-com-amazonaws-eksauth-eksauthfrontend/smithy-model/com-amazonaws-eksauth-eksauthfrontend.json) models AccessDeniedException on AssumeRoleForPodIdentity with httpError 400 and documents it as "You don't have permissions to perform the requested operation. The IAM principal making the request must have at least one IAM permissions policy attached that grants the required permissions." The ordering is recorded behavior only: for a principal whose identity policy contains an explicit Deny on Action eks-auth:AssumeRoleForPodIdentity, AWS refuses the call with HTTP 400 / AccessDeniedException BEFORE it looks at the request body, so even a request naming a clusterName that does not exist and carrying a token that is not even JWT-shaped surfaces the permission failure. The same request from an unrestricted principal is refused with a different, more specific code (the recording shows the request-shape error, i.e. InvalidParameterException for a token that violates the JwtToken pattern), which is what makes the AccessDeniedException attributable to the caller's policy rather than to the request. In the AccessDeniedException message, the fixed text is AWS's permission sentence above; nothing in it is request- or state-derived, and the requirements asserted by the check are the status (400) and the code (AccessDeniedException).
Emulator: no authorization step runs on this path at all. With the deny-carrying STS session credentials, AssumeRoleForPodIdentity(clusterName=<nonexistent, pattern-valid name>, token="not-a-jwt-at-all") is answered with the request-shape refusal InvalidParameterException; the check polled 12 times at 5s intervals (60s) and AccessDeniedException was never observed (last='InvalidParameterException'). The IAM/STS scaffolding completed — the check raises an AssertionError immediately and distinctly when create_role/put_role_policy/get_caller_identity/assume_role are unreachable, and it did not take that path — so the denied session really was presented to eks-auth. This agrees with the handler, which never consults the caller's identity or policies: services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs documents and implements layers 1-5 as token parse -> kid resolution -> issuance/lifetime -> pod-identity association -> credential minting, and constructs AccessDeniedException only for "the cluster holds no pod identity association for this (namespace, service account)"; model-constraint failures are refused ahead of the handler by the framework and worded by crate::service as InvalidParameterException. Hypothesis: the request-authorization layer is simply absent for this operation, so every caller is treated as fully permitted.
Why: this is an over-permissive divergence. A client, policy test or CI suite that asserts an IAM deny boundary on eks-auth:AssumeRoleForPodIdentity is enforced sees the request accepted into business-logic validation and refused for a body reason instead, so a guardrail that real AWS enforces passes against the emulator; and a Pod Identity Agent that distinguishes "this node role may not call eks-auth" (AccessDeniedException) from "the request is malformed" (InvalidParameterException) takes the wrong remediation path.
Fix: the operation's authorization step — a request-authorization middleware for this service, or the entry of `handle` in services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs — must evaluate the calling principal's IAM permissions for eks-auth:AssumeRoleForPodIdentity and raise AccessDeniedException (400) on an explicit deny, ahead of model-constraint validation and ahead of token parsing and cluster resolution.
Traces: eks-auth.assume-role-access-denied-explicit-deny.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 9/22 passed; eks-auth 8/21 (13 failed), eks__eks-auth 1/1. Of the 13 eks-auth failures, 12 are marked OWNED by existing open issues (20261104T053901Z-eks-auth-the-whole-pod-identity-data-plane.md and 20261029T212137Z-eks-auth-assumeroleforpodidentity-parses-the-token-s-claims.md) and are skipped; this is the one surviving failure.

[mismatch] eks-auth.assume-role-access-denied-explicit-deny:
  File "check_eks_auth_assume_role_access_denied_explicit_deny.py", line 76, in run
    _poll_until(_assume_restricted, expect="AccessDeniedException", attempts=12, delay=5)
  File "check_eks_auth_assume_role_access_denied_explicit_deny.py", line 120, in _poll_until
    raise AssertionError(f"expected {expect!r} within {attempts * delay}s of polling, last={last!r}")
  AssertionError: expected 'AccessDeniedException' within 60s of polling, last='InvalidParameterException'

Expected (AWS): HTTP 400, Error.Code = AccessDeniedException, message = AWS's permission sentence ("You don't have permissions to perform the requested operation. The IAM principal making the request must have at least one IAM permissions policy attached that grants the required permissions...") — all fixed text, no request- or state-derived parts.
Actual (emulator): the request-shape refusal InvalidParameterException (400) for every one of the 12 polled attempts; AccessDeniedException never returned.

The check's own positive control passed in the sense that it was reached only after the poll failed: an unrestricted caller issuing the identical bogus request also gets InvalidParameterException, i.e. the emulator answers the denied and the undenied principal identically — there is no caller-dependent behaviour at all.

Model grounding: AssumeRoleForPodIdentity errors = AccessDeniedException(400), ExpiredTokenException(400), InternalServerException(500), InvalidParameterException(400), InvalidRequestException(400), InvalidTokenException(400), ResourceNotFoundException(404), ServiceUnavailableException(503), ThrottlingException(429).
Code grounding: services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs — the documented and implemented layer list (lines 10-36) contains no authorization layer, and AccessDeniedException is reserved there for a missing pod-identity association.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Create an IAM role with a trust policy allowing sts:AssumeRole from arn:aws:iam::<account>:root (account from sts:GetCallerIdentity).
3. Attach an inline policy to that role with {"Effect":"Deny","Action":"eks-auth:AssumeRoleForPodIdentity","Resource":"*"}.
4. sts:AssumeRole into the role and build an eks-auth client from the returned session credentials.
5. Call AssumeRoleForPodIdentity(clusterName=<a pattern-valid name that does not exist>, token="not-a-jwt-at-all") with those credentials, retrying for up to 60s to allow for policy propagation.
   Observe: HTTP 400 / InvalidParameterException on every attempt; AccessDeniedException is never returned.
   AWS: HTTP 400 / AccessDeniedException, returned ahead of any body validation.
6. Issue the identical bogus request with the unrestricted default identity: it must NOT be AccessDeniedException (AWS answers the request-shape error here), which is what isolates step 5's refusal to the caller's policy.
7. Re-replay check eks-auth.assume-role-access-denied-explicit-deny; it must pass before closing.
