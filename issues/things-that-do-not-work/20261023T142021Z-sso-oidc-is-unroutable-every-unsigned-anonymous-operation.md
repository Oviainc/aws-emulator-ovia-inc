Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-oidc.create-token-expired-after-device-code-ttl sso-oidc.create-token-invalid-grant-unknown-device-code-rejected sso-oidc.create-token-missing-device-code-invalid-request-rejected sso-oidc.create-token-refresh-token-invalid-grant-rejected sso-oidc.create-token-refresh-token-lookup-precedes-client-existence sso-oidc.create-token-unauthorized-client-wrong-grant-type-rejected sso-oidc.create-token-unsupported-grant-type-rejected sso-oidc.register-client-accepts-malformed-redirect-uri sso-oidc.register-client-accepts-unrecognized-scope sso-oidc.register-client-confidential-type-rejected sso-oidc.register-client-default-grant-types-allow-device-flow sso-oidc.register-client-distinct-credentials-per-call sso-oidc.register-client-returns-valid-credential-pair sso-oidc.start-device-authorization-starturl-precondition-rejects-any-starturl
Issue-Shape-Id: com.amazonaws.ssooidc#AWSSSOOIDCService
Issue-Key: sso-oidc|RegisterClient|UnknownService
Issue-Key: sso-oidc|CreateToken|UnknownService
Issue-Key: sso-oidc|StartDeviceAuthorization|UnknownService
Issue-Commit: 17c220c4f82dc0c00315f2111baf55aa4ec61156

# sso-oidc is unroutable: every unsigned (anonymous) operation answers UnknownService 'could not resolve a target service from the request', so the whole device-authorization flow is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `17c220c4f82dc0c00315f2111baf55aa4ec61156`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterClient

## Description
One root cause, grounded in 14 failing behavioral checks that the replay report marked neither ALREADY FILED nor OWNED. Every one of them dies on its FIRST sso-oidc call, before any sso-oidc business logic is reached, so this single defect is the only observable behavior the service currently has.

Cause 1 of 1 - requests to sso-oidc that carry no SigV4 credential scope are not routed to the emulated service; the router answers UnknownService.

AWS: the Smithy model for AWSSSOOIDCService (sdkId 'SSO OIDC', protocol restJson1) marks RegisterClient, StartDeviceAuthorization and CreateToken with `@optionalAuth` and an empty auth list - they are sent ANONYMOUSLY (no Authorization/SigV4 header) by every SDK. Being restJson1 they also carry NO X-Amz-Target header; they are identified purely by the endpoint (oidc.<region>.amazonaws.com) and the modeled HTTP bindings POST /client/register, POST /device_authorization, POST /token (CreateTokenWithIAM: POST /token?aws_iam=t). Real AWS serves these unsigned requests and returns either the modeled success shape (RegisterClient -> clientId/clientSecret; StartDeviceAuthorization -> deviceCode/userCode/interval/expiresIn) or a modeled error from the operation's error list (InvalidClientMetadataException, InvalidRedirectUriException, InvalidScopeException, UnsupportedGrantTypeException, InvalidClientException, InvalidGrantException, AuthorizationPendingException, ExpiredTokenException, UnauthorizedClientException, or the service-common InvalidRequestException / SlowDownException). Cited: smithy_query digest of services/service-com-amazonaws-ssooidc-awsssooidcservice/smithy-model/com-amazonaws-ssooidc-awsssooidcservice.json.

Emulator: every such call is refused by the request router before dispatch with HTTP error code `UnknownService` and the message 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)' (message quoted as an example; the diverging requirement is the status/error code - AWS never answers UnknownService for these operations). This happens for RegisterClient, StartDeviceAuthorization and CreateToken alike. Contrast: sso-admin ListInstances/CreateInstance, which the same checks call first and which ARE SigV4-signed, route and answer normally - so the failure is specific to the unsigned/credential-scope-less resolution path, not to sso-oidc's handlers (the ssooidc crate is bootstrapped and its CreateTokenWithIAM stub does answer NotImplemented, proving the service is registered).

