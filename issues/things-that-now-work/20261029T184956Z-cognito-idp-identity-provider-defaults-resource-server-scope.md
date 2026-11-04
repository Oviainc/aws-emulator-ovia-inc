Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-identity-provider-management
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-identity-provider-management cognito-idp-get-identity-provider-by-identifier-resolves-matching-provider cognito-idp-resource-server-and-oauth-scope-enforcement cognito-idp-start-import-job-no-csv-precondition-error
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|CreateIdentityProvider|2xx->2xx|$.IdentityProvider.AttributeMapping.username
Issue-Key: cognito-idp|CreateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.attributes_url
Issue-Key: cognito-idp|CreateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.attributes_url_add_attributes
Issue-Key: cognito-idp|CreateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.authorize_url
Issue-Key: cognito-idp|CreateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.token_request_method
Issue-Key: cognito-idp|CreateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.token_url
Issue-Key: cognito-idp|DescribeIdentityProvider|2xx->2xx|$.IdentityProvider.AttributeMapping.username
Issue-Key: cognito-idp|DescribeIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.attributes_url
Issue-Key: cognito-idp|DescribeIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.attributes_url_add_attributes
Issue-Key: cognito-idp|DescribeIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.authorize_url
Issue-Key: cognito-idp|DescribeIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.token_request_method
Issue-Key: cognito-idp|DescribeIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.token_url
Issue-Key: cognito-idp|UpdateIdentityProvider|2xx->2xx|$.IdentityProvider.AttributeMapping.username
Issue-Key: cognito-idp|UpdateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.attributes_url
Issue-Key: cognito-idp|UpdateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.attributes_url_add_attributes
Issue-Key: cognito-idp|UpdateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.authorize_url
Issue-Key: cognito-idp|UpdateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.token_request_method
Issue-Key: cognito-idp|UpdateIdentityProvider|2xx->2xx|$.IdentityProvider.ProviderDetails.token_url
Issue-Key: cognito-idp|GetIdentityProviderByIdentifier|2xx->2xx|$.IdentityProvider.AttributeMapping.username
Issue-Key: cognito-idp|GetIdentityProviderByIdentifier|2xx->2xx|$.IdentityProvider.ProviderDetails.attributes_url
Issue-Key: cognito-idp|GetIdentityProviderByIdentifier|2xx->2xx|$.IdentityProvider.ProviderDetails.attributes_url_add_attributes
Issue-Key: cognito-idp|GetIdentityProviderByIdentifier|2xx->2xx|$.IdentityProvider.ProviderDetails.authorize_url
Issue-Key: cognito-idp|GetIdentityProviderByIdentifier|2xx->2xx|$.IdentityProvider.ProviderDetails.token_request_method
Issue-Key: cognito-idp|GetIdentityProviderByIdentifier|2xx->2xx|$.IdentityProvider.ProviderDetails.token_url
Issue-Key: cognito-idp|UpdateResourceServer|2xx->2xx|$.ResourceServer.Scopes[*].ScopeDescription
Issue-Key: cognito-idp|UpdateResourceServer|2xx->2xx|$.ResourceServer.Scopes[*].ScopeName
Issue-Key: cognito-idp|ListResourceServers|2xx->2xx|$.ResourceServers[Name="probe-api"].Scopes[*].ScopeDescription
Issue-Key: cognito-idp|ListResourceServers|2xx->2xx|$.ResourceServers[Name="probe-api"].Scopes[*].ScopeName
Issue-Key: cognito-idp|CreateUserImportJob|2xx->2xx|$.UserImportJob.JobId
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: identity-provider defaults, resource-server scope order and import-job ids diverge

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIdentityProvider

## Description
Eight recorded causes in which the emulator stores only what the request carried and so omits provider defaults the service fills in, loses request order in resource-server scopes, and mints a user-import job id of the wrong shape.

