Issue-Source: trace-sweep-divergence
Issue-Trace: billing-get-billing-view-invalid-arn-pattern-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: billing-get-billing-view-invalid-arn-pattern-rejected
Issue-Shape-Id: com.amazonaws.billing#AWSBilling
Issue-Key: billing|ListBillingViews|2xx->2xx|$.billingViews[*].description
Issue-Key: billing|ListBillingViews|2xx->2xx|$.billingViews[*].healthStatus.StatusCode
Issue-Key: billing|ListBillingViews|2xx->2xx|$.billingViews[*].healthStatus.StatusReasons
Issue-Key: billing|ListBillingViews|2xx->2xx|$.billingViews[*].healthStatus.statusReasons
Issue-Key: billing|ListBillingViews|2xx->2xx|$headers.content-type
Issue-Key: billing|GetBillingView|2xx->2xx|$.billingView.createdAt
Issue-Key: billing|GetBillingView|2xx->2xx|$.billingView.derivedViewCount
Issue-Key: billing|GetBillingView|2xx->2xx|$.billingView.description
Issue-Key: billing|GetBillingView|2xx->2xx|$.billingView.healthStatus.StatusCode
Issue-Key: billing|GetBillingView|2xx->2xx|$.billingView.healthStatus.StatusReasons
Issue-Key: billing|GetBillingView|2xx->2xx|$.billingView.healthStatus.statusReasons
Issue-Key: billing|GetBillingView|2xx->2xx|$.billingView.sourceViewCount
Issue-Key: billing|GetBillingView|2xx->2xx|$headers.content-type
Issue-Key: billing|GetBillingView|4xx->4xx|$.fieldList
Issue-Key: billing|GetBillingView|4xx->4xx|$.reason
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# billing serves the default PRIMARY view and its boundary ValidationException with the wrong wire shape

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBillingView

## Description
Three distinct causes, all in the billing service, all exercised by the same fixture: the account's default PRIMARY billing view is rendered with the wrong description and without the health-status members the wire carries, the GetBillingView rendering of that same view additionally invents lifecycle members the wire does not carry, and the boundary refusal of a malformed `arn` is serialized without the two ValidationException members the model declares for it. All three are serializer/renderer faults on the response side of billing. THIS BUNDLE HAS 3 CAUSES; all three must be closed before it is submitted.

