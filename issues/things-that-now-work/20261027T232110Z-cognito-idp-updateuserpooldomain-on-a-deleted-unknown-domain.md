Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.user-pool-domain-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.user-pool-domain-lifecycle|run#17|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# cognito-idp: UpdateUserPoolDomain on a deleted/unknown domain answers InvalidParameterException instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateUserPoolDomain

## Description
REOPENS 20261024T114943Z-cognito-idp-updateuserpooldomain-stub.md: its fix left the call refused.

That issue implemented the `UpdateUserPoolDomain` scaffold (it carried this very check, `cognito-idp.user-pool-domain-lifecycle`, in its Issue-Trace-Ids) and is marked fixed, but the implemented handler still refuses the check's final call — now with the wrong error class instead of a 501. The open issue 20261025T030229Z-cognito-idp-six-causes-confirmsignup-resendconfirmationcode-getuserattributeverificationcode-updateuserpooldomain.md also mentions UpdateUserPoolDomain, but it covers that operation answering NotImplemented, not this error-classification defect, and the suite does not mark this check as owned by it.

Cause 1 of 1: UpdateUserPoolDomain misclassifies an unresolvable UserPoolId+Domain pair.

AWS: Smithy (com.amazonaws.cognitoidentityprovider#UpdateUserPoolDomain) models `ResourceNotFoundException` (HTTP 404) — "thrown when the Amazon Cognito service can't find the requested resource" — alongside `InvalidParameterException` (HTTP 400). `Domain` and `UserPoolId` are both @required and both are pattern/length-constrained, and the request in question satisfies both constraints: the pair is well-formed, the domain record simply no longer exists because `DeleteUserPoolDomain` removed it. Recorded behavior for that case: AWS refuses `UpdateUserPoolDomain` with error code `ResourceNotFoundException` and the message `User pool domain was not found.` That message is entirely fixed text for this error class — it has no variable part, and in particular it does not echo the domain name or the pool id. Note that the sibling refusal is deliberately different and the emulator already gets it right: `DeleteUserPoolDomain` naming an already-gone domain answers `InvalidParameterException` with `No such domain or user pool exists.` (the check's own teardown accepts exactly that pair), so the two operations must not share one refusal helper.

Emulator: the call is refused with error code `InvalidParameterException` (HTTP 400) and the message `No such domain or user pool exists.` In services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_user_pool_domain.rs the handler resolves the pair at lines 214-227 and, for both the unknown-pool and the unknown-domain-of-a-known-pool branch, returns the local `no_such_domain()` helper (lines 349-351), which delegates to `invalid_parameter(crate::state::NO_SUCH_DOMAIN_MESSAGE)` — i.e. the Delete-flavoured refusal. `ResourceNotFoundException` is declared in `UpdateUserPoolDomainErrorModeled` (lines 141-150) but is never constructed anywhere in the file.

Why: a client that deletes a domain and then updates it — or that retries an update after a concurrent delete — cannot distinguish "this resource is gone" from "you sent a malformed parameter". Clients branch on the error code: `ResourceNotFoundException` is the recreate/stop-retrying signal, while a 400 `InvalidParameterException` reads as a caller bug and is typically surfaced to the user or treated as non-retryable input validation. SDK/CDK and IaC reconcilers that key the "resource no longer exists, create it" path on `ResourceNotFoundException` will instead fail hard, and any code matching on the message text (`User pool domain was not found.`) never matches. The HTTP status is wrong too (400 instead of 404), so status-based retry/alarm classification diverges as well.

Fix: the `UpdateUserPoolDomain` handler's refusal path — give this operation its own not-found helper that constructs `UpdateUserPoolDomainErrorModeled::ResourceNotFoundException` with the fixed message `User pool domain was not found.`, and use it for BOTH unresolvable branches (pool not stored, and pool stored but holding no domain of that name). Leave `DeleteUserPoolDomain`'s distinct `InvalidParameterException` / `No such domain or user pool exists.` refusal unchanged; do not fold the two into one shared helper in crate::state.

Traces: cognito-idp.user-pool-domain-lifecycle (assertion at check_cognito_idp_user_pool_domain_lifecycle.py line 80: "expected ResourceNotFoundException updating a deleted domain, got 'InvalidParameterException'"). Handler: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_user_pool_domain.rs lines 214-227 and 339-363.

## Evidence
Failing check: [mismatch] cognito-idp.user-pool-domain-lifecycle

  File "check_cognito_idp_user_pool_domain_lifecycle.py", line 80, in run
    assert code == "ResourceNotFoundException", (
  AssertionError: expected ResourceNotFoundException updating a deleted domain, got 'InvalidParameterException'

Expected (AWS, and the message the check asserts next):
  HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "User pool domain was not found."
  (fixed text in full; no variable parts)

Actual (emulator):
  HTTP 400, Error.Code = "InvalidParameterException", Error.Message = "No such domain or user pool exists."
  (crate::state::NO_SUCH_DOMAIN_MESSAGE, returned via no_such_domain() in update_user_pool_domain.rs:349)

Model evidence (smithy_query query=operation, operation=UpdateUserPoolDomain):
  errors include {"shape": "ResourceNotFoundException", "errorKind": "client", "httpError": 404}
  input: Domain (required, len 1..63, pattern ^[a-z0-9](?:[a-z0-9\\-]{0,61}[a-z0-9])?$), UserPoolId (required, len 1..55, pattern ^[\\w-]+_[0-9a-zA-Z]+$) — both satisfied by the failing request.

Control showing the two refusals are NOT the same: the check's teardown accepts DeleteUserPoolDomain answering ("InvalidParameterException", "No such domain or user pool exists.") for the same already-deleted domain, and that path passes.

Everything earlier in the check passed: create -> describe ACTIVE with ManagedLoginVersion 1 -> update to 2 -> describe shows 2 -> delete -> describe no longer ACTIVE. Only the post-delete UpdateUserPoolDomain refusal diverges.

## Steps to Reproduce
1. Boot the cognito-idp emulator.
2. CreateUserPool(PoolName=<any>) -> note UserPoolId.
3. CreateUserPoolDomain(Domain=<lowercase prefix>, UserPoolId=<id>, ManagedLoginVersion=1).
4. DescribeUserPoolDomain(Domain=<prefix>) until Status == ACTIVE.
5. DeleteUserPoolDomain(Domain=<prefix>, UserPoolId=<id>).
6. Call UpdateUserPoolDomain(Domain=<prefix>, UserPoolId=<id>, ManagedLoginVersion=1) on the now-deleted domain.
   Observed: HTTP 400 InvalidParameterException "No such domain or user pool exists."
   Expected: HTTP 404 ResourceNotFoundException "User pool domain was not found."
7. Same divergence for a Domain the pool never held, and for a UserPoolId that does not exist.
8. Re-replay: cognito-idp.user-pool-domain-lifecycle
