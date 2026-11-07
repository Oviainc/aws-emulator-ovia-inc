Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-dashboards-catalog-list-dashboards
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-dashboards-catalog-list-dashboards
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn:aws:bcm-dashboards::000000000000:dashboard/managed-6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d"]
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn:aws:bcm-dashboards::000000000000:dashboard/managed-7b3c91d4e5f2a0186c4d8e9f1a2b3c4d"]
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn:aws:bcm-dashboards::000000000000:dashboard/managed-7c2a1d4e9b0f5a3c8d6e2f1b4a7c9d0e"]
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn:aws:bcm-dashboards::000000000000:dashboard/managed-8d3b2e5f0c1a6b4d9e7f3a2c5b8d0e1f"]
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn:aws:bcm-dashboards::000000000000:dashboard/managed-9e4c3f6a1d2b7c5e0f8a4b3d6c9e1f2a"]
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn:aws:bcm-dashboards::000000000000:dashboard/managed-a8f4e2b1c3d6490ea1b7f5c8d2e94a03"]
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn:aws:bcm-dashboards::000000000000:dashboard/managed-b0e4f3ab71454cf48a7d148287222f9d"]
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn:aws:bcm-dashboards::000000000000:dashboard/managed-d2effbcc2dfd44eeb8ebec3f6e83382a"]
Issue-Commit: d5d5092b5af2686a6a1d42f09a764e6dd1daf2a1

# ListDashboards omits every published AWS-managed dashboard reference and reports invented ones

