Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.create-user-pool-client-conflicting-oauth-flows-rejected cognito-idp.invalid-oauth-flow-error cognito-idp.create-user-pool-client-invalid-scope-rejected cognito-idp.op-contract-createUserPoolClient cognito-idp.op-contract-update-user-pool-client cognito-idp.update-user-pool-client-reflects-changes cognito-idp.update-user-pool-replica-invalid-region-rejected cognito-idp.e2e-user-pool-replica-propagates-user-changes cognito-idp.feature-unavailable-in-tier-pool-and-client cognito-idp.log-delivery-configuration-lifecycle cognito-idp.user-import-job-start-without-csv-rejected cognito-idp.limit-exceeded-resource-server
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.create-user-pool-client-conflicting-oauth-flows-rejected|_client_error#7|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: cognito-idp.invalid-oauth-flow-error|_error#8|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.create-user-pool-client-invalid-scope-rejected|_error#14|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.op-contract-createUserPoolClient|_client_error#25|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cognito-idp.op-contract-update-user-pool-client|_error#20|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.update-user-pool-client-reflects-changes|run#12|AssertionError|assert desc.get("ExplicitAuthFlows", []) == [], (
Issue-Key: cognito-idp.update-user-pool-replica-invalid-region-rejected|_error#6|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cognito-idp.e2e-user-pool-replica-propagates-user-changes|run#6|AssertionError|assert negative == ("FeatureUnavailableInTierException", "The following feature is not available for the LITE pricing tier configured: Multi-Region Replication"
Issue-Key: cognito-idp.feature-unavailable-in-tier-pool-and-client|_error#2|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cognito-identity-provider|SetLogDeliveryConfiguration|InvalidParameterException
Issue-Key: cognito-idp.user-import-job-start-without-csv-rejected|_error_result#10|AssertionError|raise AssertionError(f"expected StartUserImportJob to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: service-quotas|GetAWSDefaultServiceQuota|NoSuchResourceException
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp: app-client and pool configuration validation gaps — invalid OAuth flow combinations, undefined scopes, merge-instead-of-replace updates, invalid replica region, tier gating, log config (8 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPoolClient

## Description
Eight root causes where the emulator accepts an app-client/pool configuration AWS refuses, or refuses one AWS accepts. These are net-new; 20261024T114943Z-cognito-idp-user-pool-and-app-client-configuration.md is open on other replay keys.

Cause 1 of 8 — conflicting AllowedOAuthFlows are accepted.
AWS: Smithy CreateUserPoolClient/UpdateUserPoolClient list InvalidOAuthFlowException (httpError 400). 'client_credentials' may not be combined with 'implicit' or with 'code'; AWS refuses the client creation.
Emulator: the client is created and returns AllowedOAuthFlows ['client_credentials','implicit'] / ['client_credentials','code'].
Why: a configuration AWS cannot hold exists in the emulator, so the downstream grant behavior is untestable.
Fix: AllowedOAuthFlows combination validation in the Create/UpdateUserPoolClient handler.
Traces: cognito-idp.create-user-pool-client-conflicting-oauth-flows-rejected, cognito-idp.invalid-oauth-flow-error.

Cause 2 of 8 — scopes that no resource server declares are accepted.
AWS: ScopeDoesNotExistException (httpError 400, modeled on both CreateUserPoolClient and UpdateUserPoolClient) when AllowedOAuthScopes names a scope that no resource server in the pool defines.
Emulator: the client is created/updated successfully (recorded AllowedOAuthScopes ['sim2real-behav-cip-res-.../nonexistent.scope'], ['sim2real-no-such-rs-.../no-such-scope']).
Fix: resolve AllowedOAuthScopes against the pool's resource servers in both handlers.
Traces: cognito-idp.create-user-pool-client-invalid-scope-rejected, cognito-idp.op-contract-createUserPoolClient, cognito-idp.op-contract-update-user-pool-client.

Cause 3 of 8 — UpdateUserPoolClient merges instead of replacing.
AWS: UpdateUserPoolClient replaces the whole writable attribute set; an update that names only RefreshTokenValidity clears ExplicitAuthFlows (and the other omitted writable members).
Emulator: the omitted members are retained (ExplicitAuthFlows still ['ALLOW_REFRESH_TOKEN_AUTH','ALLOW_USER_SRP_AUTH','ALLOW_CUSTOM_AUTH']).
Why: callers that rely on PUT semantics silently keep settings they intended to drop — including auth flows.
Fix: UpdateUserPoolClient must write the full writable set from the request.
Traces: cognito-idp.update-user-pool-client-reflects-changes.

Cause 4 of 8 — an invalid replica region is accepted.
AWS: a region name that does not exist is refused with InvalidParameterException.
Emulator: returns a UserPoolReplica with RegionName 'not-a-real-region', Status 'CREATING' and an ARN in that region.
Fix: region validation in the user-pool replica handler.
Traces: cognito-idp.update-user-pool-replica-invalid-region-rejected.

Cause 5 of 8 — pricing-tier gating.
AWS: FeatureUnavailableInTierException (httpError 403, modeled on SetUserPoolMfaConfig/UpdateUserPoolClient and others). Multi-Region replication on a LITE-tier pool is refused with AWS's own message, and a LITE-tier pool/client must be refused the tier-gated feature at creation time.
Emulator: the message differs ('The following feature is not available for the LITE pricing tier configured: Multi-Region user pool replication'), and in the second case the call SUCCEEDED and returned a full UserPool.
Fix: the tier gate and its message.
Traces: cognito-idp.e2e-user-pool-replica-propagates-user-changes, cognito-idp.feature-unavailable-in-tier-pool-and-client.

Cause 6 of 8 — SetLogDeliveryConfiguration refuses the userNotification event source.
AWS: userNotification is a valid EventSourceName and accepts the configured log level.
Emulator: InvalidParameterException 'Request validation Failed. Following event sources in request have an unsupported log level: [userNotification].'
Why: a valid logging configuration cannot be applied at all.
Fix: the SetLogDeliveryConfiguration event-source/log-level validation table.
Traces: cognito-idp.log-delivery-configuration-lifecycle.

Cause 7 of 8 — StartUserImportJob runs without an uploaded CSV.
AWS: starting an import job whose pre-signed URL was never populated is refused.
Emulator: the job is started and returns Status 'Pending'.
Fix: StartUserImportJob precondition on the uploaded CSV.
Traces: cognito-idp.user-import-job-start-without-csv-rejected.

Cause 8 of 8 — the cognito-idp service quotas are not registered.
AWS: GetAWSDefaultServiceQuota(ServiceCode='cognito-idp', QuotaCode='L-7CDAF993') returns the resource-server quota, which is how a client discovers the limit before provoking LimitExceededException.
Emulator: NoSuchResourceException "no quota with ServiceCode 'cognito-idp' and QuotaCode 'L-7CDAF993' exists" (raised from the service-quotas surface this cognito-idp check depends on).
Fix: register cognito-idp's default service quotas.
Traces: cognito-idp.limit-exceeded-resource-server.

## Evidence
Verbatim from the replay:
- 'real AWS rejects this call, but it SUCCEEDED and returned {... AllowedOAuthFlows: [client_credentials, implicit] ...}' and '... [client_credentials, code] ...'
- 'real AWS rejects this scope, but the call SUCCEEDED and returned {... AllowedOAuthScopes: [sim2real-behav-cip-res-a1e73d7b2d1e/nonexistent.scope] ...}'
- 'expected update_user_pool_client to be rejected with ScopeDoesNotExistException, but it SUCCEEDED'
- 'UpdateUserPoolClient replaces the whole writable attribute set, not just the named fields: an update naming only RefreshTokenValidity must clear ExplicitAuthFlows, got [ALLOW_REFRESH_TOKEN_AUTH, ALLOW_USER_SRP_AUTH, ALLOW_CUSTOM_AUTH]'
- 'expected this call to be rejected, but it SUCCEEDED and returned {UserPoolReplica: {RegionName: not-a-real-region, Status: CREATING, ...}}'
- 'expected FeatureUnavailableInTierException for a LITE-tier pool, got (FeatureUnavailableInTierException, "The following feature is not available for the LITE pricing tier configured: Multi-Region user pool replication")'
- uncaught InvalidParameterException on SetLogDeliveryConfiguration: 'Request validation Failed. Following event sources in request have an unsupported log level: [userNotification].'
- 'expected StartUserImportJob to be rejected, but it SUCCEEDED and returned {... Status: Pending ...}'
- uncaught NoSuchResourceException from GetAWSDefaultServiceQuota for ServiceCode 'cognito-idp'

## Steps to Reproduce
1. CreateUserPoolClient(AllowedOAuthFlowsUserPoolClient=true, AllowedOAuthFlows=['client_credentials','implicit']) -> expect InvalidOAuthFlowException.
2. CreateUserPoolClient(AllowedOAuthScopes=['no-such-rs/no-such-scope']) -> expect ScopeDoesNotExistException; same via UpdateUserPoolClient.
3. CreateUserPoolClient with ExplicitAuthFlows set, then UpdateUserPoolClient naming only RefreshTokenValidity -> ExplicitAuthFlows must be cleared.
4. CreateUserPoolReplica/UpdateUserPool with RegionName='not-a-real-region' -> expect InvalidParameterException.
5. SetLogDeliveryConfiguration with EventSource='userNotification' -> expect success.
6. CreateUserImportJob then StartUserImportJob without uploading the CSV -> expect a refusal.
7. GetAWSDefaultServiceQuota(ServiceCode='cognito-idp', QuotaCode='L-7CDAF993') -> expect the quota.
