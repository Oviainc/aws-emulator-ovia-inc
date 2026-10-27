Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.terms-lifecycle cognito-idp.describe-terms-by-client-server-side-consistency-with-create
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.terms-lifecycle|run#20|AssertionError|assert message == "Terms does not exist.", (
Issue-Key: cognito-idp.describe-terms-by-client-server-side-consistency-with-create|run#17|AssertionError|assert message_after_delete == "Terms does not exist.", (
Issue-Commit: 840a27c480269ac38c11f96ab699adf96523de04

# cognito-idp: the Terms not-found refusal message is emulator-authored and request-derived — DescribeTerms and DescribeTermsByClient diverge from AWS's constant "Terms does not exist." (2 causes)

> _Note: this issue was identified against the code base at commit `840a27c480269ac38c11f96ab699adf96523de04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTerms

## Description
Two net-new root causes, both in the cognito-idp Terms read plane. In both the HTTP status (404) and the error code (ResourceNotFoundException) are already correct — the divergence is purely the Error.Message, which the emulator composes from the request instead of emitting AWS's constant text. Neither cause is covered by the already-resolved Terms issues (20261025T114821Z-cognito-idp-describetermsbyclient-answers-notimplemented-instead-of-resolving.md and the CreateTerms/ListTerms/UpdateTerms/DeleteTerms stub issues): those were about the operations being refused NotImplemented or about the cross-pool ClientId scoping rule, and all of those paths now work — the calls here succeed in reaching the modelled ResourceNotFoundException and only the message text is wrong. This is NOT a reopen: nothing here is left refused that should succeed.

Cause 1 of 2 — DescribeTerms on a deleted TermsId. Failing check id: cognito-idp.terms-lifecycle.
AWS: Smithy (cognito-idp) models DescribeTerms with required input members TermsId (TermsIdType, @pattern ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$) and UserPoolId (UserPoolIdType, @length 1-55, @pattern ^[\w-]+_[0-9a-zA-Z]+$), output member Terms (TermsType), and modelled errors InvalidParameterException (400), NotAuthorizedException (403), OperationNotEnabledException (400), ResourceNotFoundException (404), TooManyRequestsException (429), InternalErrorException (500). The model does not state message text, so: recorded behavior only — for a TermsId that does not resolve in the named user pool (here: one that DeleteTerms has removed), AWS answers HTTP 404, Error.Code `ResourceNotFoundException`, Error.Message `Terms does not exist.` That message is ENTIRELY fixed text: it is a constant sentence with NO variable parts — it does not interpolate the TermsId, the TermsName, the UserPoolId or any other request or state value. The recording also requires that after the delete the document is gone from every view (ListTerms for that pool no longer reports the TermsId).
Emulator: the same call answers 404 ResourceNotFoundException but with the request-derived message `Terms documents <TermsId> do not exist.` — i.e. a pluralised template that interpolates the TermsId from the request (the replay shows `Terms documents 30c33ccc-a1db-4813-83be-ea89c3f3f2f4 do not exist.`; that UUID is a per-run generated id filling the emulator's template, not part of any requirement). Replay frame: check_cognito_idp_terms_lifecycle.py line 70 — `assert message == "Terms does not exist."` -> `AssertionError: expected the exact DescribeTerms not-found message, got 'Terms documents 30c33ccc-a1db-4813-83be-ea89c3f3f2f4 do not exist.'`.
Why: a client that matches on the refusal message — error-classification middleware, log/alarm patterns, idempotent "already deleted" tolerance that keys off the exact AWS sentence, or a test fixture pinning it — does not match the emulator's text, so a deleted-Terms refusal is reported as an unrecognised failure instead of the benign not-found it is. Worse, the emulator's message echoes the requested TermsId into the error, giving the message a per-request shape where AWS's is constant, so anything that groups or dedupes errors by message sees one distinct error per TermsId.
Fix: the DescribeTerms handler's not-found path in the cognito-idp service (the error construction for ResourceNotFoundException when the (UserPoolId, TermsId) lookup misses) must emit the constant `Terms does not exist.` and interpolate nothing.
Traces: cognito-idp.terms-lifecycle (the DescribeTerms-after-DeleteTerms assertion).

Cause 2 of 2 — DescribeTermsByClient after the Terms document is deleted. Failing check id: cognito-idp.describe-terms-by-client-server-side-consistency-with-create.
AWS: Smithy (cognito-idp) models DescribeTermsByClient with required input members ClientId (ClientIdType, @length 1-128, @pattern ^[\w+]+$, @sensitive), UserPoolId (UserPoolIdType, @length 1-55, @pattern ^[\w-]+_[0-9a-zA-Z]+$) and TermsName (TermsNameType, @pattern ^(terms-of-use|privacy-policy)$), output member Terms (TermsType), and the same modelled error set including ResourceNotFoundException (404). Message text is not modelled, so: recorded behavior only — when the client-scoped lookup resolves no terms document (here: the document was removed by DeleteTerms while the pool and app client both still exist), AWS answers HTTP 404, Error.Code `ResourceNotFoundException`, Error.Message `Terms does not exist.` — again wholly fixed text with NO variable parts: it names neither the TermsName nor the ClientId. Note this is a DIFFERENT error class from the cross-pool ClientId rejection already covered by the resolved issue 20261025T114821Z-...-describetermsbyclient-answers-notimplemented-..., whose message DOES carry request-derived parts; the "no such terms document" class does not.
Emulator: the same call answers 404 ResourceNotFoundException with the request-derived message `Terms with name <TermsName> does not exist for Client <ClientId>.` — a template interpolating the TermsName and the ClientId from the request (the replay shows `Terms with name terms-of-use does not exist for Client ff7b150005924dcebebdf41f75.`; `terms-of-use` and that client id are the per-run request values filling the template, not requirements). It also leaks the @sensitive ClientId into an error message AWS keeps free of it. Replay frame: check_cognito_idp_describe_terms_by_client_server_side_consistency_with_create.py line 71 — `assert message_after_delete == "Terms does not exist."` -> `AssertionError: expected the exact ResourceNotFoundException message 'Terms does not exist.', got 'Terms with name terms-of-use does not exist for Client ff7b150005924dcebebdf41f75.'`.
Why: the two Terms reads disagree with each other AND with AWS. A caller that deletes a terms document and then reads it back by client cannot match the refusal against AWS's constant sentence, so the expected post-delete 404 is surfaced as an unknown error; and because the emulator's text embeds a @sensitive ClientId, the refusal message carries identity material AWS deliberately omits, which propagates the client id into logs that only ever recorded a constant string against real AWS.
Fix: the DescribeTermsByClient handler's not-found path in the cognito-idp service (the ResourceNotFoundException raised when no terms document resolves for the (UserPoolId, ClientId, TermsName) triple) must emit the constant `Terms does not exist.` and interpolate nothing. The two handlers should share one not-found error constructor so the Terms read plane cannot drift again; the cross-pool/app-client-not-in-pool refusal keeps its own, distinct request-derived message.
Traces: cognito-idp.describe-terms-by-client-server-side-consistency-with-create (the DescribeTermsByClient-after-DeleteTerms assertion).

## Evidence
Cause 1 — check cognito-idp.terms-lifecycle, verdict mismatch.
Frame: check_cognito_idp_terms_lifecycle.py line 70 — `assert message == "Terms does not exist."`.
Expected (AWS, recorded): DescribeTerms(UserPoolId=<pool>, TermsId=<deleted id>) -> HTTP 404, Error.Code `ResourceNotFoundException`, Error.Message `Terms does not exist.` (entire message is fixed text; no variable parts).
Actual (emulator): HTTP 404, Error.Code `ResourceNotFoundException`, Error.Message `Terms documents <TermsId> do not exist.` — observed as `Terms documents 30c33ccc-a1db-4813-83be-ea89c3f3f2f4 do not exist.`; fixed text `Terms documents ` + ` do not exist.`, variable part = the TermsId from the request.
AssertionError: expected the exact DescribeTerms not-found message, got 'Terms documents 30c33ccc-a1db-4813-83be-ea89c3f3f2f4 do not exist.'

Cause 2 — check cognito-idp.describe-terms-by-client-server-side-consistency-with-create, verdict mismatch.
Frame: check_cognito_idp_describe_terms_by_client_server_side_consistency_with_create.py line 71 — `assert message_after_delete == "Terms does not exist."`.
Expected (AWS, recorded): DescribeTermsByClient(UserPoolId=<pool>, ClientId=<client of that pool>, TermsName='terms-of-use') after DeleteTerms -> HTTP 404, Error.Code `ResourceNotFoundException`, Error.Message `Terms does not exist.` (entire message is fixed text; neither TermsName nor ClientId appears).
Actual (emulator): HTTP 404, Error.Code `ResourceNotFoundException`, Error.Message `Terms with name <TermsName> does not exist for Client <ClientId>.` — observed as `Terms with name terms-of-use does not exist for Client ff7b150005924dcebebdf41f75.`; fixed text `Terms with name `, ` does not exist for Client ` and the trailing `.`, variable parts = the TermsName and the @sensitive ClientId from the request.
AssertionError: expected the exact ResourceNotFoundException message 'Terms does not exist.', got 'Terms with name terms-of-use does not exist for Client ff7b150005924dcebebdf41f75.'

In both causes the status code and the error code already match AWS; only Error.Message diverges, and it diverges by interpolating request values into a message AWS emits as a constant.

## Steps to Reproduce
Cause 1 (DescribeTerms):
1. cognito-idp:CreateUserPool -> pool.
2. cognito-idp:CreateUserPoolClient(UserPoolId=pool) -> clientId.
3. cognito-idp:CreateTerms(UserPoolId=pool, ClientId=clientId, TermsName='terms-of-use', TermsSource='LINK', Enforcement='NONE', Links={'cognito:default': 'https://example.com/terms'}) -> note Terms.TermsId.
4. cognito-idp:DescribeTerms / ListTerms / DescribeTermsByClient / UpdateTerms all resolve the document (these pass today).
5. cognito-idp:DeleteTerms(UserPoolId=pool, TermsId=<TermsId>).
6. cognito-idp:DescribeTerms(UserPoolId=pool, TermsId=<TermsId>) — AWS: 404 ResourceNotFoundException, message exactly `Terms does not exist.`; emulator: 404 ResourceNotFoundException, message `Terms documents <TermsId> do not exist.`

Cause 2 (DescribeTermsByClient):
1. cognito-idp:CreateUserPool -> pool.
2. cognito-idp:CreateUserPoolClient(UserPoolId=pool) -> clientId.
3. cognito-idp:CreateTerms(UserPoolId=pool, ClientId=clientId, TermsName='terms-of-use', TermsSource='LINK', Enforcement='NONE', Links={'cognito:default': 'https://example.com/terms-of-use'}) -> note Terms.TermsId.
4. cognito-idp:DescribeTermsByClient(UserPoolId=pool, ClientId=clientId, TermsName='terms-of-use') twice — both resolve identically (passes today).
5. cognito-idp:DeleteTerms(UserPoolId=pool, TermsId=<TermsId>).
6. cognito-idp:DescribeTermsByClient(UserPoolId=pool, ClientId=clientId, TermsName='terms-of-use') — AWS: 404 ResourceNotFoundException, message exactly `Terms does not exist.`; emulator: 404 ResourceNotFoundException, message `Terms with name terms-of-use does not exist for Client <clientId>.`

Re-replay to confirm a fix: cognito-idp.terms-lifecycle and cognito-idp.describe-terms-by-client-server-side-consistency-with-create.
