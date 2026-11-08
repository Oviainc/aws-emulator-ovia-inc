Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-device-management-error-paths
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-device-management-error-paths cognito-idp-admin-device-notfound-errors cognito-idp-admin-managed-device-and-mfa-settings cognito-idp-device-and-pool-diagnostics cognito-idp-device-self-service-invalid-token-errors cognito-idp-self-service-ops-reject-invalid-access-token cognito-idp-user-device-management-errors cognito-idp-device-tracking-lifecycle cognito-idp-self-service-device-remember-and-forget
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminUpdateDeviceStatus|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminUpdateDeviceStatus|4xx->4xx|status
Issue-Key: cognito-idp|AdminForgetDevice|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminForgetDevice|4xx->4xx|status
Issue-Key: cognito-idp|GetDevice|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetDevice|4xx->4xx|status
Issue-Key: cognito-idp|UpdateDeviceStatus|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateDeviceStatus|4xx->4xx|status
Issue-Key: cognito-idp|ForgetDevice|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ForgetDevice|4xx->4xx|status
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# cognito-idp device operations serve @httpError 404/403 instead of HTTP 400

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminUpdateDeviceStatus

## Description
This bundle has 7 causes; all 7 must be closed before submitting.

AWS: cognito-idp speaks awsJson1_1 and answers EVERY client-error class on its device operations with HTTP status 400, carrying the error class in the response body's `__type` and in the `x-amzn-errortype` response header (recorded value is the error shape name followed by a colon, e.g. `ResourceNotFoundException:`). Smithy: ResourceNotFoundException is annotated @httpError(404), UserNotFoundException @httpError(404) and NotAuthorizedException @httpError(403), but the recorded service does NOT apply those traits — recorded behavior wins: every recorded device refusal in this corpus has status 400. The error message text itself matched in all 7 steps, so the message template is not at issue here; only the status line and the error-type header diverge.

Emulator: the emulator reads @httpError off the error shape, so device not-found refusals are served as 404 ResourceNotFoundException and invalid-access-token refusals as 403 NotAuthorizedException. In addition, none of these responses carries an `x-amzn-errortype` header at all.

Why: a client that branches on the HTTP status takes a different path than it does against AWS. 404 reads as 'the endpoint/route does not exist' to proxies, API gateways and SDK retry classifiers, not as 'the device does not exist', so error handling for a missing or forgotten device never runs. A 403 on a data-plane Cognito call is indistinguishable from a SigV4/credential failure, so credential-refresh logic fires on what is really a bad-access-token refusal. The absent `x-amzn-errortype` header additionally defeats any proxy or logging layer that classifies AWS errors from headers without parsing the body.

Fix: the cognito-idp awsJson1_1 error serializer (the shared error-to-HTTP-status mapping used by every operation handler in the service crate), not the individual device handlers: client errors must be pinned to status 400 regardless of the shape's @httpError trait, and the same serializer must emit `x-amzn-errortype: <ErrorShapeName>:`.

Related open issues that do NOT cover these steps: 20261029T184956Z-cognito-idp-modeled-httperror-codes-are-served-where.md names ten other operations (AdminGetDevice, ConfirmDevice, DescribeUserPool, ...) and 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md owns a different set of replay keys; the seven clusters below are unowned by either.

Cause 1 of 7: AdminUpdateDeviceStatus for a device key that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-admin-device-management-error-paths, step task#6 (3 recorded steps in this cluster).
Cause 2 of 7: AdminForgetDevice for a device key that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-admin-device-management-error-paths, step task#7 (3 recorded steps).
Cause 3 of 7: GetDevice for a device key that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-device-and-pool-diagnostics, step task#8.
Cause 4 of 7: UpdateDeviceStatus with an access token that is not valid answers 403; AWS answers 400 NotAuthorizedException. Fixture cognito-idp-device-self-service-invalid-token-errors, step task#2 (2 recorded steps).
Cause 5 of 7: ForgetDevice with an access token that is not valid answers 403; AWS answers 400 NotAuthorizedException. Fixture cognito-idp-device-self-service-invalid-token-errors, step task#3 (3 recorded steps).
Cause 6 of 7: ForgetDevice for a device key that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-device-tracking-lifecycle, step task#10 (2 recorded steps).
Cause 7 of 7: UpdateDeviceStatus for a device key that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-self-service-device-remember-and-forget, step task#10 (2 recorded steps).

## Evidence
Expected (recorded AWS), all 7 clusters: HTTP 400 with the error class in body `__type` and header `x-amzn-errortype: <ErrorShapeName>:`.
Actual (emulator):
- AdminUpdateDeviceStatus: `status: expected 400, got 404`; `$headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:', got None)`
- AdminForgetDevice: `status: expected 400, got 404`; `x-amzn-errortype` missing (expected 'ResourceNotFoundException:')
- GetDevice: `status: expected 400, got 404`; `x-amzn-errortype` missing (expected 'ResourceNotFoundException:')
- UpdateDeviceStatus (bad token): `status: expected 400, got 403`; `x-amzn-errortype` missing (expected 'NotAuthorizedException:')
- ForgetDevice (bad token): `status: expected 400, got 403`; `x-amzn-errortype` missing (expected 'NotAuthorizedException:')
- ForgetDevice (unknown device): `status: expected 400, got 404`; `x-amzn-errortype` missing (expected 'ResourceNotFoundException:')
- UpdateDeviceStatus (unknown device): `status: expected 400, got 404`; `x-amzn-errortype` missing (expected 'ResourceNotFoundException:')
Smithy (model path services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/smithy-model/...): ResourceNotFoundException @httpError(404), NotAuthorizedException @httpError(403) — the recorded service does not honor these.
Fixed text vs variable parts: the error class name and the HTTP status are fixed per error class; the device key, username and user-pool id inside the message come from the request and from emulator state and are not part of the requirement.

## Steps to Reproduce
1. Replay cognito-idp-admin-device-management-error-paths step task#6 (AdminUpdateDeviceStatus with a device key that was never registered) and read the status line: emulator 404, AWS 400.
2. Replay cognito-idp-device-self-service-invalid-token-errors step task#2 (UpdateDeviceStatus with a bogus access token): emulator 403, AWS 400.
3. Inspect the response headers on each of those: no `x-amzn-errortype` is emitted; AWS sends `x-amzn-errortype: ResourceNotFoundException:` / `NotAuthorizedException:`.
4. Repeat for each fixture#step listed under the causes.
