Issue-Source: trace-sweep-divergence
Issue-Trace: mgn-network-migration-definition-tag-round-trip
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mgn-network-migration-definition-tag-round-trip
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn|TagResource|2xx->2xx|status
Issue-Key: mgn|UntagResource|2xx->2xx|status
Issue-Commit: e2fddb9cb3d46e862d22e0996eb1ba458c40bdd2

# mgn TagResource and UntagResource answer HTTP 200 instead of the recorded 204 No Content (2 causes)

> _Note: this issue was identified against the code base at commit `e2fddb9cb3d46e862d22e0996eb1ba458c40bdd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two net-new root causes in the mgn (ApplicationMigrationService) crate, both on the tag-write success path: the operations now execute (they used to answer 501) but they conclude with the wrong HTTP success status. Each must be closed separately.

Cause 1 of 2: TagResource concludes a successful tag write with HTTP 200 instead of 204 No Content. Fixture mgn-network-migration-definition-tag-round-trip, step task#4.
AWS: Smithy (com.amazonaws.mgn#TagResource) binds POST /tags/{resourceArn} with resourceArn (ARN, httpLabel, required, @length 20..2048) and tags (TagsMap, required, sensitive) and declares output shape Unit — i.e. no output members at all; the model's http success code is DEFAULTED (codeDefaulted: true), so the model does not state the wire status and the recording is authoritative. Recorded behavior only: real MGN answers a successful TagResource with HTTP 204 and no response body (an empty-output restJson1 operation). There is no error code and no message on this path; the requirement is the status line, 204 No Content, with an empty body.
Emulator: HTTP 200 with an empty body. Replay detail: "status: expected 204, got 200" — no body or header keys diverged, only the status.
Why: 204 is the signal a client uses to know the response carries no entity. A client (or any HTTP intermediary, proxy or cache) that keys off the status treats 200 as a response that SHOULD have a body and will attempt to read/parse one, and code or tests asserting on the documented 204 for MGN's tag writes fail against the emulator even though the tag state was written correctly. It also makes the emulator's success status inconsistent with the rest of the restJson1 empty-output surface.
Fix: the operation's serializer/status wiring — the declared success status for the TagResource operation must be 204 for an empty (Unit) output, not the defaulted 200. The handler logic and state writes are correct and need no change.
Traces: mgn-network-migration-definition-tag-round-trip#task#4. The 200 comes from the operation's declared success status, services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/tag_resource.rs:160 (`const SUCCESS_STATUS: u16 = 200u16;`).

Cause 2 of 2: UntagResource concludes a successful tag removal with HTTP 200 instead of 204 No Content. Fixture mgn-network-migration-definition-tag-round-trip, step task#6.
AWS: Smithy (com.amazonaws.mgn#UntagResource) binds DELETE /tags/{resourceArn} with resourceArn (ARN, httpLabel, required) and tagKeys (httpQuery "tagKeys", required, sensitive), output shape Unit (no members), http success code DEFAULTED. Recorded behavior only: real MGN answers a successful UntagResource with HTTP 204 and an empty body; no error code or message is involved on this path.
Emulator: HTTP 200 with an empty body. Replay detail: "status: expected 204, got 200".
Why: same client consequence as Cause 1 — the emulator reports "OK, body follows" for a delete that returns no entity, so status-sensitive clients, intermediaries and assertions on MGN's documented tag-removal response diverge from AWS even though the keys were removed correctly.
Fix: the operation's serializer/status wiring — declare 204 as the success status for UntagResource's empty (Unit) output.
Traces: mgn-network-migration-definition-tag-round-trip#task#6. Declared success status at services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/untag_resource.rs:158 (`const SUCCESS_STATUS: u16 = 200u16;`).

Related open issues (neither covers these two causes): 20261023T003710Z-mgn-tag-api-and-createlaunchconfigurationtemplate-answer-501-notimplemented.md reported the tag family answering 501 NotImplemented — the handlers now run, so that cause's symptom is gone and this status divergence is what remains; 20261022T132055Z-mgn-initializeservice-returns-204-success-instead-of-modeled.md owns the separate InitializeService 204-vs-400 wire divergence, which is already filed and is NOT part of this issue.

## Evidence
Wire corpus simparity-wire-aws-v1 (42 steps: 9 passed, 26 mismatched, 7 blocked; 3 divergence clusters). Two of the three clusters are these, the third is already owned by an open issue.

[mismatch] fixture mgn-network-migration-definition-tag-round-trip, step task#4, operation TagResource (POST /tags/{resourceArn})
  divergence_key: service=mgn|TagResource|status=204->200|keys=status
  Expected (recorded AWS): HTTP 204, empty body.
  Actual (emulator):       HTTP 200, empty body.
  detail: "status: expected 204, got 200"
  Nothing else diverged on the step: no body key, no header key.

[mismatch] fixture mgn-network-migration-definition-tag-round-trip, step task#6, operation UntagResource (DELETE /tags/{resourceArn}?tagKeys=...)
  divergence_key: service=mgn|UntagResource|status=204->200|keys=status
  Expected (recorded AWS): HTTP 204, empty body.
  Actual (emulator):       HTTP 200, empty body.
  detail: "status: expected 204, got 200"

Model support (services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model/com-amazonaws-mgn-applicationmigrationservice.json):
  TagResource:   POST /tags/{resourceArn}, output_shape Unit, http.code 200 with codeDefaulted=true (model states no explicit success status); errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400).
  UntagResource: DELETE /tags/{resourceArn}, output_shape Unit, http.code 200 with codeDefaulted=true; same error set.
Emulator source: tag_resource.rs:160 and untag_resource.rs:158 both declare SUCCESS_STATUS = 200.

## Steps to Reproduce
1. git_clone the mockforge-aws-emulator repo and boot the emulator built from that checkout (health endpoint /__admin/health, data listener accepting).
2. Replay the wire corpus simparity-wire-aws-v1, or just fixture mgn-network-migration-definition-tag-round-trip.
3. Step task#4 (TagResource): the fixture creates/holds an mgn network-migration-definition resource and calls POST /tags/{resourceArn} with a tag map. The emulator answers HTTP 200; the recording expects HTTP 204 with an empty body.
4. Step task#6 (UntagResource): the fixture calls DELETE /tags/{resourceArn}?tagKeys=<key> for a key it just wrote. The emulator answers HTTP 200; the recording expects HTTP 204 with an empty body.
5. After the fix, re-replay mgn-network-migration-definition-tag-round-trip#task#4 AND mgn-network-migration-definition-tag-round-trip#task#6; BOTH must pass (status 204, empty body) before closing.
