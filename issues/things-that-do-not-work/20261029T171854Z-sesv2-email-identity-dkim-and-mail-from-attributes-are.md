Issue-Source: trace-sweep-divergence
Issue-Trace: email-email-identity-verification-and-dkim-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-email-identity-verification-and-dkim-lifecycle email-identity-resource-policy-and-tagging email-tenant-creation-and-resource-association sesv2-disassociate-without-prior-association sesv2-identity-certificate-association-lifecycle sesv2-email-identity-create-with-tags
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|CreateEmailIdentity|2xx->2xx|$.DkimAttributes.NextSigningKeyLength
Issue-Key: sesv2|CreateEmailIdentity|2xx->2xx|$.DkimAttributes.SigningHostedZone
Issue-Key: sesv2|CreateEmailIdentity|2xx->2xx|$.DkimAttributes.Status
Issue-Key: sesv2|CreateEmailIdentity|4xx->4xx|$.message
Issue-Key: sesv2|GetEmailIdentity|2xx->2xx|$.DkimAttributes.NextSigningKeyLength
Issue-Key: sesv2|GetEmailIdentity|2xx->2xx|$.DkimAttributes.SigningHostedZone
Issue-Key: sesv2|GetEmailIdentity|2xx->2xx|$.MailFromAttributes
Issue-Key: sesv2|GetEmailIdentity|2xx->2xx|$.VerificationInfo.ErrorType
Issue-Key: sesv2|GetEmailIdentity|2xx->2xx|$headers.content-type
Issue-Key: sesv2|PutEmailIdentityMailFromAttributes|4xx->4xx|$.message
Issue-Key: sesv2|PutEmailIdentityMailFromAttributes|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sesv2|PutEmailIdentityMailFromAttributes|4xx->4xx|status
Issue-Key: sesv2|DeleteEmailIdentity|2xx->2xx|$headers.content-type
Issue-Key: sesv2|GetEmailIdentity|4xx->4xx|$headers.content-type
Issue-Key: sesv2|CreateEmailIdentity|2xx->2xx|$.DkimAttributes
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Email-identity DKIM and MAIL FROM attributes are wrong or missing on read

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEmailIdentity

## Description
Eight net-new causes in the SESv2 email-identity plane. Smithy: DkimAttributes {SigningEnabled, Status (DkimStatus incl. NOT_STARTED/PENDING), Tokens, SigningHostedZone, SigningAttributesOrigin, NextSigningKeyLength, CurrentSigningKeyLength, LastKeyGenerationTimestamp}; PutEmailIdentityMailFromAttributes errors BadRequestException(400)/NotFoundException(404)/TooManyRequestsException(429).

Cause 1 of 8: CreateEmailIdentity returns an incomplete DkimAttributes for a DOMAIN identity. AWS: the create response's DkimAttributes reports Status NOT_STARTED (the DKIM verification process has not been initiated yet), NextSigningKeyLength RSA_2048_BIT (the Easy-DKIM default key length) and SigningHostedZone set to the SES DKIM signing hosted zone for the region. Emulator: Status PENDING, and NextSigningKeyLength and SigningHostedZone are absent. Why: a client that waits for the documented NOT_STARTED -> PENDING -> SUCCESS progression starts in the wrong state, and one that must publish CNAME records cannot, because the hosted zone it has to point at is not reported. Fix: the CreateEmailIdentity handler's DKIM attribute construction (initial status, default next key length, regional signing hosted zone). Traces: email-email-identity-verification-and-dkim-lifecycle#task#2 (same cluster also at email-identity-resource-policy-and-tagging#task#1, email-tenant-creation-and-resource-association#setup#1, sesv2-disassociate-without-prior-association#setup#1, sesv2-identity-certificate-association-lifecycle#setup#1).

