Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.user-pool-client-lifecycle-and-secret
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.user-pool-client-lifecycle-and-secret|run#25|AssertionError|assert secret_delete_again_message == expected_message, f"expected the message {expected_message!r}, got {secret_delete_again_message!r}"
Issue-Commit: a6cd77094d82d955b2186a9b9b1faf688af70a8f

# cognito-idp: DeleteUserPoolClientSecret not-found refusal interpolates the ClientSecretId and adds a trailing period instead of AWS's fixed 'Client secret does not exist'

> _Note: this issue was identified against the code base at commit `a6cd77094d82d955b2186a9b9b1faf688af70a8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteUserPoolClientSecret

## Description
This bundle has 1 cause; it must be closed before submitting.

Related open issues that the replay suite does NOT mark as owning this check: 20261029T222840Z-cognito-idp-deleteuserpoolclientsecret-refusals-carry-emulator-invented-messages.md and 20261029T230413Z-cognito-idp-deleteuserpoolclientsecret-not-found-refusal-wrong-http.md both describe DeleteUserPoolClientSecret refusal rendering, and the first one's replay key for this very check is the HTTP-status assertion (`assert secret_delete_again_status == 400`), which now PASSES — the emulator serves 400 here. The check now fails one assertion further on, on the message text, and the suite reports it as neither ALREADY FILED nor OWNED, so it is filed here against its current replay key.

Cause 1 of 1: the unknown/already-deleted ClientSecretId refusal message is emulator prose that interpolates the secret id.
AWS: DeleteUserPoolClientSecret called with a ClientSecretId that is not currently an active secret of that app client — including one that was genuinely issued by AddUserPoolClientSecret and then deleted — is refused with ResourceNotFoundException, HTTP 400, and the message exactly 'Client secret does not exist'. The ENTIRE string is fixed text: there are no variable parts, the request's ClientSecretId is not echoed, and there is no trailing period. Smithy (com.amazonaws.cognitoidentityprovider#DeleteUserPoolClientSecret): input UserPoolId (required, @length 1-55, pattern ^[\w-]+_[0-9a-zA-Z]+$), ClientId (required, @length 1-128, @sensitive), ClientSecretId (required, @length 1-128); errors InvalidParameterException (client, @httpError 400), LimitExceededException (client, 400), ResourceNotFoundException (client, @httpError 404), TooManyRequestsException (429), InternalServerException (500). The model does not document the message, so the wording and the 400 status are recorded behavior only: the recording shows ResourceNotFoundException at HTTP 400 with that fixed sentence (the modeled @httpError(404) is not applied by the recorded service).
Emulator: the decision, the error class (ResourceNotFoundException) and the status (400) are all correct, and the secret set is left unchanged, but the message is rendered as 'Client secret <ClientSecretId> does not exist.' — different fixed text, the request's ClientSecretId interpolated into it, and an added trailing period.
Why: a client rotating app-client secrets distinguishes 'unknown/already-deleted secret' from the other DeleteUserPoolClientSecret refusals (last-remaining secret, limit exceeded) by the Message that the SDK/CLI surfaces verbatim. An equality or hash matcher written against AWS's sentence never matches here, and because the emulator's text embeds a per-request identifier it is not even stable between two runs, so no matcher can be written against it at all. It additionally echoes the caller's secret identifier back into an error string, which is exactly the material AWS keeps out of this message.
Fix: the not-found branch of the DeleteUserPoolClientSecret handler / its validation step in the cognito-idp service crate — on a secret-id lookup miss, raise ResourceNotFoundException with the fixed, non-interpolated sentence and no trailing period.
Traces: cognito-idp.user-pool-client-lifecycle-and-secret.

## Evidence
Failing check: [mismatch] cognito-idp.user-pool-client-lifecycle-and-secret

Expected (AWS): ResourceNotFoundException, HTTP 400, Message exactly 'Client secret does not exist' — fixed text, no interpolated values, no trailing period.

Actual (emulator): AssertionError: expected the message 'Client secret does not exist', got 'Client secret 61a9d6f43fb9468db17c234116--1793320498877 does not exist.'

Which parts are which: 'Client secret does not exist' is the requirement in full — all fixed text. In the emulator's string, '61a9d6f43fb9468db17c234116--1793320498877' is the ClientSecretId the request supplied (a per-run value previously returned by AddUserPoolClientSecret, not part of any requirement); the defect is that it is interpolated at all, together with the reworded surrounding text and the appended '.'.

Preceding assertions in the same check that PASSED and bound the divergence: the repeat delete was refused with Code == 'ResourceNotFoundException' and ResponseMetadata.HTTPStatusCode == 400, and ListUserPoolClientSecrets after the first (successful) delete no longer listed the secret — so the decision, error class, status and state effect are all correct and only the message text diverges.

Smithy: DeleteUserPoolClientSecret declares ResourceNotFoundException with errorKind client and @httpError(404); the recorded service does not apply that trait and answers 400 (recorded behavior only). The emulator already answers 400 here.

## Steps to Reproduce
1. CreateUserPool, then CreateUserPoolClient with GenerateSecret=True; capture ClientId. ListUserPoolClientSecrets shows one secret.
2. AddUserPoolClientSecret(UserPoolId, ClientId) -> capture ClientSecretDescriptor.ClientSecretId as <sid>. ListUserPoolClientSecrets now lists <sid>.
3. DeleteUserPoolClientSecret(UserPoolId, ClientId, ClientSecretId=<sid>) -> succeeds; <sid> disappears from ListUserPoolClientSecrets (in-file positive control).
4. Call DeleteUserPoolClientSecret again with the same <sid>. The emulator refuses with ResourceNotFoundException at HTTP 400 (both correct) but Error.Message is 'Client secret <sid> does not exist.' AWS returns the fixed 'Client secret does not exist' with no id and no trailing period.
5. Re-read ListUserPoolClientSecrets: the secret set is unchanged, confirming the defect is confined to refusal message rendering.
6. Replay cognito-idp.user-pool-client-lifecycle-and-secret to confirm the fix.
