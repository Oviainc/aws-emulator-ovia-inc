Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: socialmessaging.flow-id-pattern-violation-rejected socialmessaging.flow-categories-exceeds-max-rejected socialmessaging.template-definition-exceeds-max-bytes-rejected socialmessaging.meta-template-id-pattern-violation-rejected socialmessaging.phone-number-id-pattern-violation-rejected socialmessaging.kms-key-arn-pattern-violation-rejected socialmessaging.media-id-pattern-violation-rejected
Issue-Shape-Id: com.amazonaws.socialmessaging#SocialMessaging
Issue-Key: socialmessaging.flow-id-pattern-violation-rejected|run#8|AssertionError|assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
Issue-Key: socialmessaging.flow-categories-exceeds-max-rejected|run#8|AssertionError|assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
Issue-Key: socialmessaging.template-definition-exceeds-max-bytes-rejected|run#12|AssertionError|assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
Issue-Key: socialmessaging.meta-template-id-pattern-violation-rejected|run#8|AssertionError|assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
Issue-Key: socialmessaging.phone-number-id-pattern-violation-rejected|run#8|AssertionError|assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
Issue-Key: socialmessaging.kms-key-arn-pattern-violation-rejected|run#9|AssertionError|assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
Issue-Key: socialmessaging.media-id-pattern-violation-rejected|run#8|AssertionError|assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# socialmessaging: seven WhatsApp operations answer 501 NotImplemented instead of the resource-lookup outcome AWS returns (7 causes)

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWhatsAppFlow

## Description
Seven net-new root causes in the socialmessaging (SocialMessaging / WhatsApp) surface, found by replaying the behavioral suite simparity-behavioral-aws-v1. They share one fault family but each is a DIFFERENT operation handler and each must be closed separately: SEVEN causes must be closed before this issue is done. In every one of them the modeled input constraint IS enforced correctly at the boundary (the ValidationException half of each check passes), and then the handler body answers HTTP 501 with error code `NotImplemented` and message 'operation <Op> is scaffolded but not implemented in this emulator' (crate::scaffolded::Scaffolded::not_implemented) where AWS answers the outcome of the resource lookup. Each handler file still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker. Note that the one open socialmessaging issue (20261029T171221Z-social-messaging-whatsapp-wire-responses-diverge-from-recording.md) covers none of these seven: it is scoped to AssociateWhatsAppBusinessAccount, CreateWhatsAppDataset, DisassociateWhatsAppBusinessAccount and ListLinkedWhatsAppBusinessAccounts.

Cause 1 of 7 — GetWhatsAppFlow never performs the flow lookup.
AWS: Smithy: GetWhatsAppFlow, GET /v1/whatsapp/flow; input `id` (LinkedWhatsAppBusinessAccountId, len 1..115, pattern (^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$), @required) and `flowId` (MetaFlowId, len 1..100, pattern ^[0-9]+$, @required); modeled errors AccessDeniedByMetaException 403, DependencyException 502, InternalServiceException 500, InvalidParametersException 400, ResourceNotFoundException 404 (@error client, member `message`), ThrottledRequestException 429. Recorded behavior: a syntactically valid numeric flowId naming a WABA that is not linked to the calling account is refused 404 ResourceNotFoundException whose `message` is the wholly fixed text 'Id not found for account.' — no id, name or value from the request is interpolated into it. A flowId violating ^[0-9]+$ is refused earlier with 400 ValidationException whose text is the boundary template '1 validation error detected: Value \'<the request's flowId>\' at \'flowId\' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9]+' (fixed text; the quoted value and the member path are the request-derived slots) — that half already passes. For a WABA that IS linked and owns a flow, the same call must succeed and project the modeled output (flowId, flowName, flowStatus required).
Emulator: 501 NotImplemented for the valid-flowId call, so the operation never distinguishes 'not found' from 'not written'.
Why: a client branching on ResourceNotFoundException (404) to create-or-read a flow sees a 501 instead; SDK retry classifiers treat 501 as non-retryable but the error class and message a caller matches on are absent, and no flow can ever be read.
Fix: implement the GetWhatsAppFlow handler (its file is the only one to touch for the operation) against the service's WABA/flow state, returning ResourceNotFoundException with this operation's wording when the WABA (or flow) is not the calling account's.
Traces: socialmessaging.flow-id-pattern-violation-rejected

