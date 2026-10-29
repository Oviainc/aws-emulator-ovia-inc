Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-terms-documents-error-paths
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-terms-documents-error-paths cognito-idp-terms-not-found-errors cognito-idp-terms-errors cognito-idp-terms-lifecycle-create-describe-update-delete cognito-idp-terms-two-types-per-client cognito-idp-managed-login-branding-error-paths cognito-idp-webauthn-invalid-token-errors
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|DeleteTerms|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DeleteTerms|4xx->4xx|status
Issue-Key: cognito-idp|UpdateTerms|4xx->4xx|status
Issue-Key: cognito-idp|DescribeTermsByClient|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeTermsByClient|4xx->4xx|status
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|4xx->4xx|status
Issue-Key: cognito-idp|UpdateManagedLoginBranding|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateManagedLoginBranding|4xx->4xx|status
Issue-Key: cognito-idp|StartWebAuthnRegistration|4xx->4xx|status
Issue-Key: cognito-idp|CompleteWebAuthnRegistration|4xx->4xx|status
Issue-Related: 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md
Issue-Related: 20261029T184956Z-cognito-idp-seven-refusals-are-raised-as-the.md
Issue-Related: 20261029T184956Z-cognito-idp-validation-and-auth-refusal-message-rendering.md
Issue-Commit: 4bd656ceb6ae5ffdb198db72ede94ac7bf108594

# cognito-idp terms branding and WebAuthn refusals serve 404/403 instead of 400

> _Note: this issue was identified against the code base at commit `4bd656ceb6ae5ffdb198db72ede94ac7bf108594`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTerms

## Description
This bundle has 7 causes; all 7 must be closed before submitting.

AWS: the terms-document, managed-login-branding and WebAuthn operations answer their client-error classes with HTTP status 400 and the class in the body's `__type`, plus `x-amzn-errortype` holding the error shape name followed by a colon. Smithy: ResourceNotFoundException @httpError(404) and NotAuthorizedException @httpError(403) — the recorded service does NOT apply those traits; recorded behavior only: every one of these refusals is status 400. The message bodies matched on all seven steps.

Emulator: the emulator honors @httpError, so terms/branding not-found refusals are 404 and WebAuthn invalid-access-token refusals are 403, and none of the responses carries `x-amzn-errortype`.

Why: a 404 is read by SDK error classifiers and by proxies as a missing route rather than a modeled ResourceNotFoundException, so terms/branding reconciliation code that tolerates a missing document against AWS aborts here. A 403 on StartWebAuthnRegistration/CompleteWebAuthnRegistration is indistinguishable from a SigV4/credential failure, so a client re-signs or refreshes credentials instead of prompting the user to re-authenticate — the registration ceremony then stalls rather than restarting.

Fix: the cognito-idp awsJson1_1 error serializer / error-to-status mapping in the service crate: pin client errors to 400 instead of reading @httpError off the error shape, and emit `x-amzn-errortype: <ErrorShapeName>:` from the same serializer.

Related open issues that do NOT cover these steps: 20261029T184956Z-cognito-idp-modeled-httperror-codes-are-served-where.md (covers DescribeTerms, DescribeManagedLoginBranding, ListWebAuthnCredentials — not the operations below) and 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md.

Cause 1 of 7: DeleteTerms for a terms id that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-terms-documents-error-paths, step task#7 (2 recorded steps in this cluster).
Cause 2 of 7: UpdateTerms for a terms id that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-terms-errors, step task#6 (3 recorded steps).
Cause 3 of 7: DescribeTermsByClient for a client with no terms document of that type answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-terms-not-found-errors, step task#4 (3 recorded steps).
Cause 4 of 7: DescribeManagedLoginBrandingByClient for a client with no branding answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-managed-login-branding-error-paths, step task#7.
Cause 5 of 7: UpdateManagedLoginBranding for a branding id that does not exist answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-managed-login-branding-error-paths, step task#8.
Cause 6 of 7: StartWebAuthnRegistration with an access token that is not valid answers 403; AWS answers 400 NotAuthorizedException. Fixture cognito-idp-webauthn-invalid-token-errors, step task#2.
Cause 7 of 7: CompleteWebAuthnRegistration with an access token that is not valid answers 403; AWS answers 400 NotAuthorizedException. Fixture cognito-idp-webauthn-invalid-token-errors, step task#3.

## Evidence
Expected (recorded AWS): HTTP 400 with `__type` naming the error class and `x-amzn-errortype: ResourceNotFoundException:` / `NotAuthorizedException:`.
Actual (emulator):
- DeleteTerms / UpdateTerms / DescribeTermsByClient: `status: expected 400, got 404`; `x-amzn-errortype` missing (expected 'ResourceNotFoundException:')
- DescribeManagedLoginBrandingByClient / UpdateManagedLoginBranding: `status: expected 400, got 404`; header missing (expected 'ResourceNotFoundException:')
- StartWebAuthnRegistration / CompleteWebAuthnRegistration: `status: expected 400, got 403`; header missing (expected 'NotAuthorizedException:')
Smithy: ResourceNotFoundException @httpError(404), NotAuthorizedException @httpError(403) — not honored by the recorded service.
Fixed vs variable: the 400 status and error class name are the requirement; the terms id, branding id, client id and pool id in the message are request/state values.

## Steps to Reproduce
1. Replay cognito-idp-terms-documents-error-paths step task#7 (DeleteTerms for an unknown terms id): emulator 404, AWS 400 ResourceNotFoundException.
2. Replay cognito-idp-webauthn-invalid-token-errors step task#2 (StartWebAuthnRegistration with a bogus access token): emulator 403, AWS 400 NotAuthorizedException.
3. Inspect the response headers on both: `x-amzn-errortype` absent.
4. Repeat for each fixture#step listed under the causes.
