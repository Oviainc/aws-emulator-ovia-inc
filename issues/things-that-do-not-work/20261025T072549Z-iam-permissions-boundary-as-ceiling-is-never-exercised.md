Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.permissions-boundary-alone-never-grants-a-real-signed-call
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.permissions-boundary-alone-never-grants-a-real-signed-call|_poll#17|Timeout|time.sleep(interval_s)
Issue-Commit: c03b32b2dcc5227aba431e790e0b1e2397cf2279

# iam: permissions-boundary-as-ceiling is never exercised — the check's ListUserTags control is refused 501 and the check times out

> _Note: this issue was identified against the code base at commit `c03b32b2dcc5227aba431e790e0b1e2397cf2279`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListUserTags

## Description
One root cause.

Cause 1 of 1: a signed ListUserTags call is refused with HTTP 501 NotImplemented, which stalls the only behavioral check that tests whether a permissions boundary can GRANT a permission, so that over-permissiveness claim is never put to the emulator at all.

AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/...): operation ListUserTags takes UserName (existingUserNameType, required, @length 1..128, pattern ^[\w+=,.@-]+$) plus Marker/MaxItems, and returns Tags (tagListType, required, @length 0..50) and IsTruncated (required); its only modeled errors are NoSuchEntityException (HTTP 404) and ServiceFailureException (HTTP 500). There is no modeled error for "not implemented", so a request that satisfies those constraints against an existing user must get HTTP 200 with the tag list. For the behaviour the check is actually about — a permissions boundary attached by PutUserPermissionsBoundary is a ceiling and never a source of permissions, so an action Allowed only by the boundary is refused AccessDenied, and SimulatePrincipalPolicy reports EvalDecision implicitDeny with PermissionsBoundaryDecisionDetail.AllowedByPermissionsBoundary set — recorded behavior only; the model states the shapes but not the decision rule.

Emulator: ListUserTags is an unmodified scaffold. services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_user_tags.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its handle() body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("ListUserTags"))`, so every call answers HTTP 501 NotImplemented. The check uses ListUserTags as its credential-health control: it attaches an inline identity policy that Allows exactly iam:ListUserTags on the user's own ARN, mints an access key, and polls that signed call until it returns success. Against this emulator that poll can never converge (the wrapper records the error code "NotImplemented" forever), and the harness terminates the check at its 60s budget — the replay reports exactly `Check timed out after 60.0s`. Everything before the poll did succeed (CreateUser, PutUserPolicy, CreatePolicy, PutUserPermissionsBoundary, and GetUser echoing User.PermissionsBoundary.PermissionsBoundaryArn), so the emulator stands up the scenario and then stalls on the control. Consequently the emulator is never asked the three assertions that follow: the boundary-only ListAccessKeys refusal, the identity-grant-makes-it-succeed leg, and the grant-removed-restores-the-refusal leg. The SimulatePrincipalPolicy leg in the same check would also be refused: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs is a 501 scaffold too (that one is already filed and is reported as blocking 14 checks).

Related open issue, which does NOT cover this check: 20261025T000319Z-iam-listusertags-stub.md names the same ListUserTags scaffold, but its Issue-Trace-Ids are only `iam.abac-resource-tag-must-match-principal-tag-on-a-real-signed-call` and `iam.iam-eval-deep-iam-1-date-greater-than-past-instant-allows`. Closing it therefore does not cause `iam.permissions-boundary-alone-never-grants-a-real-signed-call` to be re-replayed, and this check is the only unit in the suite that can detect a permissions boundary being unioned into a principal's effective permissions.

Why: two things break for a client. First, any client that reads a user's tags through ListUserTags (tag-based access control tooling, ABAC policy authoring, inventory and compliance scanners) gets HTTP 501 NotImplemented — an unmodeled status the AWS SDKs surface as a hard non-retryable client error, where real AWS returns 200 and a tag list. Second, and worse, the one piece of coverage that would catch an inverted permissions boundary is silently unexercised: if the emulator implements a boundary as "one more policy in the union", a client validating least-privilege against this emulator sees a call succeed that real AWS refuses with AccessDenied, and ships a policy that is strictly more permissive than it was tested as.

Fix: the operation's handler — services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_user_tags.rs — must read the user's tags out of the service state and return them (NoSuchEntityException when the user does not exist), and the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line must then be deleted. Once the control passes, the authorization path for this check lives in the IAM request authorizer / policy-evaluation component that evaluates identity policies against an attached permissions boundary (the boundary must be intersected as a ceiling, never unioned as a grant), and in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs for the advisory PermissionsBoundaryDecisionDetail leg.

Traces: iam.permissions-boundary-alone-never-grants-a-real-signed-call (verdict mismatch; `Check timed out after 60.0s`).

## Evidence
Replay of `iam.permissions-boundary-alone-never-grants-a-real-signed-call`:

  #### [mismatch] iam.permissions-boundary-alone-never-grants-a-real-signed-call
  Error: Check timed out after 60.0s

The check body (check_iam_permissions_boundary_alone_never_grants_a_real_signed_call.py) reaches:

  ready = _poll(
      lambda: _outcome(lambda: as_user.list_user_tags(UserName=user)),
      lambda o: o == "OK", timeout_s=90, interval_s=3,
      description="the baseline grant, permitted by both documents, becomes effective")

_outcome() returns the AWS error code on any ClientError, so the predicate `o == "OK"` can never be satisfied while the call is refused.

Expected (AWS): ListUserTags -> HTTP 200 with `Tags` and `IsTruncated` (Smithy: output members Tags tagListType @length 0..50 required, IsTruncated required; modeled errors only NoSuchEntityException 404 / ServiceFailureException 500). Then ListAccessKeys under a boundary-only Allow -> error code exactly `AccessDenied`; after adding an identity-policy Allow -> success; after deleting it -> `AccessDenied` again.

Actual (emulator): ListUserTags -> HTTP 501, error code `NotImplemented`, message e.g. "operation ListUserTags is scaffolded but not implemented in this emulator" (message quoted as an example). Source, services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_user_tags.rs:
  line 4:   //! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`
  line 179: Err(crate::scaffolded::Scaffolded::not_implemented("ListUserTags"))
