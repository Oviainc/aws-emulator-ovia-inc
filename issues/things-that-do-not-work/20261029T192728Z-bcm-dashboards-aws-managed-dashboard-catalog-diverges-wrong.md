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
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# bcm-dashboards AWS_MANAGED dashboard catalog diverges: wrong rows, missing widgets, GetDashboard refuses 400

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDashboard

## Description
The AWS_MANAGED dashboard catalog the service pre-provisions does not match the catalog the recorded wire serves: the listing exposes a different managed set, the managed dashboards the corpus reads by ARN are absent so GetDashboard refuses them with HTTP 400, and the one managed dashboard the emulator does hold carries a different widget set. All three causes are the same seed-and-lookup defect seen from three operations, and each must be closed. NOTE: the resolved issue `20261023T081740Z-bcm-dashboards-state-gaps-aws-managed-catalog-missing` touched this area but is fixed and does NOT cover these failures.

Cause 1 of 3: GetDashboard on an AWS_MANAGED dashboard returns a dashboard whose widget list is not the managed dashboard's widget list.
AWS: GetDashboard(arn) answers HTTP 200 with arn, name, description, type and widgets (Smithy: GetDashboardResponse.widgets -> WidgetList, @required, @length min 0 max 20; each widget carries id, title, description, width, height, horizontalOffset and configs; type -> DashboardType enum including AWS_MANAGED). An AWS-managed dashboard's widget set is fixed by AWS for that dashboard — recorded behavior only for the specific widget inventory, and the recording shows the managed dashboard answering a widget list whose member ids, titles, descriptions, geometry and `configs.queryParameters` (granularity, groupBy, metrics, timeRange) are all present.
Emulator: HTTP 200, but every widget the recorded response carries is absent from `widgets` (nine widget ids missing, e.g. the amortized-cost-by-purchase-type widget titled "Committed vs Flexible Spend" and the budget table titled "Budget Health"); the emulator serves its own, different widget inventory for that managed dashboard.
Why: a client that renders a managed dashboard, or resolves a scheduled report's widgetIds against it, finds none of the widgets AWS publishes for that dashboard — every widget-id-scoped call built from the real catalog fails against the emulator.
Fix: the AWS_MANAGED dashboard catalog seed in the service's state — the managed dashboards' widget definitions — which the GetDashboard handler reads.
Traces: bcm-dashboards-catalog-get-dashboard#task#1.

Cause 2 of 3: GetDashboard refuses an AWS_MANAGED dashboard ARN with HTTP 400 instead of returning the dashboard.
AWS: GetDashboard(arn) for an ARN that exists in the account's catalog answers HTTP 200 with the required members arn, name, type, widgets, createdAt, updatedAt and the optional description (Smithy: GetDashboard, input arn -> DashboardArn @required, @length 20..2048, @pattern ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$; errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400). The ARNs read here are well-formed under that pattern and the recording answers each of them 200 with type AWS_MANAGED.
Emulator: HTTP 400, and the response carries none of the response members (arn, createdAt, description, name, type, updatedAt, widgets all absent). Hypothesis: the ARN resolves to nothing because the managed catalog does not contain these dashboards (see cause 3), and the miss is rendered as a 400 refusal; note that for a genuinely absent dashboard the model's miss error is ResourceNotFoundException at HTTP 404, not a 400.
Why: every read of an AWS-managed dashboard fails outright — a client that lists the catalog and fetches a managed dashboard by the ARN it was just given gets a client-error refusal instead of the dashboard, so no managed-dashboard flow can run.
Fix: the AWS_MANAGED catalog seed plus GetDashboard's ARN lookup over it (a managed ARN present in the catalog must resolve; a truly absent one must take the modelled not-found path).
Traces: bcm-dashboards-catalog-get-dashboard#task#2 (cluster also covers task#3, task#4 and task#5 of the same fixture).

