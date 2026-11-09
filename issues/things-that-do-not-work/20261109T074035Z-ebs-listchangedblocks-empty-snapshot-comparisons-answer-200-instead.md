Issue-Source: trace-sweep-divergence
Issue-Trace: ebs-list-changed-blocks-rejections
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ebs-list-changed-blocks-rejections ebs-list-changed-blocks-pagination-self-diff-and-not-found
Issue-Shape-Id: com.amazonaws.ebs#Ebs
Issue-Key: ebs|ListChangedBlocks|4xx->2xx|$
Issue-Key: ebs|ListChangedBlocks|4xx->2xx|$.Message
Issue-Key: ebs|ListChangedBlocks|4xx->2xx|$.Reason
Issue-Key: ebs|ListChangedBlocks|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: ebs|ListChangedBlocks|4xx->2xx|status
Issue-Key: ebs|ListChangedBlocks|2xx->2xx|$.BlockSize
Issue-Key: ebs|ListChangedBlocks|2xx->2xx|$.ExpiryTime
Issue-Key: ebs|ListChangedBlocks|2xx->2xx|$.VolumeSize
Issue-Commit: c0b46ff840e20ea6c77a7c2f29a71cdb0b5ecaa3

# ebs ListChangedBlocks: empty-snapshot comparisons answer 200 instead of ValidationException, and a no-changed-blocks page serializes members AWS omits