> _Note: this issue was identified against the code base at commit `d5d5092b5af2686a6a1d42f09a764e6dd1daf2a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDashboards

## Description
Cause 1 of 1: ListDashboards on an account that has created no dashboards of its own must report the AWS-managed dashboard catalog every bcm-dashboards account already holds. The emulator reports a hand-written catalog instead, so four of the five references it lists name resources that exist nowhere in AWS and the four references AWS publishes are absent.

AWS: ListDashboards returns `dashboards` -> DashboardReferenceList, @required (Smithy: services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json, ListDashboards output {dashboards required, nextToken NextPageToken length 1..8192}, input {maxResults default 20 range 1..100}, @readonly, @paginated items=dashboards; errors AccessDeniedException 403 / ValidationException 400 / ThrottlingException 429 / InternalServerException 500 — no error applies here, AWS answers 200). The model says nothing about the CONTENTS of the managed catalog (DashboardType enumerates the custom case only), so the catalog itself is recorded behavior only: the recorded 200 lists one reference per managed dashboard the fresh account holds, each with `arn`, `name`, `description`, `type` = AWS_MANAGED and the catalog's publication/revision instants in `createdAt`/`updatedAt`. Among them are the dashboards named Compute ('Compute spend, efficiency, and commitment health across your compute services'), Database ('Database spend, efficiency, and commitment health across your database services'), Reservations ('Reserved Instance cost, utilization, and coverage health') and Savings Plans ('Savings Plans utilization and coverage health'), under the resource ids managed-b0e4f3ab71454cf48a7d148287222f9d, managed-d2effbcc2dfd44eeb8ebec3f6e83382a, managed-a8f4e2b1c3d6490ea1b7f5c8d2e94a03 and managed-7b3c91d4e5f2a0186c4d8e9f1a2b3c4d respectively. (Those ids/instants are the recording's values filling the published catalog's slots; the requirement is that the emulator serve the catalog AWS publishes, not that it echo one run's literals.)

Emulator: the call answers 200 and the envelope is well formed, but the membership of `dashboards` is wrong in both directions. All four published references above are MISSING from the list, and four references AWS does not publish appear in their place — the seed's own resource ids managed-6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d (name 'Compute', description 'Cost and usage of your compute services, including Amazon EC2, AWS Lambda and AWS Fargate'), managed-7c2a1d4e9b0f5a3c8d6e2f1b4a7c9d0e ('Database'), managed-8d3b2e5f0c1a6b4d9e7f3a2c5b8d0e1f ('Reservations') and managed-9e4c3f6a1d2b7c5e0f8a4b3d6c9e1f2a ('Savings Plans'). Only one seeded entry (managed-4dc5619130f2463687fc45e003e4b930, 'Cost Overview and Trends') matches the recording, which is why the replay reports exactly four missing and four extra `$.dashboards[arn=...]` members rather than an empty list.

Why: discovery is the first call a console-like client or a report-scheduling flow makes, and every id it discovers here is unusable downstream — GetDashboard, UpdateDashboard and a scheduled report's `dashboardArn` all take an ARN obtained from this listing, and the four invented ids resolve to nothing in AWS while the four real ones are never offered. Clients also render the fabricated `description` text to users, and a client that asserts the managed catalog is present (a 'does this account have the Compute dashboard' check) concludes it is not.

Fix: the AWS-managed catalog seed in the service state — `managed_dashboard_catalog()` in services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs, which builds the five entries whose keys and descriptions diverge, installed into the store by `Default::default()`. The ListDashboards handler's projection and pagination are faithful to whatever the store holds and are not at fault, so no change belongs in the operation handler or its serializer.

Related open issue: 20261101T122305Z-bcm-dashboards-aws-managed-dashboard-catalog-diverges-from-the-published.md describes this same seed under its 'Cause 3 of 3', but it does NOT own this divergence's replay key — its Issue-Key for ListDashboards is truncated mid-JSONPath ('bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn') and its Issue-Trace-Ids names the fixture bcm-dashboards-catalog-list-dashboards with no step, so the replay reports this cluster with an empty owned_by and a fixer re-replaying that issue never exercises this step. This issue carries the binding.

Traces: bcm-dashboards-catalog-list-dashboards#task#1 (cluster of 1; the GetDashboard clusters task#1 and task#2..task#5 are owned by the related issue above and are not re-filed here).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, steps_total=50, steps_passed=44, steps_mismatched=6, divergence_count=3, score_pct=88.0.

Divergence cluster (verdict=mismatch, service=bcm-dashboards, operation=ListDashboards, stub_path empty, owned_by empty):
  fixture_id: bcm-dashboards-catalog-list-dashboards
  step_key:   task#1
  expected_status: 200, actual_status: 200
  divergence_key: service=bcm-dashboards|ListDashboards|status=200->200|keys=$.dashboards[arn="...managed-6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d"],[...7b3c91d4e5f2a0186c4d8e9f1a2b3c4d"],[...7c2a1d4e9b0f5a3c8d6e2f1b4a7c9d0e"],[...8d3b2e5f0c1a6b4d9e7f3a2c5b8d0e1f"],[...9e4c3f6a1d2b7c5e0f8a4b3d6c9e1f2a"],[...a8f4e2b1c3d6490ea1b7f5c8d2e94a03"],[...b0e4f3ab71454cf48a7d148287222f9d"],[...d2effbcc2dfd44eeb8ebec3f6e83382a"]

Expected vs actual, from the cluster detail:
  MISSING (expected, got None):
    $.dashboards[arn=".../dashboard/managed-b0e4f3ab71454cf48a7d148287222f9d"] = {name: 'Compute', description: 'Compute spend, efficiency, and commitment health across your compute services', type: 'AWS_MANAGED', createdAt, updatedAt}
    $.dashboards[arn=".../dashboard/managed-d2effbcc2dfd44eeb8ebec3f6e83382a"] = {name: 'Database', description: 'Database spend, efficiency, and commitment health across your database services', type: 'AWS_MANAGED', ...}
    $.dashboards[arn=".../dashboard/managed-a8f4e2b1c3d6490ea1b7f5c8d2e94a03"] = {name: 'Reservations', description: 'Reserved Instance cost, utilization, and coverage health', type: 'AWS_MANAGED', ...}
    $.dashboards[arn=".../dashboard/managed-7b3c91d4e5f2a0186c4d8e9f1a2b3c4d"] = {name: 'Savings Plans', description: 'Savings Plans utilization and coverage health', type: 'AWS_MANAGED', ...}
  EXTRA (expected None, got):
    $.dashboards[arn=".../dashboard/managed-6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d"] = {name: 'Compute', description: 'Cost and usage of your compute services, including Amazon EC2, AWS Lambda and AWS Fargate', ...}
    and the sibling seeded entries managed-7c2a1d4e9b0f5a3c8d6e2f1b4a7c9d0e, managed-8d3b2e5f0c1a6b4d9e7f3a2c5b8d0e1f, managed-9e4c3f6a1d2b7c5e0f8a4b3d6c9e1f2a.

No status or error-code divergence: both sides are HTTP 200 with no error, so the defect is purely the membership of the @required `dashboards` list.

Source of the fault in the checkout: services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs, `managed_dashboard_catalog()` (lines 467-515) seeds exactly the five entries whose ids appear on the actual side — 4dc5619130f2463687fc45e003e4b930 'Cost Overview and Trends' (the only one that matches the recording), 6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d 'Compute', 7c2a1d4e9b0f5a3c8d6e2f1b4a7c9d0e 'Database', 8d3b2e5f0c1a6b4d9e7f3a2c5b8d0e1f 'Reservations', 9e4c3f6a1d2b7c5e0f8a4b3d6c9e1f2a 'Savings Plans' — keyed by ARN into the store and surfaced verbatim by the listing projection.

Smithy consulted: ListDashboards signature, @required dashboards -> DashboardReferenceList, nextToken/maxResults constraints, @readonly/@paginated and the four modelled errors, via smithy_query on services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it (the bcm-dashboards service answers; /__admin/health is 200).
2. Replay the corpus: replay_wire_corpus() for trace set simparity-wire-aws-v1. The summary reports 6 mismatched steps in 3 clusters, all service=bcm-dashboards.
3. Locate the cluster with fixture_id=bcm-dashboards-catalog-list-dashboards and step_key=task#1 (operation ListDashboards, expected 200 / actual 200, verdict mismatch, owned_by empty).
4. Equivalently by hand: against a fresh account that has created no dashboards, call ListDashboards with no nextToken. AWS answers 200 listing the published AWS-managed references, including the Compute, Database, Reservations and Savings Plans dashboards under the catalog's own resource ids and descriptions. The emulator answers 200 listing five hand-written entries whose ids are managed-4dc5619130f2463687fc45e003e4b930, managed-6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d, managed-7c2a1d4e9b0f5a3c8d6e2f1b4a7c9d0e, managed-8d3b2e5f0c1a6b4d9e7f3a2c5b8d0e1f and managed-9e4c3f6a1d2b7c5e0f8a4b3d6c9e1f2a, so four published references are absent and four unpublished ones are present.
5. Confirm the source: read services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs around lines 467-515 (`managed_dashboard_catalog()`) and compare each seeded id/description with the recorded listing.
6. After fixing the seed, re-replay fixture bcm-dashboards-catalog-list-dashboards step task#1 and confirm the cluster disappears (the GetDashboard steps task#1..task#5 of bcm-dashboards-catalog-get-dashboard belong to the related issue and share this seed, so re-replay them too).
