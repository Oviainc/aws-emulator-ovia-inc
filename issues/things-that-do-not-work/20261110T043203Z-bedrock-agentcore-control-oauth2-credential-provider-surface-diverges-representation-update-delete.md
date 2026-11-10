Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-oauth2-credential-provider-delete-then-get
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-oauth2-credential-provider-delete-then-get bedrock-agentcore-control-oauth2-credential-provider-salesforce-and-errors bedrock-agentcore-control-oauth2-credential-provider-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetOauth2CredentialProvider|2xx->2xx|$.callbackUrl
Issue-Key: bedrock-agentcore-control|GetOauth2CredentialProvider|2xx->2xx|$.clientSecretJsonKey
Issue-Key: bedrock-agentcore-control|GetOauth2CredentialProvider|2xx->2xx|$.credentialProviderArn
Issue-Key: bedrock-agentcore-control|CreateOauth2CredentialProvider|2xx->2xx|$.callbackUrl
Issue-Key: bedrock-agentcore-control|CreateOauth2CredentialProvider|2xx->2xx|$.clientSecretJsonKey
Issue-Key: bedrock-agentcore-control|CreateOauth2CredentialProvider|2xx->2xx|$.credentialProviderArn
Issue-Key: bedrock-agentcore-control|GetOauth2CredentialProvider|2xx->2xx|$.oauth2ProviderConfigOutput.googleOauth2ProviderConfig.clientId
Issue-Key: bedrock-agentcore-control|CreateOauth2CredentialProvider|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|CreateOauth2CredentialProvider|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|CreateOauth2CredentialProvider|4xx->4xx|status
Issue-Key: bedrock-agentcore-control|GetOauth2CredentialProvider|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetOauth2CredentialProvider|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|GetOauth2CredentialProvider|4xx->2xx|$.oauth2ProviderConfigOutput.googleOauth2ProviderConfig.oauthDiscovery.discoveryUrl
Issue-Key: bedrock-agentcore-control|GetOauth2CredentialProvider|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|GetOauth2CredentialProvider|4xx->2xx|status
Issue-Commit: 97e87117743d236dfa23fd76b444a194861aeef0

# OAuth2 credential provider surface diverges: representation, update, delete and duplicate-name error paths

