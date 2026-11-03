Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-auth-challenge-flow
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-auth-challenge-flow cognito-idp-client-credentials-token-and-group-listing cognito-idp-provisioned-limit-errors cognito-idp-provisioned-limit-round-trip cognito-idp-provisioned-limit-roundtrip cognito-idp-self-service-device-remember-and-forget cognito-idp-user-device-management-errors cognito-idp-user-pool-replica-error-paths cognito-idp-webauthn-error-paths-and-listing
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminRespondToAuthChallenge|4xx->4xx|$.__type
Issue-Key: cognito-idp|AdminRespondToAuthChallenge|4xx->4xx|$.message
Issue-Key: cognito-idp|AdminRespondToAuthChallenge|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminRespondToAuthChallenge|4xx->4xx|status
Issue-Key: cognito-idp|GetClientToken|4xx->4xx|$.__type
Issue-Key: cognito-idp|GetClientToken|4xx->4xx|$.message
Issue-Key: cognito-idp|GetClientToken|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetProvisionedLimit|4xx->4xx|$.__type
Issue-Key: cognito-idp|GetProvisionedLimit|4xx->4xx|$.message
Issue-Key: cognito-idp|GetProvisionedLimit|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetProvisionedLimit|4xx->4xx|status
Issue-Key: cognito-idp|ConfirmDevice|4xx->4xx|$.__type
Issue-Key: cognito-idp|ConfirmDevice|4xx->4xx|$.message
Issue-Key: cognito-idp|DeleteUserPoolReplica|4xx->4xx|$.__type
Issue-Key: cognito-idp|DeleteUserPoolReplica|4xx->4xx|$.message
Issue-Key: cognito-idp|CompleteWebAuthnRegistration|4xx->4xx|$.__type
Issue-Key: cognito-idp|CompleteWebAuthnRegistration|4xx->4xx|$.message
Issue-Key: cognito-idp|CompleteWebAuthnRegistration|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md
Issue-Related: 20261029T184956Z-cognito-idp-modeled-httperror-codes-are-served-where.md
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: seven refusals are raised as the wrong modeled error class

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminRespondToAuthChallenge

## Description
Seven recorded refusals where the emulator raises a different modeled exception than AWS, so the `__type`, the message and (consequently) the HTTP status all differ. For each, AWS's error class and message text are given; the variable parts of each message are named.

