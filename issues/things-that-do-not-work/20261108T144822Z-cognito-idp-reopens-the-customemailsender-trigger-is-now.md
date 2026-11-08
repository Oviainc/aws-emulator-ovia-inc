Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.user-attribute-verification-flow
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|UpdateUserAttributes|UserLambdaValidationException
Issue-Commit: 5bdc422e571ff089197010dd8b23a6f4d1e72235

# cognito-idp: REOPENS — the CustomEmailSender trigger is now invoked but dies at module load, so UpdateUserAttributes is refused 400 UserLambdaValidationException instead of issuing a code (1 cause)

> _Note: this issue was identified against the code base at commit `5bdc422e571ff089197010dd8b23a6f4d1e72235`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateUserAttributes

## Description
REOPENS 20261107T213028Z-cognito-idp-the-customemailsender-lambda-trigger-is-never.md: its fix left the call refused.

Cause 1 of 1: a user pool whose LambdaConfig carries a CustomEmailSender trigger (plus LambdaConfig.KMSKeyID) now does invoke the configured function, but the invocation fails at module link time inside the emulator's in-process Node.js Lambda runtime, and Cognito turns that handler failure into a client-visible refusal of the user-facing operation. Failing check: cognito-idp.user-attribute-verification-flow.

  AWS:      Smithy (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice): UpdateUserAttributes takes required UserAttributes + AccessToken and answers 200 with CodeDeliveryDetailsList; it models UserLambdaValidationException / UnexpectedLambdaException / InvalidLambdaResponseException (all HTTP 400) only for the case where the configured trigger itself fails. LambdaConfigType.CustomEmailSender is a CustomEmailLambdaVersionConfigType (required LambdaVersion V1_0, required LambdaArn) accepted alongside LambdaConfig.KMSKeyID. Recorded behavior: changing a verifiable attribute invokes the function with triggerSource "CustomEmailSender_UpdateUserAttribute" and request.code carrying an AWS-Encryption-SDK message encrypted under LambdaConfig.KMSKeyID, the handler runs to completion (it decrypts with KMS Decrypt), and the caller gets 200 + CodeDeliveryDetails — not an error. The handler is a nodejs22.x ESM module doing `import { KMSClient, DecryptCommand } from "@aws-sdk/client-kms"`, which the runtime resolves and links.
  Emulator: UpdateUserAttributes is refused HTTP 400, Error.Code=UserLambdaValidationException, Error.Message="CustomEmailSender failed with error SyntaxError: Could not find export 'DecryptCommand' in module 'file:///var/runtime/node_modules/@aws-sdk/client-kms'." Fixed text of that message class is "CustomEmailSender failed with error " + the handler's own error text; here the variable part is the runtime's own link failure, i.e. the message is evidence of an emulator-internal defect being surfaced to the caller. The module path IS resolved, so the bundled @aws-sdk/client-kms exists but is an incomplete shim: it exports KMSClient and not the SDK's command classes (DecryptCommand at minimum), so any ESM named import of a command fails before the handler body ever runs.
  Why:      a client that uses the documented custom-sender trigger to deliver verification codes over its own channel cannot change or verify an attribute at all: the operation that must answer 200 and hand it a code answers 400 instead, so the pending-value -> code -> verified state machine is unreachable and the failure is reported as if the customer's handler were invalid. It is also indistinguishable, from the API, from a genuine customer bug.
  Fix:      the emulator's in-process Node.js Lambda runtime module surface — the bundled /var/runtime/node_modules/@aws-sdk/client-kms must export the client's command classes (DecryptCommand and its siblings), not only KMSClient — and, downstream of that, the cognito-idp CustomEmailSender dispatch path shared by the UpdateUserAttributes and GetUserAttributeVerificationCode handlers, which must not report a runtime-internal link error as the pool owner's Lambda validation failure. Related open issues report the same incomplete-bundled-SDK shape for other clients (20261107T205257Z-events-lambda-sns-sqs-the-lambda-runtime-s.md, 20261107T185637Z-lambda-s3-the-in-process-lambda-runtime-s-bundled-sdk.md) and the fixed 20261106T062901Z-lambda-nodejs-runtime-8-net-new-execution-fidelity.md stood the v3 SDK up in the first place; none of them carries this check's replay key.
  Traces:   cognito-idp.user-attribute-verification-flow.

## Evidence
Check cognito-idp.user-attribute-verification-flow — verdict [error].

Actual (replay output):
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.UserLambdaValidationException: An error occurred (UserLambdaValidationException) when calling the UpdateUserAttributes operation: CustomEmailSender failed with error SyntaxError: Could not find export 'DecryptCommand' in module 'file:///var/runtime/node_modules/@aws-sdk/client-kms'.

Expected: with LambdaConfig {CustomEmailSender {LambdaVersion "V1_0", LambdaArn <fn>}, KMSKeyID <key arn>}, UpdateUserAttributes(AccessToken, UserAttributes=[{Name: "email", Value: <new address>}]) answers HTTP 200 with CodeDeliveryDetailsList, having invoked <fn> once with triggerSource "CustomEmailSender_UpdateUserAttribute" and request.code = the ESDK-encrypted code; the handler's `import { KMSClient, DecryptCommand } from "@aws-sdk/client-kms"` links successfully in the nodejs22.x runtime.

Actual: HTTP 400 UserLambdaValidationException whose message carries the runtime's own SyntaxError about the missing DecryptCommand export. Usernames, function names, key ARNs and code digits are per-run values; the requirement is the 200-with-CodeDeliveryDetails outcome and a runtime whose bundled @aws-sdk/client-kms exports the command classes.

## Steps to Reproduce
1. Create a symmetric KMS key usable by cognito-idp.amazonaws.com, an SQS queue, and a nodejs22.x Lambda whose index.mjs starts with `import { KMSClient, DecryptCommand } from "@aws-sdk/client-kms";`, decrypts event.request.code and forwards the plaintext to the queue; grant lambda:InvokeFunction to cognito-idp.amazonaws.com.
2. CreateUserPool with LambdaConfig = {CustomEmailSender: {LambdaVersion: "V1_0", LambdaArn: <fn arn>}, KMSKeyID: <key arn>}; create an app client; AdminCreateUser + AdminSetUserPassword(Permanent) and obtain an AccessToken.
3. Call UpdateUserAttributes(AccessToken=<token>, UserAttributes=[{Name: "email", Value: <new address>}]).
   Expected: HTTP 200 with CodeDeliveryDetailsList, and the handler delivers the decrypted code.
   Observed: HTTP 400 UserLambdaValidationException — "CustomEmailSender failed with error SyntaxError: Could not find export 'DecryptCommand' in module 'file:///var/runtime/node_modules/@aws-sdk/client-kms'."
4. Repeat with GetUserAttributeVerificationCode — same refusal, so VerifyUserAttribute can never be driven with a real code.
