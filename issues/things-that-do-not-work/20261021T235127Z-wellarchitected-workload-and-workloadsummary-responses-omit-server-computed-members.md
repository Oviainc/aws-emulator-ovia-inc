Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-workload-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-workload-lifecycle wellarchitected-pagination-and-limit-validation
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.FTRSupported
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.IsReviewOwnerUpdateAcknowledged
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.PillarPriorities[*]
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.RiskCounts.HIGH
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.RiskCounts.MEDIUM
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.RiskCounts.NONE
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.RiskCounts.NOT_APPLICABLE
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.RiskCounts.UNANSWERED
Issue-Key: wellarchitected|GetWorkload|2xx->2xx|$.Workload.Tags
Issue-Key: wellarchitected|UpdateWorkload|2xx->2xx|$.Workload.DiscoveryConfig
Issue-Key: wellarchitected|UpdateWorkload|2xx->2xx|$.Workload.IsReviewOwnerUpdateAcknowledged
Issue-Key: wellarchitected|UpdateWorkload|2xx->2xx|$.Workload.PillarPriorities[*]
Issue-Key: wellarchitected|UpdateWorkload|2xx->2xx|$.Workload.RiskCounts.HIGH
Issue-Key: wellarchitected|UpdateWorkload|2xx->2xx|$.Workload.RiskCounts.MEDIUM
Issue-Key: wellarchitected|UpdateWorkload|2xx->2xx|$.Workload.RiskCounts.NONE
Issue-Key: wellarchitected|UpdateWorkload|2xx->2xx|$.Workload.RiskCounts.NOT_APPLICABLE
Issue-Key: wellarchitected|UpdateWorkload|2xx->2xx|$.Workload.RiskCounts.UNANSWERED
Issue-Key: wellarchitected|ListWorkloads|2xx->2xx|$.WorkloadSummaries[*].RiskCounts.UNANSWERED
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# Workload and WorkloadSummary responses omit server-computed members

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkload

## Description
The emulator stores only the members a CreateWorkload request supplied, so every read path returns a Workload/WorkloadSummary that is missing the members the real service computes and defaults when the workload is created (risk roll-up, pillar priorities, tag map, discovery config, review-owner acknowledgement). Smithy: Workload has members RiskCounts (map Risk -> Count), PillarPriorities (WorkloadPillarPriorities), Tags (TagMap), DiscoveryConfig (WorkloadDiscoveryConfig) and IsReviewOwnerUpdateAcknowledged (boolean); WorkloadSummary has WorkloadId, WorkloadArn, WorkloadName, Owner, UpdatedAt, Lenses, RiskCounts, ImprovementStatus. 3 causes, all in this service.

Cause 1 of 3 — GetWorkload returns a workload with no risk roll-up, no pillar priorities and no tag map (representative cause).
AWS: after CreateWorkload with a lens attached, GetWorkload returns Workload.RiskCounts populated for every Risk key the lens produces — UNANSWERED equal to the number of still-unanswered questions in the attached lens and HIGH/MEDIUM/NONE/NOT_APPLICABLE present as 0 — plus Workload.PillarPriorities defaulted to the service's full pillar order (recorded: operationalExcellence, security, reliability, performance, costOptimization, sustainability), Workload.Tags present as an empty map when no tags were supplied, and IsReviewOwnerUpdateAcknowledged true when the create request supplied a ReviewOwner. The recording also carries FTRSupported false (recorded behavior only — not in the on-disk Workload shape).
Emulator: all of RiskCounts, PillarPriorities, Tags and FTRSupported are absent from the response, and IsReviewOwnerUpdateAcknowledged is false.
Why: a client that reads RiskCounts to report review progress, or PillarPriorities to order a review, gets nothing back and either divides by zero/empty or reports a workload with no questions; the false acknowledgement flag makes a workload that does have a review owner look like one that will be restricted after 60 days.
Fix: CreateWorkload's state initialisation (seed the risk roll-up, the default pillar priority order, an empty tag map and the acknowledgement flag) plus the Workload serializer that must emit them.
Traces: wellarchitected-workload-lifecycle#task#2.

