Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.create-partnership-invalid-email-format-error b2bi.create-partnership-nonexistent-profile-rejected b2bi.get-profile-not-found b2bi.create-transformer-idempotent-token-concurrent-dedup b2bi.create-transformer-mixed-legacy-and-new-fields-error b2bi.get-transformer-job-cross-transformer-isolation-rejected b2bi.tag-resource-nonexistent-arn-not-found b2bi.test-conversion-unparsable-source-content-error b2bi.test-mapping-invalid-jsonata-template-rejected
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi.create-partnership-invalid-email-format-error|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: b2bi.create-partnership-nonexistent-profile-rejected|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: b2bi.get-profile-not-found|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: b2bi.create-transformer-idempotent-token-concurrent-dedup|run#17|AssertionError|assert all(error == ("ConflictException", CONFLICT_MESSAGE) for error in errors), (
Issue-Key: b2bi.create-transformer-mixed-legacy-and-new-fields-error|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: b2bi.get-transformer-job-cross-transformer-isolation-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: b2bi.tag-resource-nonexistent-arn-not-found|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: b2bi.test-conversion-unparsable-source-content-error|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: b2bi.test-mapping-invalid-jsonata-template-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 24dcbcf55b45bb9d0573a38e9c06262a825c087d

# b2bi composes its own refusal text: eight error messages diverge from AWS's wording

