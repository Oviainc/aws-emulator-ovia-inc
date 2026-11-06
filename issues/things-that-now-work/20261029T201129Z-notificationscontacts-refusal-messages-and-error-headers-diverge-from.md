Issue-Source: trace-sweep-divergence
Issue-Trace: notificationscontacts-activate-email-contact-invalid-code-pattern-rejected
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: notificationscontacts-activate-email-contact-invalid-code-pattern-rejected notificationscontacts-tag-operations-on-nonexistent-contact-not-found notificationscontacts-tag-resource-exceeds-max-entries-rejected
Issue-Shape-Id: com.amazonaws.notificationscontacts#NotificationsContacts
Issue-Key: notificationscontacts|ActivateEmailContact|4xx->4xx|$.message
Issue-Key: notificationscontacts|TagResource|4xx->4xx|$.message
Issue-Key: notificationscontacts|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: notificationscontacts|ListTagsForResource|4xx->4xx|$.message
Issue-Key: notificationscontacts|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: notificationscontacts|UntagResource|4xx->4xx|$.message
Issue-Key: notificationscontacts|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: e0f3b342c2995e24bb81f3397837b3776f1c553b

# notificationscontacts refusal messages and error headers diverge from AWS wording

> _Note: this issue was identified against the code base at commit `e0f3b342c2995e24bb81f3397837b3776f1c553b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Every refusal path exercised on this service's tag plane and on ActivateEmailContact returns the correct HTTP status and error code but an emulator-authored message, and the tag-plane not-found refusal additionally serializes an x-amzn-errortype header the recorded response does not carry in that shape. Three root causes, all in notificationscontacts.

Cause 1 of 3 - ActivateEmailContact pattern ValidationException echoes the @sensitive code and reports the anchored pattern.
AWS: Smithy: ActivateEmailContact http {method: PUT, uri: "/emailcontacts/{arn}/activate/{code}"}; ActivateEmailContactRequest.code -> Token (@httpLabel, required, @length min 7 max 7, @pattern "^[a-z0-9]{7}$", @sensitive); error ValidationException (httpError 400). Recorded behavior: status 400, code ValidationException, message "1 validation error detected: Value at 'code' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]{7}". Fixed text: "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Variable parts: the violation count, the member path from the request ('code'), and the member's modelled pattern with its leading ^ and trailing $ stripped. The rejected VALUE is never interpolated - the member is @sensitive.
Emulator: status 400 and code ValidationException match, but the message is "1 validation error detected: Value 'ABC1234' at 'code' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]{7}$" - it interpolates the submitted code (a @sensitive value AWS keeps out of the response) and renders the pattern with its anchors.
Why: a client that scrubs or asserts on this service's validation text gets a different sentence; a @sensitive activation code AWS guarantees never to reflect is echoed back into the caller's error object and any log of it; and a client that re-uses the advertised pattern for client-side pre-validation receives a different regex string than AWS returns.
Fix: ActivateEmailContact's input validation / the service's ValidationException message renderer (the httpLabel-member constraint branch) - emit "Value at '<member>'" with no value interpolation and strip the pattern anchors. The open behavioral issue 20261023T000909Z-notificationscontacts-error-message-fidelity-divergences-in-validation-and describes this same renderer fault for CreateEmailContact's name/emailAddress members; it does not cover the ActivateEmailContact 'code' label path recorded here, which still diverges on the wire.
Traces: notificationscontacts-activate-email-contact-invalid-code-pattern-rejected#task#2.

Cause 2 of 3 - tag-plane ResourceNotFoundException uses the email-contact lookup template and adds an x-amzn-errortype the recording does not assert.
AWS: Smithy: TagResource (POST /tags/{arn}), ListTagsForResource (GET /tags/{arn}) and UntagResource (DELETE /tags/{arn}) each bind ResourceNotFoundException (httpError 404, message required); arn -> EmailContactArn (@httpLabel, @pattern "^arn:aws:notifications-contacts::[0-9]{12}:emailcontact/[a-z0-9]{27}$"). Recorded behavior: for a well-formed but nonexistent contact ARN all three answer status 404 with the message "Resource was not found" - the message is entirely fixed text for this refusal class, with NO variable part: the requested ARN is not interpolated. The recorded step also does not assert the shaped x-amzn-errortype value the emulator sends.
Emulator: status 404 is right, but the message is "no email contact exists with the arn <arn>" (crate-authored wording that both names a different resource phrase and leaks the request's ARN into a message AWS keeps parameter-free), and the response carries x-amzn-errortype: ResourceNotFoundException where the recording's header does not match that shape. Hypothesis: the recorded tag-plane refusal carries no x-amzn-errortype header (or one shaped differently from the bare shape name), so the emulator's generic restJson1 error serializer is adding a header this plane does not emit.
Why: a client that surfaces, matches or localizes this service's not-found sentence sees a different sentence for the same condition on all three tag operations; a client that reads the error type off the header rather than the body sees a header AWS did not send for this refusal, so emulator-based tests pass against behavior AWS does not exhibit.
Fix: the shared email-contact lookup error construction used by the TagResource / ListTagsForResource / UntagResource handlers (emit the fixed "Resource was not found" text, no ARN interpolation) and this plane's error serializer (do not add the x-amzn-errortype shape name the recording lacks).
Traces: notificationscontacts-tag-operations-on-nonexistent-contact-not-found#task#1 (TagResource), #task#2 (ListTagsForResource), #task#3 (UntagResource).

Cause 3 of 3 - over-quota tag map rejected with the member-constraint message family instead of the schema-cardinality one.
AWS: Smithy: TagResourceRequest.tags -> TagMap (required, @length min 0 max 200); error ValidationException (httpError 400). Recorded behavior only: a tag map carrying more entries than the modelled maximum is refused with status 400, code ValidationException and the schema-style message "[object has too many properties (found <n> but schema requires at most <max>)]" - fixed text "[object has too many properties (found ", " but schema requires at most ", ")]" with the submitted entry count and the modelled maximum as the only variable parts. This is a different message family from the "<n> validation error detected: ..." one the service uses for string/label constraints.
Emulator: status 400 and code ValidationException match, but the message is "1 validation error detected: Value '201 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 200" - the generic member-constraint family plus a synthesized "'<n> entries'" value echo that has no counterpart in AWS.
Why: a client distinguishing a map-cardinality rejection from a per-member constraint rejection, or parsing the found/at-most counts out of the message, cannot do so against the emulator.
Fix: TagResource's input validation / the map-length branch of the service's validation-message builder - emit the too-many-properties message with the actual and modelled counts.
Traces: notificationscontacts-tag-resource-exceeds-max-entries-rejected#task#2. This cause is also described as cause 3 of the open behavioral issue 20261023T000909Z-notificationscontacts-error-message-fidelity-divergences-in-validation-and; it is filed here because this wire step is not owned by that issue and carries its own replay key, and it must be closed against this fixture step too.

ARNs, the specific rejected code string and the 201-entry count in the recordings are per-run values filling the templates above, not requirements.

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator (emulator_up, score 67.6%, 12/37 steps mismatched, 7 divergence clusters - all notificationscontacts). None of the clusters below carries a stub_path or an owned_by, so all are net-new.

Cause 1 - cluster service=notificationscontacts|ActivateEmailContact|status=400->400|keys=$.message, fixture notificationscontacts-activate-email-contact-invalid-code-pattern-rejected step task#2:
  $.message value_mismatch - expected "1 validation error detected: Value at 'code' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]{7}"; got "1 validation error detected: Value 'ABC1234' at 'code' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]{7}$". Status 400 on both sides.

Cause 2 - three clusters on fixture notificationscontacts-tag-operations-on-nonexistent-contact-not-found, all expected_status 404 / actual_status 404:
  TagResource (task#1), ListTagsForResource (task#2), UntagResource (task#3), each reporting:
    $.message value_mismatch - expected 'Resource was not found'; got 'no email contact exists with the arn arn:aws:notifications-contacts::000000000000:emailcontact/zzzzzzzzzzzzzzzzzzzzzzzzzzz'
    $headers.x-amzn-errortype shape_mismatch - expected '<shaped-not-asserted>'; got 'ResourceNotFoundException'
  (the zzz... ARN is the fixture's nonexistent-contact value, not part of any requirement)

Cause 3 - cluster service=notificationscontacts|TagResource|status=400->400|keys=$.message, fixture notificationscontacts-tag-resource-exceeds-max-entries-rejected step task#2:
  $.message value_mismatch - expected '[object has too many properties (found 201 but schema requires at most 200)]'; got "1 validation error detected: Value '201 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 200". Status 400 on both sides.

## Steps to Reproduce
Client: botocore `notificationscontacts`, region us-east-1, endpoint = the emulator's data listener.

Cause 1: CreateEmailContact(name=<unique>, emailAddress="<unique>@example.com") -> arn. Call ActivateEmailContact(arn=<arn>, code="ABC1234") (uppercase, so it violates the ^[a-z0-9]{7}$ pattern) and inspect Error.Code/Error.Message: ValidationException/400 on both sides; compare the message to "1 validation error detected: Value at 'code' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]{7}".

Cause 2: pick a well-formed but never-created ARN, e.g. arn:aws:notifications-contacts::000000000000:emailcontact/<27 lowercase alphanumerics>. Call TagResource(arn=<arn>, tags={"k":"v"}), ListTagsForResource(arn=<arn>) and UntagResource(arn=<arn>, tagKeys=["k"]). Each returns 404 ResourceNotFoundException; compare Error.Message to the fixed text "Resource was not found" and inspect the raw response headers for x-amzn-errortype.

Cause 3: CreateEmailContact a contact, then TagResource(arn=<arn>, tags={f"k{i}": "v" for i in range(201)}) -> 400 ValidationException; compare Error.Message to "[object has too many properties (found 201 but schema requires at most 200)]".

Replay: replay_wire_corpus and re-run notificationscontacts-activate-email-contact-invalid-code-pattern-rejected (task#2), notificationscontacts-tag-operations-on-nonexistent-contact-not-found (task#1, task#2, task#3) and notificationscontacts-tag-resource-exceeds-max-entries-rejected (task#2).
