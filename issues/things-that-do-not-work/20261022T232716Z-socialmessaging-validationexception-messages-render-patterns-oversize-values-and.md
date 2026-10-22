Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: socialmessaging.cross-resource-type-id-rejected socialmessaging.dataset-id-pattern-violation-rejected socialmessaging.flow-id-pattern-violation-rejected socialmessaging.kms-key-arn-pattern-violation-rejected socialmessaging.list-tags-resource-arn-missing-prefix-rejected socialmessaging.media-id-pattern-violation-rejected socialmessaging.meta-template-id-pattern-violation-rejected socialmessaging.phone-number-id-pattern-violation-rejected socialmessaging.waba-id-pattern-violation-rejected socialmessaging.associate-business-account-access-token-exceeds-max-rejected socialmessaging.template-definition-exceeds-max-bytes-rejected socialmessaging.event-destinations-exceeds-max-rejected socialmessaging.flow-categories-exceeds-max-rejected socialmessaging.get-linked-waba-not-found socialmessaging.waba-id-exceeds-max-length-rejected socialmessaging.dataset-id-below-min-length-rejected
Issue-Shape-Id: com.amazonaws.socialmessaging#SocialMessaging
Issue-Key: socialmessaging.cross-resource-type-id-rejected|run#6|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.dataset-id-pattern-violation-rejected|run#7|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.flow-id-pattern-violation-rejected|run#6|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.kms-key-arn-pattern-violation-rejected|run#6|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.list-tags-resource-arn-missing-prefix-rejected|run#5|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.media-id-pattern-violation-rejected|run#6|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.meta-template-id-pattern-violation-rejected|run#6|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.phone-number-id-pattern-violation-rejected|run#5|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.waba-id-pattern-violation-rejected|run#5|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.associate-business-account-access-token-exceeds-max-rejected|run#6|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.template-definition-exceeds-max-bytes-rejected|run#8|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.event-destinations-exceeds-max-rejected|run#10|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.flow-categories-exceeds-max-rejected|run#6|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.get-linked-waba-not-found|run#6|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: socialmessaging.waba-id-exceeds-max-length-rejected|run#13|AssertionError|assert msg2 == expected_msg2, f"expected message {expected_msg2!r}, got {msg2!r}"
Issue-Key: socialmessaging.dataset-id-below-min-length-rejected|run#10|AssertionError|assert code2 == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code2}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# socialmessaging: ValidationException messages render patterns, oversize values and list values unlike AWS; ResourceNotFoundException message is emulator-specific; SendWhatsAppConversionEvent answers NotImplemented (5 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLinkedWhatsAppBusinessAccount

## Description
Five distinct root causes in the socialmessaging emulator, all grounded in failing behavioral checks. Every check reaches the emulator and gets a response, so the service is serving; the responses diverge from AWS. The status codes and error codes are correct except in Cause 5. For Causes 1-3 the diverging text is a modeled-constraint validation message that AWS builds from the request (it names the member and the constraint it violated), so the rule that produces it is known and is a requirement, not an incidental string.

