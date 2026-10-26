Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.delete-identity-pool-not-authorized-cross-account
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.delete-identity-pool-not-authorized-cross-account|_error#17|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: 140be5bc82ff93d772ac6ec97ebe27a7ad9b5162

# cognito-identity: DeleteIdentityPool performs no IAM authorization — a zero-permission principal deletes another caller's identity pool with HTTP 200

> _Note: this issue was identified against the code base at commit `140be5bc82ff93d772ac6ec97ebe27a7ad9b5162`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIdentityPool

## Description
This issue carries ONE net-new root cause (1 of 1), reached by the behavioral check cognito-identity.delete-identity-pool-not-authorized-cross-account. It must be fixed and re-replayed before closing.

=== Cause 1 of 1: DeleteIdentityPool is not IAM-authorized — a caller with no cognito-identity permissions destroys the pool ===

AWS: every cognito-identity control-plane operation is IAM-authorized. Smithy (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json): DeleteIdentityPool input {IdentityPoolId required, @length 1..55, @pattern ^[\w-]+:[0-9a-f-]+$}, output Unit, errors [InvalidParameterException 400, NotAuthorizedException 403, ResourceNotFoundException 404, TooManyRequestsException 429, InternalErrorException 500]. The IAM denial itself is the framework-level refusal ahead of the handler: HTTP 403 with error code AccessDeniedException, and its message is AWS's standard identity-based-policy construction, built from the request and the caller — fixed text "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action"; variable parts taken from the request and state: the caller's principal ARN (the signing IAM user's ARN), the action name cognito-identity:DeleteIdentityPool, and the resource ARN arn:aws:cognito-identity:<region>:<account>:identitypool/<identity pool id>. The complete message is therefore "User: <caller arn> is not authorized to perform: cognito-identity:DeleteIdentityPool on resource: arn:aws:cognito-identity:<region>:<account>:identitypool/<pool id> because no identity-based policy allows the cognito-identity:DeleteIdentityPool action" (recorded behavior for the denial wording; the model supplies the operation/ARN shape). The pool survives the denied call: a subsequent DescribeIdentityPool by the owner still returns it. The owner's identical DeleteIdentityPool then succeeds, and after it DescribeIdentityPool and a repeated DeleteIdentityPool both answer 404 ResourceNotFoundException with message "IdentityPool '<pool id>' not found." (fixed text "IdentityPool '" ... "' not found.", the pool id taken from the request) — which is how the check proves the first refusal was the authorization boundary and not a blanket rejection of the operation.

Emulator: the request signed with a brand-new IAM user that has zero attached policies is ACCEPTED. DeleteIdentityPool answers HTTP 200 with an empty body (content-length: 2, no error member at all), so the restricted caller destroys a pool it has no permission to touch. No authorization decision is taken on the operation.

Note on progress since the last sweep: the open issue 20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md (Cause 5) describes missing per-caller authorization and lists this check, but it carries the check's OLD replay key (run#15, "the restricted probe must resolve to a distinct principal from the real caller") — credential-to-principal resolution now works, the restricted key resolves to a DISTINCT ARN, and the check progresses to the authorization assertion at line 97 where the delete succeeds. That is a different, net-new failure point for DeleteIdentityPool and is not covered by that issue's key. Related but NOT covering: 20261024T145006Z-cognito-identity-listtagsforresource-ignores-iam-authorization-a-foreign.md Cause 1 reports the same class of gap for ListTagsForResource (a read) and does not carry this check.

Why: a client relying on AWS's behavior believes its least-privilege IAM policy protects its identity pools. Against the emulator every denied-access assertion about pool deletion passes, so the policy gap is only discovered in production — and the effect here is destructive and irreversible (the pool, its identities and its role configuration are gone), not merely an information leak. Clients that branch on 403 AccessDeniedException for "not permitted" never see that branch.

Fix: an authorization check in the DeleteIdentityPool handler of the cognito-identity service (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/delete_identity_pool.rs), evaluating the resolved caller principal's identity-based policies for cognito-identity:DeleteIdentityPool on the pool ARN before the pool is removed from state, and emitting the framework 403 AccessDeniedException with AWS's message construction when no policy allows it. The same gate belongs in the shared request-authorization layer the cognito-identity ops call, so reads and mutations are covered uniformly.

Traces: cognito-identity.delete-identity-pool-not-authorized-cross-account.

## Evidence
Check cognito-identity.delete-identity-pool-not-authorized-cross-account (mismatch).

Setup performed by the check: the ambient caller creates an identity pool (CreateIdentityPool, AllowUnauthenticatedIdentities=true); an IAM user with NO attached policies is created plus an access key; sts:GetCallerIdentity with that key is polled until usable and asserted to resolve to a DIFFERENT ARN than the ambient caller (this assertion now PASSES — principal resolution is fixed); then DeleteIdentityPool(IdentityPoolId=<pool>) is signed with the zero-permission key.

expected: HTTP 403, Code 'AccessDeniedException', Message "User: <restricted user arn> is not authorized to perform: cognito-identity:DeleteIdentityPool on resource: arn:aws:cognito-identity:<region>:<account>:identitypool/<pool id> because no identity-based policy allows the cognito-identity:DeleteIdentityPool action"; the pool still describable afterwards by its owner.

actual (replay output):
  File "check_cognito_identity_delete_identity_pool_not_authorized_cross_account.py", line 97, in run
    denied = _error(lambda: restricted_ci.delete_identity_pool(IdentityPoolId=pool_id))
  File "...", line 177, in _error
    raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
  AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Model evidence (smithy_query query=operation operation=DeleteIdentityPool): input {IdentityPoolId required @length 1..55 @pattern ^[\w-]+:[0-9a-f-]+$}, output Unit, errors [InvalidParameterException 400, NotAuthorizedException 403, ResourceNotFoundException 404, TooManyRequestsException 429, InternalErrorException 500] — i.e. a 403 refusal class is modeled for this operation; the identity-based-policy denial is the protocol-level AccessDeniedException.

Because the restricted delete succeeded, the check never reached its downstream legs, which are also requirements of the same operation: owner delete succeeds, then DescribeIdentityPool and a repeated DeleteIdentityPool on the dead id both answer ResourceNotFoundException "IdentityPool '<pool id>' not found."

## Steps to Reproduce
1. Boot the emulator and replay cognito-identity.delete-identity-pool-not-authorized-cross-account.
2. Minimal reproduction:
   a. With the ambient credentials: cognito-identity CreateIdentityPool(IdentityPoolName='probe-pool', AllowUnauthenticatedIdentities=True) -> note IdentityPoolId.
   b. iam CreateUser(UserName='noperm-probe') and CreateAccessKey for it; attach NO policies.
   c. sts GetCallerIdentity signed with the new key -> confirm its Arn differs from the ambient caller's Arn (it does today).
   d. cognito-identity DeleteIdentityPool(IdentityPoolId=<pool id>) signed with the zero-permission key.
      expect: HTTP 403, Code AccessDeniedException, Message "User: <that user's arn> is not authorized to perform: cognito-identity:DeleteIdentityPool on resource: arn:aws:cognito-identity:<region>:<account>:identitypool/<pool id> because no identity-based policy allows the cognito-identity:DeleteIdentityPool action".
      observe: HTTP 200, empty body, and the pool is gone.
   e. With the ambient credentials: DescribeIdentityPool(IdentityPoolId=<pool id>) -> expect the pool to still exist (it must survive the denied delete).
   f. Then attach a policy allowing exactly cognito-identity:DeleteIdentityPool on that pool ARN to the probe user and repeat (d): the identical call must now succeed, proving the refusal was the authorization boundary.
   g. After the authorized delete: DescribeIdentityPool and DeleteIdentityPool on the same id must both answer 404 ResourceNotFoundException with "IdentityPool '<pool id>' not found.".
