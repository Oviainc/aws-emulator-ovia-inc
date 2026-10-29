Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.delete-last-user-pool-client-secret-rejected cognito-idp.limit-exceeded-client-secret-delete cognito-idp.user-pool-client-secret-lifecycle cognito-idp.delete-user-pool-client-secret-nonexistent-id-rejected cognito-idp.delete-user-pool-client-secret-when-absent-fails cognito-idp.user-pool-client-lifecycle-and-secret
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.delete-last-user-pool-client-secret-rejected|run#10|AssertionError|assert message == "Cannot delete the only client secret.", f"unexpected message {message!r}"
Issue-Key: cognito-idp.limit-exceeded-client-secret-delete|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.user-pool-client-secret-lifecycle|run#30|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.delete-user-pool-client-secret-nonexistent-id-rejected|run#14|AssertionError|assert message == "Client secret does not exist", f"unexpected message {message!r}"
Issue-Key: cognito-idp.delete-user-pool-client-secret-when-absent-fails|run#20|AssertionError|assert message == "Client secret does not exist", f"unexpected message {message!r}"
Issue-Key: cognito-idp.user-pool-client-lifecycle-and-secret|run#23|AssertionError|assert secret_delete_again_status == 400, (
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# cognito-idp: DeleteUserPoolClientSecret refusals carry emulator-invented messages and serve 404 instead of 400 (3 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteUserPoolClientSecret

## Description
This bundle has 3 causes; all 3 must be closed before submitting. All three are in the app-client secret rotation surface (AddUserPoolClientSecret / ListUserPoolClientSecrets / DeleteUserPoolClientSecret). In every one of the six grounding checks the emulator reached the right *decision* and the right error *class* — it refuses what AWS refuses, and the secret set is left untouched — so what diverges is purely how the refusal is rendered: its message text and, in one case, its HTTP status.

Related open issues that do NOT cover these checks: 20261029T182116Z-cognito-idp-not-found-and-duplicate-resource-error.md and 20261029T184956Z-cognito-idp-ten-not-found-refusal-messages-diverge.md own other operations' not-found wording (e.g. DescribeUserPoolClient's 'User pool client does not exist.'), and 20261029T205621Z-cognito-idp-app-client-group-resource-server-refusals.md owns the 404->400 status defect for ListUserPoolClients / AddUserPoolClientSecret / CreateUserPoolClient / CreateGroup / resource-server and identity-provider reads. None of them carries a DeleteUserPoolClientSecret replay key, and the suite marks none of these six checks as owned.

Cause 1 of 3: the last-remaining-secret refusal message is emulator prose.
AWS: an app client must always retain at least one active secret, so DeleteUserPoolClientSecret aimed at a client's sole remaining ClientSecretId is refused with InvalidParameterException and the message 'Cannot delete the only client secret.' That string is entirely fixed text — it has no variable parts, it interpolates neither the client id nor the secret id. Smithy: DeleteUserPoolClientSecret declares InvalidParameterException (errorKind client, @httpError 400); the model does not document the message, so the wording is recorded behavior only, and the recorded refusal is a 400 InvalidParameterException.
Emulator: the error class is right (InvalidParameterException), but the message is 'Cannot delete the last remaining client secret of user pool client <ClientId>.' — different fixed text plus an interpolated ClientId taken from the request, and a trailing period on a sentence AWS words differently.
Why: callers that rotate client secrets branch on this refusal to decide 'add a second secret first, then retry the delete'. Operators and tests that match the documented sentence — and the many SDK/CLI wrappers that surface Message verbatim in logs and alarms — never match it here, and because the message embeds a per-client id it is not even stable across clients, so no equality or hash-based matcher can be written against it.
Fix: the DeleteUserPoolClientSecret handler's last-secret precondition branch in the cognito-idp service crate (the ops/delete_user_pool_client_secret path) must emit the fixed sentence with no interpolation.
Traces: cognito-idp.delete-last-user-pool-client-secret-rejected, cognito-idp.limit-exceeded-client-secret-delete, cognito-idp.user-pool-client-secret-lifecycle.

Cause 2 of 3: the unknown-ClientSecretId refusal message is emulator prose.
AWS: DeleteUserPoolClientSecret given a ClientSecretId that was never issued for this client — or one that was issued and has already been deleted — is refused with ResourceNotFoundException and the message 'Client secret does not exist'. The whole string is fixed text: there is no interpolated secret id and, notably, no trailing period. Smithy: DeleteUserPoolClientSecret declares ResourceNotFoundException (errorKind client); the message is recorded behavior only.
Emulator: the error class is right (ResourceNotFoundException), but the message is 'Client secret <ClientSecretId> does not exist.' — it interpolates the ClientSecretId from the request and appends a period. Observed with all three shapes of bad id the checks use: an arbitrary 32-hex id, a random UUID, and a previously-valid id that was just deleted.
Why: same class of breakage as cause 1, and here it also leaks the request's secret identifier back into an error string, which is exactly the material AWS keeps out of this message. Clients distinguishing 'unknown secret' from 'last secret' by message text see neither AWS sentence.
Fix: the not-found branch of the same DeleteUserPoolClientSecret handler / its validation step — look up the secret, and on miss raise ResourceNotFoundException with the fixed, non-interpolated sentence.
Traces: cognito-idp.delete-user-pool-client-secret-nonexistent-id-rejected, cognito-idp.delete-user-pool-client-secret-when-absent-fails, cognito-idp.user-pool-client-secret-lifecycle, cognito-idp.user-pool-client-lifecycle-and-secret.

