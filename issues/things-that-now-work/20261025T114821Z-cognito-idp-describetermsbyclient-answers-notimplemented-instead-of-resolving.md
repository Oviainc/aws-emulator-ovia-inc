Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.describe-terms-by-client-mismatched-client-id-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.describe-terms-by-client-mismatched-client-id-rejected|run#11|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Commit: 949cf2f2a2f42313b725a3f96914da0fa4cf14da

# cognito-idp DescribeTermsByClient answers NotImplemented instead of resolving the terms document, so the cross-pool ClientId rejection (ResourceNotFoundException) never happens (1 cause)

> _Note: this issue was identified against the code base at commit `949cf2f2a2f42313b725a3f96914da0fa4cf14da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTermsByClient

## Description
Cause 1 of 1: DescribeTermsByClient is refused with the non-modeled error `NotImplemented` for every request, so neither its success path nor its cross-pool rejection path exists. Failing check id: cognito-idp.describe-terms-by-client-mismatched-client-id-rejected.

AWS: Smithy (cognito-idp) models DescribeTermsByClient with required input members ClientId (ClientIdType, @length 1-128, @pattern ^[\w+]+$, @sensitive), UserPoolId (UserPoolIdType, @length 1-55, @pattern ^[\w-]+_[0-9a-zA-Z]+$) and TermsName (TermsNameType, @pattern ^(terms-of-use|privacy-policy)$); output member Terms (TermsType); modeled errors InvalidParameterException (400), NotAuthorizedException (403), OperationNotEnabledException (400), ResourceNotFoundException (404), TooManyRequestsException (429), InternalErrorException (500). `NotImplemented` is not among them. Two behaviors are required of the operation:
  (a) Resolution: for a UserPoolId/ClientId/TermsName triple that exists, it returns 200 with Terms whose TermsName is the requested name and whose ClientId is the ClientId given in the request.
  (b) Scoping: a ClientId that is a real app client but belongs to a DIFFERENT user pool than the one named by UserPoolId must be refused, not resolved against the named pool's terms. Recorded behavior for that class: HTTP 404, error code `ResourceNotFoundException`, message `User pool client <ClientId> does not exist in user pool <UserPoolId>.` — the fixed text is `User pool client `, ` does not exist in user pool ` and the trailing `.`; the two variable parts are the ClientId from the request and the UserPoolId from the request (never a value from one recording). No terms document in the named pool may be created, mutated or removed by the failed lookup (ListTerms on that pool must still report exactly the documents that were created).

Emulator: every DescribeTermsByClient call — the cross-pool one AND the in-pool positive control — is refused with error code `NotImplemented` and the message `operation DescribeTermsByClient is scaffolded but not implemented in this emulator`. The replay fails at the first scoping assertion: `AssertionError: expected ResourceNotFoundException, got NotImplemented`. Everything upstream in the check succeeds (CreateUserPool x2, CreateUserPoolClient x2, CreateTerms in pool A), so the pool/client/terms state the operation should read is present and only the read is refused. Note the related auto-filed scaffold-stub issue for this same operation and handler file (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/describe_terms_by_client.rs), reported as blocking 3 checks; that issue names the stub, while this one states the two behaviors the implementation has to satisfy, including the cross-pool refusal code and message. The now-fixed issue 20261024T114943Z-cognito-idp-createterms-stub.md made CreateTerms work, which is why the check reaches this call at all.

Why: a client that reads the terms a given app client must present gets an unmodeled `NotImplemented` error its SDK cannot map to any modeled DescribeTermsByClient error, so the call is neither a success nor a recognizable failure, and the terms-presentation path cannot be exercised at all. Once the operation is implemented without the pool/client scoping rule, the failure mode is worse than an error: a caller passing an app client from another pool would be answered with THIS pool's terms document, leaking the terms configuration of a pool the ClientId has no relationship to, where AWS answers 404 ResourceNotFoundException.

Fix: the DescribeTermsByClient handler in the cognito-idp service (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/describe_terms_by_client.rs) plus its lookup against user-pool/app-client/terms state: resolve the terms document by (UserPoolId, ClientId, TermsName) only when the app client is a member of the named pool, and raise the modeled ResourceNotFoundException with AWS's message template otherwise; the lookup must be read-only with respect to stored terms.

Traces: cognito-idp.describe-terms-by-client-mismatched-client-id-rejected.

## Evidence
Check cognito-idp.describe-terms-by-client-mismatched-client-id-rejected, verdict mismatch. Frame: check_cognito_idp_describe_terms_by_client_mismatched_client_id_rejected.py line 51 — `assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"` -> `AssertionError: expected ResourceNotFoundException, got NotImplemented`.

Expected (AWS): DescribeTermsByClient(UserPoolId=<pool A>, ClientId=<client of pool B>, TermsName='terms-of-use') -> HTTP 404, Error.Code 'ResourceNotFoundException', Error.Message 'User pool client <that ClientId> does not exist in user pool <pool A id>.'; then ListTerms(UserPoolId=<pool A>) still returns exactly one document named 'terms-of-use'; then the matching triple DescribeTermsByClient(UserPoolId=<pool A>, ClientId=<client of pool A>, TermsName='terms-of-use') -> 200 with Terms.TermsName='terms-of-use' and Terms.ClientId=<client of pool A>.

Actual (emulator): Error.Code 'NotImplemented' with message 'operation DescribeTermsByClient is scaffolded but not implemented in this emulator' for the cross-pool call (and, by the same stub, for the in-pool positive control, which the check never reaches). 'NotImplemented' is not a modeled error of DescribeTermsByClient.

## Steps to Reproduce
1. cognito-idp:CreateUserPool twice -> pool A and pool B.
2. cognito-idp:CreateUserPoolClient in pool A -> clientA; CreateUserPoolClient in pool B -> clientB.
3. cognito-idp:CreateTerms(UserPoolId=poolA, ClientId=clientA, TermsName='terms-of-use', TermsSource='LINK', Enforcement='NONE', Links={'cognito:default': 'https://example.com/terms'}).
4. cognito-idp:DescribeTermsByClient(UserPoolId=poolA, ClientId=clientB, TermsName='terms-of-use') — AWS answers 404 ResourceNotFoundException 'User pool client <clientB> does not exist in user pool <poolA>.'; the emulator answers error code NotImplemented.
5. cognito-idp:ListTerms(UserPoolId=poolA) — must still list exactly the one 'terms-of-use' document.
6. cognito-idp:DescribeTermsByClient(UserPoolId=poolA, ClientId=clientA, TermsName='terms-of-use') — AWS answers 200 with Terms.TermsName='terms-of-use' and Terms.ClientId=clientA; the emulator answers NotImplemented.