Cause 2 of 3 — UpdateWorkload's response body has the same omissions, plus DiscoveryConfig.
AWS: UpdateWorkload returns the full updated Workload — same RiskCounts and PillarPriorities population as cause 1, IsReviewOwnerUpdateAcknowledged reflecting the review owner, and DiscoveryConfig present with its TrustedAdvisorIntegrationStatus (recorded: DISABLED when integration was never enabled).
Emulator: RiskCounts, PillarPriorities and DiscoveryConfig are absent and IsReviewOwnerUpdateAcknowledged is false.
Why: callers that use the update response as the new authoritative copy of the workload (instead of re-reading it) lose the same fields, and the absent DiscoveryConfig is indistinguishable from 'feature unknown' rather than 'disabled'.
Fix: UpdateWorkload handler/serializer — return the same fully-populated Workload projection as GetWorkload, defaulting DiscoveryConfig rather than dropping it.
Traces: wellarchitected-workload-lifecycle#task#4.

Cause 3 of 3 — ListWorkloads summaries carry an incomplete RiskCounts map.
AWS: each WorkloadSummary in ListWorkloads carries RiskCounts including the UNANSWERED key with the unanswered-question count for the workload's lenses (recorded: 57 for the wellarchitected lens).
Emulator: WorkloadSummaries[*].RiskCounts omits UNANSWERED.
Why: list views that sum UNANSWERED across workloads to show outstanding review work read a missing key as zero and report every workload as fully reviewed.
Fix: the WorkloadSummary projection used by ListWorkloads — derive RiskCounts from the same roll-up as cause 1 instead of emitting a partial map.
Traces: wellarchitected-pagination-and-limit-validation#task#2, and the same cluster member wellarchitected-workload-lifecycle#task#3.

All three causes share one root: the workload record is never given its server-computed defaults, so every projection of it is short. The specific counts in the recording (UNANSWERED 57) are examples of the lens's question count, not literals to hard-code.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, no state-reset failures). All three clusters returned the correct status 200; only the body diverged.

Cause 1 wellarchitected-workload-lifecycle#task#2 GetWorkload: 200/200 — $.Workload.Tags: missing (expected {}); $.Workload.IsReviewOwnerUpdateAcknowledged: value_mismatch (expected True, got False); $.Workload.FTRSupported: missing (expected False); $.Workload.RiskCounts.UNANSWERED: missing (expected 57); $.Workload.RiskCounts.HIGH / .MEDIUM / .NONE / .NOT_APPLICABLE: missing (expected 0); $.Workload.PillarPriorities[0..5]: missing (expected 'operationalExcellence', 'security', 'reliability', 'performance', 'costOptimization', 'sustainability').
Cause 2 wellarchitected-workload-lifecycle#task#4 UpdateWorkload: 200/200 — $.Workload.IsReviewOwnerUpdateAcknowledged: value_mismatch (expected True, got False); $.Workload.DiscoveryConfig: missing (expected {'TrustedAdvisorIntegrationStatus': 'DISABLED'}); $.Workload.RiskCounts.UNANSWERED: missing (expected 57); $.Workload.RiskCounts.HIGH / .MEDIUM / .NONE / .NOT_APPLICABLE: missing (expected 0); $.Workload.PillarPriorities[0..5]: missing.
Cause 3 wellarchitected-pagination-and-limit-validation#task#2 ListWorkloads (cluster also covers wellarchitected-workload-lifecycle#task#3): 200/200 — $.WorkloadSummaries[0].RiskCounts.UNANSWERED: missing (expected 57).

Smithy citations: Workload.RiskCounts/PillarPriorities/Tags/DiscoveryConfig/IsReviewOwnerUpdateAcknowledged; WorkloadSummary.RiskCounts; RiskCounts = map<Risk enum, Count integer range min 0>. FTRSupported is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from the checkout (wellarchitected is bootstrapped) and replay trace set simparity-wire-aws-v1.
2. Representative cause: fixture wellarchitected-workload-lifecycle, step task#2 — create a workload with a lens and a ReviewOwner, then GET /workloads/{WorkloadId}; the response omits RiskCounts, PillarPriorities and Tags and reports IsReviewOwnerUpdateAcknowledged false.
3. Replay wellarchitected-workload-lifecycle#task#4 (PATCH /workloads/{id} — response omits the same members plus DiscoveryConfig) and wellarchitected-pagination-and-limit-validation#task#2 (POST /workloadsSummaries — summary RiskCounts omits UNANSWERED).
4. All three causes must pass before this issue is closed.