Cause 2 of 7 — CreateWhatsAppFlow never performs the WABA lookup.
AWS: Smithy: CreateWhatsAppFlow, POST /v1/whatsapp/flow/create; input `id` (@required, same WABA pattern), `flowName` (len 1..200, @required), `categories` (list<MetaFlowCategory>, @length 1..9, @required), optional flowJson/publish/cloneFlowId/endpointUri; modeled errors include ResourceNotFoundException 404. Recorded behavior: with exactly 9 categories — the documented maximum — the request passes structural validation and is refused 404 ResourceNotFoundException with the fixed message 'Id not found for account.'; with 10 categories it is refused 400 ValidationException '1 validation error detected: Value \'[<the request's category values, comma-separated>]\' at \'categories\' failed to satisfy constraint: Member must have length less than or equal to 9' (that half already passes).
Emulator: 501 NotImplemented for the 9-category call.
Why: the boundary is right at the limit but the operation itself does nothing, so a client cannot create a flow at all and cannot tell an unlinked WABA from an unimplemented API.
Fix: implement the CreateWhatsAppFlow handler, including the WABA-ownership gate that yields ResourceNotFoundException.
Traces: socialmessaging.flow-categories-exceeds-max-rejected

Cause 3 of 7 — CreateWhatsAppMessageTemplate never performs the WABA lookup.
AWS: Smithy: CreateWhatsAppMessageTemplate, POST /v1/whatsapp/template/put; input `templateDefinition` (blob, @length 1..6000, @required) and `id` (@required, WABA pattern); modeled errors include ResourceNotFoundException 404. Recorded behavior: a small well-formed templateDefinition against an unlinked WABA is refused 404 ResourceNotFoundException with the fixed message 'Id not found for account.'; a definition over 6000 bytes is refused 400 ValidationException '1 validation error detected: Value at \'templateDefinition\' failed to satisfy constraint: Member must have length less than or equal to 6000' (blob values are not echoed; that half already passes).
Emulator: 501 NotImplemented for the in-limit definition.
Why: template creation is unavailable and the 404 contract is unobservable.
Fix: implement the CreateWhatsAppMessageTemplate handler and its WABA-ownership refusal.
Traces: socialmessaging.template-definition-exceeds-max-bytes-rejected

Cause 4 of 7 — GetWhatsAppMessageTemplate never performs the template lookup.
AWS: Smithy: GetWhatsAppMessageTemplate, GET /v1/whatsapp/template; input `id` (@required, WABA pattern), `metaTemplateId` (len 1..100, pattern ^[0-9]+$), templateName, templateLanguageCode; modeled errors include ResourceNotFoundException 404. Recorded behavior: a numeric metaTemplateId against an unlinked WABA is refused 404 ResourceNotFoundException 'Id not found for account.'; a non-numeric metaTemplateId is refused 400 ValidationException with the pattern template for member 'metaTemplateId' (already passing).
Emulator: 501 NotImplemented for the numeric metaTemplateId.
Why: no template can ever be read, and the not-found class a client keys on never appears.
Fix: implement the GetWhatsAppMessageTemplate handler and its not-found refusal.
Traces: socialmessaging.meta-template-id-pattern-violation-rejected

