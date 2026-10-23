Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-build-and-batch-registry-reads
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-build-and-batch-registry-reads codebuild-project-registry-and-absent-project-errors codebuild-fleet-registry-and-validation-errors codebuild-report-group-config-and-trend codebuild-report-group-create-get-delete codebuild-build-batch-environment-variables-preserved codebuild-report-inspection-testcases-coverage
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|ListBuilds|2xx->2xx|$
Issue-Key: codebuild|ListProjects|2xx->2xx|$
Issue-Key: codebuild|BatchGetFleets|2xx->2xx|$.fleets
Issue-Key: codebuild|BatchGetFleets|2xx->2xx|$.fleetsNotFound[*]
Issue-Key: codebuild|BatchGetReportGroups|2xx->2xx|$.reportGroups[*].tags
Issue-Key: codebuild|DeleteProject|2xx->2xx|$
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatchesNotFound
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].artifacts.location
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].artifacts.type
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].buildBatchConfig.restrictions
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].buildBatchConfig.timeoutInMins
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].buildBatchStatus
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].buildGroups
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].cache
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].currentPhase
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].encryptionKey
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].environment.imagePullCredentialsType
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].environment.privilegedMode
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*]
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*].durationInSeconds
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*].endTime
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*].phaseStatus
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].phases[*].phaseType
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].queuedTimeoutInMinutes
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].secondarySourceVersions
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].secondarySources
Issue-Key: codebuild|BatchGetBuildBatches|2xx->2xx|$.buildBatches[*].source.insecureSsl
Issue-Key: codebuild|DescribeCodeCoverages|4xx->4xx|$.message
Issue-Related: 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CodeBuild read responses omit modeled members and misrender values

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBuilds

## Description
CodeBuild read/list operations in the emulator answer 200 but with a body that is missing members AWS always serialises: whole registry listings, empty lists, defaulted members, and batch detail. SEVEN distinct causes are bundled here; all seven must be closed before this issue is resolved.

