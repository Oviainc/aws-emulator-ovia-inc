Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.repository-permissions-policy-enforced-for-cross-principal-access
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.repository-permissions-policy-enforced-for-cross-principal-access|_error#22|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 6bf56903357d8bdb827814619137753aca334420

# CodeArtifact GetRepositoryEndpoint is not authorized: a role named in neither an identity policy nor the repository permissions policy is served HTTP 200 instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `6bf56903357d8bdb827814619137753aca334420`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositoryEndpoint

## Description
One root cause (Cause 1 of 1): the CodeArtifact data-plane-endpoint read GetRepositoryEndpoint is dispatched without ever making an authorization decision, so the repository permissions policy is not enforced for cross-principal access.

AWS: GetRepositoryEndpoint is an authorized control-plane read. Smithy (com.amazonaws.codeartifact#CodeArtifactControlPlaneService, GetRepositoryEndpoint, GET /v1/repository/endpoint, code 200) models AccessDeniedException with httpError 403 among its errors (alongside ResourceNotFoundException 404, ValidationException 400, ThrottlingException 429, InternalServerException 500). A caller is permitted only when some policy allows codeartifact:GetRepositoryEndpoint on the repository resource: either the caller's identity policy, or the repository permissions policy naming that caller as Principal. A principal that is named in NEITHER is refused with AccessDeniedException / HTTP 403, and the message AWS emits for a request denied purely for lack of an identity-based grant is:

  User: arn:aws:sts::<account>:assumed-role/<roleName>/<sessionName> is not authorized to perform: codeartifact:GetRepositoryEndpoint on resource: arn:aws:codeartifact:<region>:<account>:repository/<domainName>/<repositoryName> because no identity-based policy allows the codeartifact:GetRepositoryEndpoint action

In that template the FIXED text is exactly: "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action". The VARIABLE parts are taken from the request and from state, never from any one recording: the caller's assumed-role ARN (account id, role name and role-session name of the signing session), the action name (service-qualified operation being denied), and the repository ARN (partition, region, account, domain name and repository name resolved from the request's domain/repository). The complementary requirement in the same check is the positive one: the role that the repository permissions policy DOES name as Principal for codeartifact:GetRepositoryEndpoint, while holding no identity-based CodeArtifact grant at all, must succeed via the resource grant and receive an https repositoryEndpoint. The emulator already satisfies that half, which is what isolates the defect to the deny path rather than to resource-policy storage.

Emulator: the un-granted sibling role's call is answered HTTP 200 with a full body — content-type application/json, content-length 139, {'repositoryEndpoint': 'https://<domain>-<account>.d.codeartifact.<region>.amazonaws.com/npm/<repository>/'} — for an assumed-role session that has no identity-based CodeArtifact permission and is not a Principal in the repository permissions policy. No AccessDeniedException is produced, so neither the identity-policy evaluation nor the resource-policy evaluation runs for this operation; the stored repository policy is inert as an authorization input and is only honoured incidentally because everything is allowed.

Why: a client relying on AWS's behavior gets the inverse of the security contract it tested. Every negative authorization assertion around repository endpoints passes vacuously against the emulator: a repository fenced off to one named role is readable by any principal in the account, and code or CI that proves "this role cannot discover/reach the repository endpoint" is validated green against an emulator that hands the endpoint URL — and with it the npm/pypi registry host — to everyone. Because the handed-back endpoint is the gateway to the package data plane, the vacuous pass hides a privilege escalation, not just a wrong status code. A client branching on the refusal (catching AccessDeniedException to fall back to a different repository, or surfacing an access error to the user) never takes that branch, and a client parsing the refusal message for the action or resource name has nothing to parse.

This is a DIFFERENT operation and policy surface from the open issue 20261022T171312Z-codeartifact-no-iam-authorization-on-reads-publishpackageversion-digest.md, whose Cause 1 is grounded only in DescribeRepository's identity-policy check (trace codeartifact.describe-repository-missing-permission-denied) and does not carry this check's replay key; that issue does not mention GetRepositoryEndpoint or the cross-principal repository-permissions-policy path, so it does not cover this failure. If the two are fixed by one shared authorization pipeline change, closing both requires replaying both trace lists.

