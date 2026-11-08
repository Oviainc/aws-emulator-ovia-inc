Issue-Source: trace-sweep-divergence
Issue-Trace: notifications-tag-resource-list-tags-untag-roundtrip
Issue-Step: task#5
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: notifications-tag-resource-list-tags-untag-roundtrip
Issue-Shape-Id: com.amazonaws.notifications#Notifications
Issue-Key: notifications|TagResource|2xx->2xx|$headers.content-length
Issue-Key: notifications|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: 7a00204bd93c8ed9dd88a2629fdcaac9e0c865d8

# notifications tag-plane 204 responses omit the Content-Length header

> _Note: this issue was identified against the code base at commit `7a00204bd93c8ed9dd88a2629fdcaac9e0c865d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Both successful tag-plane mutations on the notifications service answer with the correct HTTP status (204) and the correct empty body, but the emulator emits NO Content-Length response header where the recorded AWS response carries one. Two root causes, both in the notifications service, both the same omission on a different operation's empty-output response path.

Cause 1 of 2 - TagResource's 204 response carries no Content-Length header.
AWS: Smithy (services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json): TagResource http {method: POST, uri: "/tags/{arn}"}, output shape TagResourceResponse with NO members (empty output); input arn -> NotificationConfigurationArn (@httpLabel, required, pattern ^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$) and tags -> TagMap (required, @length min 0 max 200); modeled errors AccessDeniedException 403, ValidationException 400, ResourceNotFoundException 404, ThrottlingException 429, InternalServerException 500. The model states nothing about response headers, so: recorded behavior only - the recorded success response is status 204 with an empty body and a Content-Length header present. The recording asserts the header's presence/shape ('<shaped-not-asserted>'), not a particular value; for an empty body it is the zero length AWS computes for that body, never a number copied from one run.
Emulator: status 204 and the empty body match, but the response carries no content-length header at all - replay reports "$headers.content-length: missing (expected '<shaped-not-asserted>', got None)".
Why: a client or HTTP intermediary that frames a response from Content-Length rather than from the status's implicit no-body rule, or that asserts on the header set of a successful mutation, sees a response AWS does not send; a proxy normalizing between HTTP/1.1 and HTTP/2 can be pushed into a different framing decision than it would make against real AWS, and emulator-based tests of header handling pass on behavior AWS does not exhibit.
Fix: the notifications response serializer for empty-output operations - the 204/no-content branch that writes TagResource's response - must set Content-Length for the empty body instead of dropping the header. This is almost certainly the same code path as Cause 2, so one change in the empty-output serialization path can close both; both must still be re-replayed.
Traces: notifications-tag-resource-list-tags-untag-roundtrip#task#5.

Cause 2 of 2 - UntagResource's 204 response carries no Content-Length header.
AWS: Smithy: UntagResource http {method: DELETE, uri: "/tags/{arn}"}, output shape UntagResourceResponse with NO members (empty output); input arn -> NotificationConfigurationArn (@httpLabel, required, same ARN pattern) and tagKeys -> TagKeys (@httpQuery "tagKeys", required, @length min 0 max 200); same modeled error set as TagResource; operation is @idempotent. Recorded behavior only: the recorded success response is status 204 with an empty body and a Content-Length header present.
Emulator: status 204 and the empty body match, but no content-length header is emitted - replay reports "$headers.content-length: missing (expected '<shaped-not-asserted>', got None)".
Why: same client-visible consequence as Cause 1, on the delete half of the tag plane: a successful untag answers with a header set AWS does not produce, so framing-sensitive clients and header assertions diverge from AWS.
Fix: the notifications response serializer for empty-output operations, UntagResource's 204/no-content branch - emit Content-Length for the empty body.
Traces: notifications-tag-resource-list-tags-untag-roundtrip#task#7.

Neither cause is a status, error-code or message divergence, so neither is covered by the open notifications issues 20261106T194201Z-notifications-the-published-managed-notification-catalog-diverges-from.md (the managed-notification catalog's entries and prose, which owns the third divergence cluster in this replay) or 20261104T055216Z-notifications-cloudwatch-alarm-state-changes-never-produce-a.md (alarm-driven NotificationEvents and ConflictException wording). The open issue 20261031T001043Z-notificationscontacts-tag-plane-204-responses-omit-the-content.md describes the identical omission but on the SEPARATE notificationscontacts service (EmailContactArn tag plane) and carries only that service's replay keys, so it does not cover these two notifications clusters. Hosts, ports, the generated notification-configuration ARN and the specific tag keys in the fixture are per-run values, not requirements.

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator (emulator_up: true, /__admin/health answered 200 and the data listener accepts; score_pct 75.0, 12 steps total, 9 passed, 3 mismatched, 0 errored/blocked, 3 divergence clusters, all service=notifications). One of the three clusters (ListManagedNotificationConfigurations) is owned_by the open issue 20261106T194201Z-notifications-the-published-managed-notification-catalog-diverges-from.md and is skipped. The two clusters below carry an empty stub_path and an empty owned_by, so they are net-new.

Cause 1 - cluster service=notifications|TagResource|status=204->204|keys=$headers.content-length, representative fixture notifications-tag-resource-list-tags-untag-roundtrip step task#5, affected_count 1:
  expected_status 204, actual_status 204 (status and body agree)
  $headers.content-length: missing (expected '<shaped-not-asserted>', got None)

Cause 2 - cluster service=notifications|UntagResource|status=204->204|keys=$headers.content-length, representative fixture notifications-tag-resource-list-tags-untag-roundtrip step task#7, affected_count 1:
  expected_status 204, actual_status 204 (status and body agree)
  $headers.content-length: missing (expected '<shaped-not-asserted>', got None)

Model consulted: services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json - TagResource (POST /tags/{arn}, output TagResourceResponse with no members) and UntagResource (DELETE /tags/{arn}, output UntagResourceResponse with no members). Neither operation models any response header, so the requirement is the recorded one: an empty 204 body accompanied by Content-Length.

## Steps to Reproduce
Client: botocore `notifications`, region us-east-1, endpoint = the emulator's data listener. Inspect the RAW HTTP response headers (e.g. a botocore before-send/after-call event hook, or an HTTP proxy in front of the endpoint), not just the parsed result - the divergence is header-only, the status and body already match.

Setup: CreateNotificationConfiguration(name=<unique>, description=<any>) -> arn matching ^arn:aws:notifications::<account>:configuration/<27 lowercase alphanumerics>$.

Cause 1 (TagResource): call TagResource(arn=<arn>, tags={"k": "v"}). The response is HTTP 204 with an empty body on both sides; list the response headers and look for content-length. The emulator sends none; the recorded AWS response has the header present.

Cause 2 (UntagResource): with the same configuration tagged as above, call UntagResource(arn=<arn>, tagKeys=["k"]). The response is HTTP 204 with an empty body; list the response headers and look for content-length - absent on the emulator.

Replay: call replay_wire_corpus and re-run fixture notifications-tag-resource-list-tags-untag-roundtrip, checking both step task#5 (TagResource) and step task#7 (UntagResource). Both must pass; closing only one leaves the other cause live.