Cause 2 of 8: CreateEmailIdentity duplicate refusal message. AWS: 400 AlreadyExistsException whose sentence ends 'already exist.' — the fixed text is 'Email identity ' + identity + ' already exist.' (AWS's wording is not pluralised); the variable part is the EmailIdentity from the request. Emulator: 'Email identity <identity> already exists.'. Why: a client matching AWS's sentence does not recognise the duplicate-identity refusal. Fix: the CreateEmailIdentity handler's duplicate refusal message. Traces: email-email-identity-verification-and-dkim-lifecycle#task#3.

Cause 3 of 8: GetEmailIdentity omits MailFromAttributes and invents VerificationInfo.ErrorType. AWS: a domain identity's read returns MailFromAttributes with BehaviorOnMxFailure USE_DEFAULT_VALUE (and MailFromDomain/MailFromDomainStatus unset) even before any MAIL FROM domain is configured, returns the DKIM NextSigningKeyLength/SigningHostedZone of Cause 1, and OMITS VerificationInfo.ErrorType while verification has not failed. Emulator: MailFromAttributes absent, the two DKIM members absent, and VerificationInfo.ErrorType set to TYPE_NOT_FOUND although nothing failed; the response also carries the wrong content-type (application/json instead of application/x-amz-json-1.1 — recorded behavior only). Why: a client reading the identity sees a verification error that does not exist and cannot read the default MX-failure behavior. Fix: the GetEmailIdentity serializer and the identity state's MAIL FROM/verification defaults. Traces: email-email-identity-verification-and-dkim-lifecycle#task#4.

Cause 4 of 8: the same read defect after the DKIM signing key length is changed. AWS: GetEmailIdentity reports the configured NextSigningKeyLength (RSA_1024_BIT here) and the SigningHostedZone, and no VerificationInfo.ErrorType. Emulator: both DKIM members absent and ErrorType TYPE_NOT_FOUND present. Why: a client cannot confirm its own PutEmailIdentityDkimSigningAttributes change. Fix: as Cause 3, plus persisting the requested key length. Traces: email-email-identity-verification-and-dkim-lifecycle#task#10.

Cause 5 of 8: PutEmailIdentityMailFromAttributes on an unknown identity answers the wrong error. AWS: 400 BadRequestException (x-amzn-errortype: BadRequestException) with 'Identity <IDENTITY> does not exist.' — the fixed text is 'Identity ' + '<' + identity + '>' + ' does not exist.', the variable part being the EmailIdentity path label. Emulator: 404 NotFoundException with 'Email identity <identity> does not exist.' (no angle brackets). Why: a client branching on the HTTP status or the modeled error type takes the not-found branch where AWS signals a bad request, so retry/validation handling diverges. Fix: the PutEmailIdentityMailFromAttributes handler's unknown-identity refusal (error class and message). Traces: email-email-identity-verification-and-dkim-lifecycle#task#12.

Cause 6 of 8: content-type on the successful DeleteEmailIdentity response (200): application/json instead of application/x-amz-json-1.1. Fix: the service's response content-type default. Traces: email-email-identity-verification-and-dkim-lifecycle#task#13.

Cause 7 of 8: same content-type defect on the GetEmailIdentity 404 (NotFoundException) response. Traces: email-email-identity-verification-and-dkim-lifecycle#task#14.

Cause 8 of 8: CreateEmailIdentity returns DkimAttributes for an EMAIL-ADDRESS identity. AWS: creating an email-address identity returns no DkimAttributes at all (DKIM applies to domain identities). Emulator: returns DkimAttributes {SigningEnabled: false, Status: NOT_STARTED, SigningAttributesOrigin: AWS_SES}. Why: a client that treats the presence of DkimAttributes as 'this identity supports DKIM' mis-classifies address identities. Fix: the CreateEmailIdentity handler must emit DkimAttributes only for domain identities. Traces: sesv2-email-identity-create-with-tags#setup#1.

No open issue covers these; 20261029T085623Z-sesv2-two-causes-the-dns-baseline-hosted-zone.md concerns route53 discoverability and feedback attributes.

## Evidence
Replay of trace set simparity-wire-aws-v1.
Cause 1 (task#2, 200/200): $.DkimAttributes.Status expected 'NOT_STARTED', got 'PENDING'; $.DkimAttributes.NextSigningKeyLength expected 'RSA_2048_BIT', got absent; $.DkimAttributes.SigningHostedZone expected 'dkim.amazonses.com', got absent (5 steps in cluster).
Cause 2 (task#3, 400/400): $.message expected 'Email identity sim2real-probe-sesv2-identity-2f7e3f4c.com already exist.', got '... already exists.'
Cause 3 (task#4, 200/200): $.MailFromAttributes expected {'BehaviorOnMxFailure': 'USE_DEFAULT_VALUE', 'MailFromDomain': None, 'MailFromDomainStatus': None}, got absent; $.DkimAttributes.NextSigningKeyLength/SigningHostedZone expected set, got absent; $.VerificationInfo.ErrorType expected absent, got 'TYPE_NOT_FOUND'; content-type application/json vs application/x-amz-json-1.1.
Cause 4 (task#10, 200/200): $.DkimAttributes.NextSigningKeyLength expected 'RSA_1024_BIT', got absent; SigningHostedZone expected 'dkim.amazonses.com', got absent; $.VerificationInfo.ErrorType expected absent, got 'TYPE_NOT_FOUND'.
Cause 5 (task#12): status expected 400, got 404; $headers.x-amzn-errortype expected 'BadRequestException', got 'NotFoundException'; $.message expected 'Identity <nonexistent-sim2real-probe-sesv2-id-2f7e3f4c.com> does not exist.', got 'Email identity nonexistent-sim2real-probe-sesv2-id-2f7e3f4c.com does not exist.'
Causes 6,7 (task#13 200/200, task#14 404/404): $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json'.
Cause 8 (sesv2-email-identity-create-with-tags#setup#1, 200/200): $.DkimAttributes expected absent, got {'SigningEnabled': False, 'Status': 'NOT_STARTED', 'SigningAttributesOrigin': 'AWS_SES'}.

## Steps to Reproduce
1. Replay fixture email-email-identity-verification-and-dkim-lifecycle (steps task#2, task#3, task#4, task#10, task#12, task#13, task#14) and fixture sesv2-email-identity-create-with-tags (step setup#1).
2. Cause 1/8: CreateEmailIdentity for a domain and for an email address; compare DkimAttributes in each response.
3. Cause 2: CreateEmailIdentity twice for the same domain; compare the 400 message.
4. Causes 3/4: GetEmailIdentity on the fresh domain identity, then after PutEmailIdentityDkimSigningAttributes with RSA_1024_BIT; compare MailFromAttributes, DkimAttributes.NextSigningKeyLength/SigningHostedZone and VerificationInfo.ErrorType.
5. Cause 5: PutEmailIdentityMailFromAttributes for an identity that does not exist; compare status, x-amzn-errortype and message.
6. Causes 6/7: inspect the content-type header on DeleteEmailIdentity and on GetEmailIdentity for a deleted identity.