The boundary legs and the SimulatePrincipalPolicy leg are never reached, so the emulator's permissions-boundary decision is unmeasured. services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs is likewise a 501 scaffold.

Steps before the stall all succeeded, which is what localises the failure: CreateUser, GetUser (User.Arn), PutUserPolicy (baseline Allow of iam:ListUserTags), CreatePolicy (boundary allowing iam:ListUserTags and iam:ListAccessKeys on *), PutUserPermissionsBoundary, and GetUser reporting User.PermissionsBoundary.PermissionsBoundaryArn equal to the boundary ARN.

## Steps to Reproduce
1. Boot the iam emulator from this checkout.
2. CreateUser with a fresh name; GetUser to capture its ARN.
3. PutUserPolicy on that user with an inline document Allowing `iam:ListUserTags` on the user's own ARN (and nothing else).
4. CreatePolicy with a document Allowing [`iam:ListUserTags`, `iam:ListAccessKeys`] on `*`, then PutUserPermissionsBoundary on the user with that policy ARN. GetUser confirms User.PermissionsBoundary.PermissionsBoundaryArn.
5. CreateAccessKey for the user and build an iam client signing with that key.
6. Call ListUserTags(UserName=<user>) with those credentials. Observe HTTP 501 / error code `NotImplemented`; AWS returns HTTP 200 with the (empty) Tags list.
7. Because of 6, the rest of the scenario is never reached. To see it after ListUserTags is implemented: call ListAccessKeys(UserName=<user>) with the same key — AWS refuses it with error code exactly `AccessDenied` because only the boundary allows it; then PutUserPolicy a second inline document Allowing `iam:ListAccessKeys` on the user ARN and the identical call must succeed; then DeleteUserPolicy of that second document and the identical call must be refused `AccessDenied` again, with the boundary attached and unchanged throughout.
8. Equivalently: replay the behavioral check `iam.permissions-boundary-alone-never-grants-a-real-signed-call`.
