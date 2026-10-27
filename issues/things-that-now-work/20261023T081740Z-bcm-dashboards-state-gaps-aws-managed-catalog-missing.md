Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-dashboards-catalog-list-dashboards
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-dashboards-catalog-list-dashboards bcm-dashboards-catalog-get-dashboard bcm-dashboards-list-dashboards-max-results-out-of-range-rejected bcm-dashboards-dashboard-error-paths bcm-dashboards-dashboard-tagging bcm-dashboards-dashboard-widget-variety
Issue-Shape-Id: com.amazonaws.bcmdashboards#AWSBCMDashboardsService
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards[*]
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$headers.content-type
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.arn
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.createdAt
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.description
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.name
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.type
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.updatedAt
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$.widgets
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|$headers.content-type
Issue-Key: bcm-dashboards|GetDashboard|2xx->4xx|status
Issue-Key: bcm-dashboards|ListDashboards|2xx->2xx|$.dashboards
Issue-Key: bcm-dashboards|CreateDashboard|4xx->2xx|$.__type
Issue-Key: bcm-dashboards|CreateDashboard|4xx->2xx|$.message
Issue-Key: bcm-dashboards|CreateDashboard|4xx->2xx|status
Issue-Key: bcm-dashboards|ListTagsForResource|2xx->2xx|$.resourceTags[key="team"]
Issue-Key: bcm-dashboards|GetResourcePolicy|2xx->2xx|$.policyDocument
Issue-Key: bcm-dashboards|GetResourcePolicy|2xx->2xx|$headers.content-type
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# bcm-dashboards state gaps: AWS_MANAGED catalog missing, name pattern unenforced, tag dropped, empty resource policy

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDashboards

## Description
Six wire divergences in the bcm-dashboards emulator's stored state and response payloads. SIX causes; all six must be closed before this issue is submitted. (The response content-type header also diverges on several of these steps - that is one shared serializer defect and is filed separately as the bcm-dashboards content-type issue; do not re-fix it here.)

=== Cause 1 of 6: ListDashboards does not return the AWS-managed dashboard catalog ===
AWS: recorded behavior only - the model does not describe this catalog at all (Smithy: DashboardType is an enum whose only value is CUSTOM, yet the recorded references carry type 'AWS_MANAGED'; ListDashboards.dashboards targets DashboardReferenceList and is @required). The recording shows that a brand-new account ALREADY lists five AWS_MANAGED DashboardReference entries - named 'Cost Overview and Trends', 'Compute', 'Database', 'Reservations' and 'Savings Plans', each with an arn of the form arn:aws:bcm-dashboards::<account>:dashboard/managed-<32 hex>, a description, type AWS_MANAGED and createdAt/updatedAt timestamps - before the caller creates anything. Requirement: the service pre-seeds a fixed set of managed dashboards that ListDashboards returns alongside the caller's own CUSTOM dashboards, with type AWS_MANAGED accepted on the wire. The specific ids, names and timestamps in the recording are the seed data's example values.
Emulator: ListDashboards returns an empty `dashboards` list on a fresh state - all five expected entries are reported missing.
Why: a client enumerating dashboards on a fresh account sees nothing, so every flow that starts from the managed catalog (open a managed dashboard, clone it, filter CUSTOM vs AWS_MANAGED) has no input and cannot be developed against. It also makes pagination over the catalog untestable.
Fix: the service's initial state / seed data for the dashboard store, plus the DashboardType handling in ListDashboards' serializer so AWS_MANAGED is representable.
Traces: bcm-dashboards-catalog-list-dashboards#task#1

=== Cause 2 of 6: GetDashboard on a managed dashboard ARN answers 404 instead of the dashboard ===
AWS: recorded behavior only - GetDashboard on one of the seeded managed ARNs returns HTTP 200 with the full dashboard: arn, name, description, type AWS_MANAGED, createdAt/updatedAt and a populated `widgets` list (recorded example: widgets with title/description/id/width/height/horizontalOffset and configs carrying displayConfig.graph and queryParameters.costAndUsage with granularity, metrics, groupBy and a relative timeRange). Smithy: GetDashboard output requires arn, name, type, widgets, createdAt, updatedAt.
Emulator: HTTP 404 with an empty body - every output member is missing.
Why: the managed dashboards are the only dashboards that exist before a client writes anything, so reading one is the first call a console-like client makes; it fails outright. Same root as cause 1 (nothing seeded), but it is a distinct recorded operation and its widget payload is an additional requirement, so it needs its own verification.
Fix: the same seed data as cause 1, which must carry each managed dashboard's full widget list, plus the GetDashboard handler's lookup over the seeded store.
Traces: bcm-dashboards-catalog-get-dashboard#task#1 (cluster of 5: #task#1 through #task#5)

