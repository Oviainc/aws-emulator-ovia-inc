Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.update-identity-provider-concurrent-modification-conflict
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.update-identity-provider-concurrent-modification-conflict|run#18|AssertionError|assert (update_code, update_message) == ("ResourceNotFoundException", expected_update), (
Issue-Commit: 4cb124a92bc472448f3b420460157b630c09e49d

# UpdateIdentityProvider's ResourceNotFoundException uses the DescribeIdentityProvider "for tenantId" wording instead of "in User Pool"

> _Note: this issue was identified against the code base at commit `4cb124a92bc472448f3b420460157b630c09e49d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIdentityProvider

## Description
One net-new root cause in cognito-idp.

Cause 1 of 1: UpdateIdentityProvider refuses a provider name the pool does not hold with the wrong ResourceNotFoundException message text. Failing check id: cognito-idp.update-identity-provider-concurrent-modification-conflict.

AWS: UpdateIdentityProvider models ResourceNotFoundException (HTTP 404, errorKind client) — Smithy: UpdateIdentityProvider errors include ResourceNotFoundException httpError 404; input members UserPoolId (UserPoolIdType, pattern ^[\w-]+_[0-9a-zA-Z]+$) and ProviderName (ProviderNameType, len 1..32) are both required. The model does not carry message text, so this is recorded behavior: when the named IdP does not exist in the pool, UpdateIdentityProvider's refusal message is the fixed text "Identity provider " + <ProviderName> + " in User Pool " + <UserPoolId> + " does not exist." — i.e. fixed parts "Identity provider ", " in User Pool ", " does not exist.", with the ProviderName and UserPoolId taken verbatim from the request. This wording is specific to UpdateIdentityProvider: the read path (DescribeIdentityProvider) words the same condition differently, as "Identity provider <ProviderName> for tenantId <UserPoolId> does not exist.", and the same check asserts and passes that Describe form, so the two operations genuinely differ in AWS.

Emulator: the emulator answers UpdateIdentityProvider with ResourceNotFoundException carrying the Describe wording — "Identity provider <ProviderName> for tenantId <UserPoolId> does not exist." In the replay: expected ResourceNotFoundException 'Identity provider s2rIdpebe5434b46bd in User Pool us-west-2_ed62c0e18 does not exist.', got ResourceNotFoundException 'Identity provider s2rIdpebe5434b46bd for tenantId us-west-2_ed62c0e18 does not exist.' (those particular provider and pool ids are per-run values that fill the template, not part of it). The error code and the 404 status are correct; only the message text diverges. The handler builds the message from the shared helper used by every IdP-resolving operation, so Update inherits the read path's wording.

Why: a client that races an UpdateIdentityProvider against a DeleteIdentityProvider (or otherwise updates a provider that was just removed) and branches on, logs, or surfaces the refusal message sees text AWS never produces for this operation. Error-message assertions, user-facing messages and log-scraping rules written against real Cognito do not match, and a client cannot tell the Update-path refusal from the Describe-path refusal because the emulator collapses the two distinct messages into one.

Fix: belongs in UpdateIdentityProvider's own refusal construction — the operation's handler must word its provider-not-found ResourceNotFoundException with the "in User Pool" form rather than delegating to the shared "for tenantId" message helper that the read path uses. Leave the Describe/Delete wording as it is; that form is asserted and passing in the same check.

Traces: cognito-idp.update-identity-provider-concurrent-modification-conflict (the update/delete race branch where the delete lands first and the losing UpdateIdentityProvider is graded on its ResourceNotFoundException message).

## Evidence
Check cognito-idp.update-identity-provider-concurrent-modification-conflict [mismatch]:

  AssertionError: expected the losing UpdateIdentityProvider to get ResourceNotFoundException
  'Identity provider s2rIdpebe5434b46bd in User Pool us-west-2_ed62c0e18 does not exist.',
  got ResourceNotFoundException 'Identity provider s2rIdpebe5434b46bd for tenantId us-west-2_ed62c0e18 does not exist.'

Expected (AWS, UpdateIdentityProvider): HTTP 404, Code=ResourceNotFoundException,
Message="Identity provider <ProviderName> in User Pool <UserPoolId> does not exist."
Actual (emulator): HTTP 404, Code=ResourceNotFoundException,
Message="Identity provider <ProviderName> for tenantId <UserPoolId> does not exist."

The same check asserts, and the emulator passes, the Describe form
"Identity provider <ProviderName> for tenantId <UserPoolId> does not exist." for
DescribeIdentityProvider on the deleted provider — so the emulator serves one message where
AWS serves two.

Source: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/update_identity_provider.rs lines 199-202 (provider lookup -> provider_not_found) and lines 327-335, where provider_not_found delegates to crate::state::identity_provider_not_found_message (the shared "for tenantId" wording).

Smithy: UpdateIdentityProvider -> errors ResourceNotFoundException (httpError 404, client); required input UserPoolId, ProviderName.

## Steps to Reproduce
1. CreateUserPool, then CreateIdentityProvider with ProviderType="OIDC" and an arbitrary ProviderName (OIDC permits a custom name).
2. DeleteIdentityProvider for that pool/provider (or race a DeleteIdentityProvider against an UpdateIdentityProvider so the delete lands first).
3. Call UpdateIdentityProvider(UserPoolId=<pool>, ProviderName=<provider>, AttributeMapping={"email": "email_final"}).
4. Observe the refusal: code is ResourceNotFoundException at HTTP 404, but the message reads "Identity provider <provider> for tenantId <pool> does not exist." AWS returns "Identity provider <provider> in User Pool <pool> does not exist." for this operation.
5. For contrast, call DescribeIdentityProvider with the same arguments: the "for tenantId" wording IS correct there, and the emulator returns the identical string for both.