Fix: the authorization decision in the request pipeline for the CodeArtifact control-plane operations — identity-policy evaluation plus repository/domain resource-policy evaluation must run ahead of the GetRepositoryEndpoint handler, and the handler must have an AccessDeniedException (403) path that renders the not-authorized message from the caller's session ARN, the service-qualified action and the resolved repository ARN. The deny path, not the resource-policy store, is the component at fault: the allow-by-resource-policy half already behaves.

Traces: codeartifact.repository-permissions-policy-enforced-for-cross-principal-access.

## Evidence
Check codeartifact.repository-permissions-policy-enforced-for-cross-principal-access [mismatch]:

  File "check_codeartifact_repository_permissions_policy_enforced_for_cross_principal_access.py", line 75, in run
    code, message = _error(
  File "check_codeartifact_repository_permissions_policy_enforced_for_cross_principal_access.py", line 181, in _error
    raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
  AssertionError: expected the call to be denied, but it SUCCEEDED and returned
    {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '139', ...}, 'RetryAttempts': 0},
     'repositoryEndpoint': 'https://<domain>-<account>.d.codeartifact.<region>.amazonaws.com/npm/<repository>/'}
  (host, generated domain/repository names and the account id above are per-run values, not requirements.)

Expected (AWS):
  HTTP status: 403
  Error code:  AccessDeniedException
  Error message: "User: arn:aws:sts::<account>:assumed-role/<otherRoleName>/other is not authorized to perform: codeartifact:GetRepositoryEndpoint on resource: arn:aws:codeartifact:<region>:<account>:repository/<domain>/<repository> because no identity-based policy allows the codeartifact:GetRepositoryEndpoint action"
    - fixed text: "User: " / " is not authorized to perform: " / " on resource: " / " because no identity-based policy allows the " / " action"
    - from the request/state: the caller's assumed-role ARN (account, role name, role-session name of the signing SigV4 session), the action codeartifact:GetRepositoryEndpoint, and the repository ARN built from the request's domain and repository.

Actual (emulator): HTTP 200 and the repositoryEndpoint body, for a session with no CodeArtifact identity grant that is not a Principal in the repository permissions policy.

Smithy citation: GetRepositoryEndpoint errors -> AccessDeniedException (errorKind client, httpError 403), ResourceNotFoundException 404, ValidationException 400, ThrottlingException 429, InternalServerException 500; http { method GET, uri /v1/repository/endpoint, code 200 }; required input members domain, repository, format.

Control within the same check that the emulator passes: the role the repository policy names as Principal for codeartifact:GetRepositoryEndpoint, holding no identity-based grant, receives 200 and an https endpoint — so the divergence is the missing refusal, not a broken resource policy store.

## Steps to Reproduce
1. codeartifact.create_domain(domain=D); codeartifact.create_repository(domain=D, repository=R).
2. iam.create_role twice to create two bare roles, GRANTED and OTHER, each with a trust policy allowing sts:AssumeRole from arn:aws:iam::<account>:root and with NO inline or attached CodeArtifact permissions.
3. codeartifact.put_repository_permissions_policy(domain=D, repository=R, policyDocument=<policy with one Allow statement: Principal {"AWS": <GRANTED role ARN>}, Action ["codeartifact:GetRepositoryEndpoint"], Resource "*">).
4. sts.assume_role into GRANTED (session name "granted") and into OTHER (session name "other"); build a codeartifact client from each set of session credentials.
5. With the GRANTED client: get_repository_endpoint(domain=D, repository=R, format='npm') -> succeeds with an https repositoryEndpoint (this half already behaves).
6. With the OTHER client: get_repository_endpoint(domain=D, repository=R, format='npm').
   Expected: ClientError with HTTP 403, Error.Code == 'AccessDeniedException' and Error.Message == "User: arn:aws:sts::<account>:assumed-role/<OTHER role name>/other is not authorized to perform: codeartifact:GetRepositoryEndpoint on resource: arn:aws:codeartifact:<region>:<account>:repository/D/R because no identity-based policy allows the codeartifact:GetRepositoryEndpoint action".
   Observed: HTTP 200 with {'repositoryEndpoint': 'https://...'} and no error at all.