=== Cause 3 of 6: ListDashboards is empty in the maxResults fixture too ===
AWS: recorded behavior only - same catalog as cause 1: after a rejected out-of-range maxResults call, a valid ListDashboards returns the five seeded AWS_MANAGED references.
Emulator: `dashboards` is [].
Why: confirms the catalog gap is in the store and not specific to one fixture; recorded separately because this step is the one that proves a valid ListDashboards still answers correctly after a ValidationException on maxResults (the rejection itself passed).
Fix: as cause 1.
Traces: bcm-dashboards-list-dashboards-max-results-out-of-range-rejected#task#2

=== Cause 4 of 6: CreateDashboard accepts a name that violates the modeled DashboardName pattern ===
AWS: Smithy - CreateDashboard.name targets DashboardName, @required, @length 2..50, @pattern ^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$ (the negative lookahead forbids two consecutive spaces); CreateDashboard models ValidationException at @httpError 400. The recorded response to a name containing a double space is HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' and a request-derived message of the standard form "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: <the pattern>", and no dashboard is created.
Emulator: HTTP 200 - the dashboard is created and an ARN minted; __type and message are absent.
Why: a client relying on AWS to refuse malformed names gets a resource the real service would never store, and the name-validation error path cannot be exercised. Later reads then return a name outside the modeled pattern, which a strict output validator rejects. NOTE: the open behavioral issue "bcm-dashboards: DashboardName pattern unenforced, constraint/not-found error messages diverge, ..." describes this same root cause, but it carries no wire replay key for this step, so this wire step is recorded here and must be re-replayed even if that issue is closed first.
Fix: the request-validation layer for this service's constrained strings - enforce DashboardName's @pattern on CreateDashboard (and UpdateDashboard, where the shape is also @required) before the handler runs, answering ValidationException at 400 and leaving state untouched.
Traces: bcm-dashboards-dashboard-error-paths#task#3

=== Cause 5 of 6: ListTagsForResource omits a tag the resource carries ===
AWS: recorded behavior only - Smithy: ListTagsForResource.resourceTags targets ResourceTagList (@length 0..200) of ResourceTag{key,value}. In the recorded tagging flow the response lists every tag currently attached to the dashboard, including key 'team' with value 'billing'.
Emulator: HTTP 200 and the other tags present, but the entry for key 'team' is missing entirely.
Why: a client that tags a dashboard and then reads its tags back cannot see the tag, so tag-driven flows (cost allocation, filtering, policy on tags) silently lose data. Hypothesis: the tag write that should have produced this entry is not persisted - the open stub issues "Implement bcm-dashboards TagResource" and "Implement bcm-dashboards UntagResource" may be the upstream cause; if so this step is the fidelity check that implementation must satisfy, and it is not closed merely by making TagResource answer.
Fix: the tag store in this service's state plus whichever handler applies tags on this path (TagResource and/or CreateDashboard.resourceTags), with ListTagsForResource reading the full set back.
Traces: bcm-dashboards-dashboard-tagging#task#4

=== Cause 6 of 6: GetResourcePolicy returns an empty string instead of an empty JSON document ===
AWS: recorded behavior only - Smithy: GetResourcePolicy output requires policyDocument (GenericString, @length 0..1024), and the recorded response for a dashboard with no policy attached carries policyDocument '{}' - an empty JSON object, not an empty string.
Emulator: policyDocument is '' (empty string).
Why: callers feed policyDocument straight into a JSON parser; '{}' parses to an empty policy while '' raises a parse error, so the no-policy case breaks a client that works against the recorded service.
Fix: the GetResourcePolicy handler / its serializer - represent the absent policy as the empty JSON document.
Traces: bcm-dashboards-dashboard-widget-variety#task#3