> _Note: this issue was identified against the code base at commit `24dcbcf55b45bb9d0573a38e9c06262a825c087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePartnership

## Description
EIGHT net-new causes, one shared fault: on every one of these refusal paths the emulator returns the right HTTP status and the right error code, but a message it composed itself instead of the message AWS returns. In every case the emulator's text is MORE verbose than AWS's — it echoes the offending value, names the input member, or re-renders an underlying library's diagnostic — where AWS's text is fixed, or fixed text plus a narrowly defined set of values. A client or test that keys off the message cannot distinguish these refusals from one another. Related open issue 20261029T171003Z-b2bi-response-text-diverges-pagination-refusal-wording-starter.md records the same class of fault on ListProfiles' pagination refusal; none of the eight causes below is covered by it.

Cause 1 of 8: CreatePartnership's email pattern-violation message echoes the sensitive value and the anchored pattern.
AWS: Smithy com.amazonaws.b2bi#Email is @sensitive, @length 5..254, @pattern "^[\\w\\.\\-]+@[\\w\\.\\-]+$"; CreatePartnershipRequest.email targets it and is required. The refusal is the common ValidationException (member `Message`, HTTP 400). Recorded message: `1 validation error detected: Value at 'email' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w\.\-]+@[\w\.\-]+`. Fixed text: "1 validation error detected: Value at '" + the member path + "' failed to satisfy constraint: Member must satisfy regular expression pattern: " + the pattern. Two parts of that template are the requirement and are what the emulator gets wrong: because the member is @sensitive the offending VALUE is omitted entirely (AWS writes "Value at 'email'", not "Value '<value>' at 'email'"), and the pattern is rendered WITHOUT its ^ and $ anchors.
Emulator: `1 validation error detected: Value 'not-an-email-address' at 'email' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w\.\-]+@[\w\.\-]+$` — it interpolates the rejected value (the value shown is this run's request value, not part of the requirement) and prints the anchored pattern.
Why: the emulator leaks a @sensitive member's value into an error message that is logged and surfaced to users, which is exactly what @sensitive forbids, and a client matching AWS's constraint text does not match.
Fix: the shared constraint-violation message formatter for pattern violations (suppress the value for @sensitive members; strip the ^/$ anchors when rendering the pattern).
Traces: b2bi.create-partnership-invalid-email-format-error.

Cause 2 of 8: profile-not-found text.
AWS: ResourceNotFoundException (member `message`) for a syntactically valid but nonexistent profileId. Recorded message: "Profile with profileId <profileId> not found." — fixed text "Profile with profileId ", the profileId from the request, " not found." (same wording the already-filed wire issue records for ListPartnerships).
Emulator: "Profile ID <profileId> not found." on both GetProfile and CreatePartnership's profile resolution.
Why: a client that branches on the not-found text (or a test asserting it) fails; the two operations are consistent with each other but not with AWS.
Fix: the profile-lookup not-found message used by GetProfile and by CreatePartnership's profileId resolution.
Traces: b2bi.get-profile-not-found, b2bi.create-partnership-nonexistent-profile-rejected.

Cause 3 of 8: CreateTransformer in-flight clientToken ConflictException text echoes the token.
AWS: CreateTransformerRequest.clientToken is @idempotencyToken; concurrent requests sharing one token are refused with ConflictException (member `message`, HTTP 409). Recorded message: "This clientToken has been used for a previous request that is still being processed." — ENTIRELY fixed text, with no echo of the token.
Emulator: "Another request to create a transformer with clientToken <token> is in progress; retry once it has completed to receive its result" — the emulator's own sentence, interpolating the token (the token in the evidence is this run's generated value, not part of the requirement).
Why: losing racers in a concurrent idempotent-create cannot be recognised by their message; a retry wrapper keyed on AWS's text does not fire.
Fix: CreateTransformer's idempotency/conflict path message.
Traces: b2bi.create-transformer-idempotent-token-concurrent-dedup.

Cause 4 of 8: CreateTransformer mixed legacy/new field refusal text.
AWS: CreateTransformerRequest carries both the current members (inputConversion, mapping, outputConversion, sampleDocuments) and the @deprecated legacy ones (fileFormat, mappingTemplate, ediType, sampleDocument); mixing the two sets is refused with ValidationException (400, member `Message`). Recorded message: "A Transformer must use the fields input, mapping, and output, or the fields fileFormat, mappingTemplate, ediType, and sampleDocument. These fields must not be mixed" — entirely fixed text; it does NOT report which members the request actually supplied.
Emulator: "A transformer must be configured with the fields inputConversion, mapping, outputConversion, sampleDocuments or with the legacy fields ediType, fileFormat, mappingTemplate, sampleDocument, not both: this request specifies inputConversion and ediType, fileFormat, mappingTemplate" — different fixed wording, different member spellings, plus an appended per-request listing AWS never sends.
Fix: CreateTransformer's legacy/new field-set validation message.
Traces: b2bi.create-transformer-mixed-legacy-and-new-fields-error.

Cause 5 of 8: GetTransformerJob not-found text names the wrong scope.
AWS: GetTransformerJobRequest requires transformerJobId (@length 25..25) and transformerId; an unknown job is refused with ResourceNotFoundException (member `message`). Recorded message: "Found no transformer job <transformerJobId> for customer <accountId>" — fixed text "Found no transformer job ", the requested transformerJobId, " for customer ", the caller's account id. AWS scopes the refusal to the ACCOUNT, not to the transformerId in the request.
Emulator: "Transformer job with transformerJobId <jobId> not found for transformerId <transformerId>" — its own wording, and it scopes the message to the transformerId.
Why: the message is what tells a caller whether a job id is unknown to the account at all versus merely not owned by the transformer it asked about; the emulator reverses that meaning.
Fix: GetTransformerJob's not-found message (scope the text to the account as AWS does).
Traces: b2bi.get-transformer-job-cross-transformer-isolation-rejected.

Cause 6 of 8: TagResource bad-ARN refusal text.
AWS: TagResourceRequest.ResourceARN targets AmazonResourceName (@length 1..1011); an ARN that names no live resource in the account is refused with the message "Invalid input ARN!" — the COMPLETE message, entirely fixed text, with no echo of the ARN and no explanation.
Emulator: "Invalid ARN <arn>: it names no resource in this account" — its own sentence, interpolating the ARN (the ARN in the evidence is this run's generated profile ARN, not part of the requirement).
Fix: TagResource's ARN-resolution refusal message.
Traces: b2bi.tag-resource-nonexistent-arn-not-found.

Cause 7 of 8: TestConversion unparsable-source refusal text.
AWS: TestConversionRequest.source is required (ConversionSource); content that is not the declared fileFormat is refused with ValidationException (400, member `Message`). The recorded message is the upstream Jackson parser's own diagnostic, surfaced verbatim: "Unexpected character ('<char>' (code <code>)): was expecting double-quote to start field name at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <line>, column: <column>]". Fixed text: "Unexpected character ('", " (code ", ")): ", the expectation clause, " at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: ", ", column: ", "]"; the offending character, its numeric code, and the line/column come from the submitted content. Notably AWS names NO input member and REDACTES the source.
Emulator: "source.inputFile.fileContent is declared as JSON but is not valid JSON: key must be a string at line 1 column 3" — its own sentence naming the member path and its own JSON library's wording. (The refusal path is implemented here even though the conversion itself is still scaffolded, so this is not the filed TestConversion stub.)
Fix: TestConversion's source-parse refusal message in services/service-com-amazonaws-b2bi-b2bi/src/ops/test_conversion.rs.
Traces: b2bi.test-conversion-unparsable-source-content-error.

Cause 8 of 8: TestMapping invalid-JSONata refusal text.
AWS: TestMappingRequest.mappingTemplate is required (MappingTemplate, @length 0..350000); a template that is not a valid JSONata expression is refused with ValidationException (400, member `Message`). Recorded message: 'JSONata error. Failed to map input file: Expected ")" before end of expression. From line 1 column 2 to line 1 column 7.' Fixed text: "JSONata error. Failed to map input file: " followed by the JSONata engine's own diagnostic sentence and a "From line <l> column <c> to line <l> column <c>." span; the token, lines and columns come from the submitted template.
Emulator: "mappingTemplate is not a valid JSONata expression: S0211 @ 6: The symbol `(end)` cannot be used as a unary operator" — no "JSONata error. Failed to map input file: " prefix, the member name instead, and a raw S-code/offset form rather than AWS's line/column span.
Fix: TestMapping's template-compilation refusal message.
Traces: b2bi.test-mapping-invalid-jsonata-template-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 (23/42 passed; all 19 failures in b2bi).
Cause 1 — b2bi.create-partnership-invalid-email-format-error: expected message "1 validation error detected: Value at 'email' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w\.\-]+@[\w\.\-]+", got "1 validation error detected: Value 'not-an-email-address' at 'email' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w\.\-]+@[\w\.\-]+$".
Cause 2 — b2bi.get-profile-not-found: expected 'Profile with profileId p-c590437e1a079ab15 not found.', got 'Profile ID p-c590437e1a079ab15 not found.'; b2bi.create-partnership-nonexistent-profile-rejected: expected 'Profile with profileId nonexistent-profile-xa6055fbd66f not found.', got 'Profile ID nonexistent-profile-xa6055fbd66f not found.' (the ids are per-run values).
Cause 3 — b2bi.create-transformer-idempotent-token-concurrent-dedup: all four losing racers returned ('ConflictException', 'Another request to create a transformer with clientToken sim2real-b2bi-token-7e0cf98d51cb is in progress; retry once it has completed to receive its result'); expected ('ConflictException', 'This clientToken has been used for a previous request that is still being processed.').
Cause 4 — b2bi.create-transformer-mixed-legacy-and-new-fields-error: expected 'A Transformer must use the fields input, mapping, and output, or the fields fileFormat, mappingTemplate, ediType, and sampleDocument. These fields must not be mixed', got 'A transformer must be configured with the fields inputConversion, mapping, outputConversion, sampleDocuments or with the legacy fields ediType, fileFormat, mappingTemplate, sampleDocument, not both: this request specifies inputConversion and ediType, fileFormat, mappingTemplate'.
Cause 5 — b2bi.get-transformer-job-cross-transformer-isolation-rejected: expected 'Found no transformer job 7661eb5084ae3ea987175971f for customer 000000000000', got 'Transformer job with transformerJobId 7661eb5084ae3ea987175971f not found for transformerId t-34efa536f6bdade2d'.
Cause 6 — b2bi.tag-resource-nonexistent-arn-not-found: expected 'Invalid input ARN!', got 'Invalid ARN arn:aws:b2bi:us-west-2:000000000000:profile/p-5adf850d96c5e04ee: it names no resource in this account'.
Cause 7 — b2bi.test-conversion-unparsable-source-content-error: expected "Unexpected character ('t' (code 116)): was expecting double-quote to start field name at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 3]", got 'source.inputFile.fileContent is declared as JSON but is not valid JSON: key must be a string at line 1 column 3'.
Cause 8 — b2bi.test-mapping-invalid-jsonata-template-rejected: expected 'JSONata error. Failed to map input file: Expected ")" before end of expression. From line 1 column 2 to line 1 column 7.', got 'mappingTemplate is not a valid JSONata expression: S0211 @ 6: The symbol `(end)` cannot be used as a unary operator'.
In all eight the HTTP status and the error code matched; only the message member diverged.

## Steps to Reproduce
1. Clone the repo and boot the emulator from mainline. 2. Run the simparity-behavioral-aws-v1 behavioral suite. 3. Re-replay the nine checks in trace_ids: b2bi.create-partnership-invalid-email-format-error, b2bi.create-partnership-nonexistent-profile-rejected, b2bi.get-profile-not-found, b2bi.create-transformer-idempotent-token-concurrent-dedup, b2bi.create-transformer-mixed-legacy-and-new-fields-error, b2bi.get-transformer-job-cross-transformer-isolation-rejected, b2bi.tag-resource-nonexistent-arn-not-found, b2bi.test-conversion-unparsable-source-content-error, b2bi.test-mapping-invalid-jsonata-template-rejected. 4. In each, the operation is refused with the right status and error code but a message the emulator composed; compare the `Message`/`message` member against the AWS text quoted per cause above.
