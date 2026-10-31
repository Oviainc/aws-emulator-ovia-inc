Issue-Source: trace-sweep-divergence
Issue-Trace: notificationscontacts-tag-operations-on-nonexistent-contact-not-found
Issue-Step: task#5
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: notificationscontacts-tag-operations-on-nonexistent-contact-not-found notificationscontacts-tag-resource-exceeds-max-entries-rejected notificationscontacts-tag-resource-then-list-tags-roundtrip notificationscontacts-untag-resource-nonexistent-key-noop notificationscontacts-untag-resource-removes-specific-key
Issue-Shape-Id: com.amazonaws.notificationscontacts#NotificationsContacts
Issue-Key: notificationscontacts|TagResource|2xx->2xx|$headers.content-length
Issue-Key: notificationscontacts|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: f4f656b1bd7a513d31ed52598c20b4db3f989ef9

# notificationscontacts tag-plane 204 responses omit the Content-Length header

> _Note: this issue was identified against the code base at commit `f4f656b1bd7a513d31ed52598c20b4db3f989ef9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Both successful tag-plane mutations on this service answer with the right HTTP status (204) and the right empty body, but the emulator emits NO Content-Length response header where the recorded AWS response carries one. Two root causes, both in notificationscontacts, both the same omission on a different operation's empty-output response path.

Cause 1 of 2 - TagResource's 204 response carries no Content-Length header.
AWS: Smithy: TagResource http {method: POST, uri: "/tags/{arn}"}, output shape TagResourceResponse with NO members (empty output), input arn -> EmailContactArn (@httpLabel, required) and tags -> TagMap (required, @length min 0 max 200). The model states nothing about response headers, so: recorded behavior only - the recorded success response is status 204 with an empty body and a Content-Length header present (the recording asserts the header's presence/shape, not a particular value; for an empty body it is the zero length AWS computes for that body, never a value copied from one run).
Emulator: status 204 and the empty body match, but the response has no content-length header at all - replay reports "$headers.content-length: missing (expected '<shaped-not-asserted>', got None)".
Why: a client or proxy that frames the response from Content-Length rather than from the status's implicit no-body rule, or that asserts on the header set of a successful mutation, sees a response AWS does not send; an HTTP intermediary normalizing between HTTP/1.1 and HTTP/2 can be forced into a different framing decision than against real AWS, and emulator-based tests of header handling pass on behavior AWS does not exhibit.
Fix: the notificationscontacts response serializer for empty-output operations (the 204/no-content branch that writes TagResource's response) - set Content-Length for the empty body instead of dropping the header. Likely shared with Cause 2, so one change in the empty-output serialization path can close both.
Traces: notificationscontacts-tag-operations-on-nonexistent-contact-not-found#task#5; the same cluster also covers notificationscontacts-tag-resource-exceeds-max-entries-rejected#task#4 and notificationscontacts-tag-resource-then-list-tags-roundtrip#task#2, so it is not specific to one fixture's setup.

Cause 2 of 2 - UntagResource's 204 response carries no Content-Length header.
AWS: Smithy: UntagResource http {method: DELETE, uri: "/tags/{arn}"}, output shape UntagResourceResponse with NO members (empty output), input arn -> EmailContactArn (@httpLabel, required) and tagKeys -> TagKeys (@httpQuery "tagKeys", required). Recorded behavior only: the recorded success response is status 204 with an empty body and a Content-Length header present, both when the requested key existed and when it did not (the removal of an absent key is a no-op that still answers 204).
Emulator: status 204 and the empty body match, but no content-length header is emitted - replay reports "$headers.content-length: missing (expected '<shaped-not-asserted>', got None)".
Why: same client-visible consequence as Cause 1, on the delete half of the tag plane: a successful untag answers with a header set AWS does not produce, so framing-sensitive clients and header assertions diverge.
Fix: the notificationscontacts response serializer for empty-output operations, UntagResource's 204/no-content branch - emit Content-Length for the empty body.
Traces: notificationscontacts-untag-resource-nonexistent-key-noop#task#3; the same cluster also covers notificationscontacts-untag-resource-removes-specific-key#task#3.

Neither cause is a message or status divergence, so neither is covered by the open issue 20261029T201129Z-notificationscontacts-refusal-messages-and-error-headers-diverge-from.md (refusal wording and x-amzn-errortype on 4xx) nor by 20261023T000909Z-notificationscontacts-error-message-fidelity-divergences-in-validation-and.md; both of those concern error responses, while these two clusters are on successful 204 responses and carry their own replay keys. Hosts, ports, the generated contact ARNs and the specific tag keys in the fixtures are per-run values, not requirements.

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator (emulator_up: true, /__admin/health 200; score 73.0%, 10/37 steps mismatched, 7 divergence clusters, all notificationscontacts). Five of the seven clusters are owned_by the open issue 20261029T201129Z-notificationscontacts-refusal-messages-and-error-headers-diverge-from.md and are skipped; the two clusters below carry no stub_path and no owned_by, so they are net-new.

Cause 1 - cluster service=notificationscontacts|TagResource|status=204->204|keys=$headers.content-length, representative fixture notificationscontacts-tag-operations-on-nonexistent-contact-not-found step task#5, affected_count 3:
  expected_status 204, actual_status 204 (status and body agree)
  $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  other cluster members: notificationscontacts-tag-resource-exceeds-max-entries-rejected#task#4, notificationscontacts-tag-resource-then-list-tags-roundtrip#task#2

Cause 2 - cluster service=notificationscontacts|UntagResource|status=204->204|keys=$headers.content-length, representative fixture notificationscontacts-untag-resource-nonexistent-key-noop step task#3, affected_count 2:
  expected_status 204, actual_status 204 (status and body agree)
  $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  other cluster member: notificationscontacts-untag-resource-removes-specific-key#task#3

Model consulted: services/service-com-amazonaws-notificationscontacts-notificationscontacts/smithy-model/com-amazonaws-notificationscontacts-notificationscontacts.json - TagResource (POST /tags/{arn}, output TagResourceResponse with no members) and UntagResource (DELETE /tags/{arn}, output UntagResourceResponse with no members).

## Steps to Reproduce
Client: botocore `notificationscontacts`, region us-east-1, endpoint = the emulator's data listener. Inspect the raw HTTP response headers (e.g. via a botocore before-send/after-call event hook or an HTTP proxy), not just the parsed result, since the divergence is header-only.

Cause 1 (TagResource): CreateEmailContact(name=<unique>, emailAddress="<unique>@example.com") -> arn. Call TagResource(arn=<arn>, tags={"k": "v"}). The response is HTTP 204 with an empty body on both sides; list the response headers and check for content-length. The emulator sends none; the recording has the header present.

Cause 2 (UntagResource): with the same contact tagged as above, call UntagResource(arn=<arn>, tagKeys=["k"]) and again UntagResource(arn=<arn>, tagKeys=["does-not-exist"]). Both answer HTTP 204 with an empty body; list the response headers and check for content-length - absent on the emulator in both the real-removal and the no-op case.

Replay: replay_wire_corpus and re-run notificationscontacts-tag-operations-on-nonexistent-contact-not-found (task#5), notificationscontacts-tag-resource-exceeds-max-entries-rejected (task#4), notificationscontacts-tag-resource-then-list-tags-roundtrip (task#2), notificationscontacts-untag-resource-nonexistent-key-noop (task#3) and notificationscontacts-untag-resource-removes-specific-key (task#3).
