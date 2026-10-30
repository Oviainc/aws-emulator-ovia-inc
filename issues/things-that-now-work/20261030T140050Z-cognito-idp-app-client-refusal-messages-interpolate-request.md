Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.user-pool-client-lifecycle-and-secret cognito-idp.user-pool-client-secret-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.user-pool-client-lifecycle-and-secret|run#35|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.user-pool-client-secret-lifecycle|run#35|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 07e872c7593558b582731e73c173338530c40226

# cognito-idp: app-client refusal messages interpolate request ids instead of AWS's fixed sentences (2 causes) — DescribeUserPoolClient/UpdateUserPoolClient not-found, and DeleteUserPoolClientSecret last-secret

> _Note: this issue was identified against the code base at commit `07e872c7593558b582731e73c173338530c40226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUserPoolClient

## Description
This bundle has 2 causes; BOTH must be closed before submitting. Both are app-client refusal-rendering defects: the emulator reaches the correct decision and the correct error class, but renders the message as its own prose with a request-derived identifier spliced in, where AWS answers a fixed sentence with no variable parts.

Related open issues that do NOT carry either grounding check's replay key (the suite marks neither check as owned, so neither issue covers them): 20261029T182116Z-cognito-idp-not-found-and-duplicate-resource-error.md and 20261029T184956Z-cognito-idp-ten-not-found-refusal-messages-diverge.md own other operations' not-found wording; 20261029T222840Z-cognito-idp-deleteuserpoolclientsecret-refusals-carry-emulator-invented-messages.md describes the same last-secret wording in its Cause 1, but its recorded replay key for cognito-idp.user-pool-client-secret-lifecycle no longer matches where that check now fails (its Cause 2, the 'Client secret does not exist' wording, and the 404-vs-400 status cause, are now passing — fixed by 20261030T004457Z-cognito-idp-deleteuserpoolclientsecret-not-found-refusal-interpolates-the.md and 20261029T205621Z-cognito-idp-app-client-group-resource-server-refusals.md — which is why both checks have advanced to the asserts named below).

Cause 1 of 2: the user-pool-client not-found refusal splices the ClientId into the message.
AWS: a read or mutation addressed to an app client that does not exist in the named pool — including one that existed and has just been deleted — is refused with ResourceNotFoundException whose Message is exactly 'User pool client does not exist.' That whole string is fixed text: it has no variable parts and in particular does not echo the ClientId back. Smithy (com.amazonaws.cognitoidentityprovider): DescribeUserPoolClient declares ResourceNotFoundException (errorKind client) and UpdateUserPoolClient declares the same error; the model documents no message, so the wording is recorded behavior only — the recording shows a 400 with Error.Code ResourceNotFoundException and that fixed sentence. Note ClientIdType is marked @sensitive in the model, which is consistent with AWS keeping it out of the error string.
Emulator: the class and status are right (ResourceNotFoundException, HTTP 400), but Message is 'User pool client <ClientId> does not exist.' — the emulator's own template with the request's ClientId interpolated. Observed on DescribeUserPoolClient immediately after a successful DeleteUserPoolClient; the same not-found arm is what UpdateUserPoolClient on the deleted client exercises in the same check.
Why: clients and operators branch on, log and alarm on Message verbatim. An exact-match or hashed matcher written against AWS's sentence never fires here, and because the text embeds a per-client identifier it is not even stable between two clients hitting the same condition, so no equality-based matcher can be written at all. It also leaks a @sensitive ClientId into an error string AWS deliberately keeps free of it.
Fix: the not-found branch of the DescribeUserPoolClient and UpdateUserPoolClient handlers (and whatever shared app-client lookup helper they call) in the cognito-idp service crate — raise ResourceNotFoundException with the fixed sentence and no interpolation, instead of formatting the request's ClientId into it.
Traces: cognito-idp.user-pool-client-lifecycle-and-secret.

Cause 2 of 2: the last-remaining-client-secret refusal is emulator prose with the ClientId spliced in.
AWS: an app client must always retain at least one active secret, so DeleteUserPoolClientSecret aimed at the client's sole remaining ClientSecretId is refused with InvalidParameterException and the Message 'Cannot delete the only client secret.' That string is entirely fixed text — it interpolates neither the client id nor the secret id. Smithy: DeleteUserPoolClientSecret declares InvalidParameterException (errorKind client, @httpError 400); the model documents no message, so the wording is recorded behavior only, and the recording shows a 400 InvalidParameterException with that sentence.
Emulator: the class is right (InvalidParameterException), but Message is 'Cannot delete the last remaining client secret of user pool client <ClientId>.' — different fixed text plus a ClientId taken from the request.
Why: secret-rotation code branches on this refusal to decide 'add a second secret first, then retry the delete'. It never matches AWS's documented sentence, cannot be matched by equality because of the embedded per-client id, and cannot be distinguished from the unknown-secret refusal by text as AWS words them.
Fix: the last-secret precondition branch of the DeleteUserPoolClientSecret handler in the cognito-idp service crate — emit the fixed sentence with no interpolation. This is the same cause 20261029T222840Z's Cause 1 describes; it is re-filed here because that issue's replay key for this check is stale and the suite therefore replays this check unowned.
Traces: cognito-idp.user-pool-client-secret-lifecycle.

## Evidence
Cause 1 — expected (AWS): ResourceNotFoundException, HTTP 400, Message exactly 'User pool client does not exist.' (fixed text, no variable parts).
Actual (emulator), cognito-idp.user-pool-client-lifecycle-and-secret:
  AssertionError: expected the message 'User pool client does not exist.', got 'User pool client 4844a6632fd14109afa193d69f does not exist.'
The id 4844a6632fd14109afa193d69f is the per-run ClientId the request supplied — variable fill, not part of the requirement. The requirement is the fixed sentence with nothing interpolated. The error class (ResourceNotFoundException) and status (400) asserted just above this line both passed, so only the rendering diverges, and the client really was gone: ListUserPoolClients no longer listed it.
Smithy (DescribeUserPoolClient): errors InvalidParameterException (client 400), NotAuthorizedException (403), OperationNotEnabledException (400), ResourceNotFoundException (client), TooManyRequestsException (429), InternalErrorException (500). UpdateUserPoolClient declares the same ResourceNotFoundException. Input member ClientId targets ClientIdType, which is @sensitive.

Cause 2 — expected (AWS): InvalidParameterException, Message exactly 'Cannot delete the only client secret.' (fixed text, no variable parts).
Actual (emulator), cognito-idp.user-pool-client-secret-lifecycle:
  AssertionError: expected message 'Cannot delete the only client secret.', got 'Cannot delete the last remaining client secret of user pool client 75f783c7d178469a957d7c2013.'
The id 75f783c7d178469a957d7c2013 is the per-run ClientId — variable fill, not requirement text. The divergence is the different fixed wording plus the interpolation.
Smithy (DeleteUserPoolClientSecret): errors InvalidParameterException (client, @httpError 400), LimitExceededException (client 400), ResourceNotFoundException (client), TooManyRequestsException (429), InternalServerException (500).

Effect-absence held in both checks: every refusal above left state untouched — the deleted client stayed absent from ListUserPoolClients, and the last remaining secret was still listed by ListUserPoolClientSecrets after the refusal. Each check's in-file positive controls also succeeded (AddUserPoolClientSecret issued a second live secret whose value differed from the original while DescribeUserPoolClient kept reporting the original; deleting that non-last secret succeeded and removed it from the list; UpdateUserPoolClient renamed the live client and DescribeUserPoolClient read the new name back). So neither operation is broken for every call — only the refusal rendering diverges.

## Steps to Reproduce
Cause 1 (replay cognito-idp.user-pool-client-lifecycle-and-secret):
1. CreateUserPool, then CreateUserPoolClient with GenerateSecret=True; note the ClientId.
2. DeleteUserPoolClient for that ClientId (succeeds; the client disappears from ListUserPoolClients).
3. Call DescribeUserPoolClient with the same UserPoolId/ClientId. The emulator refuses with ResourceNotFoundException and HTTP 400 (both correct) but Error.Message reads 'User pool client <ClientId> does not exist.'; AWS's message is the fixed sentence 'User pool client does not exist.' with no id.
4. Call UpdateUserPoolClient (e.g. with a new ClientName) on the same deleted ClientId — same not-found arm, same divergence.

Cause 2 (replay cognito-idp.user-pool-client-secret-lifecycle):
5. CreateUserPool, then CreateUserPoolClient with GenerateSecret=True. ListUserPoolClientSecrets shows exactly one secret; record its ClientSecretId.
6. AddUserPoolClientSecret (positive control: a second, distinct secret is created and listed, its ClientSecretValue differs from the create-time ClientSecret, and DescribeUserPoolClient still reports the original secret).
7. DeleteUserPoolClientSecret for the added secret (positive control: succeeds, the id leaves ListUserPoolClientSecrets, the original id remains).
8. DeleteUserPoolClientSecret for the one remaining (original) ClientSecretId. The emulator refuses with InvalidParameterException (correct class) but Error.Message reads 'Cannot delete the last remaining client secret of user pool client <ClientId>.'; AWS's message is the fixed sentence 'Cannot delete the only client secret.'
9. Re-read ListUserPoolClientSecrets after each refusal: the secret set must be unchanged — it already is, confirming the defect is confined to refusal rendering.
