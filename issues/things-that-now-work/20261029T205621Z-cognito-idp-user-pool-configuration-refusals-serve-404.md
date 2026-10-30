Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-user-pool-core-lifecycle
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-user-pool-core-lifecycle cognito-idp-log-delivery-configuration cognito-idp-resource-tagging-and-custom-attributes cognito-idp-risk-configuration-lifecycle cognito-idp-risk-configuration-pool-not-found-error cognito-idp-ui-customization-errors cognito-idp-user-import-job-errors cognito-idp-user-pool-domain-errors cognito-idp-user-pool-replica-errors cognito-idp-user-pool-replica-lifecycle cognito-idp-user-pool-replica-notfound-errors
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|UpdateUserPool|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateUserPool|4xx->4xx|status
Issue-Key: cognito-idp|GetLogDeliveryConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetLogDeliveryConfiguration|4xx->4xx|status
Issue-Key: cognito-idp|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|TagResource|4xx->4xx|status
Issue-Key: cognito-idp|DescribeRiskConfiguration|4xx->4xx|status
Issue-Key: cognito-idp|GetUICustomization|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetUICustomization|4xx->4xx|status
Issue-Key: cognito-idp|CreateUserImportJob|4xx->4xx|status
Issue-Key: cognito-idp|UpdateUserPoolDomain|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateUserPoolDomain|4xx->4xx|status
Issue-Key: cognito-idp|ListUserPoolReplicas|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ListUserPoolReplicas|4xx->4xx|status
Issue-Key: cognito-idp|UpdateUserPoolReplica|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateUserPoolReplica|4xx->4xx|status
Issue-Related: 20261029T184956Z-cognito-idp-validation-and-auth-refusal-message-rendering.md
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# cognito-idp user pool configuration refusals serve 404 instead of 400

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateUserPool

## Description
This bundle has 9 causes; all 9 must be closed before submitting.

AWS: pool-level configuration operations (pool update, tagging, UI customization, log delivery, risk configuration, import jobs, domains, replicas) answer a missing-resource refusal with HTTP status 400, ResourceNotFoundException in the body's `__type`, and the `x-amzn-errortype` response header set to the error shape name followed by a colon. Smithy: ResourceNotFoundException @httpError(404) — the recorded service does NOT apply the trait; recorded behavior only: status is 400. Message bodies matched on all nine steps.

Emulator: the emulator honors @httpError(404) and serves 404 for every one of these refusals, and omits `x-amzn-errortype` entirely.

Why: infrastructure and configuration tooling (CloudFormation-style reconcilers, Terraform-style providers, CLI scripts) distinguishes 'the resource is gone' (a modeled 400 ResourceNotFoundException it can treat as drift) from a 404 it treats as an endpoint/routing failure and aborts or retries on. Teardown and idempotent-delete paths that tolerate a missing pool against AWS therefore fail against the emulator, and the absent error-type header defeats header-based error classification in proxies and logs.

Fix: the cognito-idp awsJson1_1 error serializer / error-to-HTTP-status mapping in the service crate: pin client errors to 400 instead of reading the shape's @httpError, and emit `x-amzn-errortype: <ErrorShapeName>:` from the same place. No per-operation handler change is needed — each handler already selects ResourceNotFoundException.

Related open issues that do NOT cover these steps: 20261029T184956Z-cognito-idp-modeled-httperror-codes-are-served-where.md (ten other operations, including DeleteUserPoolReplica but not these) and 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md.

Cause 1 of 9: UpdateUserPool against a pool that has been deleted answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-user-pool-core-lifecycle, step teardown#9.
Cause 2 of 9: GetLogDeliveryConfiguration for a pool that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-log-delivery-configuration, step task#6.
Cause 3 of 9: TagResource for an ARN whose resource does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-resource-tagging-and-custom-attributes, step task#7.
Cause 4 of 9: DescribeRiskConfiguration for a pool that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-risk-configuration-lifecycle, step task#6 (2 recorded steps in this cluster).
Cause 5 of 9: GetUICustomization for a pool/client that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-ui-customization-errors, step task#3.
Cause 6 of 9: CreateUserImportJob in a pool that does not exist answers 404; AWS answers 400. Fixture cognito-idp-user-import-job-errors, step task#6.
Cause 7 of 9: UpdateUserPoolDomain for a domain that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-user-pool-domain-errors, step task#6.
Cause 8 of 9: ListUserPoolReplicas for a pool that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-user-pool-replica-errors, step task#3.
Cause 9 of 9: UpdateUserPoolReplica for a replica that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-user-pool-replica-errors, step task#5 (3 recorded steps).

## Evidence
Expected (recorded AWS): HTTP 400 with `__type` naming the error class and `x-amzn-errortype: ResourceNotFoundException:` (a few steps assert the header shape only).
Actual (emulator): `status: expected 400, got 404` on all nine clusters, plus `$headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:', got None)`.
Smithy: ResourceNotFoundException errorKind client, @httpError(404) — not honored on the wire by the recorded service.
Fixed vs variable: the 400 status and the error class name are the requirement; the pool id, ARN, domain, replica region and job name inside the message are request/state values.

## Steps to Reproduce
1. Replay cognito-idp-user-pool-core-lifecycle step teardown#9 (UpdateUserPool on a deleted pool): emulator 404, AWS 400 ResourceNotFoundException.
2. Replay cognito-idp-user-pool-replica-errors step task#3 (ListUserPoolReplicas for a missing pool): emulator 404, AWS 400.
3. Inspect the response headers: no `x-amzn-errortype`; AWS sends `ResourceNotFoundException:`.
4. Repeat for each fixture#step listed under the causes.
