Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-dashboards-catalog-get-dashboard
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-dashboards-catalog-get-dashboard bcm-dashboards-catalog-list-dashboards
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$.widgets[id="2a7e51a22e1c00af3843c5cccb82b046"]
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$.widgets[id="35d71261755663fa7f5aa289556472ef"]
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$.widgets[id="48381c58fdf60999c778deb93fc9284a"]
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$.widgets[id="a15901be3f2d2c9f07f2c90938ca73d3"]
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$.widgets[id="a7a5c864d98bfa0bcb1359bd4e3529f3"]
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$.widgets[id="a80cc87050b1e60b97c65e38be2876ae"]
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$.widgets[id="b8fe9c2e86f1737da142b62be6e865a8"]
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$.widgets[id="ce2a10fcd1ab3961dc32b6a3c595166c"]
Issue-Key: bcm-dashboards|GetDashboard|2xx->2xx|$.widgets[id="d771f500c22cacaa952f1bba40260445"]
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.arn
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.createdAt
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.description
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.name
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.type
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.updatedAt
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.widgets
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|status
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[arn="arn
Issue-Commit: a8592554c455d14ba514ff49d112a563888e0a32

# AWS-managed dashboard catalog diverges from the published catalog: invented ARNs/descriptions, one widget per dashboard, and GetDashboard 400 on four published ARNs

> _Note: this issue was identified against the code base at commit `a8592554c455d14ba514ff49d112a563888e0a32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDashboard

## Description
The AWS-managed dashboard catalog every bcm-dashboards account already holds is hand-written in the emulator's state seed instead of being the catalog AWS publishes. Three distinct causes follow from that one seed, all on the read surface a console-like client hits first. (No open bcm-dashboards issue covers the catalog: the existing GetDashboard issue 20261024T164554Z-bcm-dashboards-getdashboard-error-response.md is about authorizing the caller, and the name/not-found-wording issue 20261022T215638Z-bcm-dashboards-dashboardname-pattern-unenforced-constraint-not-found.md is about constraint and refusal text, not catalog contents.)

Cause 1 of 3: GetDashboard on an AWS-managed dashboard serves one synthesized widget instead of the published widget list.
AWS: GetDashboard returns the stored dashboard's full widgets list (Smithy: GetDashboard output widgets -> WidgetList, @required, length 0..20; members id/title/description/width/height/horizontalOffset/configs). The model says nothing about managed-catalog contents (DashboardType enumerates CUSTOM only), so this is recorded behavior only: the recorded response for the managed dashboard read in this step carries nine widgets — ids 2a7e51a22e1c00af3843c5cccb82b046, 35d71261755663fa7f5aa289556472ef, 48381c58fdf60999c778deb93fc9284a, a15901be3f2d2c9f07f2c90938ca73d3, a7a5c864d98bfa0bcb1359bd4e3529f3, a80cc87050b1e60b97c65e38be2876ae, b8fe9c2e86f1737da142b62be6e865a8, ce2a10fcd1ab3961dc32b6a3c595166c and d771f500c22cacaa952f1bba40260445 — each with its own title, description, layout and queryParameters (e.g. 'Committed vs Flexible Spend': graph display AmortizedCost/STACK over a MONTHLY costAndUsage query grouped by the PURCHASE_TYPE dimension with a RELATIVE -P12M..P0M range; 'Budget Health': a table display over a budget query).
Emulator: the response is 200 and every scalar member of the dashboard matches, but widgets holds only the single widget the seed builds for that entry, so all nine published widgets are absent from the body.
Why: a client that renders the managed dashboard, or a scheduled report whose widgetIds name a published widget, finds no such widget — the dashboard reads as a one-panel dashboard that AWS never published, and widget-id-scoped calls against it fail.
Fix: the managed-catalog seed in the service state (the per-entry widget builders feeding managed_dashboard_catalog), not the GetDashboard handler, which projects the record faithfully.
Traces: bcm-dashboards-catalog-get-dashboard#task#1.

Cause 2 of 3: GetDashboard refuses four published managed ARNs with a not-found refusal.
AWS: GetDashboard on an AWS-managed dashboard ARN answers 200 with arn, name, description, type=AWS_MANAGED, widgets, createdAt and updatedAt (Smithy: GetDashboard input arn -> DashboardArn, pattern ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$, so the recorded ARNs are well formed). Recorded behavior: the four requested ARNs resolve to the managed dashboards named Compute ('Compute spend, efficiency, and commitment health across your compute services'), Database ('Database spend, efficiency, and commitment health across your database services'), Reservations ('Reserved Instance cost, utilization, and coverage health') and Savings Plans ('Savings Plans utilization and coverage health'), each with its published widget list and its catalog publication/revision instants.
Emulator: every one of the four answers HTTP 400 with error code ResourceNotFoundException and the message 'Resource <arn> does not exist.' — fixed text 'Resource ' ... ' does not exist.' with the ARN taken verbatim from the request — and an empty body as far as the modelled output members go (arn, name, description, type, createdAt, updatedAt, widgets all absent). The catalog seed stores those dashboards under different, emulator-invented resource ids, so the published ARNs match no key in the store and the handler's not-found path runs.
Why: the published managed dashboards are unreachable by the ARNs AWS issues, so any client holding a real catalog ARN (console, CLI, a scheduled report's dashboardArn) gets a refusal where AWS serves data; the refusal is additionally the wrong status/shape for a read AWS never fails.
Fix: the managed-catalog seed in the service state must store the published ARNs/metadata (services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs, managed_dashboard_catalog around lines 467-515); the GetDashboard handler lookup itself (src/ops/get_dashboard.rs lines 176-183) is correct given a correct store.
Traces: bcm-dashboards-catalog-get-dashboard#task#2 (same cause at task#3, task#4, task#5).

Cause 3 of 3: ListDashboards reports an invented managed catalog membership.
AWS: ListDashboards returns dashboards -> DashboardReferenceList (@required), one reference per dashboard the account holds, including the AWS-managed ones a brand-new account already has. Recorded behavior: the listing contains references whose arn/name/description/createdAt/updatedAt are the published catalog's — among them managed-b0e4f3ab71454cf48a7d148287222f9d (Compute), managed-d2effbcc2dfd44eeb8ebec3f6e83382a (Database), managed-a8f4e2b1c3d6490ea1b7f5c8d2e94a03 (Reservations) and managed-7b3c91d4e5f2a0186c4d8e9f1a2b3c4d (Savings Plans), each with type AWS_MANAGED.
Emulator: those four references are absent and unpublished ones appear instead (e.g. a 'Compute' reference under managed-6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d described as 'Cost and usage of your compute services, including Amazon EC2, AWS Lambda and AWS Fargate'), i.e. the resource ids and the descriptions of four of the five seeded entries are emulator inventions.
Why: a client that discovers dashboards by listing and then reads or schedules one gets ids that exist nowhere in AWS and never sees the dashboards AWS actually publishes, so discovery-then-use flows break and descriptions shown to users are fabricated.
Fix: the same managed-catalog seed in the service state; the ListDashboards projection/pagination is not at fault.
Traces: bcm-dashboards-catalog-list-dashboards#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 88.0%, 6 mismatched steps in 3 clusters, all service=bcm-dashboards):

1) bcm-dashboards-catalog-get-dashboard#task#1, GetDashboard, expected 200 / actual 200, verdict mismatch. detail: '$.widgets[id="48381c58fdf60999c778deb93fc9284a"]: missing (expected {...title: Committed vs Flexible Spend, configs: graph AmortizedCost/STACK over MONTHLY costAndUsage groupBy PURCHASE_TYPE, timeRange -P12M..P0M...}, got None)', likewise for d771f500c22cacaa952f1bba40260445 ('Budget Health', table display over a budget query), 2a7e51a22e1c00af3843c5cccb82b046 (UnblendedCost STACK grouped by SERVICE) and the other six published widget ids listed in the cluster key (35d71261755663fa7f5aa289556472ef, a15901be3f2d2c9f07f2c90938ca73d3, a7a5c864d98bfa0bcb1359bd4e3529f3, a80cc87050b1e60b97c65e38be2876ae, b8fe9c2e86f1737da142b62be6e865a8, ce2a10fcd1ab3961dc32b6a3c595166c).

2) bcm-dashboards-catalog-get-dashboard#task#2 (affected: task#2, task#3, task#4, task#5), GetDashboard, expected 200 / actual 400. detail: 'status: expected 200, got 400' plus '$.arn: missing (expected arn:aws:bcm-dashboards::<account>:dashboard/managed-b0e4f3ab71454cf48a7d148287222f9d, got None)', '$.name: missing (expected Compute)', '$.description: missing (expected Compute spend, efficiency, and commitment health across your compute services)', '$.type: missing (expected AWS_MANAGED)', '$.createdAt'/'$.updatedAt' missing, '$.widgets: missing (expected [...Cost Per Compute Service..., ...])'. The emulator body is the not-found refusal rendered by src/scaffolded.rs: HTTP 400, code ResourceNotFoundException, message 'Resource <requested arn> does not exist.'

3) bcm-dashboards-catalog-list-dashboards#task#1, ListDashboards, expected 200 / actual 200, verdict mismatch. detail: four published references missing — managed-b0e4f3ab71454cf48a7d148287222f9d (Compute), managed-d2effbcc2dfd44eeb8ebec3f6e83382a (Database), managed-a8f4e2b1c3d6490ea1b7f5c8d2e94a03 (Reservations), managed-7b3c91d4e5f2a0186c4d8e9f1a2b3c4d (Savings Plans) — and an extra reference managed-6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d ('Compute', 'Cost and usage of your compute services, including Ama...') present instead.

Source of the fault: services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs — managed_dashboard_catalog() (lines 467-515) seeds five hand-written entries (resource ids 4dc5619130f2463687fc45e003e4b930, 6b1f0c5d2a8e4f3b9c7d1e0a5f8b2c4d, 7c2a1d4e9b0f5a3c8d6e2f1b4a7c9d0e, 8d3b2e5f0c1a6b4d9e7f3a2c5b8d0e1f, 9e4c3f6a1d2b7c5e0f8a4b3d6c9e1f2a) with one widget each (cost_overview_widgets/cost_by_dimension_widgets/reservation_widgets/savings_plans_widgets, lines 324-423) and installs them in Default::default() (line 892). Smithy consulted: GetDashboard and ListDashboards signatures/errors from services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json (GetDashboard.arn @pattern ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$; widgets @required WidgetList length 0..20; ResourceNotFoundException @httpError 404).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus: replay_wire_corpus() for trace set simparity-wire-aws-v1.
2. Cause 2 (hard failure): call GetDashboard with arn=arn:aws:bcm-dashboards::<account>:dashboard/managed-b0e4f3ab71454cf48a7d148287222f9d (likewise the Database, Reservations and Savings Plans managed ARNs). AWS answers 200 with the managed dashboard; the emulator answers 400 ResourceNotFoundException 'Resource <arn> does not exist.' — fixture bcm-dashboards-catalog-get-dashboard, steps task#2..task#5.
3. Cause 3: call ListDashboards on a fresh account. AWS lists the four published managed references above; the emulator lists five entries whose resource ids and descriptions (except the Cost Overview and Trends entry) appear nowhere in AWS — fixture bcm-dashboards-catalog-list-dashboards, step task#1.
4. Cause 1: call GetDashboard on the one managed ARN the seed does resolve. AWS returns the nine published widgets; the emulator returns a single synthesized widget — fixture bcm-dashboards-catalog-get-dashboard, step task#1.