Cause 1 of 7 — ListBuilds returns an empty body instead of the build-id registry (fixture codebuild-build-and-batch-registry-reads, step task#2).
AWS: ListBuilds returns the account's build ids, newest first, in the `ids` member. Smithy: ListBuildsOutput { ids: BuildIds (@length 1..100), nextToken }; paginated with items=ids.
Emulator: HTTP 200 with a completely empty JSON object `{}` even though the fixture had already started builds (the recording lists 15 ids of form "<project>:<uuid>").
Why: ListBuilds is the only account-wide way to discover builds; an empty body makes every build invisible, so paginators yield nothing and discovery/cleanup flows believe the account has no builds.
Fix: ListBuilds' handler — enumerate the build registry from state and serialise `ids` (plus nextToken when paging).
Traces: codebuild-build-and-batch-registry-reads#task#2.

Cause 2 of 7 — ListProjects omits the `projects` member when there are no projects (fixture codebuild-project-registry-and-absent-project-errors, step task#4).
AWS: returns {"projects": []} — the list member is present and empty. Smithy: ListProjectsOutput { projects: ProjectNames, nextToken }; paginated with items=projects.
Emulator: HTTP 200 with `{}` — `projects` absent entirely.
Why: SDK paginators and clients that index response["projects"] get a missing-key/None instead of an empty list, turning a normal empty result into a client-side error.
Fix: ListProjects' serializer — always emit the list member, empty when the registry is empty.
Traces: codebuild-project-registry-and-absent-project-errors#task#4.

Cause 3 of 7 — BatchGetFleets partitions an unknown fleet name wrongly and drops the empty `fleets` list (fixture codebuild-fleet-registry-and-validation-errors, step task#1).
AWS (recorded behavior only; the model does not state the partition rule): the response carries `fleets: []` and no `fleetsNotFound` member for the probed name. Smithy: BatchGetFleetsOutput { fleets: Fleets, fleetsNotFound: FleetNames }.
Emulator: HTTP 200 with `fleets` absent and `fleetsNotFound[0]` set to the requested name.
Why: callers that read `fleets` unconditionally see a missing member, and callers that treat a non-empty `fleetsNotFound` as a hard error fail on a request AWS answers with a plain empty result.
Fix: BatchGetFleets' handler/serializer — always emit `fleets` (empty when nothing matched) and align the not-found partition with the recorded behavior.
Traces: codebuild-fleet-registry-and-validation-errors#task#1.

Cause 4 of 7 — BatchGetReportGroups omits empty collections (fixture codebuild-report-group-config-and-trend, step task#2; also codebuild-report-group-create-get-delete#task#2).
AWS: returns `reportGroupsNotFound: []` and, for a report group created without tags, `reportGroups[0].tags: []`. Smithy: BatchGetReportGroupsOutput { reportGroups, reportGroupsNotFound }; ReportGroup.tags is a TagList.
Emulator: HTTP 200 with both members absent.
Why: tag-reconciliation and not-found-diffing code must special-case missing keys; a client comparing desired tags against `reportGroups[0].tags` sees None rather than "no tags".
Fix: BatchGetReportGroups' serializer / the report-group projection — emit empty lists rather than omitting them.
Traces: codebuild-report-group-config-and-trend#task#2, codebuild-report-group-create-get-delete#task#2.

Cause 5 of 7 — DeleteProject returns an empty body where AWS reports the webhook outcome (fixture codebuild-project-registry-and-absent-project-errors, step task#13).
AWS (recorded behavior only — DeleteProjectOutput is modeled with no members): the recorded response body is {"webhookDeletedStatus": "no_webhook"}, i.e. the service reports what happened to the project's webhook on delete.
Emulator: HTTP 200 with `{}`.
Why: clients and tooling that read the webhook-deletion outcome from DeleteProject get nothing, so a webhook-cleanup assertion cannot be made. Status matches, only the body member is missing.
Fix: DeleteProject's handler/serializer — report the webhook deletion status for the deleted project.
Traces: codebuild-project-registry-and-absent-project-errors#task#13.

Cause 6 of 7 — BatchGetBuildBatches reports a batch as FAULT and omits most of its detail (fixture codebuild-build-batch-environment-variables-preserved, step task#7).
AWS: a completed batch reads buildBatchStatus SUCCEEDED and currentPhase SUCCEEDED, and the batch object carries artifacts {location, type}, buildBatchConfig {restrictions.maximumBuildsAllowed, timeoutInMins}, buildGroups (one entry per group with currentBuildSummary/identifier/ignoreFailure/dependsOn), cache, encryptionKey, environment.imagePullCredentialsType and environment.privilegedMode, queuedTimeoutInMinutes, secondarySources/secondarySourceVersions, source.insecureSsl, and a full phases list with phaseStatus/durationInSeconds per phase. Smithy: BatchGetBuildBatchesOutput { buildBatches: BuildBatches, buildBatchesNotFound }.
Emulator: HTTP 200 but buildBatchStatus 'FAULT', currentPhase 'FAILED', phases[1].phaseStatus 'FAULT' with durationInSeconds 0, later phases (phases[2..4]) absent, and the members listed above all missing.
Why: the batch never reaches a terminal success, so any client that waits for SUCCEEDED or reads per-group build summaries/environment configuration off the batch gets a failure it cannot explain and cannot see the batch's configuration round-tripped.
Fix: the build-batch execution/state model plus BatchGetBuildBatches' projection — drive batches to a terminal SUCCEEDED state and serialise the full batch (groups, phases, defaults).
Note: open issue 20261022T172625Z-codebuild-builds-never-execute-every-build-faults-at.md describes the same FAULT-at-PROVISIONING family behaviorally but carries no replay key for this fixture step, so it does not cover this cause.
Traces: codebuild-build-batch-environment-variables-preserved#task#7.

Cause 7 of 7 — DescribeCodeCoverages renders a Double constraint value as an integer in its validation error (fixture codebuild-report-inspection-testcases-coverage, step task#6).
AWS: minLineCoveragePercentage is a double with @range 0..100, and the standard validation error echoes the submitted value as a double: "1 validation error detected: Value '150.0' at 'minLineCoveragePercentage' failed to satisfy constraint: Member must have value less than or equal to 100". Smithy: DescribeCodeCoverages.minLineCoveragePercentage targets Percentage (double, range min 0 max 100); error InvalidInputException (client, httpError 400). The message rule is request-derived (value + member + constraint), so the rendering of the value is part of it.
Emulator: HTTP 400 with the same code and the same template, but the value is rendered as '150' — the double is formatted as an integer.
Why: assertions and log-scrapers that match AWS's validation text for a double-typed member do not match, and the response misrepresents the member's wire type.
Fix: the shared range-constraint validation message formatter used by DescribeCodeCoverages — format double members with their decimal representation.
Traces: codebuild-report-inspection-testcases-coverage#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 105 steps, 34 passed, 71 mismatched, score 32.4%). Per cause, expected (recorded AWS) vs actual (emulator):

1. ListBuilds @ codebuild-build-and-batch-registry-reads#task#2 — status 200 both sides; expected body {'ids': [15 ids, e.g. 'sim2real-cb-proj-24400d3bb44b:95adb01d-fc08-4791-b801-3b29f3727cc5', ...]}; actual {} (detail: "$: empty_body").
2. ListProjects @ codebuild-project-registry-and-absent-project-errors#task#4 — status 200 both sides; expected {'projects': []}; actual {} ("$: empty_body").
3. BatchGetFleets @ codebuild-fleet-registry-and-validation-errors#task#1 — status 200 both sides; '$.fleets: missing (expected [], got None)'; '$.fleetsNotFound[0]: extra (expected None, got sim2real-probe-codebuild-codebuild-absent-fleet-9881713b)'.
4. BatchGetReportGroups @ codebuild-report-group-config-and-trend#task#2 (cluster also covers codebuild-report-group-create-get-delete#task#2) — status 200 both sides; '$.reportGroups[0].tags: missing (expected [], got None)'; '$.reportGroupsNotFound: missing (expected [], got None)'.
5. DeleteProject @ codebuild-project-registry-and-absent-project-errors#task#13 — status 200 both sides; expected {'webhookDeletedStatus': 'no_webhook'}; actual {} ("$: empty_body").
6. BatchGetBuildBatches @ codebuild-build-batch-environment-variables-preserved#task#7 — status 200 both sides; '$.buildBatches[0].buildBatchStatus: expected SUCCEEDED, got FAULT'; '$.buildBatches[0].currentPhase: expected SUCCEEDED, got FAILED'; '$.buildBatches[0].phases[1].phaseStatus: expected SUCCEEDED, got FAULT'; '$.buildBatches[0].phases[1].durationInSeconds: expected 15, got 0'; phases[2..4] and buildGroups, cache, encryptionKey, artifacts.location/type, buildBatchConfig.restrictions/timeoutInMins, environment.imagePullCredentialsType/privilegedMode, queuedTimeoutInMinutes, secondarySources, secondarySourceVersions, source.insecureSsl all reported missing.
7. DescribeCodeCoverages @ codebuild-report-inspection-testcases-coverage#task#6 — status 400 both sides, same error code; expected $.message "1 validation error detected: Value '150.0' at 'minLineCoveragePercentage' failed to satisfy constraint: Member must have value less than or equal to 100"; actual the same string with "Value '150'".

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — ListBuildsOutput.ids (BuildIds, paginated items=ids), ListProjectsOutput.projects (ProjectNames, paginated items=projects), BatchGetFleetsOutput {fleets, fleetsNotFound}, BatchGetReportGroupsOutput {reportGroups, reportGroupsNotFound}, DeleteProjectOutput (no modeled members), BatchGetBuildBatchesOutput {buildBatches, buildBatchesNotFound}, DescribeCodeCoverages.minLineCoveragePercentage -> Percentage (double, @range 0..100) with InvalidInputException 400.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Representative cause: fixture codebuild-build-and-batch-registry-reads, step task#2 — call ListBuilds after builds have been started; emulator returns {} instead of {'ids': [...]}.
4. Fixture codebuild-project-registry-and-absent-project-errors, step task#4 — ListProjects with no projects: {} instead of {'projects': []}.
5. Fixture codebuild-fleet-registry-and-validation-errors, step task#1 — BatchGetFleets with one unknown fleet name: 'fleets' omitted and the name echoed in 'fleetsNotFound'.
6. Fixture codebuild-report-group-config-and-trend, step task#2 — BatchGetReportGroups on an untagged report group: 'reportGroups[0].tags' and 'reportGroupsNotFound' omitted.
7. Fixture codebuild-project-registry-and-absent-project-errors, step task#13 — DeleteProject: empty body instead of {'webhookDeletedStatus': 'no_webhook'}.
8. Fixture codebuild-build-batch-environment-variables-preserved, step task#7 — StartBuildBatch then BatchGetBuildBatches: batch reads FAULT/FAILED with phases and buildGroups/cache/encryptionKey/environment defaults missing instead of SUCCEEDED with full detail.
9. Fixture codebuild-report-inspection-testcases-coverage, step task#6 — DescribeCodeCoverages with minLineCoveragePercentage=150: validation message renders "Value '150'" instead of "Value '150.0'".