Cause 5 of 7 — GetWhatsAppBusinessPublicKey never performs the phone-number lookup.
AWS: Smithy: GetWhatsAppBusinessPublicKey, GET /v1/whatsapp/business-public-key; input `originationPhoneNumberId` (len 1..115, pattern (^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$), @required); modeled errors include AccessDeniedException 403 and ResourceNotFoundException 404. Recorded behavior: a well-formed but unknown phone-number id is refused 404 ResourceNotFoundException with the fixed text 'Origination phone number id not found for account.' (note this is DIFFERENT fixed text from the WABA-scoped 'Id not found for account.' — AWS words its 404s per resource); a value failing the pattern is refused 400 ValidationException with the pattern template for member 'originationPhoneNumberId' (already passing). For a phone number that IS linked to the account the call must succeed (the check's positive control reads it from ListLinkedWhatsAppBusinessAccounts / GetLinkedWhatsAppBusinessAccount).
Emulator: 501 NotImplemented for the well-formed id.
Why: the two distinct AWS outcomes (bad syntax vs unknown resource vs success) collapse into 'not implemented', so encryption-setup flows cannot be driven at all.
Fix: implement the GetWhatsAppBusinessPublicKey handler against the stored phone numbers, with the per-resource not-found wording.
Traces: socialmessaging.phone-number-id-pattern-violation-rejected

Cause 6 of 7 — PutWhatsAppBusinessPublicKey never performs the phone-number lookup.
AWS: Smithy: PutWhatsAppBusinessPublicKey, PUT /v1/whatsapp/business-public-key, @idempotent; input `originationPhoneNumberId` (@required, phone-number-id pattern), `businessPublicKey` (len 1..8192), `kmsKeyArn` (len 1..256, pattern ^arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9]{12}:(key/.+|alias/.+)$); modeled errors include ResourceNotFoundException 404. Recorded behavior: a syntactically valid KMS key ARN against an unknown phone-number id is refused 404 ResourceNotFoundException 'Origination phone number id not found for account.' — the phone-number lookup fails before any KMS ownership check; a malformed kmsKeyArn is refused 400 ValidationException with the pattern template for member 'kmsKeyArn' (already passing).
Emulator: 501 NotImplemented for the valid ARN.
Why: the ordering contract (syntax, then resource) is unobservable and the public key can never be set.
Fix: implement the PutWhatsAppBusinessPublicKey handler with the phone-number lookup ahead of any key handling.
Traces: socialmessaging.kms-key-arn-pattern-violation-rejected

Cause 7 of 7 — DeleteWhatsAppMessageMedia never performs the phone-number/media lookup.
AWS: Smithy: DeleteWhatsAppMessageMedia, DELETE /v1/whatsapp/media, @idempotent; input `mediaId` (len 1..100, pattern ^[A-Za-z0-9]+$, @required) and `originationPhoneNumberId` (@required, phone-number-id pattern); modeled errors include ResourceNotFoundException 404. Recorded behavior: an alphanumeric mediaId against an unknown phone-number id is refused 404 ResourceNotFoundException 'Origination phone number id not found for account.'; a mediaId containing spaces/punctuation is refused 400 ValidationException with the pattern template for member 'mediaId' (already passing).
Emulator: 501 NotImplemented for the well-formed mediaId.
Why: media cleanup is unavailable and an idempotent delete never reports the not-found class.
Fix: implement the DeleteWhatsAppMessageMedia handler and its not-found refusal.
Traces: socialmessaging.media-id-pattern-violation-rejected

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout: 10/19 passed, all 9 failures in socialmessaging, emulator up and serving (implemented operations such as ListLinkedWhatsAppBusinessAccounts answer 200). None of these checks was marked ALREADY FILED or OWNED by an open issue, and no open issue names any of these seven operations.

Each failing check asserts the ValidationException boundary FIRST (that assertion passes in all seven) and then the resource-lookup outcome, which is where it fails:

- socialmessaging.flow-id-pattern-violation-rejected [mismatch]: GetWhatsAppFlow(id='waba-00000000000000000000000000000000', flowId='123456789012') — AssertionError: expected ResourceNotFoundException, got NotImplemented (expected message 'Id not found for account.').
- socialmessaging.flow-categories-exceeds-max-rejected [mismatch]: CreateWhatsAppFlow(id=<unlinked waba>, flowName=..., categories=['OTHER']*9) — expected ResourceNotFoundException, got NotImplemented.
- socialmessaging.template-definition-exceeds-max-bytes-rejected [mismatch]: CreateWhatsAppMessageTemplate(id=<unlinked waba>, templateDefinition=b'{"name":"x"}') — expected ResourceNotFoundException, got NotImplemented.
- socialmessaging.meta-template-id-pattern-violation-rejected [mismatch]: GetWhatsAppMessageTemplate(id=<unlinked waba>, metaTemplateId='123456789012345') — expected ResourceNotFoundException, got NotImplemented.
- socialmessaging.phone-number-id-pattern-violation-rejected [mismatch]: GetWhatsAppBusinessPublicKey(originationPhoneNumberId='phone-number-id-00000000000000000000000000000000') — expected ResourceNotFoundException ('Origination phone number id not found for account.'), got NotImplemented.
- socialmessaging.kms-key-arn-pattern-violation-rejected [mismatch]: PutWhatsAppBusinessPublicKey(originationPhoneNumberId=<well-formed unknown id>, kmsKeyArn='arn:aws:kms:us-east-1:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab') — expected ResourceNotFoundException, got NotImplemented.
- socialmessaging.media-id-pattern-violation-rejected [mismatch]: DeleteWhatsAppMessageMedia(mediaId='abcDEF123', originationPhoneNumberId=<well-formed unknown id>) — expected ResourceNotFoundException, got NotImplemented.

Source evidence in the checkout (each handler's body is `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))`, and each file header still carries '//! SCAFFOLDED, NOT IMPLEMENTED'):
- services/service-com-amazonaws-socialmessaging-socialmessaging/src/ops/get_whats_app_flow.rs (handle, line 191)
- .../src/ops/create_whats_app_flow.rs (header line 4)
- .../src/ops/create_whats_app_message_template.rs (header line 4)
- .../src/ops/get_whats_app_message_template.rs (header line 4)
- .../src/ops/get_whats_app_business_public_key.rs (header line 4)
- .../src/ops/put_whats_app_business_public_key.rs (header line 4)
- .../src/ops/delete_whats_app_message_media.rs (header line 4)
- .../src/scaffolded.rs lines 273-291 (code 'NotImplemented', HTTP 501)
Model/contract: .../smithy-model/com-amazonaws-socialmessaging-socialmessaging.json and OPERATIONS.md lines 87-99, 138-160, 245-288 (ResourceNotFoundException HTTP 404 declared on every one of these operations).

## Steps to Reproduce
Replay the socialmessaging checks of simparity-behavioral-aws-v1 against the emulator built from this checkout, then re-replay the seven check ids in trace_ids. Equivalently, by hand with botocore against the emulator endpoint:

1. Cause 1: sm.get_whatsapp_flow(id='waba-00000000000000000000000000000000', flowId='123456789012') -> expect 404 ResourceNotFoundException, message 'Id not found for account.'; emulator answers 501 NotImplemented. Also confirm GetWhatsAppFlow succeeds for a flow of a WABA that IS linked.
2. Cause 2: sm.create_whatsapp_flow(id=<unlinked waba id>, flowName='sim2real-test-flow', categories=['OTHER']*9) -> expect 404 'Id not found for account.'; got 501.
3. Cause 3: sm.create_whatsapp_message_template(id=<unlinked waba id>, templateDefinition=b'{"name":"x"}') -> expect 404 'Id not found for account.'; got 501.
4. Cause 4: sm.get_whatsapp_message_template(id=<unlinked waba id>, metaTemplateId='123456789012345') -> expect 404 'Id not found for account.'; got 501.
5. Cause 5: sm.get_whatsapp_business_public_key(originationPhoneNumberId='phone-number-id-00000000000000000000000000000000') -> expect 404 'Origination phone number id not found for account.'; got 501. For a phone number listed under a linked WABA the call must succeed.
6. Cause 6: sm.put_whatsapp_business_public_key(originationPhoneNumberId=<well-formed unknown id>, kmsKeyArn='arn:aws:kms:us-east-1:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab') -> expect 404 'Origination phone number id not found for account.'; got 501.
7. Cause 7: sm.delete_whatsapp_message_media(mediaId='abcDEF123', originationPhoneNumberId=<well-formed unknown id>) -> expect 404 'Origination phone number id not found for account.'; got 501.

In every case also confirm the malformed-input call still answers 400 ValidationException with the boundary wording (it does today and must keep doing so). All seven checks must pass before this issue is closed.
