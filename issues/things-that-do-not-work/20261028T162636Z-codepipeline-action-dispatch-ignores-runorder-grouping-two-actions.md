Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.run-order-two-starts-only-after-run-order-one-finishes
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.run-order-two-starts-only-after-run-order-one-finishes|run#23|AssertionError|assert par_gap < seq_gap, (
Issue-Commit: eba09938c5757f116b88f59710a766eb7f22de4f

# CodePipeline action dispatch ignores runOrder grouping: two actions at the SAME runOrder are serialized with the same fixed stagger as a runOrder=1 -> runOrder=2 pair

> _Note: this issue was identified against the code base at commit `eba09938c5757f116b88f59710a766eb7f22de4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListActionExecutions

## Description
Cause 1 of 1: within a single stage, CodePipeline's scheduler does not treat runOrder as a concurrency grouping. The emulator walks a stage's actions one at a time in declaration order with a fixed per-action stagger, so a stage whose actions all declare runOrder=1 produces exactly the same start-time spacing as a stage that declares runOrder=1 then runOrder=2. Failing check: codepipeline.run-order-two-starts-only-after-run-order-one-finishes.

AWS: Smithy: ActionDeclaration.runOrder (integer, member doc "The order in which actions are run."); the action windows are observed through ListActionExecutions -> ActionExecutionDetail.startTime / lastUpdateTime. runOrder partitions a stage's actions into sequential GROUPS: all actions sharing a runOrder value are dispatched together and run concurrently, and a group with a higher runOrder is not dispatched until every action in the preceding group has reached its final lastUpdateTime. Declaration order and action-name order are not part of the contract — only runOrder is. Consequence that the model states and the recording relies on: for a stage with both actions at runOrder=1 the two startTime values must be close together (one dispatch), while for a stage with runOrder 1 then 2 the second startTime must be at or after the first action's lastUpdateTime. The strictly larger spread of the sequential stage over the parallel stage is what distinguishes real sequencing from uniform timestamp stamping. Smithy says nothing further about dispatch latency, so the concurrency requirement beyond that is recorded behavior only: the recording shows the parallel pair starting materially closer together than the sequential pair.

Emulator: both stages are staggered identically. For the control pipeline whose Deploy stage declares Alpha at runOrder=1 AND Bravo at runOrder=1, the emulator reported start times 5.013s apart — and in DECLARATION order, not runOrder order: Bravo (declared first) started at epoch 1793204539.269 and Alpha at 1793204544.282, i.e. the runOrder=1 pair that must run concurrently was serialized, with Bravo ahead of Alpha purely because it appears first in the `stages[].actions[]` list. The sequential pipeline's gap was 5.013s as well, the identical value, so `par_gap < seq_gap` is false (assertion: "parallel gap 5.013s is not less than sequential gap 5.013s"). The equality of the two gaps shows the stagger is a constant per-action cadence applied to the action list, not a consequence of runOrder: the non-overlap the earlier, weaker assertion saw in the sequential pipeline is a property of the emulator's uniform dispatch cadence and would hold for ANY runOrder assignment, including an inverted one.

Why: a client relying on AWS's behavior gets two things wrong. (1) Fan-out is lost: a stage that deliberately puts N independent actions at the same runOrder to run them in parallel is executed serially, so stage wall-clock time grows linearly in N and any test or pipeline that depends on concurrent actions (parallel deploys to several regions/buckets, a build plus a lint at one runOrder) is mis-timed and can exceed its own deadline. (2) runOrder is not actually enforced: because sequencing comes from list position rather than runOrder, a declaration whose runOrder disagrees with declaration order is run in the wrong order, and a client validating ordering guarantees against ListActionExecutions timestamps sees the correct-looking result for the wrong reason — the emulator will happily run a runOrder=2 action before a runOrder=1 action whenever the runOrder=2 action is declared first.

Fix: the codepipeline stage/action dispatch scheduler in the service's execution engine (the component that advances a stage to its next action and writes ActionExecutionDetail.startTime / lastUpdateTime). It must bucket a stage's actions by their declared runOrder, dispatch each bucket as a unit (all actions in a bucket started without waiting for a sibling in the same bucket, so their startTimes coincide), and only begin the next-higher bucket once every action in the current bucket is terminal — rather than iterating `actions` in list order on a fixed interval. GetPipeline must keep echoing the declared runOrders (it did: the check's declaration readback assertions were not the ones that failed), so the defect is purely in dispatch, not in declaration storage.

Traces: codepipeline.run-order-two-starts-only-after-run-order-one-finishes. Related but distinct open issues, neither of which covers this cause: 20261025T032228Z-codepipeline-execution-engine-still-stalls-an-action-never.md (actions stuck InProgress / budget exhaustion — here both pipelines DID reach Succeeded and both artifacts were delivered) and 20261028T112857Z-codepipeline-execution-plane-5-net-new-root-causes.md (artifact-store CMK, approval attribution, source-failure classification — nothing about intra-stage concurrency).

## Evidence
Check codepipeline.run-order-two-starts-only-after-run-order-one-finishes, verdict [mismatch].

Setup: two V2/QUEUED pipelines, each Source(S3) -> one "Deploy" stage holding two S3 Deploy actions, with "Bravo" declared FIRST in both so declaration order and alphabetical order both disagree with runOrder.
  - sequential pipeline: Bravo runOrder=2, Alpha runOrder=1
  - parallel control:   Bravo runOrder=1, Alpha runOrder=1
Both executions reached Succeeded and both deployed the exact source bytes to <pipeline>/alpha.zip and <pipeline>/bravo.zip, so the comparison is on real, completed action windows read from ListActionExecutions (startTime / lastUpdateTime, matched by actionName).

Expected (AWS): par_gap = |parallel Alpha.startTime - parallel Bravo.startTime| must be strictly LESS than seq_gap = sequential Bravo.startTime - sequential Alpha.startTime, because the parallel control's two runOrder=1 actions are dispatched together while the sequential pipeline's runOrder=2 action waits for the runOrder=1 action's lastUpdateTime.

Actual (emulator): par_gap == seq_gap == 5.013s.
AssertionError: "the parallel control must start its two actions closer together than the sequential pipeline does, otherwise 'windows do not overlap' is a property of this target's timestamp reporting rather than of runOrder: parallel gap 5.013s is not less than sequential gap 5.013s (parallel Alpha start 1793204544.282, Bravo start 1793204539.269)"

Two facts from that output (the 5.013s value and the epoch seconds are this run's values filling the template, not requirements):
  1. The two gaps are EQUAL, i.e. the stagger is a constant per-action cadence, independent of runOrder.
  2. In the parallel control the earlier startTime belongs to Bravo, the action declared first, while Alpha starts one cadence later — same-runOrder actions are ordered and serialized by list position.

No status/error-code divergence is involved: every call succeeded (HTTP 200) and GetPipeline still reported Alpha=1/Bravo=2 for the sequential pipeline and Alpha=1/Bravo=1 for the control, so the declaration was not rewritten. The divergence is in dispatch concurrency and the reported startTime values alone.

## Steps to Reproduce
1. Create an S3 artifact bucket (versioning enabled) and upload a zip as source.zip; create an IAM role trusted by codepipeline.amazonaws.com with read/write on that bucket.
2. CreatePipeline "P-par": pipelineType V2, executionMode QUEUED, artifactStore S3 -> the bucket. Stage "Source": one AWS/S3 Source action (S3Bucket=bucket, S3ObjectKey=source.zip, PollForSourceChanges=false, outputArtifacts=[SourceOutput], runOrder 1). Stage "Deploy": two AWS/S3 Deploy actions, declared in this order:
     - name "Bravo", runOrder 1, configuration ObjectKey=P-par/bravo.zip, Extract=false, inputArtifacts=[SourceOutput]
     - name "Alpha", runOrder 1, configuration ObjectKey=P-par/alpha.zip, Extract=false, inputArtifacts=[SourceOutput]
3. CreatePipeline "P-seq": identical, except "Bravo" declares runOrder 2 (Alpha stays runOrder 1) and the object keys are P-seq/bravo.zip and P-seq/alpha.zip.
4. Wait for each auto-started execution (ListPipelineExecutions) to reach Succeeded via GetPipelineExecution, and confirm both alpha.zip and bravo.zip exist in the bucket for both pipelines.
5. For each pipeline call ListActionExecutions with filter {pipelineExecutionId: <that execution>}, keep the stageName=="Deploy" details, and read startTime/lastUpdateTime per actionName.
6. Compute par_gap = abs(Alpha.startTime - Bravo.startTime) for P-par and seq_gap = Bravo.startTime - Alpha.startTime for P-seq.
   AWS: par_gap < seq_gap, and in P-par the two startTimes are close together (one dispatch).
   Emulator: par_gap == seq_gap (both equal the fixed per-action cadence, ~5s), and in P-par Bravo's startTime precedes Alpha's by that cadence because Bravo is declared first.
7. Also confirm GetPipeline still echoes the declared runOrders ({Alpha:1, Bravo:2} and {Alpha:1, Bravo:1}) — it does, isolating the defect to action dispatch.
