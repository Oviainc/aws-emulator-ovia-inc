Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-device-tracking-status-management
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-device-tracking-status-management cip-cfg-alias-email-duplicate-signup-and-alias-claim cognito-idp-self-service-signup-and-token-session cognito-idp-self-service-signup-confirm-and-forgot-password cognito-idp-attribute-management-edge-cases-and-invalid-input cognito-idp-user-pool-core-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|ConfirmSignUp|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetUserAttributeVerificationCode|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ListUserPools|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261029T205621Z-cognito-idp-device-operations-serve-httperror-404-403.md
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# cognito-idp refusals already at 400 still omit x-amzn-errortype header

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ForgetDevice

## Description
This bundle has 7 causes; all 7 must be closed before submitting.

AWS: under the AWS JSON 1.1 protocol cognito-idp returns the error shape name in the `x-amzn-errortype` response header on EVERY error response, in addition to the body's `__type`. The recorded header value is the error shape name followed by a colon — e.g. `InvalidUserPoolConfigurationException:`, `InvalidParameterException:`. Smithy: InvalidUserPoolConfigurationException @httpError(400), InvalidParameterException @httpError(400), AliasExistsException @httpError(400); these clusters are the ones whose HTTP status the emulator already gets right, so the header is the sole divergence.

Emulator: the emulator serves the correct 400 status and the correct error class in the body for all seven steps, but emits no `x-amzn-errortype` header at all.

Why: any component that classifies an AWS error without parsing the JSON body — API gateways, service proxies, request-logging and observability sidecars, and SDK middleware that reads the header first — sees an unclassified 400. Error metrics and alarm rules keyed on the Cognito error type never fire, and middleware that maps the header to a retry/no-retry decision falls back to a generic classification that differs from AWS.

Fix: the cognito-idp awsJson1_1 error serializer in the service crate must set `x-amzn-errortype` to the error shape name (followed by a colon, as the recorded service does) for every error it renders; this is the same serializer that also mis-maps the status on other operations.

Related open issue that does NOT cover these steps: 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md carries a different set of replay keys; the seven clusters below are unowned by it.

Cause 1 of 7: ForgetDevice refused with InvalidUserPoolConfigurationException (device tracking not configured) returns 400 but no `x-amzn-errortype`. Fixture cognito-idp-device-tracking-status-management, step task#7.
Cause 2 of 7: UpdateDeviceStatus refused with InvalidUserPoolConfigurationException returns 400 but no `x-amzn-errortype`. Fixture cognito-idp-device-tracking-status-management, step task#8.
Cause 3 of 7: AdminForgetDevice refused with InvalidUserPoolConfigurationException returns 400 but no `x-amzn-errortype`. Fixture cognito-idp-device-tracking-status-management, step task#9.
Cause 4 of 7: AdminUpdateDeviceStatus refused with InvalidUserPoolConfigurationException returns 400 but no `x-amzn-errortype`. Fixture cognito-idp-device-tracking-status-management, step task#10.
Cause 5 of 7: ConfirmSignUp refused on a duplicate email alias returns 400 but no `x-amzn-errortype`. Fixture cip-cfg-alias-email-duplicate-signup-and-alias-claim, step task#6 (3 recorded steps in this cluster).
Cause 6 of 7: GetUserAttributeVerificationCode refused with InvalidParameterException returns 400 but no `x-amzn-errortype`. Fixture cognito-idp-attribute-management-edge-cases-and-invalid-input, step task#9.
Cause 7 of 7: ListUserPools refused on an invalid request returns 400 but no `x-amzn-errortype`. Fixture cognito-idp-user-pool-core-lifecycle, step task#6.

## Evidence
Expected (recorded AWS): header `x-amzn-errortype` present on the 400 response, value = error shape name + ':' (recorded: 'InvalidUserPoolConfigurationException:' for the four device steps, 'InvalidParameterException:' for GetUserAttributeVerificationCode, header-shape-only assertion for ConfirmSignUp and ListUserPools).
Actual (emulator): `$headers.x-amzn-errortype: missing (expected '<value>', got None)`; the HTTP status (400) and the body's error class matched on all seven steps, so the header is the only divergence.
Smithy: InvalidUserPoolConfigurationException @httpError(400), InvalidParameterException @httpError(400) — status already correct here.
Fixed vs variable: the header name and the error shape name (plus the trailing colon) are the requirement; nothing in the header is request- or run-specific.

## Steps to Reproduce
1. Replay cognito-idp-device-tracking-status-management step task#7 (ForgetDevice on a pool with device tracking not configured) and dump the response headers: the body carries InvalidUserPoolConfigurationException and the status is 400, but there is no `x-amzn-errortype` header; AWS sends `x-amzn-errortype: InvalidUserPoolConfigurationException:`.
2. Replay cognito-idp-attribute-management-edge-cases-and-invalid-input step task#9 (GetUserAttributeVerificationCode with an invalid parameter): same — no header, AWS sends `InvalidParameterException:`.
3. Repeat for each fixture#step listed under the causes.
