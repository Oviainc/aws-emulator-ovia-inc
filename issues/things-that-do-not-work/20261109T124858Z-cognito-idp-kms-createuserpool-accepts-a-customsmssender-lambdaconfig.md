Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__kms.custom-sms-sender-without-kms-key-rejected
Issue-Key: cognito-idp__kms.custom-sms-sender-without-kms-key-rejected|_refusal#6|AssertionError|raise AssertionError(
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# cognito-idp__kms: CreateUserPool accepts a CustomSMSSender LambdaConfig with no KMSKeyID instead of refusing InvalidParameterException

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPool

## Description
Cause 1 of 1: the KMS-key requirement of the custom sender triggers is not enforced.
AWS: Smithy: CreateUserPool takes LambdaConfig (LambdaConfigType) and declares InvalidParameterException (httpError 400); LambdaConfigType.KMSKeyID is documented as 'The ARN of an KMS key. Amazon Cognito uses the key to encrypt codes and temporary passwords sent to custom sender Lambda triggers.' A CreateUserPool that declares CustomSMSSender (or CustomEmailSender) without KMSKeyID is therefore refused 400 InvalidParameterException with message 'KMSKeyId parameter is required when Custom SMS/Email Lambda trigger is used.' (fixed text, no variable part) and NO pool is created; with a KMSKeyID the same LambdaConfig is accepted and stored unchanged.
Emulator: the call SUCCEEDS — it returns a UserPool whose LambdaConfig.CustomSMSSender is stored with no KMSKeyID, so a pool exists that AWS would never have created.
Why: a customer gets a pool whose custom-sender trigger can never receive an encrypted code (there is no key to encrypt it under), and the misconfiguration surfaces much later as an undeliverable message instead of at the create call; CI that asserts the refusal passes silently, and the leftover pool is state AWS would not have.
Fix: the cognito-idp CreateUserPool handler's LambdaConfig validation (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/create_user_pool.rs) — require KMSKeyID whenever CustomSMSSender/CustomEmailSender is present, and create nothing when refusing. The same gate belongs on UpdateUserPool.
Traces: cognito-idp__kms.custom-sms-sender-without-kms-key-rejected.

## Evidence
AssertionError: the call was expected to be rejected but SUCCEEDED, which is the defect: {'UserPool': {'Id': 'us-west-2_e10527523', 'Name': 'sim2real-xsvc-pool-nokey-…', …, 'LambdaConfig': {'CustomSMSSender': {'LambdaVersion': 'V1_0', 'LambdaArn': 'arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-fn-…'}}, …}}
Expected: ('InvalidParameterException', 'KMSKeyId parameter is required when Custom SMS/Email Lambda trigger is used.') and no pool left behind.

## Steps to Reproduce
1) Call CreateUserPool with LambdaConfig={'CustomSMSSender': {'LambdaVersion':'V1_0','LambdaArn': <any function ARN>}} and no KMSKeyID.
2) AWS refuses 400 InvalidParameterException 'KMSKeyId parameter is required when Custom SMS/Email Lambda trigger is used.' and creates nothing; the emulator returns 200 with the pool.
3) ListUserPools filtered by the PoolName shows the pool the refused call should never have created.
