Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackagev2-harvest-job-lifecycle
Issue-Step: task#6
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackagev2-harvest-job-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2|GetHarvestJob|4xx->4xx|$.Message
Issue-Key: mediapackagev2|CancelHarvestJob|4xx->4xx|$.Message
Issue-Commit: fa14c0f225749a1f587b9baa3463a5a1c30960f9

# mediapackagev2 harvest-job ResourceNotFoundException message wording diverges from AWS

> _Note: this issue was identified against the code base at commit `fa14c0f225749a1f587b9baa3463a5a1c30960f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHarvestJob

## Description
Replay of the mediapackagev2 wire fixtures (86/96 steps passed, score 89.6%) surfaced two net-new divergences that are NOT covered by the open issue 20261022T103649Z-mediapackagev2-emulator-diverges-from-recorded-aws-responses-missing.md (that issue explicitly excludes them from its five causes) nor by 20261022T023331Z-mediapackagev2-listtagsforresource-stub.md (a different operation). Both are the SAME defect: the harvest-job not-found error carries the wrong message text. They are bundled because they share one service and one root cause; a fixer MUST close BOTH causes below before submitting.

Cause 1 of 2: GetHarvestJob's ResourceNotFoundException message text is not AWS's text.
  AWS: GetHarvestJob models ResourceNotFoundException as a client error with @httpError 404 (Smithy: GetHarvestJob errors -> ResourceNotFoundException, errorKind client, httpError 404; shape ResourceNotFoundException members Message (String), ResourceTypeNotFound (enum), doc "The specified resource doesn't exist."). The model does not state the message text, so this is recorded behavior only: for a harvest job that does not exist under an existing channel group / channel / origin endpoint, AWS answers 404 with Message "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job id or create a harvest job and then try again". That whole sentence is FIXED text for this error class — it interpolates nothing from the request or from state (no job name, arn, group or endpoint name appears in it), so there is no variable part to fill in and nothing in it is a per-run value.
  Emulator: answers the right status (404) and the right error class, but with the text "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job name or add a harvest job and then try again" - i.e. the emulator's own paraphrase, substituting "harvest job name" for AWS's "harvest job id" and "add a harvest job" for AWS's "create a harvest job".
  Why: the Message is the only part of a ResourceNotFoundException that distinguishes a missing harvest job from a missing channel/endpoint (ResourceTypeNotFound is absent from the recorded response), so clients, CLI output, log scrapers and tests that match or surface the service's own sentence see text AWS never sends; anything asserting on the AWS wording fails against the emulator even though the status and error type are right.
  Fix: the GetHarvestJob handler's not-found path / the shared mediapackagev2 harvest-job not-found message constant - emit AWS's fixed sentence verbatim rather than a locally worded one.
  Traces: mediapackagev2-harvest-job-lifecycle#task#6 (GetHarvestJob, status 404 on both sides, $.Message value_mismatch).

Cause 2 of 2: CancelHarvestJob's ResourceNotFoundException message text is not AWS's text, in the same way.
  AWS: CancelHarvestJob models the same error (Smithy: CancelHarvestJob errors -> ResourceNotFoundException, errorKind client, httpError 404; PUT /channelGroup/{ChannelGroupName}/channel/{ChannelName}/originEndpoint/{OriginEndpointName}/harvestJob/{HarvestJobName}, idempotent). Recorded behavior only: cancelling a harvest job that does not exist answers 404 with the identical fixed sentence "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job id or create a harvest job and then try again" - AWS uses one text for the harvest-job-not-found class across both the read and the cancel path, and again interpolates nothing.
  Emulator: 404 with the same paraphrase as Cause 1 ("...Verify your harvest job name or add a harvest job and then try again").
  Why: same impact as Cause 1 on the cancel path; a client retry/cleanup loop that reports or branches on the service sentence gets wording AWS never emits.
  Fix: the CancelHarvestJob handler's not-found path - it should resolve to the same corrected shared message as Cause 1, so both paths are fixed by one constant.
  Traces: mediapackagev2-harvest-job-lifecycle#task#7 (CancelHarvestJob, status 404 on both sides, $.Message value_mismatch).

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, steps_total=96, passed=86, mismatched=10, errored=0, blocked=0, bootstrap_signal=false, score_pct=89.6. Neither of the two clusters below carries a stub_path, and both report owned_by=[] - no open issue holds their replay key.

Cause 1 - mediapackagev2-harvest-job-lifecycle#task#6, GetHarvestJob, divergence_key service=mediapackagev2|GetHarvestJob|status=404->404|keys=$.Message:
  status: expected 404, got 404 (matches)
  $.Message expected: "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job id or create a harvest job and then try again"
  $.Message actual:   "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job name or add a harvest job and then try again"
  differing tokens: "harvest job id" -> "harvest job name"; "create a harvest job" -> "add a harvest job". Everything in this message is fixed text for the error class; no request value, resource name, id, count or timestamp appears in it.

Cause 2 - mediapackagev2-harvest-job-lifecycle#task#7, CancelHarvestJob, divergence_key service=mediapackagev2|CancelHarvestJob|status=404->404|keys=$.Message:
  status: expected 404, got 404 (matches)
  $.Message expected and actual: byte-identical to Cause 1's expected/actual pair, confirming AWS reuses one fixed sentence for this error class and the emulator reuses one paraphrase of it.

Smithy evidence: GetHarvestJob / CancelHarvestJob both bind ResourceNotFoundException (errorKind client, httpError 404); shape ResourceNotFoundException = structure{Message: String, ResourceTypeNotFound: enum}, doc "The specified resource doesn't exist." The model constrains the status and the shape, not the sentence, hence "recorded behavior only" for the text.

## Steps to Reproduce
Boot the emulator from this checkout and replay the mediapackagev2 wire fixtures (replay_wire_corpus against trace set simparity-wire-aws-v1).

Cause 1: fixture mediapackagev2-harvest-job-lifecycle, step task#6 - GET /channelGroup/{ChannelGroupName}/channel/{ChannelName}/originEndpoint/{OriginEndpointName}/harvestJob/{HarvestJobName} for a harvest job name that was never created (the group, channel and origin endpoint do exist). Observe 404 with ResourceNotFoundException, then byte-compare $.Message against AWS's fixed sentence "MediaPackage can't process your request because we can't find your harvest job. Verify your harvest job id or create a harvest job and then try again"; the emulator says "...harvest job name or add a harvest job...".

Cause 2: same fixture, step task#7 - PUT /channelGroup/{ChannelGroupName}/channel/{ChannelName}/originEndpoint/{OriginEndpointName}/harvestJob/{HarvestJobName} (CancelHarvestJob) for that same nonexistent harvest job. Observe 404 and compare $.Message the same way.