Cause 1 of 5 — ValidationException echoes the raw Smithy pattern trait instead of AWS's rendered pattern.
AWS: for a @pattern violation AWS answers HTTP 400 / ValidationException with "1 validation error detected: Value '<v>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <rendered pattern>". The rendered pattern is NOT the model literal: outer anchors are dropped, and a pattern that is not outer-anchored is wrapped in '.*' ... '.*'. Recorded expectations vs Smithy model (services/service-com-amazonaws-socialmessaging-socialmessaging/smithy-model/com-amazonaws-socialmessaging-socialmessaging.json): WhatsAppDatasetId/MetaFlowId/MetaTemplateId @pattern "^[0-9]+$" -> AWS renders "[0-9]+"; Arn @pattern "^arn:.*$" -> "arn:.*"; WhatsAppMediaId @pattern "^[A-Za-z0-9]+$" -> "[A-Za-z0-9]+"; the kmsKeyArn pattern "^arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9]{12}:(key/.+|alias/.+)$" -> "arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9]{12}:(key/.+|alias/.+)"; WhatsAppPhoneNumberId @pattern "(^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$)" -> ".*(^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$).*"; LinkedWhatsAppBusinessAccountId @pattern "(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$)" -> ".*(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$).*".
Emulator: emits the model's pattern string verbatim — "^[0-9]+$", "^arn:.*$", "^[A-Za-z0-9]+$", "^arn:aws[a-z-]*:kms:...$", "(^waba-.*$)|(^arn:...$)" — for every one of those members. Code (ValidationException) and status (400) match; only the pattern rendering diverges.
Why: clients and test suites that parse the validation message to surface the accepted id/ARN format to users (or that assert on the documented message) see a different regex than AWS reports, and tooling that round-trips the reported pattern against the value disagrees with the service.
Fix: the shared constraint-validation message formatter for socialmessaging's restJson1 input validation — render the pattern the way AWS does (strip outer ^/$; wrap a non-outer-anchored pattern in .* … .*) rather than passing the model literal through.
Traces: socialmessaging.cross-resource-type-id-rejected, socialmessaging.dataset-id-pattern-violation-rejected, socialmessaging.flow-id-pattern-violation-rejected, socialmessaging.kms-key-arn-pattern-violation-rejected, socialmessaging.list-tags-resource-arn-missing-prefix-rejected, socialmessaging.media-id-pattern-violation-rejected, socialmessaging.meta-template-id-pattern-violation-rejected, socialmessaging.phone-number-id-pattern-violation-rejected, socialmessaging.waba-id-pattern-violation-rejected.

Cause 2 of 5 — length violations on oversize/binary members echo a size summary instead of omitting the value.
AWS: when the offending value is large or binary, the length-constraint message omits the value entirely: "1 validation error detected: Value at 'signupCallback.accessToken' failed to satisfy constraint: Member must have length less than or equal to 1000" and "1 validation error detected: Value at 'templateDefinition' failed to satisfy constraint: Member must have length less than or equal to 6000". Smithy: WhatsAppSignupCallback.accessToken is a String with @length max 1000; MetaTemplateDefinition is a blob with @length min 1 max 6000; both violations are ValidationException / HTTP 400. Short values ARE echoed by AWS — the same suite's 116-char id case ("Value 'waba-000…' at 'id' failed to satisfy constraint: Member must have length less than or equal to 115") passes against the emulator. Hypothesis: AWS suppresses the value above a rendering threshold and for blob members.
Emulator: substitutes a size summary for the value — "Value 'aaaa…[1001 bytes]' at 'signupCallback.accessToken' failed…" and "Value '6115 bytes' at 'templateDefinition' failed…" — a form AWS never emits.
Why: a client asserting on, or displaying, the service's validation message for an oversize payload gets emulator-invented text (and the emulator reflects back part of a caller-supplied access token, which AWS deliberately does not).
Fix: the same constraint-validation message formatter — omit the value segment for blob members and for values past the echo threshold instead of synthesising a byte-count placeholder.
Traces: socialmessaging.associate-business-account-access-token-exceeds-max-rejected, socialmessaging.template-definition-exceeds-max-bytes-rejected.

Cause 3 of 5 — list length violations report an item count instead of the rendered list.
AWS: a @length violation on a list member renders the list's elements in the message: "1 validation error detected: Value '[OTHER, OTHER, OTHER, OTHER, OTHER, OTHER, OTHER, OTHER, OTHER, OTHER]' at 'categories' failed to satisfy constraint: Member must have length less than or equal to 9" and "1 validation error detected: Value '[WhatsAppBusinessAccountEventDestination(eventDestinationArn=…, roleArn=…), WhatsAppBusinessAccountEventDestination(…)]' at 'eventDestinations' failed to satisfy constraint: Member must have length less than or equal to 1". Smithy: MetaFlowCategoryList @length min 1 max 9 (UpdateWhatsAppFlow.categories); WhatsAppBusinessAccountEventDestinations @length max 1 (PutWhatsAppBusinessAccountEventDestinations.eventDestinations); both ValidationException / HTTP 400. The requirement is the rendering rule (bracketed, comma-separated elements, structures as Name(member=value, …)), not the particular ARNs/ids in the recording.
Emulator: renders the element count — "Value '10 items' at 'categories' …", "Value '2 items' at 'eventDestinations' …".
Why: the message no longer identifies which elements were submitted, so a caller debugging an over-limit list gets strictly less information than AWS gives and any assertion on AWS's message form fails.
Fix: the same constraint-validation message formatter — render list values element-by-element rather than as a count.
Traces: socialmessaging.event-destinations-exceeds-max-rejected, socialmessaging.flow-categories-exceeds-max-rejected.

