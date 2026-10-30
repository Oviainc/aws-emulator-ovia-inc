Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.limit-exceeded-resource-server
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.limit-exceeded-resource-server|run#8|AssertionError|raise AssertionError(
Issue-Commit: fdbc3a0ffd43a28553464e8afd6021169d0df514

# cognito-idp: CreateResourceServer enforces no per-user-pool maximum — 301 resource servers on one pool all succeed instead of LimitExceededException

> _Note: this issue was identified against the code base at commit `fdbc3a0ffd43a28553464e8afd6021169d0df514`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResourceServer

## Description
Cause 1 of 1: CreateResourceServer accepts an unbounded number of resource servers on one user pool.

AWS: resource servers per user pool is an enforced, Service-Quotas-adjustable quota (cognito-idp quota code L-7CDAF993, AWS default 25, adjustable up to 300). CreateResourceServer on a pool that already holds the enforced maximum is refused with LimitExceededException (Smithy: CreateResourceServer errors include LimitExceededException, errorKind client, HTTP 400 — "This exception is thrown when a user exceeds the limit for a requested AWS resource"). The refusal message's fixed text is "The limit for user pool resource servers is: " followed by the enforced maximum and a "." — the number is the variable part, taken from the account's quota value for L-7CDAF993, not from the request. The refused resource server must NOT persist (ListResourceServers still shows exactly the successfully-created ones), and the refusal tracks the live count: after DeleteResourceServer frees one slot, the identical previously-refused CreateResourceServer succeeds and the pool is back at the boundary.
Emulator: no per-pool ceiling is enforced anywhere. The check walked past the documented adjustable maximum and every single create succeeded — "301 creates on one pool all SUCCEEDED" — so LimitExceededException is never raised, its message is never produced, and the free-a-slot/retry restore step can never be reached.
Why: an over-permissive control plane. A client that provisions resource servers in a loop (or a test asserting the quota boundary) sees unlimited success on the emulator and a hard LimitExceededException on AWS at the 25th (or the account's raised value); capacity-planning and quota-handling code paths are never exercised, and state that AWS would have rejected is persisted.
Fix: CreateResourceServer's validation in its handler must count the pool's stored resource servers before storing a new one and refuse with LimitExceededException, worded as AWS words it, when the pool is already at the enforced maximum; the refused create must leave the pool's resource-server set unchanged. The enforced maximum should be the same value the service's quota surface reports for L-7CDAF993 (see also 20261030T160310Z-service-quotas-listawsdefaultservicequotas-cognito-idp-default-catalog-serves-a-re.md, which covers that quota's catalog entry and NOT this enforcement gap).
Traces: cognito-idp.limit-exceeded-resource-server. Code site: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/create_resource_server.rs.

Note on neighbouring open issues: 20261029T184956Z-cognito-idp-identity-provider-defaults-resource-server-scope.md concerns resource-server SCOPE ORDER, and 20261029T184956Z-cognito-idp-ten-request-validation-and-precondition-gates.md is a static-surface bundle; neither carries this check's replay key, and the replay report marked this failure neither ALREADY FILED nor OWNED.

## Evidence
Replay output (behavioral suite):

[mismatch] cognito-idp.limit-exceeded-resource-server
  AssertionError: real AWS rejects CreateResourceServer past the per-pool maximum, but 301 creates on one pool all SUCCEEDED

Expected (what the check asserts after the first rejection at count N): error code LimitExceededException; message exactly "The limit for user pool resource servers is: N." where N is the enforced maximum and is >= the account's Service Quotas value for cognito-idp L-7CDAF993 (AWS default 25); the overflow identifier absent from ListResourceServers; then after DeleteResourceServer of one identifier, the identical CreateResourceServer(Identifier=<overflow identifier>) succeeds and ListResourceServers is back to N entries.
Actual: 301 consecutive CreateResourceServer calls on a single freshly created user pool all returned success; no LimitExceededException was ever raised, so no message, no non-persistence and no restore behavior could be observed.

## Steps to Reproduce
1. CreateUserPool.
2. Call CreateResourceServer(UserPoolId=<pool>, Identifier=<unique-i>, Name=<unique-i>) in a loop, i = 0..300, retrying only TooManyRequestsException.
3. Observe every call returns 200 and ListResourceServers pages back all 301 identifiers. On AWS the create is refused with LimitExceededException (HTTP 400) once the pool holds the enforced maximum (default 25), with message "The limit for user pool resource servers is: <maximum>.", the refused resource server is not listed, and after deleting one the identical refused create succeeds.