Cause 3 of 3: ListDashboards omits the account's AWS_MANAGED dashboards and lists a different managed row instead.
AWS: ListDashboards answers HTTP 200 with `dashboards` (Smithy: ListDashboardsResponse.dashboards -> DashboardReferenceList, @required; each reference carries arn, name, description, type, createdAt, updatedAt; paginated by nextToken/maxResults). The listing includes the AWS-managed dashboards AWS pre-provisions for the account — recorded behavior only for the exact inventory, and the recording shows four AWS_MANAGED references, named Compute, Database, Reservations and Savings Plans, each with its own description (e.g. Compute: "Compute spend, efficiency, and commitment health across your compute services") and its own createdAt/updatedAt.
Emulator: HTTP 200, but none of those four managed references is present; it serves a different managed row for the name "Compute" (a different dashboard ARN and a different description), and the Database, Reservations and Savings Plans managed dashboards are missing entirely.
Why: a client enumerating the account's dashboards sees neither the managed dashboards it expects nor usable identifiers for them, and the ARNs it does get back do not describe the managed dashboards AWS publishes. (Note: the per-dashboard ARN suffix is a seeded identifier, not a requirement in itself; the requirement is that the four managed dashboards exist in the catalog with their names, descriptions and widget sets and that each listed ARN resolves through GetDashboard.)
Fix: the AWS_MANAGED dashboard catalog seed in the service's state, read by the ListDashboards handler.
Traces: bcm-dashboards-catalog-list-dashboards#task#1.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, three divergence clusters on the managed-dashboard catalog:

1. GetDashboard, bcm-dashboards-catalog-get-dashboard#task#1, status 200 -> 200:
   `$.widgets[id="48381c58fdf60999c778deb93fc9284a"]: missing (expected {... 'title': 'Committed vs Flexible Spend', 'configs': [{'displayConfig': {'graph': {'AmortizedCost': {'visualType': 'STACK'}}}, 'queryParameters': {'costAndUsage': {'granularity': 'MONTHLY', 'groupBy': [{'key': 'PURCHASE_TYPE', 'type': 'DIMENSION'}], 'metrics': ['AmortizedCost'], ...}}}]}, got None)`
   plus eight further widget ids reported missing in the same cluster (divergence_key lists ids 2a7e51a2..., 35d71261..., 48381c58..., a15901be..., a7a5c864..., a80cc870..., b8fe9c2e..., ce2a10fc..., d771f500...).

2. GetDashboard, bcm-dashboards-catalog-get-dashboard#task#2 (+ task#3, task#4, task#5), status expected 200, got 400:
   `status: expected 200, got 400`
   `$.arn: missing (expected 'arn:aws:bcm-dashboards::000000000000:dashboard/managed-b0e4f3ab71454cf48a7d148287222f9d', got None)`
   `$.name: missing (expected 'Compute', got None)`
   `$.type: missing (expected 'AWS_MANAGED', got None)`
   `$.widgets: missing (...)`, `$.description`, `$.createdAt`, `$.updatedAt` likewise missing.

3. ListDashboards, bcm-dashboards-catalog-list-dashboards#task#1, status 200 -> 200:
   four managed references reported missing — Compute, Database ("Database spend, efficiency, and commitment health across your database services"), Reservations ("Reserved Instance cost, utilization, and coverage health"), Savings Plans ("Savings Plans utilization and coverage health"), each `type: 'AWS_MANAGED'` — and one reported `extra`: a managed dashboard named "Compute" under a different ARN with the description "Cost and usage of your compute services, ...".

Model corroboration: smithy_query(query="operation", operation="GetDashboard,ListDashboards", model_path="services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json") — GetDashboard output requires arn/name/type/widgets/createdAt/updatedAt and models ResourceNotFoundException at 404 for a miss; ListDashboards output requires `dashboards`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Replay fixture bcm-dashboards-catalog-list-dashboards step task#1: ListDashboards answers 200 but the four AWS_MANAGED references (Compute, Database, Reservations, Savings Plans) recorded by the wire are absent and a different managed "Compute" row is returned.
3. Replay fixture bcm-dashboards-catalog-get-dashboard step task#2 (and task#3-#5): GetDashboard on each recorded AWS_MANAGED dashboard ARN answers HTTP 400 with no response members, where the recording answers 200 with type AWS_MANAGED.
4. Replay the same fixture's step task#1: GetDashboard answers 200 but none of the nine recorded widget ids appears in `widgets`.