Cause 4 of 5 — ResourceNotFoundException carries an emulator-authored message that embeds the requested id.
AWS: GetLinkedWhatsAppBusinessAccount with a well-formed but unlinked WABA id answers HTTP 404 / ResourceNotFoundException with the fixed message "Requested resource not found." Recorded behavior only (the Smithy model documents the error only as "The resource was not found."); the message is constant across requests — two recordings with different ids (a 36-char id and a 115-char id) both produce exactly that text, so it is not derived from the request.
Emulator: returns ResourceNotFoundException (code and 404 are right) with a per-request message that embeds the id, e.g. "no linked WhatsApp Business Account with id 'waba-00000000000000000000000000000000'".
Why: the message is request-dependent where AWS's is constant, so clients that match on the service's not-found text behave differently, and the emulator leaks the lookup key into an error AWS keeps opaque.
Fix: the GetLinkedWhatsAppBusinessAccount handler's not-found error construction (and the shared not-found error builder it uses) — emit the service's constant message instead of a formatted one.
Traces: socialmessaging.get-linked-waba-not-found, socialmessaging.waba-id-exceeds-max-length-rejected (its second leg).

Cause 5 of 5 — SendWhatsAppConversionEvent is not implemented, so its not-found path is unreachable.
AWS: SendWhatsAppConversionEvent (POST /v1/whatsapp/waba/dataset/events) with a well-formed, validation-passing datasetId (10-20 digits, @pattern ^[0-9]+$) for a WABA/dataset that does not exist answers HTTP 404 / ResourceNotFoundException (recorded message example: "Id not found for account."); the operation's modeled errors include ResourceNotFoundException. Smithy: SendWhatsAppConversionEventInput.datasetId target WhatsAppDatasetId @length min 10 max 20; errors include ResourceNotFoundException (404).
Emulator: input validation works (the too-short datasetId is correctly refused with ValidationException), but the valid-input call returns error code NotImplemented instead of ResourceNotFoundException — the operation handler is a stub, so no conversion-event behavior exists. This check is not marked ALREADY FILED and no open socialmessaging issue covers it (a query of the issue list for "socialmessaging" returns none).
Why: any client sending conversion events gets a 501-class NotImplemented for every call, valid or not, so the whole Conversions API surface is unusable and the not-found classification is unobservable.
Fix: implement the SendWhatsAppConversionEvent handler and its dataset lookup/state, including the 404 ResourceNotFoundException path for an unknown WABA/dataset.
Traces: socialmessaging.dataset-id-below-min-length-rejected.

## Evidence
Suite: 3/19 passed for socialmessaging, 16 mismatches, 0 errors.

Cause 1 (pattern rendering) — representative diffs (expected = AWS, got = emulator):
- dataset-id-pattern-violation-rejected: expected "…at 'datasetId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9]+", got "… pattern: ^[0-9]+$".
- flow-id-pattern-violation-rejected: expected "… at 'flowId' … pattern: [0-9]+", got "… pattern: ^[0-9]+$".
- meta-template-id-pattern-violation-rejected: expected "… at 'metaTemplateId' … pattern: [0-9]+", got "… pattern: ^[0-9]+$".
- media-id-pattern-violation-rejected: expected "… at 'mediaId' … pattern: [A-Za-z0-9]+", got "… pattern: ^[A-Za-z0-9]+$".
- list-tags-resource-arn-missing-prefix-rejected: expected "… at 'resourceArn' … pattern: arn:.*", got "… pattern: ^arn:.*$".
- kms-key-arn-pattern-violation-rejected: expected "… at 'kmsKeyArn' … pattern: arn:aws[a-z-]*:kms:[a-z0-9-]+:[0-9]{12}:(key/.+|alias/.+)", got the same regex wrapped in ^…$.
- phone-number-id-pattern-violation-rejected / cross-resource-type-id-rejected: expected "… pattern: .*(^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$).*", got "… pattern: (^phone-number-id-.*$)|(^arn:.*:phone-number-id/[0-9a-zA-Z]+$)".
- waba-id-pattern-violation-rejected: expected "… pattern: .*(^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$).*", got "… pattern: (^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$)".
In every case the emulator's string equals the Smithy @pattern literal, confirming the formatter passes the model value through unrendered.

