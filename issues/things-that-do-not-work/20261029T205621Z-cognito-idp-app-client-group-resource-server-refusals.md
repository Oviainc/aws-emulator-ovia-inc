Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-app-client-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-app-client-error-paths cognito-idp-client-secret-error-paths cognito-idp-user-pool-client-secret-errors cognito-idp-user-pool-client-secret-lifecycle cognito-idp-user-pool-client-secret-management cognito-idp-create-group-in-nonexistent-pool-fails cognito-idp-create-resource-server-op-contract cognito-idp-group-lifecycle cognito-idp-user-pool-group-membership-lifecycle cognito-idp-invalid-inputs-and-missing-resources cognito-idp-list-users-in-group cognito-idp-list-resource-servers-scopes-and-notfound-error cognito-idp-resource-server-and-oauth-scope-enforcement cognito-idp-resource-server-and-scoped-client cognito-idp-user-pool-client-lifecycle cognito-idp-identity-provider-management
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|ListUserPoolClients|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ListUserPoolClients|4xx->4xx|status
Issue-Key: cognito-idp|AddUserPoolClientSecret|4xx->4xx|status
Issue-Key: cognito-idp|CreateGroup|4xx->4xx|status
Issue-Key: cognito-idp|CreateResourceServer|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateResourceServer|4xx->4xx|status
Issue-Key: cognito-idp|GetGroup|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetGroup|4xx->4xx|status
Issue-Key: cognito-idp|ListUsersInGroup|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ListUsersInGroup|4xx->4xx|status
Issue-Key: cognito-idp|ListResourceServers|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ListResourceServers|4xx->4xx|status
Issue-Key: cognito-idp|DescribeResourceServer|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeResourceServer|4xx->4xx|status
Issue-Key: cognito-idp|CreateUserPoolClient|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateUserPoolClient|4xx->4xx|status
Issue-Key: cognito-idp|DescribeIdentityProvider|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeIdentityProvider|4xx->4xx|status
Issue-Related: 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md
Issue-Related: 20261029T184956Z-cognito-idp-ten-duplicate-resource-and-policy-refusal.md
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# cognito-idp app client group resource-server refusals serve 404 instead of 400

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListUserPoolClients

## Description
This bundle has 10 causes; all 10 must be closed before submitting.

AWS: for app-client, group, resource-server and identity-provider operations, cognito-idp answers a missing-resource refusal with HTTP status 400 and ResourceNotFoundException in the body's `__type`, plus the `x-amzn-errortype` response header holding the error shape name followed by a colon (`ResourceNotFoundException:`). Smithy: ResourceNotFoundException @httpError(404) — the recorded service does NOT apply the trait; recorded behavior only: all of these refusals are status 400. Message bodies matched on every step here, so only the status and the header diverge.

Emulator: the emulator honors @httpError(404), so every one of these refusals is served as HTTP 404, and no response carries an `x-amzn-errortype` header.

Why: a 404 from an AWS JSON RPC endpoint is conventionally 'no such operation/route', so SDK error classifiers, API gateways and reverse proxies in front of the emulator treat these as transport/routing failures rather than as a modeled ResourceNotFoundException. Provisioning code that creates a group, resource server, app client or identity provider in a pool that does not exist therefore never sees the modeled error it branches on, and retry/alarm logic misfires on what AWS reports as an ordinary 400 client error.

Fix: the cognito-idp awsJson1_1 error serializer / error-to-status mapping in the service crate (shared by all of these handlers): pin client errors to 400 rather than reading @httpError off the shape, and emit `x-amzn-errortype: <ErrorShapeName>:` there too.

Related open issues that do NOT cover these steps: 20261029T184956Z-cognito-idp-modeled-httperror-codes-are-served-where.md and 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md own different replay keys; these ten clusters are unowned.

Cause 1 of 10: ListUserPoolClients against a user-pool id that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-app-client-error-paths, step task#4.
Cause 2 of 10: AddUserPoolClientSecret for a pool/client that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-client-secret-error-paths, step task#3 (4 recorded steps in this cluster).
Cause 3 of 10: CreateGroup in a user pool that does not exist answers 404; AWS answers 400. Fixture cognito-idp-create-group-in-nonexistent-pool-fails, step task#1.
Cause 4 of 10: CreateResourceServer in a user pool that does not exist answers 404; AWS answers 400. Fixture cognito-idp-create-resource-server-op-contract, step task#4.
Cause 5 of 10: GetGroup for a group name that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-group-lifecycle, step task#9 (3 recorded steps).
Cause 6 of 10: ListUsersInGroup for a group that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-invalid-inputs-and-missing-resources, step task#8 (2 recorded steps).
Cause 7 of 10: ListResourceServers against a pool that does not exist answers 404; AWS answers 400. Fixture cognito-idp-list-resource-servers-scopes-and-notfound-error, step task#4.
Cause 8 of 10: DescribeResourceServer for an identifier that does not exist answers 404; AWS answers 400. Fixture cognito-idp-resource-server-and-oauth-scope-enforcement, step task#9 (2 recorded steps).
Cause 9 of 10: CreateUserPoolClient in a pool that does not exist answers 404; AWS answers 400. Fixture cognito-idp-user-pool-client-lifecycle, step task#3.
Cause 10 of 10: DescribeIdentityProvider for a provider name that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-identity-provider-management, step task#9.

## Evidence
Expected (recorded AWS) for all ten clusters: HTTP 400 with `__type` naming the error class and `x-amzn-errortype: ResourceNotFoundException:` (some steps assert the header shape only).
Actual (emulator): `status: expected 400, got 404` on every cluster, plus `$headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:', got None)`.
Smithy: ResourceNotFoundException @httpError(404), errorKind client — the recorded service serves 400 regardless.
Fixed vs variable: HTTP 400 and the error class name are the requirement; the pool id, client id, group name, resource-server identifier and provider name embedded in the message come from the request and are not part of it.

## Steps to Reproduce
1. Replay cognito-idp-app-client-error-paths step task#4 (ListUserPoolClients with a user-pool id that does not exist): emulator 404, AWS 400 ResourceNotFoundException.
2. Replay cognito-idp-create-group-in-nonexistent-pool-fails step task#1 (CreateGroup in a missing pool): emulator 404, AWS 400.
3. Inspect headers on both responses: `x-amzn-errortype` absent; AWS sends `ResourceNotFoundException:`.
4. Repeat for each fixture#step listed under the causes.
