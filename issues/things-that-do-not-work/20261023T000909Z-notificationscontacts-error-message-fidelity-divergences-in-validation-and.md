Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: notificationscontacts.create-email-contact-invalid-email-rejected notificationscontacts.create-email-contact-name-length-exceeded-rejected notificationscontacts.tag-resource-exceeds-max-entries-rejected notificationscontacts.delete-email-contact-removes-resource notificationscontacts.delete-nonexistent-contact-not-found notificationscontacts.activate-wrong-code-state-conflict-rejected
Issue-Shape-Id: com.amazonaws.notificationscontacts#NotificationsContacts
Issue-Key: notificationscontacts.create-email-contact-invalid-email-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: notificationscontacts.create-email-contact-name-length-exceeded-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: notificationscontacts.tag-resource-exceeds-max-entries-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: notificationscontacts.delete-email-contact-removes-resource|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: notificationscontacts.delete-nonexistent-contact-not-found|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: notificationscontacts.activate-wrong-code-state-conflict-rejected|run#9|AssertionError|assert "did not match" in message.lower(), (
Issue-Commit: 3e963f69e1d0897f40654d15e23b2fa6367d6b7c

# notificationscontacts: error-message fidelity divergences in validation and not-found responses (5 causes)

> _Note: this issue was identified against the code base at commit `3e963f69e1d0897f40654d15e23b2fa6367d6b7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEmailContact

## Description
Five distinct root causes in the notificationscontacts emulator, all surfaced by the behavioral suite. In every case the error CODE and HTTP status match real AWS; what diverges is the message the emulator builds, and in two of the causes the divergence also leaks or mis-reports request/model data. All five live in this one service.

Cause 1 of 5 — ValidationException messages echo the offending member value (CreateEmailContact).
AWS: Smithy model com-amazonaws-notificationscontacts-notificationscontacts.json: CreateEmailContactRequest.name -> EmailContactName (@length min 1 max 64, @pattern "[\\w-.~]+", @sensitive), CreateEmailContactRequest.emailAddress -> EmailContactAddress (@length 6..254, @pattern "^(.+)@(.+)$"); error ValidationException (httpError 400, common error). Recorded behavior: the service renders the standard constraint-violation message WITHOUT echoing the submitted value, i.e. the rule is "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must <constraint>" — the member name and the constraint come from the request/model, the value is never included. Recorded examples: "1 validation error detected: Value at 'emailAddress' failed to satisfy constraint: Member must satisfy regular expression pattern: (.+)@(.+)" and "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must have length less than or equal to 64".
Emulator: interpolates the rejected value into the message — "...Value 'not-an-email' at 'emailAddress' failed..." and "...Value 'aaaa…a' (65 chars) at 'name' failed...". For `name` this echoes a member the model marks @sensitive, so a value AWS deliberately keeps out of responses is reflected back to the caller (and into any client log of the error).
Why: clients and log pipelines that scrub or assert on this service's validation messages see a different message shape, and @sensitive input values appear in emulator responses where AWS guarantees they do not — tests built against the emulator will not catch a real sensitive-data expectation, and message parsers break.
Fix: the service's request-validation layer / ValidationException message builder for notificationscontacts — emit "Value at '<member>'" with no value interpolation (unconditionally for this service; mandatory for @sensitive members).
Traces: notificationscontacts.create-email-contact-invalid-email-rejected, notificationscontacts.create-email-contact-name-length-exceeded-rejected.

Cause 2 of 5 — pattern constraint reported with regex anchors.
AWS: model pattern on EmailContactAddress is "^(.+)@(.+)$"; the recorded ValidationException message reports it unanchored: "Member must satisfy regular expression pattern: (.+)@(.+)". Rule: the reported pattern is the member's model pattern with its leading ^ / trailing $ stripped.
Emulator: reports "Member must satisfy regular expression pattern: ^(.+)@(.+)$".
Why: a client that compares or displays the advertised pattern (or re-uses it for client-side pre-validation) gets a different regex string than AWS returns for the same rejection.
Fix: the pattern-constraint renderer in the same validation-message builder — strip the anchors when formatting the pattern.
Traces: notificationscontacts.create-email-contact-invalid-email-rejected.

Cause 3 of 5 — over-quota tag map rejected with the wrong message family (TagResource).
AWS: model TagResourceRequest.tags -> TagMap (@length min 0 max 200), error ValidationException (400). Recorded behavior only: an over-quota tag map is rejected with the schema-style message "[object has too many properties (found <n> but schema requires at most <max>)]" — parameterized by the submitted entry count and the modeled maximum (recorded example, 201 entries against the 200 maximum: "[object has too many properties (found 201 but schema requires at most 200)]"). This is a different message family from the "<n> validation error detected: ..." one this service uses for string constraints.
Emulator: returns the generic member-constraint message instead — "1 validation error detected: Value '201 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 200" — including a synthesized "'<n> entries'" value echo that has no counterpart in AWS.
Why: callers distinguishing a map-cardinality rejection from a per-member constraint rejection (or parsing the found/at-most counts) cannot do so against the emulator.
Fix: TagResource's input validation / the map-length branch of the validation-message builder — emit the schema-style too-many-properties message with the actual and maximum counts. Note: this check also exercises TagResource and ListTagsForResource, which are separately filed as unimplemented scaffold stubs; the message rule above is the net-new defect.
Traces: notificationscontacts.tag-resource-exceeds-max-entries-rejected.

Cause 4 of 5 — ResourceNotFoundException message for an unknown email contact (GetEmailContact, DeleteEmailContact).
AWS: model ResourceNotFoundException (httpError 404) is bound to GetEmailContact and DeleteEmailContact; its message is required. Recorded behavior: the message is the template "Email Contact with arn <arn> not found", built from the request's `arn` httpLabel.
Emulator: returns "no email contact exists with the arn <arn>" for both the GET after a delete and the DELETE of a never-existing ARN. Code (ResourceNotFoundException) and status match; only the message template differs.
Why: a client that surfaces or matches this service's not-found text (including the "Email Contact" resource wording AWS uses) sees a different sentence for the same condition, and the emulator's wording does not identify the resource type the way AWS's does.
Fix: the shared not-found error construction used by the email-contact lookup in the GetEmailContact / DeleteEmailContact handlers (and anything else resolving an email-contact ARN) — use the "Email Contact with arn <arn> not found" template.
Traces: notificationscontacts.delete-email-contact-removes-resource, notificationscontacts.delete-nonexistent-contact-not-found.

Cause 5 of 5 — ActivateEmailContact wrong-but-well-formed code: message does not report a token mismatch.
AWS: model ActivateEmailContactRequest.code -> Token (@length 7..7, @pattern "^[a-z0-9]{7}$", @sensitive), arn -> EmailContactArn; errors include ValidationException (400) and ResourceNotFoundException (404). Recorded behavior: when an activation code WAS issued via SendActivationCode but the supplied code does not match it, the service answers ValidationException with the template "Input token did not match stored Activation Token for arn <arn>" — a token-mismatch statement parameterized by the request's arn (and distinct from the no-code-ever-issued case, which is ResourceNotFoundException "Activation Token with arn <arn> not found").
Emulator: answers ValidationException (correct code) with "code is not the activation code outstanding for this email contact" — it neither states a token mismatch in AWS's terms nor includes the arn, so the message carries no resource identity.
Why: a client that distinguishes mismatch from not-issued by inspecting the message, or that correlates the failure to a contact ARN from the error text, cannot do so against the emulator.
Fix: ActivateEmailContact's handler / its activation-token comparison error path — emit the arn-parameterized token-mismatch message.
Traces: notificationscontacts.activate-wrong-code-state-conflict-rejected.

No existing open issue covers any of these five causes (list_existing_issues query 'notificationscontacts' returned none beyond the scaffold-stub issues auto-filed for TagResource and ListTagsForResource, which cover only the NotImplemented stubs).

## Evidence
Behavioral suite: 6/20 passed (30%), notificationscontacts 6/20, 14 failed.

Cause 1:
- create-email-contact-invalid-email-rejected: expected message "1 validation error detected: Value at 'emailAddress' failed to satisfy constraint: Member must satisfy regular expression pattern: (.+)@(.+)"; got "1 validation error detected: Value 'not-an-email' at 'emailAddress' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(.+)@(.+)$". Error code ValidationException matched.
- create-email-contact-name-length-exceeded-rejected: expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must have length less than or equal to 64"; got "1 validation error detected: Value 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' at 'name' failed to satisfy constraint: Member must have length less than or equal to 64" — the echoed value is the @sensitive EmailContactName.

Cause 2: same check as above shows the pattern rendered as ^(.+)@(.+)$ where AWS renders (.+)@(.+).

Cause 3: tag-resource-exceeds-max-entries-rejected: expected message "[object has too many properties (found 201 but schema requires at most 200)]"; got "1 validation error detected: Value '201 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 200". Error code ValidationException matched.

Cause 4:
- delete-email-contact-removes-resource (GetEmailContact after delete): expected "Email Contact with arn <contact-arn> not found"; got "no email contact exists with the arn <contact-arn>". Code ResourceNotFoundException matched.
- delete-nonexistent-contact-not-found (DeleteEmailContact of an unknown ARN): expected "Email Contact with arn <contact-arn> not found"; got "no email contact exists with the arn <contact-arn>". Code ResourceNotFoundException matched.

Cause 5: activate-wrong-code-state-conflict-rejected: after SendActivationCode, ActivateEmailContact with a well-formed but wrong 7-char code expected ValidationException with "Input token did not match stored Activation Token for arn <contact-arn>"; got ValidationException with "code is not the activation code outstanding for this email contact" (assertion "did not match" in message.lower() failed).

(ARNs, generated names and the specific 65-char padding above are per-run examples, not requirements; the requirements are the status, the error code and the message templates.)

## Steps to Reproduce
Client: botocore `notificationscontacts`, region us-east-1, against the emulator.

Cause 1 & 2: CreateEmailContact(name=<unique>, emailAddress="not-an-email") -> inspect Error.Message. Then CreateEmailContact(name="a"*65, emailAddress="<unique>@example.com") -> inspect Error.Message. Both return ValidationException; compare the message to AWS's "Value at '<member>' ..." form and, for the email case, to the unanchored pattern.

Cause 3: CreateEmailContact a contact, then TagResource(arn=<arn>, tags={f"k{i}":"v" for i in range(201)}) -> ValidationException; compare Error.Message to "[object has too many properties (found 201 but schema requires at most 200)]".

Cause 4: CreateEmailContact a contact, DeleteEmailContact(arn), then GetEmailContact(arn) -> ResourceNotFoundException; compare Error.Message to "Email Contact with arn <arn> not found". Separately, DeleteEmailContact on a well-formed but never-created ARN -> same comparison.

Cause 5: CreateEmailContact a contact, SendActivationCode(arn=<arn>), then ActivateEmailContact(arn=<arn>, code="0000000") -> ValidationException; compare Error.Message to "Input token did not match stored Activation Token for arn <arn>".

Replay: run_behavioral_suite and re-run the five checks listed in trace_ids.