Cause 2 (oversize/blob value echo):
- associate-business-account-access-token-exceeds-max-rejected: expected "1 validation error detected: Value at 'signupCallback.accessToken' failed to satisfy constraint: Member must have length less than or equal to 1000"; got "1 validation error detected: Value 'aaaa…[1001 bytes]' at 'signupCallback.accessToken' failed to satisfy constraint: Member must have length less than or equal to 1000".
- template-definition-exceeds-max-bytes-rejected: expected "1 validation error detected: Value at 'templateDefinition' failed …equal to 6000"; got "…Value '6115 bytes' at 'templateDefinition' failed …equal to 6000".
Contrast (passes): waba-id-exceeds-max-length-rejected's first leg, where the 116-char id IS echoed by both AWS and the emulator.

Cause 3 (list value rendering):
- flow-categories-exceeds-max-rejected: expected "Value '[OTHER, OTHER, OTHER, OTHER, OTHER, OTHER, OTHER, OTHER, OTHER, OTHER]' at 'categories' … less than or equal to 9"; got "Value '10 items' at 'categories' …".
- event-destinations-exceeds-max-rejected: expected "Value '[WhatsAppBusinessAccountEventDestination(eventDestinationArn=…, roleArn=…), WhatsAppBusinessAccountEventDestination(…)]' at 'eventDestinations' … less than or equal to 1"; got "Value '2 items' at 'eventDestinations' …".

Cause 4 (not-found message):
- get-linked-waba-not-found: expected code ResourceNotFoundException + message 'Requested resource not found.'; got the right code with "no linked WhatsApp Business Account with id 'waba-00000000000000000000000000000000'".
- waba-id-exceeds-max-length-rejected (leg 2, 115-char id): expected 'Requested resource not found.'; got "no linked WhatsApp Business Account with id 'waba-000…000'" (different id, same emulator template; AWS returns the identical constant text for both ids).

Cause 5 (unimplemented operation):
- dataset-id-below-min-length-rejected: leg 1 (datasetId="123", parameter_validation disabled) correctly returns ValidationException with AWS's message; leg 2 (datasetId="1234567890", id="waba-00000000000000000000000000000000") expected ResourceNotFoundException, got error code NotImplemented.

## Steps to Reproduce
Boot the emulator and point a botocore socialmessaging client at it (region us-east-1, any credentials).

Cause 1: sm.get_linked_whatsapp_business_account_phone_number(id="waba-00000000000000000000000000000000") and sm.get_linked_whatsapp_business_account(id="not-a-valid-waba-id") and sm.list_tags_for_resource(resourceArn="not-an-arn") and sm.get_whatsapp_message_media(mediaId="bad id!", originationPhoneNumberId="phone-number-id-x") and sm.get_whatsapp_message_template(metaTemplateId="abc", id="waba-0000…") and sm.get_whatsapp_flow(id="waba-0000…", flowId="not-numeric") — each returns 400 ValidationException whose message echoes the Smithy @pattern literal (anchored) instead of AWS's rendered pattern.

Cause 2: sm.associate_whatsapp_business_account(signupCallback={"accessToken": "a"*1001}) and sm.create_whatsapp_message_template(id="waba-0000…", templateDefinition=b"x"*6115) — the message contains a 'Value <size>' segment where AWS emits no value at all.

Cause 3: sm.update_whatsapp_flow(id="waba-0000…", flowId="1", categories=["OTHER"]*10) and sm.put_whatsapp_business_account_event_destinations(id="waba-0000…", eventDestinations=[two destinations]) — the message says "N items" instead of rendering the list elements.

Cause 4: sm.get_linked_whatsapp_business_account(id="waba-00000000000000000000000000000000") — 404 ResourceNotFoundException whose message embeds the id instead of the constant 'Requested resource not found.'; repeat with a 115-char id ("waba-" + "0"*110) and the emulator's message again varies while AWS's does not.

Cause 5: sm.send_whatsapp_conversion_event(id="waba-00000000000000000000000000000000", datasetId="1234567890", eventData=b'{"event_name":"Purchase"}') — returns error code NotImplemented instead of 404 ResourceNotFoundException.

Replay: the check ids in trace_ids.