Cause 1 of 7: AdminRespondToAuthChallenge with a session that does not match the challenge. AWS: CodeMismatchException, message `Invalid code/session provided` (fixed text, no variable part). Emulator: NotAuthorizedException with `Invalid session for the user.` and HTTP 403. Why: clients retry a code mismatch by re-prompting, but treat NotAuthorized as a dead session and restart the whole flow. Fix: the AdminRespondToAuthChallenge challenge-verification branch. Fixture cognito-idp-admin-auth-challenge-flow, step task#5.
Cause 2 of 7: GetClientToken with a malformed client secret. AWS: NotAuthorizedException, message `Invalid client or secret` — deliberately non-specific so the caller cannot distinguish a bad client id from a bad secret. Emulator: InvalidParameterException carrying a generated regex-constraint message that echoes the submitted secret. Why: besides the wrong class, echoing the submitted secret into the message leaks a credential AWS never reflects. Fix: GetClientToken credential verification must run before (not after) input-pattern validation of `secret`. Fixture cognito-idp-client-credentials-token-and-group-listing, step task#8.
Cause 3 of 7: GetProvisionedLimit for a category name that is not a known API category. AWS: InvalidParameterException, message `Unknown API_CATEGORY category: <value>` — fixed text with the category value from the request. Emulator: ResourceNotFoundException with `There is no limit for API category <value> in this account.` and HTTP 404. Why: an unknown enum value is a request defect, not a missing resource; clients that create-on-not-found loop forever. Fix: GetProvisionedLimit must validate the category before looking up account state. Fixture cognito-idp-provisioned-limit-errors, step task#1 (3 recorded steps).
Cause 4 of 7: ConfirmDevice with a device key that already exists. AWS: InvalidParameterException, message `Invalid device key given.`. Emulator: DeviceKeyExistsException with `Device key already exists.`. Fix: the ConfirmDevice duplicate branch. Fixture cognito-idp-self-service-device-remember-and-forget, step task#7.
Cause 5 of 7: ConfirmDevice with no device secret/verifier supplied. AWS: InvalidParameterException, message `Invalid device credentials given, no credentials given`. Emulator: ResourceNotFoundException with `Device does not exist.` and HTTP 404. Fix: ConfirmDevice must validate the supplied device credentials before resolving the device. Fixture cognito-idp-user-device-management-errors, step task#6.
Cause 6 of 7: DeleteUserPoolReplica for a replica that is not in a deletable state. AWS: ResourceNotFoundException, message `Replica not found in the requested region`. Emulator: InvalidParameterException with a state-describing sentence naming the pool, region and status. Fix: the DeleteUserPoolReplica branch. Fixture cognito-idp-user-pool-replica-error-paths, step task#5.
Cause 7 of 7: CompleteWebAuthnRegistration with malformed credential data on a pool where WebAuthn is enabled. AWS: InvalidParameterException, message `Credential data is not valid`. Emulator: WebAuthnNotEnabledException with `WebAuthn is not enabled for this user pool.` — it reports the wrong precondition entirely. Fix: CompleteWebAuthnRegistration must validate the credential payload and only report the not-enabled precondition when the pool really has WebAuthn off. Fixture cognito-idp-webauthn-error-paths-and-listing, step task#8.

Related open issue that does not carry these replay keys: 20261029T182116Z-cognito-idp-authentication-refusals-use-the-wrong-error.md.

## Evidence
AdminRespondToAuthChallenge (cognito-idp-admin-auth-challenge-flow#task#5): status expected 400 got 403; $.__type expected 'CodeMismatchException' got 'NotAuthorizedException'; $.message expected 'Invalid code/session provided' got 'Invalid session for the user.'
GetClientToken (cognito-idp-client-credentials-token-and-group-listing#task#8): $.__type expected 'NotAuthorizedException' got 'InvalidParameterException'; $.message expected 'Invalid client or secret' got "1 validation error detected: Value '<submitted secret>' at 'secret' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+]+$".
GetProvisionedLimit (cognito-idp-provisioned-limit-errors#task#1): status expected 400 got 404; $.__type expected 'InvalidParameterException' got 'ResourceNotFoundException'; $.message expected 'Unknown API_CATEGORY category: NotARealCategory' got 'There is no limit for API category NotARealCategory in this account.'
ConfirmDevice (cognito-idp-self-service-device-remember-and-forget#task#7): $.__type expected 'InvalidParameterException' got 'DeviceKeyExistsException'; $.message expected 'Invalid device key given.' got 'Device key already exists.'
ConfirmDevice (cognito-idp-user-device-management-errors#task#6): status expected 400 got 404; $.__type expected 'InvalidParameterException' got 'ResourceNotFoundException'; $.message expected 'Invalid device credentials given, no credentials given' got 'Device does not exist.'
DeleteUserPoolReplica (cognito-idp-user-pool-replica-error-paths#task#5): $.__type expected 'ResourceNotFoundException' got 'InvalidParameterException'; $.message expected 'Replica not found in the requested region'.
CompleteWebAuthnRegistration (cognito-idp-webauthn-error-paths-and-listing#task#8): $.__type expected 'InvalidParameterException' got 'WebAuthnNotEnabledException'; $.message expected 'Credential data is not valid' got 'WebAuthn is not enabled for this user pool.'

## Steps to Reproduce
1. Replay cognito-idp-admin-auth-challenge-flow step task#5 and read __type/message/status.
2. Replay each remaining fixture#step named in the causes and compare __type and message with the recording.