=== Cause 1 of 3: ListBillingViews renders the default PRIMARY view without healthStatus and with a description the service invents (fixture billing-get-billing-view-invalid-arn-pattern-rejected, step task#1) ===

AWS: recorded behavior only for the values; the members are modeled. Smithy: ListBillingViews.billingViews -> BillingViewList of BillingViewElement; BillingViewElement$description (BillingViewDescription, @length 0..1024, pattern ^([ a-zA-Z0-9_\+=\.\-@]+)?$, @sensitive) and BillingViewElement$healthStatus -> BillingViewHealthStatus{statusCode: BillingViewStatus (HEALTHY|UNHEALTHY|CREATING|UPDATING), statusReasons: BillingViewStatusReasons}. The recording shows the real endpoint returning, for the account's default PRIMARY view, a description built as the fixed text "Primary billing view for accountId: " followed by the owning account id taken from the caller's identity (the recorded run's 000000000000 is that run's account id, not part of the requirement), and a healthStatus object carrying the status code HEALTHY plus an EMPTY status-reason list. The recorded object spells those keys StatusCode / StatusReasons (and also carries an empty statusReasons), i.e. not the camelCase spelling the Smithy member names would produce; the same response is served with content-type application/x-amz-json-1.1, while this service's model declares awsJson1_0 — the wire, not the model, is the requirement here, and the recorded error type namespace (com.amazonaws.awsinsightsindexservice.v0#...) in cause 3 confirms the billing-view surface is served by the JSON 1.1 insights backend.

Emulator: serves description "Represents the billing data available by default" (the hard-coded PRIMARY_VIEW_DESCRIPTION) regardless of account, serves a healthStatus with none of StatusCode / StatusReasons / statusReasons present (no status code reaches the wire and the empty reason list is omitted entirely), and answers content-type application/x-amz-json-1.0.

Why: a client that reads billingViews[0].healthStatus.StatusCode to decide whether a view is usable reads null against the emulator and can never observe HEALTHY, so health-gated code paths are untestable; a client that asserts on or displays the PRIMARY view's description sees a sentence AWS never sends and never sees the account id AWS embeds in it. A client (or SDK/proxy) that keys its JSON-RPC parsing or signing off the response content-type sees a different protocol version than AWS announces for this surface.

Fix: the ListBillingViews response renderer plus the shared BillingViewElement rendering/serializer for healthStatus, and the seeding of the default PRIMARY view's description in this service's state (it must be derived from the account id, not a constant). The content-type belongs in this service's protocol/serializer wiring, which must announce JSON 1.1 for the billing-view surface.

Traces: billing-get-billing-view-invalid-arn-pattern-rejected#task#1 (ListBillingViews, 200 vs 200, body/header mismatch). Code: services/service-com-amazonaws-billing-awsbilling/src/state.rs:171 (PRIMARY_VIEW_DESCRIPTION), services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs.

=== Cause 2 of 3: GetBillingView renders the default PRIMARY view with synthesized createdAt/derivedViewCount/sourceViewCount that the wire omits, and with the same missing healthStatus and wrong description (fixture billing-get-billing-view-invalid-arn-pattern-rejected, step task#2) ===

AWS: recorded behavior only. Smithy: GetBillingView.billingView -> BillingViewElement (required); createdAt/updatedAt/viewDefinitionLastUpdatedAt are Timestamps and derivedViewCount/sourceViewCount Integers, all OPTIONAL members. The recording shows the real endpoint returning the account's default PRIMARY view with NO createdAt, NO derivedViewCount and NO sourceViewCount member at all — the service-managed default view is not a view the caller created and the endpoint reports no creation time or source/derived counts for it — plus the same description ("Primary billing view for accountId: " + the owning account id) and the same healthStatus{StatusCode: HEALTHY, StatusReasons: []} as cause 1, under content-type application/x-amz-json-1.1.

Emulator: emits createdAt as a synthesized epoch value, derivedViewCount 0 and sourceViewCount 0 for the default view, emits the constant description, emits a healthStatus without StatusCode/StatusReasons/statusReasons, and answers content-type application/x-amz-json-1.0.

Why: a client cannot distinguish the service-managed PRIMARY view from a caller-created view, because the emulator attaches creation metadata AWS does not report for it; code that treats "createdAt present" as "this is a customer-created view", or that round-trips a GetBillingView result into an update, behaves differently against the emulator than against AWS. The missing status code and the invented description have the same consequences as in cause 1, and the GetBillingView rendering is also internally inconsistent with the same view as ListBillingViews renders it.

Fix: the GetBillingView handler's BillingViewElement construction (it must leave createdAt and the two counts absent for the service-managed PRIMARY view rather than always setting them) together with the shared healthStatus rendering and the default view's description in state; same serializer/protocol wiring for the content-type.

Traces: billing-get-billing-view-invalid-arn-pattern-rejected#task#2 (GetBillingView, 200 vs 200, body/header mismatch). Code: services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_view.rs:191-211 (created_at/derived_view_count/source_view_count always Some, status_reasons None).

=== Cause 3 of 3: the boundary refusal of a malformed GetBillingView arn omits ValidationException$reason and $fieldList (fixture billing-get-billing-view-invalid-arn-pattern-rejected, step task#3) ===

AWS: Smithy: GetBillingView.arn (BillingViewArn) pattern ^arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/[a-zA-Z0-9/:_\+=\.\-@]{0,75}[a-zA-Z0-9]$; error ValidationException (httpError 400) whose members are message (required), reason (ValidationExceptionReason, REQUIRED, one of unknownOperation|cannotParse|fieldValidationFailed|other) and fieldList (list of ValidationExceptionField{name required, message required}). The recording confirms the wire: HTTP 400, reason "fieldValidationFailed", and a one-entry fieldList whose name is the offending request member ("arn") and whose message is the fixed text "Value " + the offending value (rendered "[redacted]" because the member is sensitive) + " failed to satisfy constraint: Member must satisfy regular expression pattern: " + that member's modeled pattern — here the BillingViewArn pattern above. The variable parts are the member name, the value and the pattern, taken from the request and the model; the rest is fixed text.

Emulator: answers 400 but with neither member: $.reason is absent (expected "fieldValidationFailed") and $.fieldList is absent (expected the single arn entry above). Only message is serialized. Note that this service's HANDLER-written refusals do populate both (see invalid_field / resolve_arns_filter / decode_page_token in list_billing_views.rs:142-218) — it is the pattern refusal raised at the request boundary, before the handler runs, that loses them.

Why: reason is a required member of the modeled error, so a generated SDK client deserializing this response gets a ValidationException missing a required field; clients and tests that branch on reason == fieldValidationFailed, or that surface fieldList[].name to tell a user WHICH member was rejected, get nothing and cannot distinguish a malformed-member refusal from any other 400.

Fix: the boundary/constraint validation path that converts a modeled pattern (and length/range) violation into this service's ValidationException — it must fill reason = fieldValidationFailed and a fieldList entry naming the violating member, the same shape the hand-written handler refusals already produce.

Traces: billing-get-billing-view-invalid-arn-pattern-rejected#task#3 (GetBillingView, 400 vs 400, missing $.reason and $.fieldList), and the same cluster's billing-get-billing-view-invalid-arn-pattern-rejected#task#4. Code: services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_view.rs:149-156 (notes the boundary validator owns the pattern refusal).

No open issue covers any of these three: the open billing-family issues are all billingconductor or marketplace-agreement behavioral findings (e.g. 20261024T045911Z-billingconductor-createbillinggroup-event-propagation.md), none of them about the billing service's billing-view rendering or its boundary ValidationException shape.

## Evidence
Replay of fixture billing-get-billing-view-invalid-arn-pattern-rejected (trace set simparity-wire-aws-v1), 4 steps, 4 mismatched, 0 passed.

Step task#1, ListBillingViews, expected 200 / actual 200:
  $.billingViews[0].description: expected 'Primary billing view for accountId: <account-id>' (recorded: 'Primary billing view for accountId: 000000000000'), got 'Represents the billing data available by default'
  $.billingViews[0].healthStatus.StatusCode: expected 'HEALTHY', got None
  $.billingViews[0].healthStatus.StatusReasons: expected [], got None
  $.billingViews[0].healthStatus.statusReasons: expected [], got None
  $headers.content-type: expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0'

Step task#2, GetBillingView, expected 200 / actual 200:
  $.billingView.createdAt: expected absent, got a synthesized timestamp (recorded run: 1793293502.539239)
  $.billingView.derivedViewCount: expected absent, got 0
  $.billingView.sourceViewCount: expected absent, got 0
  $.billingView.description: expected 'Primary billing view for accountId: <account-id>', got 'Represents the billing data available by default'
  $.billingView.healthStatus.StatusCode: expected 'HEALTHY', got None
  $.billingView.healthStatus.StatusReasons: expected [], got None
  $.billingView.healthStatus.statusReasons: expected [], got None
  $headers.content-type: expected 'application/x-amz-json-1.1', got 'application/x-amz-json-1.0'

Step task#3 (and task#4 in the same cluster), GetBillingView with an arn that violates the modeled pattern, expected 400 / actual 400:
  $.reason: expected 'fieldValidationFailed', got None
  $.fieldList: expected [{'name': 'arn', 'message': 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/[a-zA-Z0-9/:_\+=\.\-@]{0,75}[a-zA-Z0-9]', '__type': 'com.amazonaws.awsinsightsindexservice.v0#ValidationExceptionField'}], got None

Model checked with smithy_query on services/service-com-amazonaws-billing-awsbilling/smithy-model/com-amazonaws-billing-awsbilling.json: ValidationException.reason is @required; ValidationExceptionReason values are unknownOperation|cannotParse|fieldValidationFailed|other; BillingViewHealthStatus has statusCode (BillingViewStatus: HEALTHY|UNHEALTHY|CREATING|UPDATING) and statusReasons; BillingViewElement.createdAt/derivedViewCount/sourceViewCount are optional; the service declares awsJson1_0 while the recorded wire for this surface is application/x-amz-json-1.1.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture billing-get-billing-view-invalid-arn-pattern-rejected from trace set simparity-wire-aws-v1.
3. Step task#1 — call ListBillingViews with no filters as the emulated account. Inspect the response: billingViews[0].healthStatus has no StatusCode/StatusReasons, description is the constant 'Represents the billing data available by default' instead of 'Primary billing view for accountId: <caller account id>', and the response content-type is application/x-amz-json-1.0 instead of application/x-amz-json-1.1.
4. Step task#2 — call GetBillingView with the default PRIMARY view's arn (arn:aws:billing::<account-id>:billingview/primary). The returned billingView additionally carries createdAt, derivedViewCount and sourceViewCount, which the recorded AWS response omits for the service-managed PRIMARY view, and repeats the healthStatus/description/content-type divergences.
5. Steps task#3/task#4 — call GetBillingView with an arn that violates the modeled BillingViewArn pattern (e.g. 'not-an-arn'). The emulator answers 400 with only `message`: no `reason` ('fieldValidationFailed') and no `fieldList` entry naming member 'arn' with the 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: <BillingViewArn pattern>' message.
