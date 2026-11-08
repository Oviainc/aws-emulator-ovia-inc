Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-api-key-credential-provider-lifecycle
Issue-Step: setup#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-api-key-credential-provider-lifecycle bedrock-agentcore-control-apikey-credential-provider-errors bedrock-agentcore-control-apikey-credential-provider-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateApiKeyCredentialProvider|2xx->2xx|$.apiKeySecretJsonKey
Issue-Key: bedrock-agentcore-control|CreateApiKeyCredentialProvider|2xx->2xx|$.credentialProviderArn
Issue-Key: bedrock-agentcore-control|GetApiKeyCredentialProvider|2xx->2xx|$.apiKeySecretJsonKey
Issue-Key: bedrock-agentcore-control|GetApiKeyCredentialProvider|2xx->2xx|$.credentialProviderArn
Issue-Commit: f392f82803b1854878505550560cb55ee4e9c61d

# API key credential provider representation diverges: acps ARN namespace and an invented apiKeySecretJsonKey

> _Note: this issue was identified against the code base at commit `f392f82803b1854878505550560cb55ee4e9c61d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiKeyCredentialProvider

## Description
This bundle has 2 causes: the same API-key credential provider representation is wrong on both the create and the read path - the credentialProviderArn uses the wrong ARN service namespace, and an apiKeySecretJsonKey member AWS does not return is invented.

Cause 1 of 2 - CreateApiKeyCredentialProvider (fixture bedrock-agentcore-control-api-key-credential-provider-lifecycle, step setup#2).
AWS: POST /identities/CreateApiKeyCredentialProvider -> 201. Recorded behavior: credentialProviderArn is 'arn:aws:bedrock-agentcore:<region>:<account>:token-vault/default/apikeycredentialprovider/<name>' - the bedrock-agentcore ARN service namespace - and apiKeySecretJsonKey is ABSENT when the request does not supply a secret-reference json key (Smithy: CreateApiKeyCredentialProviderResponse.apiKeySecretJsonKey -> SecretJsonKeyType, optional, length 1..128; note the model's ApiKeyCredentialProviderArnType @pattern still says the legacy 'acps' namespace, so for the ARN namespace the recording, not the pattern, is the requirement).
Emulator: returns 'arn:aws:acps:<region>:<account>:token-vault/default/apikeycredentialprovider/<name>' and always sets apiKeySecretJsonKey to 'apiKey'.
Why: the ARN is the provider's identity - a client that stores it, passes it to an agent runtime's credential configuration, or builds IAM/resource policies from it gets an ARN whose service namespace does not match anything AWS issues, and the extra apiKeySecretJsonKey makes a provider created without a json key look like one that has one.
Fix: CreateApiKeyCredentialProvider's response construction - build the ARN in the bedrock-agentcore namespace and only populate apiKeySecretJsonKey when the request/stored secret reference actually carries one.

Cause 2 of 2 - GetApiKeyCredentialProvider (fixture bedrock-agentcore-control-api-key-credential-provider-lifecycle, step task#3).
AWS: POST /identities/GetApiKeyCredentialProvider -> 200 returning the same representation: bedrock-agentcore-namespaced credentialProviderArn and no apiKeySecretJsonKey for a provider created without one.
Emulator: the read echoes the stored record with the acps ARN and apiKeySecretJsonKey 'apiKey'.
Why: the divergence is persistent, not just a create-time artifact - re-reading the provider never yields the AWS representation.
Fix: the shared API-key credential provider state/view used by Get (and the ARN builder it shares with Create).

Related open issue (different cause, not a duplicate): 20261108T132909Z-bedrock-agentcore-control-three-net-new-divergences-createapikeycredentialprovider.md covers CreateApiKeyCredentialProvider accepting an undecryptable EXTERNAL secret, not the ARN namespace or apiKeySecretJsonKey.

Traces: bedrock-agentcore-control-api-key-credential-provider-lifecycle#setup#2, bedrock-agentcore-control-api-key-credential-provider-lifecycle#task#3. The same two clusters also recur on bedrock-agentcore-control-apikey-credential-provider-errors#setup#1, bedrock-agentcore-control-apikey-credential-provider-lifecycle#task#1 and #task#2, and on api-key-credential-provider-lifecycle#task#6.

## Evidence
Cause 1 (api-key-credential-provider-lifecycle#setup#2) CreateApiKeyCredentialProvider 201 - '$.credentialProviderArn: value_mismatch (expected arn:aws:bedrock-agentcore:us-west-2:<account>:token-vault/default/apikeycredentialprovider/<name>, got arn:aws:acps:us-west-2:<account>:token-vault/default/apikeycredentialprovider/<name>)' and '$.apiKeySecretJsonKey: type_mismatch (expected None, got apiKey)'.
Cause 2 (api-key-credential-provider-lifecycle#task#3) GetApiKeyCredentialProvider 200 - identical two divergences on the read path.
Only the ARN's service-namespace segment and the presence of apiKeySecretJsonKey are the requirement here; the region, account and generated provider name in the recorded value are per-environment values.

## Steps to Reproduce
1. Replay bedrock-agentcore-control-api-key-credential-provider-lifecycle step setup#2: CreateApiKeyCredentialProvider with a plain apiKey and no secret json key - the 201 body carries an 'arn:aws:acps:...' credentialProviderArn and apiKeySecretJsonKey 'apiKey'.
2. Replay step task#3 of the same fixture: GetApiKeyCredentialProvider returns the same two wrong fields.