Why: no client can complete, or even begin, the IAM Identity Center device-authorization flow. A client expecting a clientId/clientSecret pair gets a hard routing error; every negative case gets the wrong error class entirely (expected InvalidGrantException / InvalidClientMetadataException / InvalidRequestException / InvalidClientException, got UnknownService), which botocore surfaces as an unmodeled error the SDK's retry and error-handling logic cannot interpret. Any tool that authenticates via `aws sso login` is unusable against the emulator.

Fix: the request router's service-resolution step - an anonymous restJson1 request must be attributable without a credential scope or X-Amz-Target, by the endpoint/Host prefix (oidc) together with the modeled HTTP method+URI bindings, resolving to the same registered service that a SigV4-signed sso-oidc request would resolve to. Note: open issue 20261023T125454Z-cognito-idp-cognito-every-unsigned-anonymous-operation-answers-unknownservice-admininitiateauth.md records the analogous unsigned-routing defect for cognito-idp/cognito-identity (json1.1, resolved by X-Amz-Target prefix); it does not carry any sso-oidc replay key and its fix path is the target-prefix lookup, so it does not cover this.

Traces: the 14 failing check ids listed under trace_ids; 11 surfaced as [error] (the ClientError propagated out of the first register_client/start_device_authorization call) and 3 as [mismatch] (the check caught the ClientError and asserted on its code: 'expected InvalidGrantException, got UnknownService', 'expected InvalidClientMetadataException, got UnknownService', 'expected InvalidRequestException or InvalidClientException, got UnknownService').

## Evidence
Expected (AWS): RegisterClient(clientName, clientType='public', grantTypes=['urn:ietf:params:oauth:grant-type:device_code']) sent unsigned -> HTTP 200 with clientId/clientSecret; StartDeviceAuthorization -> 200 with deviceCode/userCode/interval/expiresIn; CreateToken with a fabricated deviceCode -> HTTP 400 InvalidGrantException; RegisterClient with clientType='confidential' -> HTTP 400 InvalidClientMetadataException; StartDeviceAuthorization with an unusable startUrl -> HTTP 400 InvalidRequestException or InvalidClientException.

Actual (emulator):
- botocore.exceptions.ClientError: An error occurred (UnknownService) when calling the RegisterClient operation: could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header) - 11 checks error out here on the first sso-oidc call.
- AssertionError: expected InvalidGrantException, got 'UnknownService' (sso-oidc.create-token-refresh-token-lookup-precedes-client-existence).
- AssertionError: expected InvalidClientMetadataException, got 'UnknownService' (sso-oidc.register-client-confidential-type-rejected).
- AssertionError: expected InvalidRequestException or InvalidClientException, got 'UnknownService' (sso-oidc.start-device-authorization-starturl-precondition-rejects-any-starturl).

Suite totals: sso-oidc 0/16 (0%), 16 failed (5 mismatch, 11 error). The preceding SigV4-signed sso-admin ListInstances/CreateInstance calls in the same checks succeed, isolating the failure to unsigned-request service resolution.

## Steps to Reproduce
1. Boot the emulator and point a botocore sso-oidc client at it (the sso-oidc client sends RegisterClient/StartDeviceAuthorization/CreateToken UNSIGNED, per @optionalAuth in the model).
2. Call RegisterClient(clientName='x', clientType='public', grantTypes=['urn:ietf:params:oauth:grant-type:device_code']). Expect HTTP 200 with clientId/clientSecret; observe HTTP error UnknownService 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'.
3. Call StartDeviceAuthorization(clientId, clientSecret, startUrl=...) and CreateToken(clientId, clientSecret, grantType='urn:ietf:params:oauth:grant-type:device_code', deviceCode='fabricated'). Expect 200 and HTTP 400 InvalidGrantException respectively; observe UnknownService for both.
4. For contrast, call sso-admin ListInstances with the same credentials (SigV4-signed): it routes and answers normally.