Cause 1 of 8: CreateIdentityProvider for a well-known social provider returns only the ProviderDetails the request supplied. AWS fills in the provider's endpoint defaults — for a Facebook provider: `authorize_url`, `token_url`, `attributes_url`, `attributes_url_add_attributes` and `token_request_method` — and defaults `AttributeMapping.username` to the provider's subject claim. Emulator returns None for all six. Why: a client that reads the provider back to build an authorize redirect has no URL to redirect to, and federated sign-in cannot be configured from the API. Fix: the identity-provider state must seed the per-ProviderType default ProviderDetails and AttributeMapping at create time. Fixture cognito-idp-identity-provider-management, step task#2.
Cause 2 of 8: DescribeIdentityProvider has the same omission. Fixture cognito-idp-identity-provider-management, step task#4.
Cause 3 of 8: UpdateIdentityProvider has the same omission (an update must not drop the defaults either). Fixture cognito-idp-identity-provider-management, step task#5.
Cause 4 of 8: GetIdentityProviderByIdentifier has the same omission. Fixture cognito-idp-identity-provider-management, step task#7.
Cause 5 of 8: GetIdentityProviderByIdentifier on an OIDC-style provider omits `AttributeMapping.username` (AWS defaults it to `sub`) and `ProviderDetails.attributes_url_add_attributes` (AWS defaults it to `false`). Fixture cognito-idp-get-identity-provider-by-identifier-resolves-matching-provider, step task#4 (2 recorded steps).
Cause 6 of 8: UpdateResourceServer returns Scopes in a different order than the request. AWS preserves the request order. Why: clients diff the scope list to detect drift and build consent screens from its order. Fix: store ResourceServerScopeListType as an ordered sequence. Fixture cognito-idp-resource-server-and-oauth-scope-enforcement, step task#4.
Cause 7 of 8: ListResourceServers returns the same reordered Scopes. Fixture cognito-idp-resource-server-and-oauth-scope-enforcement, step task#5.
Cause 8 of 8: CreateUserImportJob mints a JobId of the wrong shape — a UUID appended to the `import-` prefix. Smithy: UserImportJobIdType @pattern `^import-[0-9a-zA-Z-]+$` @length 1..55; the recorded service's ids are `import-` followed by a fixed-width alphanumeric token, and the corpus's negative controls use that width. Why: clients that persist or validate the job id against the service's own width/charset reject the emulator's. Fix: the user-import-job id generator. Fixture cognito-idp-start-import-job-no-csv-precondition-error, step task#3.

## Evidence
CreateIdentityProvider (cognito-idp-identity-provider-management#task#2, 200 both sides):
  $.IdentityProvider.AttributeMapping.username: expected 'id', got None
  $.IdentityProvider.ProviderDetails.authorize_url / token_url / attributes_url / attributes_url_add_attributes / token_request_method: all expected the provider's default endpoint values, got None
GetIdentityProviderByIdentifier (cognito-idp-get-identity-provider-by-identifier-resolves-matching-provider#task#4): AttributeMapping.username expected 'sub' got None; ProviderDetails.attributes_url_add_attributes expected 'false' got None.
UpdateResourceServer (cognito-idp-resource-server-and-oauth-scope-enforcement#task#4): Scopes[1].ScopeName expected 'admin' got 'write'; Scopes[2].ScopeName expected 'write' got 'admin' (descriptions swapped with them).
CreateUserImportJob (cognito-idp-start-import-job-no-csv-precondition-error#task#3): $.UserImportJob.JobId shape mismatch — got 'import-<uuid-with-dashes>'.

## Steps to Reproduce
1. Replay cognito-idp-identity-provider-management steps task#2, task#4, task#5, task#7 and inspect ProviderDetails/AttributeMapping.
2. Replay cognito-idp-get-identity-provider-by-identifier-resolves-matching-provider step task#4.
3. Replay cognito-idp-resource-server-and-oauth-scope-enforcement steps task#4 and task#5 and compare Scopes order with the request.
4. Replay cognito-idp-start-import-job-no-csv-precondition-error step task#3 and inspect the minted JobId.
