Issue-Source: trace-sweep-divergence
Issue-Trace: notificationscontacts-tag-operations-on-nonexistent-contact-not-found
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: notificationscontacts-tag-operations-on-nonexistent-contact-not-found notificationscontacts-tag-resource-exceeds-max-entries-rejected notificationscontacts-tag-resource-then-list-tags-roundtrip notificationscontacts-untag-resource-nonexistent-key-noop notificationscontacts-untag-resource-removes-specific-key
Issue-Shape-Id: com.amazonaws.notificationscontacts#NotificationsContacts
Issue-Key: notificationscontacts|TagResource|2xx->2xx|status
Issue-Key: notificationscontacts|UntagResource|2xx->2xx|status
Issue-Commit: e0f3b342c2995e24bb81f3397837b3776f1c553b

# notificationscontacts TagResource and UntagResource answer 200 instead of 204

> _Note: this issue was identified against the code base at commit `e0f3b342c2995e24bb81f3397837b3776f1c553b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The two tag-mutation operations of notifications-contacts return an empty body with HTTP 200 where AWS answers HTTP 204 No Content. Two causes, one per operation, same serializer fault.

Cause 1 of 2 - TagResource success status.
AWS: Smithy (com-amazonaws-notificationscontacts-notificationscontacts.json): TagResource, http {method: POST, uri: "/tags/{arn}"}, output shape TagResourceResponse has NO members and the operation carries no explicit @http code (code 200 is the protocol default, reported as codeDefaulted). Recorded behavior only: a successful TagResource on an existing email-contact ARN answers status 204 with an empty body - the service narrows the bodyless success to 204 rather than taking the restJson1 default of 200.
Emulator: answers status 200 (empty body) for the same successful call.
Why: a client or SDK middleware that branches on the success status (treating 204 as "no content, do not attempt to parse", or asserting 204 for a bodyless mutation) sees a different status than AWS returns; HTTP caches, proxies and conditional-logic layers that distinguish 200-with-empty-body from 204 behave differently against the emulator, and tests written against the emulator would not catch a real 204 expectation.
Fix: the TagResource handler's response construction / the service's restJson1 response serializer for bodyless outputs - emit 204 for this operation's success instead of the defaulted 200.
Traces: fixture notificationscontacts-tag-operations-on-nonexistent-contact-not-found step task#5 (representative); the same cluster also covers notificationscontacts-tag-resource-exceeds-max-entries-rejected#task#4, notificationscontacts-tag-resource-then-list-tags-roundtrip#task#2, notificationscontacts-untag-resource-nonexistent-key-noop#setup#2 and notificationscontacts-untag-resource-removes-specific-key#setup#2.

Cause 2 of 2 - UntagResource success status.
AWS: Smithy: UntagResource, http {method: DELETE, uri: "/tags/{arn}"}, output shape UntagResourceResponse has NO members, no explicit @http code (200 defaulted). Recorded behavior only: a successful UntagResource answers status 204 with an empty body, both when the supplied tagKeys exist and when a supplied key is absent (the absent-key case is a no-op success, still 204).
Emulator: answers status 200 (empty body) in both the removing and the no-op case.
Why: same client impact as cause 1 - the bodyless-success status is part of the wire contract for this operation and a 200 misreports it, including for the idempotent no-op removal path.
Fix: the UntagResource handler's response construction / the same bodyless-output serializer path.
Traces: fixture notificationscontacts-untag-resource-nonexistent-key-noop step task#3 (representative of this cause); the same cluster also covers notificationscontacts-untag-resource-removes-specific-key#task#3.

No open issue covers either cause: the only similar filing is 20261023T000909Z-notificationscontacts-error-message-fidelity-divergences-in-validation-and, which is about ValidationException/ResourceNotFoundException message wording and says nothing about success status codes. (The open Wisdom issue 20261029T194637Z-wisdom-tagresource-and-untagresource-answer-200-instead-of describes the same shape of fault in a different service and does not cover notificationscontacts.)

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator (score 67.6%, 12/37 steps mismatched):

Cause 1 - cluster service=notificationscontacts|TagResource|status=204->200|keys=status: expected_status 204, actual_status 200, detail "status: expected 204, got 200". Representative step notificationscontacts-tag-operations-on-nonexistent-contact-not-found#task#5; affected_count 5 (also notificationscontacts-tag-resource-exceeds-max-entries-rejected#task#4, notificationscontacts-tag-resource-then-list-tags-roundtrip#task#2, notificationscontacts-untag-resource-nonexistent-key-noop#setup#2, notificationscontacts-untag-resource-removes-specific-key#setup#2). No stub_path and no owned_by, so the failure is net-new.

Cause 2 - cluster service=notificationscontacts|UntagResource|status=204->200|keys=status: expected_status 204, actual_status 200, detail "status: expected 204, got 200". Representative step notificationscontacts-untag-resource-nonexistent-key-noop#task#3; affected_count 2 (also notificationscontacts-untag-resource-removes-specific-key#task#3). No stub_path, no owned_by.

Only the status diverges in both clusters: the body is empty on both sides and no other JSON pointer or header was reported. Contact ARNs, tag keys and generated names in the fixtures are per-run values, not requirements; the requirement is the 204 status on a successful bodyless tag mutation.

## Steps to Reproduce
Client: botocore `notificationscontacts`, region us-east-1, endpoint = the emulator's data listener.

Cause 1: CreateEmailContact(name=<unique>, emailAddress="<unique>@example.com") -> take the returned arn. Call TagResource(arn=<arn>, tags={"k1": "v1"}) and inspect ResponseMetadata.HTTPStatusCode: AWS/the fixture records 204, the emulator returns 200.

Cause 2: with the same contact tagged, call UntagResource(arn=<arn>, tagKeys=["k1"]) and then UntagResource(arn=<arn>, tagKeys=["does-not-exist"]); inspect ResponseMetadata.HTTPStatusCode on both: 204 expected, 200 returned.

Replay: replay_wire_corpus and re-run fixtures notificationscontacts-tag-operations-on-nonexistent-contact-not-found (task#5) and notificationscontacts-untag-resource-nonexistent-key-noop (task#3).