> _Note: this issue was identified against the code base at commit `c0b46ff840e20ea6c77a7c2f29a71cdb0b5ecaa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChangedBlocks

## Description
The EBS direct-API `ListChangedBlocks` handler (services/service-com-amazonaws-ebs-ebs/src/ops/list_changed_blocks.rs) treats a snapshot that holds no blocks as a perfectly comparable snapshot, and it fills every optional output member on every answer. Three recorded steps diverge; they are 3 distinct causes.

Cause 1 of 3: a SecondSnapshotId naming an EMPTY snapshot is not refused.
AWS: ListChangedBlocks models `ValidationException` (HTTP 400, errorKind client, `x-amzn-errortype: ValidationException`) with members `Message` and `Reason`, where `Reason` is one of `ValidationExceptionReason`'s values (Smithy: com.amazonaws.ebs#ListChangedBlocks errors -> ValidationException httpError 400; ValidationException members Message, Reason). The model does not spell out the empty-snapshot rule, so for that rule: recorded behavior only — the recording shows AWS refusing the comparison with HTTP 400, `Reason` = `INVALID_SNAPSHOT_ID` and `Message` = `Snapshot (<snapshotId>) is empty`. In that message the fixed text is `Snapshot (` ... `) is empty`; the only variable part is the snapshot id taken verbatim from the request's `SecondSnapshotId` path label (the recording's `snap-3a39389d74d909f36` is one run's value, not part of the template). A snapshot is "empty" when it carries no written blocks, i.e. nothing was ever put into it before it was completed.
Emulator: the request is answered HTTP 200 with a success body `{"ChangedBlocks": [], "ExpiryTime": ..., "VolumeSize": 1, "BlockSize": 524288}`; no `Message`, no `Reason`, and no `x-amzn-errortype` header. The step's poll ran 37 attempts to its 180 s cap waiting for the 400 that never comes. The handler's `resolve()` only distinguishes malformed ids (INVALID_SNAPSHOT_ID), unknown/not-final ids (404 SNAPSHOT_NOT_FOUND) and unrelated lineages (UNRELATED_SNAPSHOTS) — emptiness is never checked.
Why: a client that uses the refusal to detect "this snapshot has no content to diff" instead receives a successful empty diff and cannot distinguish an empty snapshot from two identical snapshots; retry/validation branches keyed on `Reason == INVALID_SNAPSHOT_ID` never run, and error-handling paths are silently dead against the emulator.
Fix: validation in the ListChangedBlocks handler (services/service-com-amazonaws-ebs-ebs/src/ops/list_changed_blocks.rs) — after resolving the second snapshot, refuse an empty one with the modeled ValidationException via the existing `validation("INVALID_SNAPSHOT_ID", ..)` helper, with the message text templated as above (a shared message builder in src/state.rs alongside `snapshot_not_found_message` / `unrelated_snapshots_message` is the natural home for its wording).
Traces: ebs-list-changed-blocks-rejections#task#3.

Cause 2 of 3: a FirstSnapshotId naming an EMPTY snapshot is not refused either.
AWS: same modeled error channel (Smithy: ListChangedBlocks -> ValidationException, HTTP 400, members Message/Reason). The rule itself is recorded behavior only: the recording shows HTTP 400 with `Reason` = `INVALID_SNAPSHOT_ID` and `Message` = `First snapshot should not be empty or null`. That message is entirely fixed text with no variable part — it names the member role ("First snapshot"), not the id — so it must be reproduced verbatim and not substituted with the id or with the Cause 1 wording.
Emulator: HTTP 200 with a success body, `Message` absent, `Reason` absent, `x-amzn-errortype` absent. `resolve(&store, "firstSnapshotId", ..)` accepts the empty snapshot and `changed_indexes()` then reports an empty (or whole-second-snapshot) diff.
Why: the two refusals are distinct contracts in AWS — which side was empty is carried only by the message — so a client surfacing or branching on them gets neither, and an invalid comparison is reported as a valid one.
Fix: validation in the same handler, on the `FirstSnapshotId` branch, emitting the fixed `First snapshot should not be empty or null` text with `Reason` `INVALID_SNAPSHOT_ID`.
Traces: ebs-list-changed-blocks-rejections#task#4.

Cause 3 of 3: a page carrying no changed blocks still serializes ExpiryTime, VolumeSize and BlockSize.
AWS: the model marks `ChangedBlocks`, `ExpiryTime`, `VolumeSize`, `BlockSize` and `NextToken` all optional on `ListChangedBlocksResponse` (Smithy: ListChangedBlocks output — no member required; `ExpiryTime` is documented as "The time when the BlockToken expires"). The presence rule is recorded behavior only: the recording of a 200 answer that reports no changed blocks (a self-diff / exhausted comparison) carries the empty `ChangedBlocks` array and OMITS `ExpiryTime`, `VolumeSize` and `BlockSize` entirely — consistent with `ExpiryTime` existing only when block tokens were actually handed out. Hypothesis: AWS populates the volume/block geometry and token expiry only alongside returned block tokens.
Emulator: those three members are always present. The replayed step reports `$.BlockSize: type_mismatch (expected None, got 524288)`, `$.VolumeSize: expected None, got 1`, `$.ExpiryTime: expected None, got <epoch>` — the handler's final `Ok(ListChangedBlocksOutput { .. })` unconditionally wraps `expiry_time`, `volume_size` and `block_size` in `Some(..)`.
Why: a client that tests member presence to decide whether any block token came back (or that asserts on an exactly-shaped empty-diff response) reads geometry and an expiry for tokens that were never issued, so empty-result handling diverges from AWS.
Fix: the ListChangedBlocks serializer/handler output construction in services/service-com-amazonaws-ebs-ebs/src/ops/list_changed_blocks.rs — set `expiry_time`, `volume_size` and `block_size` only when the page actually reports changed blocks.
Traces: ebs-list-changed-blocks-pagination-self-diff-and-not-found#task#8.

Note on existing issues: the open issue `20261109T031246Z-ebs-listchangedblocks-stub.md` ("Implement ebs ListChangedBlocks: unimplemented scaffold stub") is a behavioral-corpus issue against an older commit; the operation is now implemented (it answers 200, and the replay reports no stub for these steps), so it does not cover these three wire divergences. The open `20261109T031246Z-ebs-2-net-new-causes-a-mismatched-changedblockscount.md` and `20261105T142645Z-ebs-three-more-direct-api-checks-hang-at.md` concern CompleteSnapshot/PutSnapshotBlock and read-path convergence, not these ListChangedBlocks refusals or the empty-page response shape.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (61 steps, 58 passed, 3 mismatched):

1) ebs-list-changed-blocks-rejections#task#3 — ListChangedBlocks
   expected: HTTP 400, body {"Message": "Snapshot (snap-3a39389d74d909f36) is empty", "Reason": "INVALID_SNAPSHOT_ID"}, header x-amzn-errortype present
   actual:   HTTP 200, body {"ChangedBlocks": [], "ExpiryTime": 1794213356.1845353, "VolumeSize": 1, "BlockSize": 524288}
   detail:   status: expected 400, got 200; $.Message missing; $.Reason missing; $headers.x-amzn-errortype missing; poll: status 200 not in [400] after 37 attempt(s) (capped at 180s)

2) ebs-list-changed-blocks-rejections#task#4 — ListChangedBlocks
   expected: HTTP 400, body {"Message": "First snapshot should not be empty or null", "Reason": "INVALID_SNAPSHOT_ID"}, header x-amzn-errortype present
   actual:   HTTP 200 success body; $.Message missing, $.Reason missing, $headers.x-amzn-errortype missing

3) ebs-list-changed-blocks-pagination-self-diff-and-not-found#task#8 — ListChangedBlocks
   expected: HTTP 200 with ExpiryTime / VolumeSize / BlockSize ABSENT
   actual:   HTTP 200 with $.BlockSize = 524288, $.VolumeSize = 1, $.ExpiryTime = 1794213292.3293514 (detail: type_mismatch, expected None)

Model support (services/service-com-amazonaws-ebs-ebs/smithy-model/com-amazonaws-ebs-ebs.json): ListChangedBlocks GET /snapshots/{SecondSnapshotId}/changedblocks, errors include ValidationException (client, httpError 400) with members Message and Reason; output members ChangedBlocks/ExpiryTime/VolumeSize/BlockSize/NextToken are all optional.

Code: services/service-com-amazonaws-ebs-ebs/src/ops/list_changed_blocks.rs — `resolve()` (lines 327-346) checks only malformed-id, not-found and readability; the lineage check (lines 225-232) covers only UNRELATED_SNAPSHOTS; the success construction (lines 301-317) sets expiry_time/volume_size/block_size to `Some(..)` unconditionally.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture ebs-list-changed-blocks-rejections, step task#3 — call ListChangedBlocks with SecondSnapshotId naming a completed snapshot that holds no blocks. Observe HTTP 200 with an empty ChangedBlocks body instead of HTTP 400 ValidationException with Reason INVALID_SNAPSHOT_ID and Message "Snapshot (<snapshotId>) is empty".
3. Cause 2: replay the same fixture, step task#4 — call ListChangedBlocks with FirstSnapshotId naming an empty snapshot. Observe HTTP 200 instead of HTTP 400 with Reason INVALID_SNAPSHOT_ID and Message "First snapshot should not be empty or null".
4. Cause 3: replay fixture ebs-list-changed-blocks-pagination-self-diff-and-not-found, step task#8 — call ListChangedBlocks so the comparison reports no changed blocks (e.g. the same snapshot on both sides, or a page past the last changed index). Observe ExpiryTime, VolumeSize and BlockSize present in the 200 body where AWS omits all three.