Cause 3 of 3: that ResourceNotFoundException is served as HTTP 404 where AWS answers HTTP 400.
AWS: cognito-idp answers a missing-resource refusal on DeleteUserPoolClientSecret with HTTP status 400 and ResourceNotFoundException in the awsJson1_1 body's __type. Smithy: ResourceNotFoundException is modeled @httpError(404) on this operation, but the recorded service does not apply that trait — recorded behavior only: the refusal arrives as 400.
Emulator: honors @httpError(404) and serves HTTP 404 for the same refusal (error class and body __type are correct).
Why: a 404 from an AWS JSON-RPC endpoint conventionally means 'no such route/operation', so SDK error classifiers, API gateways and reverse proxies in front of the emulator treat a modeled ResourceNotFoundException as a transport or routing failure instead of an ordinary 400 client error — retry and alarm logic misfires, and secret-rotation code that branches on the modeled error never sees it.
Fix: the cognito-idp awsJson1_1 error serializer / error-to-status mapping in the service crate — pin client errors to 400 rather than reading @httpError off the error shape — the same mapping named by 20261029T205621Z-cognito-idp-app-client-group-resource-server-refusals.md, extended to cover DeleteUserPoolClientSecret.
Traces: cognito-idp.user-pool-client-lifecycle-and-secret.

## Evidence
Cause 1 — expected (AWS): InvalidParameterException, Message exactly 'Cannot delete the only client secret.' (fixed text; no variable parts).
Actual (emulator), cognito-idp.delete-last-user-pool-client-secret-rejected: AssertionError: unexpected message 'Cannot delete the last remaining client secret of user pool client efc84b713e3d41e2b7d89c6afc.'
Actual (emulator), cognito-idp.limit-exceeded-client-secret-delete: AssertionError: expected message 'Cannot delete the only client secret.', got 'Cannot delete the last remaining client secret of user pool client 28f06c4318cb4e91a3ec81e426.'
The quoted client ids (efc84b71..., 28f06c43...) are per-run values from the request, not part of the requirement; the requirement is the fixed AWS sentence with nothing interpolated.

Cause 2 — expected (AWS): ResourceNotFoundException, Message exactly 'Client secret does not exist' (fixed text, no trailing period, no secret id).
Actual (emulator), cognito-idp.delete-user-pool-client-secret-nonexistent-id-rejected: AssertionError: unexpected message 'Client secret 0123456789abcdef0123456789abcdef does not exist.'
Actual (emulator), cognito-idp.delete-user-pool-client-secret-when-absent-fails: AssertionError: unexpected message 'Client secret 0e042c11-84c3-4b28-a91a-831ab2bbb450 does not exist.'
Actual (emulator), cognito-idp.user-pool-client-secret-lifecycle: AssertionError: expected message 'Client secret does not exist', got 'Client secret d02ac0165ab94eb3865114e8aa--1793312286552 does not exist.'
The quoted secret ids are the values the request supplied (or a previously issued id) — variable fill, not requirement text. The divergence is the added interpolation and the added period.

Cause 3 — expected (AWS): HTTP 400 with ResourceNotFoundException for DeleteUserPoolClientSecret on an already-deleted secret.
Actual (emulator), cognito-idp.user-pool-client-lifecycle-and-secret: AssertionError: deleting an already-deleted secret: expected HTTP 400, got 404. Error class and body __type were correct; only the status diverged.
Smithy (DeleteUserPoolClientSecret): errors InvalidParameterException (client, @httpError 400), LimitExceededException (client, 400), ResourceNotFoundException (client, @httpError 404 — not applied by the recorded service), TooManyRequestsException (429), InternalServerException (500).

Effect-absence held everywhere: in all six checks the rejected delete left the client's secret set unchanged, and each check's in-file positive control (deleting a non-last, genuinely issued secret) succeeded — so DeleteUserPoolClientSecret is not broken for every call; only the refusal rendering diverges.

## Steps to Reproduce
1. CreateUserPool, then CreateUserPoolClient with GenerateSecret=True. ListUserPoolClientSecrets shows exactly one secret.
2. Call DeleteUserPoolClientSecret with that sole ClientSecretId. Emulator refuses with InvalidParameterException but the message reads 'Cannot delete the last remaining client secret of user pool client <ClientId>.'; AWS's message is the fixed sentence 'Cannot delete the only client secret.' (Cause 1 — replay cognito-idp.delete-last-user-pool-client-secret-rejected and cognito-idp.limit-exceeded-client-secret-delete.)
3. Call AddUserPoolClientSecret to rotate in a second secret (positive control: the identical delete from step 2 now succeeds and the secret disappears from ListUserPoolClientSecrets).
4. Call DeleteUserPoolClientSecret with a ClientSecretId that was never issued for this client (e.g. an arbitrary 32-hex string or a random UUID), and again with the id just deleted in step 3. Emulator refuses with ResourceNotFoundException but the message reads 'Client secret <ClientSecretId> does not exist.'; AWS's message is the fixed 'Client secret does not exist' with no id and no trailing period. (Cause 2 — replay cognito-idp.delete-user-pool-client-secret-nonexistent-id-rejected, cognito-idp.delete-user-pool-client-secret-when-absent-fails, cognito-idp.user-pool-client-secret-lifecycle.)
5. Inspect ResponseMetadata.HTTPStatusCode on the refusal from step 4: the emulator serves 404; AWS serves 400 with the same ResourceNotFoundException body. (Cause 3 — replay cognito-idp.user-pool-client-lifecycle-and-secret.)
6. After each refusal, re-read ListUserPoolClientSecrets: the secret set must be unchanged — it already is, confirming the defect is confined to refusal rendering.
