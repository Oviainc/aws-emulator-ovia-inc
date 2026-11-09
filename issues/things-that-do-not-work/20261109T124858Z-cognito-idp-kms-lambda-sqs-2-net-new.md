Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__kms__lambda__sqs.custom-sms-sender-delivers-temp-password-without-sns-configuration cognito-idp__kms__lambda__sqs.custom-sms-sender-wrong-temp-password-rejected
Issue-Key: cognito-idp__kms__lambda__sqs.custom-sms-sender-delivers-temp-password-without-sns-configuration|_refusal#4|AssertionError|raise AssertionError(
Issue-Key: cognito-idp__kms__lambda__sqs.custom-sms-sender-wrong-temp-password-rejected|next_code#7|TimeoutError|raise TimeoutError(
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# cognito-idp__kms__lambda__sqs: 2 net-new causes — AdminCreateUser with DesiredDeliveryMediums=SMS succeeds on a pool with no SMS configuration, and the CustomSMSSender trigger is never invoked

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminCreateUser

## Description
Two net-new root causes on the custom-SMS-sender delivery path.

Cause 1 of 2: an SMS delivery with no SMS path configured is accepted.
AWS: Smithy: AdminCreateUser declares InvalidParameterException (httpError 400). On a pool that has neither SmsConfiguration nor a CustomSMSSender trigger, AdminCreateUser with DesiredDeliveryMediums=['SMS'] is refused 400 InvalidParameterException with message 'User pool does not have SMS configuration to send messages.' (fixed text, no variable part) and no user is created — the trigger/SNS configuration is the only delivery path, so the call cannot succeed without one.
Emulator: the call SUCCEEDS, returning 200 with the created user (UserStatus FORCE_CHANGE_PASSWORD), so a user exists whose temporary password was never deliverable.
Why: the misconfiguration is swallowed — an operator believes the invite was sent, while no message exists anywhere; the user is stuck in FORCE_CHANGE_PASSWORD with a secret nobody received, and the negative control that proves 'the trigger is the only delivery path' silently passes.
Fix: the cognito-idp AdminCreateUser handler's delivery-medium validation (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_create_user.rs) — refuse SMS delivery when the pool has neither SmsConfiguration nor a CustomSMSSender trigger, and create no user.
Traces: cognito-idp__kms__lambda__sqs.custom-sms-sender-delivers-temp-password-without-sns-configuration.

Cause 2 of 2: the CustomSMSSender trigger is never invoked.
AWS: recorded behavior only for the trigger event (it is not a Smithy operation). On a pool whose LambdaConfig has CustomSMSSender + KMSKeyID, AdminCreateUser with DesiredDeliveryMediums=['SMS'] invokes the configured Lambda with a customSMSSenderRequestV1 event whose triggerSource is CustomSMSSender_AdminCreateUser, whose userAttributes carry the user's phone_number and whose `code` is the temporary password encrypted under the pool's KMSKeyID — so the function can decrypt it and the decrypted value signs the user in (AdminInitiateAuth then answers ChallengeName NEW_PASSWORD_REQUIRED).
Emulator: nothing is ever delivered — no CustomSMSSender_AdminCreateUser event reaches the function (the function's queue stays empty for the whole ~53 s wait, with no unmatched deliveries either), so the trigger is never called.
Why: the documented custom-sender integration does not exist: a customer who routes all SMS through their own sender gets no invite, no verification code and no way to complete sign-in, and nothing about the failure is observable.
Fix: the cognito-idp message-delivery path that must invoke the pool's CustomSMSSender trigger (event construction, triggerSource, KMS encryption of `code` under the pool's KMSKeyID, and the lambda invoke) for AdminCreateUser and the other SMS-sending operations.
Traces: cognito-idp__kms__lambda__sqs.custom-sms-sender-wrong-temp-password-rejected.

## Evidence
Cause 1 — AssertionError: the call was expected to be rejected but SUCCEEDED, which is the defect: {'User': {'Username': 'sim2real-xsvc-user-…', 'Attributes': [{'Name':'phone_number','Value':'+15555550123'}, …], 'Enabled': True, 'UserStatus': 'FORCE_CHANGE_PASSWORD'}, 'ResponseMetadata': {'HTTPStatusCode': 200, …}} — expected ('InvalidParameterException', 'User pool does not have SMS configuration to send messages.').
Cause 2 — TimeoutError: no 'CustomSMSSender_AdminCreateUser' code for 'sim2real-xsvc-user-…' reached the trigger's queue within 52.7s; deliveries seen but unmatched: [].

## Steps to Reproduce
1) Cause 1: create a user pool with no SmsConfiguration and no CustomSMSSender; call AdminCreateUser with a verified phone_number and DesiredDeliveryMediums=['SMS'] — AWS refuses InvalidParameterException 'User pool does not have SMS configuration to send messages.', the emulator returns 200 and creates the user.
2) Cause 2: create a KMS key and a Lambda that decrypts request.code and forwards it to SQS; create a pool with LambdaConfig={'CustomSMSSender':{...},'KMSKeyID':<key arn>}; call AdminCreateUser with DesiredDeliveryMediums=['SMS'] — AWS invokes the trigger with a customSMSSenderRequestV1 event (triggerSource CustomSMSSender_AdminCreateUser), the emulator invokes nothing.
