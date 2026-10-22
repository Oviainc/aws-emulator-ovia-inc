Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-sdi-source-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-sdi-source-lifecycle medialive-sdi-source-quad-mode-transition medialive-sdi-source-update-changes-type
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|CreateSdiSource|2xx->2xx|$.sdiSource.tags
Issue-Key: medialive|CreateSdiSource|2xx->2xx|status
Issue-Key: medialive|DescribeSdiSource|2xx->2xx|$.sdiSource.tags
Issue-Key: medialive|UpdateSdiSource|2xx->2xx|$.sdiSource.tags
Issue-Key: medialive|DeleteSdiSource|2xx->2xx|$.sdiSource.tags
Issue-Key: medialive|DeleteSdiSource|2xx->2xx|status
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# medialive SdiSource lifecycle returns wrong statuses and omits tags

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSdiSource

## Description
The medialive SdiSource operations answer with the wrong HTTP success status on create and delete, and every SdiSource projection omits the `tags` map. This bundle has FIVE causes; all must be closed.

Cause 1 of 5: CreateSdiSource returns HTTP 200 instead of 201, and omits sdiSource.tags (fixture medialive-sdi-source-lifecycle, step task#1).
AWS: recorded behavior — POST /prod/sdiSources answers HTTP 201 on success with sdiSource.tags present as `{}` for an untagged source. (Smithy declares `@http code 200` and models SdiSource with a Tags map; the recorded status is 201, and the recording governs the status.)
Emulator: answers HTTP 200 and leaves `sdiSource.tags` absent (null).
Why: a client asserting the created-resource status (201) sees a non-create, and one that iterates tags on the returned source dereferences null instead of reading an empty map.
Fix: the CreateSdiSource handler's success status plus the shared SdiSource response projection (always emit tags as a map).
Traces: medialive-sdi-source-lifecycle#task#1 (also medialive-sdi-source-quad-mode-transition#task#1, medialive-sdi-source-update-changes-type#task#1).

Cause 2 of 5: DescribeSdiSource omits sdiSource.tags (fixture medialive-sdi-source-lifecycle, step task#2).
AWS: recorded behavior with model support — SdiSource carries a Tags map and the recording shows `tags: {}`; HTTP 200.
Emulator: 200 with `sdiSource.tags` absent.
Why: as cause 1 — the read path is where clients normally inspect tags, so null there breaks tag-driven logic.
Fix: the DescribeSdiSource response projection (shared SdiSource serializer).
Traces: medialive-sdi-source-lifecycle#task#2 (also #task#5, #teardown#7, medialive-sdi-source-quad-mode-transition#task#3, medialive-sdi-source-update-changes-type#task#3 and #teardown#5).

Cause 3 of 5: UpdateSdiSource omits sdiSource.tags (fixture medialive-sdi-source-lifecycle, step task#4).
AWS: recorded behavior — the update echoes the SdiSource with `tags: {}`; HTTP 200.
Emulator: 200 with `sdiSource.tags` absent.
Why: as cause 1.
Fix: the UpdateSdiSource response projection (shared SdiSource serializer).
Traces: medialive-sdi-source-lifecycle#task#4 (also medialive-sdi-source-quad-mode-transition#task#2, medialive-sdi-source-update-changes-type#task#2).

Cause 4 of 5: DeleteSdiSource returns HTTP 202 instead of 200, and omits sdiSource.tags (fixture medialive-sdi-source-lifecycle, step teardown#6).
AWS: recorded behavior — DELETE /prod/sdiSources/{SdiSourceId} answers HTTP 200 with the SdiSource body including `tags: {}`. (Smithy declares `@http code 202` for this operation; the recorded status is 200, and the emulator currently follows the model.)
Emulator: answers HTTP 202 and omits `sdiSource.tags`.
Why: a client that distinguishes a completed delete (200) from an accepted-but-pending one (202) takes the wrong branch and polls a resource that is already gone, or treats the delete as asynchronous when it is not.
Fix: the DeleteSdiSource handler's success status plus the shared SdiSource response projection.
Traces: medialive-sdi-source-lifecycle#teardown#6 (also medialive-sdi-source-quad-mode-transition#teardown#4).

Cause 5 of 5: DeleteSdiSource returns HTTP 202 instead of 200 in the update-changes-type fixture (fixture medialive-sdi-source-update-changes-type, step teardown#4).
AWS: recorded behavior — HTTP 200 on delete.
Emulator: HTTP 202.
Why: as cause 4; this cluster isolates the status alone (no tags delta), so the status fix must hold for a source whose type was changed by an update too.
Fix: the DeleteSdiSource handler's success status.
Traces: medialive-sdi-source-update-changes-type#teardown#4.

## Evidence
Replay of simparity-wire-aws-v1 (5 clusters, verdict=mismatch):
- CreateSdiSource status=201->200: 'status: expected 201, got 200'; $.sdiSource.tags missing (expected {}, got None) (3 affected steps).
- DescribeSdiSource 200->200: $.sdiSource.tags missing (expected {}, got None) (6 affected steps).
- UpdateSdiSource 200->200: $.sdiSource.tags missing (expected {}, got None) (3 affected steps).
- DeleteSdiSource status=200->202: 'status: expected 200, got 202'; $.sdiSource.tags missing (2 affected steps).
- DeleteSdiSource status=200->202 (update-changes-type): 'status: expected 200, got 202'.
Smithy: CreateSdiSource POST /prod/sdiSources declares code 200 (recording: 201); DeleteSdiSource DELETE /prod/sdiSources/{SdiSourceId} declares code 202 (recording: 200); both outputs carry SdiSource (jsonName 'sdiSource'), whose Tags map the recording shows as {}.

## Steps to Reproduce
1. POST /prod/sdiSources with a name, mode and type — the response is HTTP 200 where the recording shows 201, and sdiSource.tags is absent.
2. GET /prod/sdiSources/<id> and PATCH /prod/sdiSources/<id> — sdiSource.tags is absent in both.
3. DELETE /prod/sdiSources/<id> — the response is HTTP 202 where the recording shows 200, and sdiSource.tags is absent.
4. Equivalent replay: fixture medialive-sdi-source-lifecycle step task#1, plus the steps in trace_ids.