## Evidence
Corpus simparity-wire-aws-v1, 50 steps, 17 passed (34%), 14 divergence clusters - all bcm-dashboards. emulator_up=true, bootstrap_signal=false, state_reset_failures=0 (so none of these ran against a stale fixture's state).

Cause 1 - bcm-dashboards-catalog-list-dashboards#task#1 (ListDashboards, 200 -> 200) [mismatch]:
  $.dashboards[0..4]: missing - expected five AWS_MANAGED references, e.g. {'arn': 'arn:aws:bcm-dashboards::000000000000:dashboard/managed-4dc5619130f2463687fc45e003e4b930', 'createdAt': 1786644000.0, 'description': 'High-level view of your AWS costs, usage trends, and forecasts', 'name': 'Cost Overview and Trends', 'type': 'AWS_MANAGED', 'updatedAt': 1787079600.0}; got None for each.

Cause 2 - bcm-dashboards-catalog-get-dashboard#task#1 (GetDashboard) [mismatch], cluster of 5 (#task#1..#task#5):
  status: expected 200, got 404
  $.arn, $.name, $.description, $.type, $.createdAt, $.updatedAt, $.widgets: all missing (got None). Expected type 'AWS_MANAGED' and a widgets list whose first entry is {'id': 'a18b58807f3934dbcfebd21ad359afaa', 'title': 'Total Current And Forecasted Cost', 'description': 'Historical monthly unblended cost with forecasted cost.', 'width': 2, 'height': 4, 'horizontalOffset': 0, 'configs': [{'displayConfig': {'graph': {'UnblendedCost': {'visualType': 'BAR'}}}, 'queryParameters': {'costAndUsage': {'granularity': 'MONTHLY', 'metrics': ['UnblendedCost'], 'groupBy': [], 'timeRange': {'startTime': {'type': 'RELATIVE', 'value': '-P12M'}, 'endTime': {'type': 'RELATIVE', 'value': 'P4M'}}}}}]}.

Cause 3 - bcm-dashboards-list-dashboards-max-results-out-of-range-rejected#task#2 (ListDashboards, 200 -> 200) [mismatch]:
  $.dashboards: value_mismatch (expected the five AWS_MANAGED references, got []).

Cause 4 - bcm-dashboards-dashboard-error-paths#task#3 (CreateDashboard) [mismatch]:
  status: expected 400, got 200
  $.__type: missing (expected 'com.amazon.coral.validate#ValidationException', got None)
  $.message: missing (expected "1 validation error detected: Value 'Bad  Name' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]", got None)

Cause 5 - bcm-dashboards-dashboard-tagging#task#4 (ListTagsForResource, 200 -> 200) [mismatch]:
  $.resourceTags[key="team"]: missing (expected {'key': 'team', 'value': 'billing'}, got None)

Cause 6 - bcm-dashboards-dashboard-widget-variety#task#3 (GetResourcePolicy, 200 -> 200) [mismatch]:
  $.policyDocument: value_mismatch (expected '{}', got '')

## Steps to Reproduce
Boot the emulator on fresh state and point a sigv4 client at the bcm-dashboards endpoint.

Cause 1: call ListDashboards before creating anything. Expect five AWS_MANAGED DashboardReference entries; observe dashboards == [].
Cause 2: call GetDashboard with arn=arn:aws:bcm-dashboards::000000000000:dashboard/managed-<one of the seeded managed ids>. Expect HTTP 200 with name/description/type AWS_MANAGED/widgets/createdAt/updatedAt; observe HTTP 404 and an empty body.
Cause 3: call ListDashboards with maxResults=0 (or any value outside 1..100) and confirm the ValidationException, then call ListDashboards with no maxResults. Expect the five managed references; observe [].
Cause 4: CreateDashboard with name='Bad  Name' (two consecutive spaces) and one valid widget. Expect HTTP 400, __type com.amazon.coral.validate#ValidationException, and no dashboard created; observe HTTP 200 and a minted ARN.
Cause 5: run the tagging flow - create a dashboard with tags and/or TagResource it so it carries key 'team' value 'billing', then ListTagsForResource on its ARN. Expect the 'team' entry among the returned tags; observe it absent.
Cause 6: create a dashboard, then GetResourcePolicy on its ARN with no policy attached. Expect policyDocument '{}'; observe ''.

Then replay the fixtures named in Traces.
