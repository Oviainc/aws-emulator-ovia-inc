Issue-Source: trace-sweep-divergence
Issue-Trace: notificationscontacts-tag-operations-on-nonexistent-contact-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: notificationscontacts-tag-operations-on-nonexistent-contact-not-found
Issue-Shape-Id: com.amazonaws.notificationscontacts#NotificationsContacts
Issue-Key: notificationscontacts|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: notificationscontacts|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: notificationscontacts|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 07e9367022a66882e4e3c540bf6dceaeefec3f72

# notificationscontacts tag-plane 404 responses emit a wrongly shaped x-amzn-errortype header

> _Note: this issue was identified against the code base at commit `07e9367022a66882e4e3c540bf6dceaeefec3f72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
All three tag-plane operations of this service now return the correct HTTP status (404), the correct error code (ResourceNotFoundException) and the correct message for a well-formed but nonexistent email-contact ARN, but each of them serializes an x-amzn-errortype response header whose VALUE SHAPE is not the one the recorded AWS response carries. Three root causes, one per operation's error-response path; they are almost certainly the same shared restJson1 error serializer reached from three handlers, so one change can close all three, but each carries its own replay key and each must be re-replayed.

NOTE ON OWNERSHIP: these three clusters were previously part of the issue 20261029T201129Z-notificationscontacts-refusal-messages-and-error-headers-diverge-from.md, which is now RESOLVED (status fixed). That fix closed the $.message half of its Cause 2 — the message is no longer the crate-authored 'no email contact exists with the arn <arn>' and no longer diverges — but left the x-amzn-errortype half of the same cause open: replay still reports the header divergence on all three operations with no owned_by and no stub_path. A resolved issue does not cover a still-failing step, so these are filed here afresh. Not covered by the open issue 20261031T001043Z-notificationscontacts-tag-plane-204-responses-omit-the-content.md either (that one is about Content-Length on successful 204 responses, not about 4xx error headers), nor by 20261023T000909Z-notificationscontacts-error-message-fidelity-divergences-in-validation-and.md (message bodies only).

Cause 1 of 3 — TagResource's 404 response carries a bare-shape-name x-amzn-errortype.
AWS: Smithy (services/service-com-amazonaws-notificationscontacts-notificationscontacts/smithy-model/com-amazonaws-notificationscontacts-notificationscontacts.json): TagResource http {method: POST, uri: "/tags/{arn}"}, input arn -> EmailContactArn (@httpLabel, required, @pattern "^arn:aws:notifications-contacts::[0-9]{12}:emailcontact/[a-z0-9]{27}$"), tags -> TagMap (required, @length min 0 max 200); modeled errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400). The model says nothing about the x-amzn-errortype header's value form, so: recorded behavior only — the recorded response for a nonexistent contact ARN is status 404, error code ResourceNotFoundException, message the fixed text "Resource was not found" (no variable part), AND an x-amzn-errortype header whose value is present but does NOT have the shape of a bare PascalCase exception name; the recording asserts the header's shape, not a literal value. Requirement: the header AWS sends for this refusal class, in the value form AWS uses for it; the status (404), the code (ResourceNotFoundException) and the message text are the requirements the response already meets.
Emulator: status 404, code ResourceNotFoundException and the message all agree, but the response header is x-amzn-errortype: ResourceNotFoundException — replay reports "$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')". Hypothesis: the generic restJson1 error serializer writes the bare Smithy shape name, where the recorded AWS response uses a decorated form for this value (the fully-qualified shape id "com.amazonaws.notificationscontacts#ResourceNotFoundException", or the Coral-suffixed "ResourceNotFoundException:<uri>" form) — the recording only pins the shape, so the exact decoration must be read off the fixture, not guessed from this issue.
Why: a client that reads the error TYPE off the x-amzn-errortype header instead of off the JSON body (botocore does consult this header, as do many hand-rolled and non-AWS-SDK clients and HTTP-level error routers) parses a value in a form AWS does not send for this service's tag plane. A client whose parser strips a '#'-namespace prefix or a ':'-suffix before matching gets a different token against the emulator than against AWS, so error-type dispatch that works in test fails in production, and emulator-based tests of header-driven error handling pass on behavior AWS does not exhibit.
Fix: TagResource's error-response serializer for notificationscontacts (the restJson1 error path that stamps the x-amzn-errortype header for a modeled exception) — emit the header value in the form the recording carries rather than the bare shape name. Shared with Causes 2 and 3.
Traces: notificationscontacts-tag-operations-on-nonexistent-contact-not-found#task#1.

Cause 2 of 3 — ListTagsForResource's 404 response carries the same bare-shape-name x-amzn-errortype.
AWS: Smithy: ListTagsForResource http {method: GET, uri: "/tags/{arn}"}, @readonly, input arn -> EmailContactArn (@httpLabel, required, same @pattern), output tags -> TagMap; binds the same five errors including ResourceNotFoundException(404). Recorded behavior only: a GET of a well-formed but nonexistent contact ARN answers 404 / ResourceNotFoundException with the x-amzn-errortype header in a value form that is not the bare exception name (shape-asserted, as above).
Emulator: status 404 and the body agree; the response sends x-amzn-errortype: ResourceNotFoundException — replay reports "$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')".
Why: same client-visible consequence as Cause 1, on the read half of the tag plane — a header-driven error classifier sees a token in the wrong form for an ordinary not-found, which is the single most commonly branched-on error of a read operation.
Fix: ListTagsForResource's error-response serializer / the shared notificationscontacts restJson1 error header writer — same change as Cause 1.
Traces: notificationscontacts-tag-operations-on-nonexistent-contact-not-found#task#2.

Cause 3 of 3 — UntagResource's 404 response carries the same bare-shape-name x-amzn-errortype.
AWS: Smithy: UntagResource http {method: DELETE, uri: "/tags/{arn}"}, @idempotent, input arn -> EmailContactArn (@httpLabel, required, same @pattern), tagKeys -> TagKeys (@httpQuery "tagKeys", required); binds the same five errors including ResourceNotFoundException(404). Recorded behavior only: a DELETE against a nonexistent contact ARN answers 404 / ResourceNotFoundException with the x-amzn-errortype header in the non-bare value form (shape-asserted).
Emulator: status 404 and the body agree; the response sends x-amzn-errortype: ResourceNotFoundException — replay reports "$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')".
Why: same as Causes 1 and 2, on the delete half of the tag plane. Because UntagResource is @idempotent, retry wrappers frequently decide retry-vs-give-up from the error type read off this header; a wrongly shaped value changes that decision relative to real AWS.
Fix: UntagResource's error-response serializer / the shared notificationscontacts restJson1 error header writer — same change as Cause 1.
Traces: notificationscontacts-tag-operations-on-nonexistent-contact-not-found#task#3.

The nonexistent ARN used by the fixture (arn:aws:notifications-contacts::000000000000:emailcontact/<27 lowercase alphanumerics>), the emulator host/port and the per-run request ids are per-run values, not requirements. The requirements are the 404 status, the ResourceNotFoundException code, the fixed message text "Resource was not found", and the x-amzn-errortype header value FORM AWS uses for this refusal.

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator (emulator_up: true, "/__admin/health answered 200 and the :36485 data listener accepts"; score_pct 78.4, 8 fixtures replayed, 37 steps total, 29 passed, 8 mismatched, 0 errored, 0 blocked, 5 divergence clusters, all notificationscontacts). Two of the five clusters (TagResource 204->204 and UntagResource 204->204 on $headers.content-length) are owned_by 20261031T001043Z-notificationscontacts-tag-plane-204-responses-omit-the-content.md and are skipped. The three clusters below carry stub_path "" and owned_by [] — net-new.

Cause 1 — cluster divergence_key "service=notificationscontacts|TagResource|status=404->404|keys=$headers.x-amzn-errortype", fixture notificationscontacts-tag-operations-on-nonexistent-contact-not-found, step task#1, affected_count 1:
  expected_status 404, actual_status 404 (status, error code and message all agree)
  detail: $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')

Cause 2 — cluster divergence_key "service=notificationscontacts|ListTagsForResource|status=404->404|keys=$headers.x-amzn-errortype", same fixture, step task#2, affected_count 1:
  expected_status 404, actual_status 404
  detail: $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')

Cause 3 — cluster divergence_key "service=notificationscontacts|UntagResource|status=404->404|keys=$headers.x-amzn-errortype", same fixture, step task#3, affected_count 1:
  expected_status 404, actual_status 404
  detail: $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')

In all three the ONLY diverging key is $headers.x-amzn-errortype; the previously reported $.message divergence on these same three steps (recorded fixed text "Resource was not found" vs the emulator's "no email contact exists with the arn <arn>") no longer appears, confirming the message half of the resolved issue 20261029T201129Z-notificationscontacts-refusal-messages-and-error-headers-diverge-from.md was fixed while the header half was not.

Model consulted: services/service-com-amazonaws-notificationscontacts-notificationscontacts/smithy-model/com-amazonaws-notificationscontacts-notificationscontacts.json — TagResource (POST /tags/{arn}), ListTagsForResource (GET /tags/{arn}, @readonly), UntagResource (DELETE /tags/{arn}, @idempotent), each binding ResourceNotFoundException with httpError 404; EmailContactArn @pattern "^arn:aws:notifications-contacts::[0-9]{12}:emailcontact/[a-z0-9]{27}$".

## Steps to Reproduce
Client: botocore `notificationscontacts`, region us-east-1, endpoint = the emulator's data listener. The divergence is header-only, so inspect the RAW HTTP response headers (a botocore before-send / after-call event hook, or an HTTP proxy in front of the endpoint), not just the parsed error object.

Pick a well-formed but never-created contact ARN, e.g. arn:aws:notifications-contacts::000000000000:emailcontact/<27 lowercase alphanumerics> (it must satisfy EmailContactArn's @pattern so the request reaches the lookup rather than input validation).

Cause 1 (TagResource): call TagResource(arn=<that arn>, tags={"k": "v"}). The response is HTTP 404 with code ResourceNotFoundException and message "Resource was not found" on both sides; read the x-amzn-errortype response header. The emulator sends the bare shape name "ResourceNotFoundException"; the recorded AWS response carries that header in a different value form (shape-asserted in the fixture, so read the recorded form off notificationscontacts-tag-operations-on-nonexistent-contact-not-found task#1 rather than from this issue).

Cause 2 (ListTagsForResource): call ListTagsForResource(arn=<that arn>) and read the same header off the 404 response.

Cause 3 (UntagResource): call UntagResource(arn=<that arn>, tagKeys=["k"]) and read the same header off the 404 response.

Replay: replay_wire_corpus and re-run notificationscontacts-tag-operations-on-nonexistent-contact-not-found at task#1 (TagResource), task#2 (ListTagsForResource) and task#3 (UntagResource). All three must report no $headers.x-amzn-errortype divergence before the issue is closed.
