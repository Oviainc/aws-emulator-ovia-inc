Issue-Source: trace-sweep-divergence
Issue-Trace: mgn-network-migration-definition-tag-round-trip
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mgn-network-migration-definition-tag-round-trip
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn|TagResource|2xx->2xx|$headers.content-length
Issue-Key: mgn|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: be3d4fabea2ba96776d6b09377dcc7f47a91fefd

# mgn empty-body 204 tag responses omit the Content-Length header (2 causes)

> _Note: this issue was identified against the code base at commit `be3d4fabea2ba96776d6b09377dcc7f47a91fefd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The emulator's mgn tagging operations now return the right status (HTTP 204 with no payload) but serialize that empty response WITHOUT a Content-Length response header, where AWS always frames the empty body explicitly. Two causes, one per operation, both from the same response-serialization defect for a Unit-output operation.

Cause 1 of 2: TagResource (fixture mgn-network-migration-definition-tag-round-trip, step task#4).
AWS: Smithy (com.amazonaws.mgn, ApplicationMigrationService) models TagResource as POST /tags/{resourceArn} with output shape Unit (no output members) and @idempotent; the @http code is defaulted (200) and the recorded AWS response for this step is HTTP 204 with an empty body. For the header itself the model says nothing - recorded behavior only: the recording shows AWS answering this empty-body 204 WITH a Content-Length header present (the corpus asserts presence, not a value, so the recorded value is not part of the requirement; the requirement is that an empty-body mgn response carries an explicit Content-Length whose value is the byte length of the body, i.e. 0 for an empty body). Status 204 and the absence of a body are matched by the emulator and are not at issue here.
Emulator: replay reports "$headers.content-length: missing (expected '<shaped-not-asserted>', got None)" - the emulator answers HTTP 204 with the header entirely absent, so the response's length is never declared.
Why: a client that frames responses off Content-Length (strict HTTP clients, proxies, and SDK/transport layers that distinguish "zero-length body" from "length unknown") sees an undelimited response for a call AWS delimits explicitly. Against a keep-alive connection that is the difference between a cleanly completed response and one whose end is inferred, so a tagging round-trip that works against AWS can hang, be read as chunked/EOF-terminated, or trip a strict-framing assertion against the emulator.
Fix: belongs in the mgn response serializer / HTTP response-writing layer shared by Unit-output operations, not in the TagResource business logic - the empty-body (204) path must emit Content-Length: 0 rather than omitting the header. Note the open issue "mgn tag API and CreateLaunchConfigurationTemplate answer 501 NotImplemented, and the account-initialization gate is not modeled (3 causes)" covers the tag API answering 501 at all; it does not cover the framing of the now-served 204 response, which is what this issue is about.
Traces: fixture mgn-network-migration-definition-tag-round-trip, step task#4 (sole member of its cluster).

Cause 2 of 2: UntagResource (fixture mgn-network-migration-definition-tag-round-trip, step task#6).
AWS: Smithy models UntagResource as DELETE /tags/{resourceArn} with tagKeys bound to the httpQuery "tagKeys", output shape Unit, @idempotent, @http code defaulted (200); the recorded AWS response is HTTP 204 with an empty body and, recorded behavior only, a Content-Length header present. Same requirement as Cause 1: the empty-body response declares its length (0).
Emulator: replay reports "$headers.content-length: missing (expected '<shaped-not-asserted>', got None)" - HTTP 204, no Content-Length header.
Why: identical client impact to Cause 1, on the untag half of the round trip, so a tag/untag sequence diverges from AWS in response framing on both of its mutating calls.
Fix: same serializer path as Cause 1 (empty/Unit-output response writer for mgn); fixing the shared writer should close both causes, but both steps must be re-replayed to confirm.
Traces: fixture mgn-network-migration-definition-tag-round-trip, step task#6 (sole member of its cluster).

## Evidence
Replay of the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout (emulator_up: true, bootstrap_signal: false).

Cause 1 - fixture mgn-network-migration-definition-tag-round-trip, step task#4, operation TagResource, service mgn, verdict mismatch:
  expected_status 204, actual_status 204 (status matches)
  detail verbatim: "$headers.content-length: missing (expected '<shaped-not-asserted>', got None)"
  divergence_key: service=mgn|TagResource|status=204->204|keys=$headers.content-length

Cause 2 - fixture mgn-network-migration-definition-tag-round-trip, step task#6, operation UntagResource, service mgn, verdict mismatch:
  expected_status 204, actual_status 204 (status matches)
  detail verbatim: "$headers.content-length: missing (expected '<shaped-not-asserted>', got None)"
  divergence_key: service=mgn|UntagResource|status=204->204|keys=$headers.content-length

Expected (recorded AWS, both): HTTP 204, empty body, Content-Length header PRESENT (value asserted only as shaped, i.e. presence is the requirement; for an empty body the correct value is 0).
Actual (emulator, both): HTTP 204, empty body, NO Content-Length header at all.

Model support: TagResource POST /tags/{resourceArn} and UntagResource DELETE /tags/{resourceArn}, both output_shape Unit (no members), both @idempotent, @http code defaulted; errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400).

## Steps to Reproduce
1. git_clone the mockforge-aws-emulator repo for this run and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus), or replay fixture mgn-network-migration-definition-tag-round-trip alone.
3. Observe step task#4: POST /tags/{resourceArn} (TagResource) against the mgn endpoint. The emulator answers HTTP 204 with an empty body and no Content-Length response header; the recorded AWS response for the same step carries a Content-Length header.
4. Observe step task#6: DELETE /tags/{resourceArn}?tagKeys=... (UntagResource). Same divergence - HTTP 204 with the Content-Length header absent.
5. Inspect the raw response headers (e.g. curl -i) to confirm Content-Length is missing rather than present-with-another-value.
6. After the fix, re-replay BOTH mgn-network-migration-definition-tag-round-trip#task#4 and mgn-network-migration-definition-tag-round-trip#task#6 to confirm each empty-body 204 now declares Content-Length: 0.
