Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-device-lifecycle cognito-idp.device-lifecycle-admin cognito-idp.device-key-round-trip-through-list-and-get cognito-idp.device-remember-status-ttl-persists cognito-idp.remembered-device-ttl-affects-mfa cognito-idp.self-service-list-devices-reflects-confirmed-device cognito-idp.deep-cognito-idp-2-token-rotation-device-key-omitted-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.admin-device-lifecycle|run#8|AssertionError|assert new_device is not None, "expected NewDeviceMetadata on first sign-in with device tracking enabled"
Issue-Key: cognito-idp.device-lifecycle-admin|run#13|AssertionError|assert meta and meta.get("DeviceKey"), (
Issue-Key: cognito-idp.device-key-round-trip-through-list-and-get|run#13|KeyError|new_device = result["NewDeviceMetadata"]
Issue-Key: cognito-idp.device-remember-status-ttl-persists|run#10|AssertionError|assert new_device is not None, "expected NewDeviceMetadata on first sign-in with device tracking enabled"
Issue-Key: cognito-idp.remembered-device-ttl-affects-mfa|run#12|AssertionError|assert meta and meta.get("DeviceKey"), (
Issue-Key: cognito-idp.self-service-list-devices-reflects-confirmed-device|_create_user_and_sign_in#5|KeyError|device_key = auth["NewDeviceMetadata"]["DeviceKey"]
Issue-Key: cognito-idp.deep-cognito-idp-2-token-rotation-device-key-omitted-rejected|run#13|AssertionError|assert device_key, f"device-tracking pool issued no NewDeviceMetadata on sign-in: {ndm!r}"
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# cognito-idp: device remembering is inert — AuthenticationResult never carries NewDeviceMetadata, so no device can be confirmed, listed, forgotten or remembered (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
Cause 1 of 1 — a sign-in against a user pool that has DeviceConfiguration set returns an AuthenticationResult with no NewDeviceMetadata member, so the device key that every device operation consumes is never minted.

AWS: Smithy (cognito-idp) AuthenticationResultType declares the member NewDeviceMetadata -> NewDeviceMetadataType {DeviceKey (length 1..55, pattern ^[\w-]+_[0-9a-f-]+$), DeviceGroupKey}. Its documentation states it is 'Information that your user pool responds with in AuthenticationResult when you configure it to remember devices and a user signs in with an unrecognized device', and UserPoolType.DeviceConfiguration documents that 'When you provide a value for any property of DeviceConfiguration, you activate the device remembering for the user pool'. So InitiateAuth / AdminInitiateAuth on a device-tracking pool, for an unrecognized device, must return AuthenticationResult.NewDeviceMetadata.DeviceKey; that DeviceKey is the required input to ConfirmDevice, UpdateDeviceStatus, GetDevice/AdminGetDevice, ListDevices/AdminListDevices and AdminForgetDevice, and it is what makes a remembered device skip the MFA challenge.

Emulator: AuthenticationResult comes back with only AccessToken, ExpiresIn, TokenType, RefreshToken and IdToken; NewDeviceMetadata is absent entirely. Replay evidence: 'expected NewDeviceMetadata.DeviceKey on login, got keys=[AccessToken, ExpiresIn, TokenType, RefreshToken, IdToken]', 'device-tracking pool issued no NewDeviceMetadata on sign-in: {}', 'expected NewDeviceMetadata on first sign-in with device tracking enabled', and consumers that index the member raise KeyError: 'NewDeviceMetadata'. This is the same on the admin flow (AdminInitiateAuth) and the self-service flow (InitiateAuth).

Why: a client that implements 'Remember me on this device' has nothing to remember — it cannot call ConfirmDevice at all, so no device is ever confirmed or listed, remembered-device MFA suppression can never happen, and device-key-bound refresh-token rotation cannot be exercised. Code that reads AuthenticationResult['NewDeviceMetadata']['DeviceKey'] crashes rather than degrading.

Fix: the device state of the user pool plus the auth handlers that mint it — AdminInitiateAuth/InitiateAuth must create a device record for an unrecognized device on a pool with DeviceConfiguration and project NewDeviceMetadata into AuthenticationResult; the ConfirmDevice/GetDevice/ListDevices/UpdateDeviceStatus/ForgetDevice handlers then need that state.

Traces: cognito-idp.admin-device-lifecycle, cognito-idp.device-lifecycle-admin, cognito-idp.device-key-round-trip-through-list-and-get, cognito-idp.device-remember-status-ttl-persists, cognito-idp.remembered-device-ttl-affects-mfa, cognito-idp.self-service-list-devices-reflects-confirmed-device, cognito-idp.deep-cognito-idp-2-token-rotation-device-key-omitted-rejected.

## Evidence
Expected: AuthenticationResult.NewDeviceMetadata.DeviceKey present (pattern ^[\w-]+_[0-9a-f-]+$) on the first sign-in of a user on a pool created with DeviceConfiguration.
Actual (replay):
- cognito-idp.device-lifecycle-admin / cognito-idp.remembered-device-ttl-affects-mfa: AssertionError: expected NewDeviceMetadata.DeviceKey on login, got keys=['AccessToken', 'ExpiresIn', 'TokenType', 'RefreshToken', 'IdToken']
- cognito-idp.admin-device-lifecycle / cognito-idp.device-remember-status-ttl-persists: AssertionError: expected NewDeviceMetadata on first sign-in with device tracking enabled
- cognito-idp.deep-cognito-idp-2-token-rotation-device-key-omitted-rejected: AssertionError: device-tracking pool issued no NewDeviceMetadata on sign-in: {}
- cognito-idp.device-key-round-trip-through-list-and-get and cognito-idp.self-service-list-devices-reflects-confirmed-device: KeyError: 'NewDeviceMetadata'
None of these checks carried an 'ALREADY FILED' or 'OWNED by' marker in the replay report.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateUserPool with DeviceConfiguration={'ChallengeRequiredOnNewDevice': True, 'DeviceOnlyRememberedOnUserPrompt': False} and CreateUserPoolClient with ALLOW_ADMIN_USER_PASSWORD_AUTH.
3. AdminCreateUser + AdminSetUserPassword(Permanent=True), then AdminInitiateAuth(AuthFlow='ADMIN_USER_PASSWORD_AUTH').
4. Inspect the response: expect AuthenticationResult.NewDeviceMetadata.DeviceKey; observe the member is absent.
5. Repeat with InitiateAuth(USER_PASSWORD_AUTH) — same result; ConfirmDevice then has no DeviceKey to pass.
6. Re-replay the seven check ids in trace_ids.