> _Note: this issue was identified against the code base at commit `97e87117743d236dfa23fd76b444a194861aeef0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOauth2CredentialProvider

## Description
The emulator's Oauth2CredentialProvider resource diverges from AWS in eight independent ways: the shape it serializes, the fact that an update is not applied, the fact that a delete does not remove the provider, and the status/code/message of its duplicate-name and not-found refusals. 8 causes, all on service bedrock-agentcore-control.

Cause 1 of 8: GetOauth2CredentialProvider omits callbackUrl and invents clientSecretJsonKey.
AWS: Smithy GetOauth2CredentialProvider.output declares `callbackUrl` (String) and `clientSecretJsonKey` (SecretJsonKeyType, @length 1..128) as OPTIONAL members. The recording shows AWS populating `callbackUrl` with the provider's OAuth2 redirect URL for every provider it returns, and OMITTING `clientSecretJsonKey` entirely when the create request did not supply a JSON key for the secret (recorded behavior for the omission rule).
Emulator: `callbackUrl` is absent from the response, and `clientSecretJsonKey` is present with the synthesized value `clientSecret` where AWS sends no member at all.
Why: a client that builds the provider's authorization redirect from `callbackUrl` gets a null and cannot complete the 3-legged OAuth flow; a client that round-trips a Get into an Update sends back a `clientSecretJsonKey` the user never configured, changing which JSON key the secret is read from.
Fix: the GetOauth2CredentialProvider output serializer / the credential-provider state record, which must persist and emit the callback URL and must leave clientSecretJsonKey unset unless the request set it.
Traces: bedrock-agentcore-control-oauth2-credential-provider-delete-then-get#task#2, bedrock-agentcore-control-oauth2-credential-provider-salesforce-and-errors#task#2.

Cause 2 of 8: credentialProviderArn is emitted in the `acps` ARN namespace instead of `bedrock-agentcore`.
AWS: the recorded response returns `arn:aws:bedrock-agentcore:<region>:<account>:token-vault/default/oauth2credentialprovider/<name>`. (Smithy CredentialProviderArnType carries the pattern `^arn:(aws|aws-us-gov):acps:...`, i.e. the model's pattern lags the service; the recording is the wire truth and governs.) The variable parts of the ARN are the partition, region, account id and provider name taken from the request and the caller's identity; the fixed text is `token-vault/default/oauth2credentialprovider/`.
Emulator: emits the same ARN with the service segment `acps` — `arn:aws:acps:<region>:<account>:token-vault/default/oauth2credentialprovider/<name>`.
Why: anything that parses the ARN's service segment to route, tag or IAM-scope the provider (policy conditions on `bedrock-agentcore:*` resources, resource-group queries) fails to match a provider the emulator created.
Fix: the ARN construction used by the credential-provider handlers (same defect the already-open issue 20261108T204036Z-bedrock-agentcore-control-api-key-credential-provider-representation-diverges-acps-arn.md reports for the API-key provider; that issue does not cover the OAuth2 provider operations).
Traces: bedrock-agentcore-control-oauth2-credential-provider-delete-then-get#task#2 (Get), bedrock-agentcore-control-oauth2-credential-provider-salesforce-and-errors#task#1 (Create).

Cause 3 of 8: CreateOauth2CredentialProvider's own response carries the same two representation defects.
AWS: Smithy CreateOauth2CredentialProvider returns HTTP 201 with `callbackUrl` populated and `clientSecretJsonKey` omitted when unset, and the bedrock-agentcore ARN namespace.
Emulator: 201 with `callbackUrl` missing, `clientSecretJsonKey: "clientSecret"` added, and the `acps` ARN.
Why: a client that uses the create response directly (rather than re-reading) never sees a callback URL, so provider setup cannot be completed from one call.
Fix: the CreateOauth2CredentialProvider output serializer.
Traces: bedrock-agentcore-control-oauth2-credential-provider-salesforce-and-errors#task#1.

Cause 4 of 8: the same omission on the lifecycle fixture's Get, confirming it is the shared serializer and not one provider vendor.
AWS: as Cause 1 — `callbackUrl` present, `clientSecretJsonKey` absent.
Emulator: `callbackUrl` missing, `clientSecretJsonKey: "clientSecret"`.
Why: as Cause 1, for a Google-vendor provider.
Fix: the GetOauth2CredentialProvider output serializer.
Traces: bedrock-agentcore-control-oauth2-credential-provider-lifecycle#task#3.

Cause 5 of 8: UpdateOauth2CredentialProvider does not apply the new provider configuration.
AWS: Smithy UpdateOauth2CredentialProvider takes a required `oauth2ProviderConfigInput` and returns the updated `oauth2ProviderConfigOutput`; a subsequent Get reflects the new configuration (the recording shows the Google provider's clientId replaced by the value sent in the update).
Emulator: after the update, GetOauth2CredentialProvider still reports the ORIGINAL `oauth2ProviderConfigOutput.googleOauth2ProviderConfig.clientId` — the update's configuration is accepted and discarded.
Why: a client that rotates an OAuth2 client id/secret believes the rotation landed while the emulator keeps authorizing with the old credentials; every later read is stale.
Fix: the UpdateOauth2CredentialProvider handler's write into credential-provider state.
Traces: bedrock-agentcore-control-oauth2-credential-provider-lifecycle#task#6.

Cause 6 of 8: a duplicate provider name is refused with 409 ConflictException instead of 400 ValidationException.
AWS: the recording shows HTTP 400 with `x-amzn-errortype: ValidationException` and message `Credential provider with name: <name> already exists` — fixed text `Credential provider with name: ` and ` already exists`, with `<name>` taken from the request's `name`. Both ValidationException (400) and ConflictException (409) are modeled on this operation, and AWS chooses ValidationException for the duplicate-name case.
Emulator: HTTP 409 with `x-amzn-errortype: ConflictException` and message `an OAuth2 credential provider named <name> already exists in this account` — emulator prose, not AWS's.
Why: SDK retry/error handling branches on the error code: a client catching ValidationException to rename-and-retry sees an unhandled ConflictException, and 409 is classified differently from 400 by generic retry layers.
Fix: the CreateOauth2CredentialProvider handler's duplicate-name validation, which must raise ValidationException (400) with AWS's wording.
Traces: bedrock-agentcore-control-oauth2-credential-provider-lifecycle#task#7, bedrock-agentcore-control-oauth2-credential-provider-salesforce-and-errors#task#3.

Cause 7 of 8: the not-found message for a missing provider is emulator prose that leaks the requested name.
AWS: ResourceNotFoundException (404) with the fixed, parameterless message `CredentialProvider not found` — no variable part at all.
Emulator: 404 with `Oauth2CredentialProvider not found for <name>`.
Why: a client or test that matches AWS's message text does not match, and the emulator's message implies a per-name template AWS does not use.
Fix: the GetOauth2CredentialProvider not-found path's error message.
Traces: bedrock-agentcore-control-oauth2-credential-provider-salesforce-and-errors#task#4.

Cause 8 of 8: DeleteOauth2CredentialProvider does not remove the provider — a later Get answers 200 instead of 404.
AWS: Smithy DeleteOauth2CredentialProvider returns 204 and the provider ceases to exist; GetOauth2CredentialProvider then answers HTTP 404 `ResourceNotFoundException` with message `CredentialProvider not found`.
Emulator: after the delete, GetOauth2CredentialProvider answers HTTP 200 with the full provider body (and, in the delete-then-get fixture, an `oauth2ProviderConfigOutput.googleOauth2ProviderConfig.oauthDiscovery.discoveryUrl` pointing at a host the emulator does not serve). Two separate fixtures show the same survival, one on the task path and one on teardown.
Why: delete is not durable, so a client cannot recreate a provider under the same name, cleanup never converges, and a revoked credential provider remains usable — a correctness and security divergence.
Fix: the DeleteOauth2CredentialProvider handler's removal from credential-provider state (and the Get path's existence check).
Traces: bedrock-agentcore-control-oauth2-credential-provider-delete-then-get#task#4, bedrock-agentcore-control-oauth2-credential-provider-lifecycle#teardown#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (296 steps, 143 passed, score 48.3%).

Cause 1/4 — GetOauth2CredentialProvider, expected 200 / got 200:
  $.callbackUrl: missing (expected the provider's callback URL, got None)
  $.clientSecretJsonKey: type_mismatch (expected None, got 'clientSecret')
Cause 2/3 — Create+Get, expected 200/201 / got 200/201:
  $.credentialProviderArn: expected 'arn:aws:bedrock-agentcore:us-west-2:<acct>:token-vault/default/oauth2credentialprovider/<name>', got 'arn:aws:acps:us-west-2:<acct>:token-vault/default/oauth2credentialprovider/<name>'
Cause 5 — GetOauth2CredentialProvider after UpdateOauth2CredentialProvider, 200/200:
  $.oauth2ProviderConfigOutput.googleOauth2ProviderConfig.clientId: expected the updated client id, got the client id from create
Cause 6 — CreateOauth2CredentialProvider duplicate name: status expected 400, got 409;
  $headers.x-amzn-errortype: expected 'ValidationException', got 'ConflictException';
  $.message: expected 'Credential provider with name: <name> already exists', got 'an OAuth2 credential provider named <name> already exists in this account'
Cause 7 — GetOauth2CredentialProvider missing name, 404/404:
  $.message: expected 'CredentialProvider not found', got 'Oauth2CredentialProvider not found for <name>'
Cause 8 — GetOauth2CredentialProvider after delete: status expected 404, got 200;
  $.message: missing (expected 'CredentialProvider not found', got None);
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException', got None)

Model citations: Smithy GetOauth2CredentialProvider.output {callbackUrl: String, clientSecretJsonKey: SecretJsonKeyType @length 1..128 (optional)}; CreateOauth2CredentialProvider http code 201, errors include ValidationException(400) and ConflictException(409); UpdateOauth2CredentialProvider.input.oauth2ProviderConfigInput @required; DeleteOauth2CredentialProvider http code 204; ResourceNotFoundException httpError 404.

## Steps to Reproduce
1. CreateOauth2CredentialProvider with a Google (or Salesforce) oauth2ProviderConfigInput and no secret JSON key -> response omits callbackUrl, adds clientSecretJsonKey='clientSecret', and the credentialProviderArn uses the acps namespace.
2. GetOauth2CredentialProvider on that name -> same three defects.
3. UpdateOauth2CredentialProvider on that name with a different googleOauth2ProviderConfig.clientId, then GetOauth2CredentialProvider -> the old clientId is still returned.
4. CreateOauth2CredentialProvider again with the SAME name -> 409 ConflictException instead of 400 ValidationException 'Credential provider with name: <name> already exists'.
5. GetOauth2CredentialProvider on a name that was never created -> 404 with 'Oauth2CredentialProvider not found for <name>' instead of 'CredentialProvider not found'.
6. DeleteOauth2CredentialProvider on the name, then GetOauth2CredentialProvider -> 200 with the full provider instead of 404 ResourceNotFoundException.
Replay: fixture bedrock-agentcore-control-oauth2-credential-provider-delete-then-get step task#2, plus the fixture#step bindings listed in trace_ids.
