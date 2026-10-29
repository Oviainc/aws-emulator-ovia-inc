Issue-Source: trace-sweep-divergence
Issue-Trace: appfabric-app-authorization-oauth2-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appfabric-app-authorization-oauth2-lifecycle appfabric-resource-not-found-and-conflict-errors
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric|CreateAppAuthorization|2xx->2xx|$.appAuthorization.authUrl
Issue-Key: appfabric|CreateAppAuthorization|2xx->2xx|$.appAuthorization.createdAt
Issue-Key: appfabric|CreateAppAuthorization|2xx->2xx|$.appAuthorization.updatedAt
Issue-Key: appfabric|GetAppAuthorization|2xx->2xx|$.appAuthorization.authUrl
Issue-Key: appfabric|GetAppAuthorization|2xx->2xx|$.appAuthorization.createdAt
Issue-Key: appfabric|GetAppAuthorization|2xx->2xx|$.appAuthorization.updatedAt
Issue-Key: appfabric|UpdateAppAuthorization|2xx->2xx|$.appAuthorization.authUrl
Issue-Key: appfabric|UpdateAppAuthorization|2xx->2xx|$.appAuthorization.createdAt
Issue-Key: appfabric|UpdateAppAuthorization|2xx->2xx|$.appAuthorization.updatedAt
Issue-Key: appfabric|ListAppAuthorizations|2xx->2xx|$.appAuthorizationSummaryList[*]
Issue-Commit: 3660361421cece15e150541af9cef0f01b201b94

# AppFabric date-time members serialize as epoch numbers instead of RFC3339 strings

> _Note: this issue was identified against the code base at commit `3660361421cece15e150541af9cef0f01b201b94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppAuthorization, GetAppAuthorization, UpdateAppAuthorization, ListAppAuthorizations

## Description
Four divergence clusters from the wire replay (score 66%, 14 mismatched steps) that all land on the same root cause: every modeled AppFabric timestamp member is emitted as a JSON NUMBER (epoch seconds with a fractional part) where AWS emits an RFC3339 date-time STRING. The four clusters are listed as four causes because each is a separate operation and a separate fixture step that must be re-replayed before this is closed, but one serializer fix is expected to close all four. A fixer must confirm ALL FOUR.

--------------------------------------------------
Cause 1 of 4: CreateAppAuthorization response createdAt/updatedAt are epoch numbers
--------------------------------------------------
AWS: AppAuthorization$createdAt and AppAuthorization$updatedAt are both @required and target com.amazonaws.appfabric#DateTime, which carries @timestampFormat("date-time"). Smithy: AppAuthorization.createdAt / .updatedAt -> DateTime, timestampFormat "date-time", required; CreateAppAuthorization http 201 POST /appbundles/{appBundleIdentifier}/appauthorizations. So the wire value is an ISO-8601/RFC3339 string with a trailing Z (the recording shows the shape of it: a date-time string, e.g. '<yyyy>-<MM>-<dd>T<HH>:<mm>:<ss>.<frac>Z'); the particular instant in any recording is a per-run value, the STRING ENCODING is the requirement.
Emulator: the member is serialized as a bare JSON number of epoch seconds (the replay reports got 1793303685.532253 where a date-time string was expected). The state layer is already correct - src/state.rs AppAuthorizationRecord::to_app_authorization builds ::aws_smithy_types::DateTime::from(self.created_at) / from(self.updated_at) - so the loss happens on the way out, in the response serializer / the member schema, which is not honouring the shape-level @timestampFormat("date-time") and is falling back to the restJson1 default of epoch-seconds.
Why: a generated AWS SDK client deserializes these members per the modeled format, so a number where a date-time string is modeled either fails deserialization outright or is read as a different type; because both members are @required, there is no tolerant path. Any client that round-trips or displays an app authorization's creation/update time is broken, and so is any consumer comparing the emulator's output against a recorded AWS response.
Fix: the response serializer for timestamp members (the generated member schemas in services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/types.rs and the runtime serializer that consumes them) must honour @timestampFormat("date-time") on the DateTime shape. The handler and state layer need no change. The same shape is used by Ingestion$createdAt/updatedAt and IngestionDestination$createdAt/updatedAt, so fix it once at the serializer rather than per operation.
Traces: appfabric-app-authorization-oauth2-lifecycle#task#2.

--------------------------------------------------
Cause 2 of 4: GetAppAuthorization response createdAt/updatedAt are epoch numbers
--------------------------------------------------
AWS: GetAppAuthorization (http 200 GET /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}) returns the same AppAuthorization shape, so createdAt/updatedAt are date-time strings and must read back identical to what the create call reported.
Emulator: same two members come back as epoch-seconds numbers.
Why: as cause 1 - a read-back path is exactly where a client parses the timestamp, so this is the most visible instance.
Fix: same component as cause 1 (serializer/member schema), no per-operation change.
Traces: appfabric-app-authorization-oauth2-lifecycle#task#3 (also observed at appfabric-resource-not-found-and-conflict-errors#task#7).

--------------------------------------------------
Cause 3 of 4: UpdateAppAuthorization response createdAt/updatedAt are epoch numbers
--------------------------------------------------
AWS: UpdateAppAuthorization returns the AppAuthorization shape with createdAt unchanged from the create and updatedAt re-stamped to the update instant, both as date-time strings.
Emulator: both members are epoch-seconds numbers (the replay shows two DIFFERENT numeric values here, i.e. the re-stamp itself is working; only the encoding is wrong).
Why: as cause 1.
Fix: same component as cause 1.
Traces: appfabric-app-authorization-oauth2-lifecycle#task#4.

--------------------------------------------------
Cause 4 of 4: ListAppAuthorizations summary updatedAt is an epoch number
--------------------------------------------------
AWS: AppAuthorizationSummary$updatedAt is @required and targets the same DateTime shape with @timestampFormat("date-time"), so each entry of appAuthorizationSummaryList carries a date-time string. Smithy: ListAppAuthorizations output appAuthorizationSummaryList @required, items AppAuthorizationSummary{appAuthorizationArn, appBundleArn, app, tenant, status, updatedAt}.
Emulator: the summary entry is otherwise correct (arn, bundle arn, app, tenant, status all match) but updatedAt is an epoch-seconds number, so the whole list item compares as divergent.
Why: as cause 1, and here it also makes the list response unparseable as a whole rather than one scalar being wrong.
Fix: same component as cause 1; src/state.rs AppAuthorizationRecord::to_summary already builds a DateTime.
Traces: appfabric-app-authorization-oauth2-lifecycle#task#5.

## Evidence
All four clusters come from replaying trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, 53 steps, 35 passed, 14 mismatched, 4 blocked, score 66%).

Cause 1 - fixture appfabric-app-authorization-oauth2-lifecycle, step task#2, CreateAppAuthorization, status 201 expected and 201 observed:
  $.appAuthorization.createdAt: expected a date-time string, got 1793303685.532253
  $.appAuthorization.updatedAt: expected a date-time string, got 1793303685.532253

Cause 2 - same fixture, step task#3, GetAppAuthorization, status 200/200:
  $.appAuthorization.createdAt: expected a date-time string, got 1793303685.532253
  $.appAuthorization.updatedAt: expected a date-time string, got 1793303685.532253
  (cluster also covers appfabric-resource-not-found-and-conflict-errors#task#7)

Cause 3 - same fixture, step task#4, UpdateAppAuthorization, status 200/200:
  $.appAuthorization.createdAt: got 1793303685.532253
  $.appAuthorization.updatedAt: got 1793303685.5800717 (re-stamped, still numeric)

Cause 4 - same fixture, step task#5, ListAppAuthorizations, status 200/200:
  $.appAuthorizationSummaryList[0]: expected an entry whose updatedAt is a date-time string; got the same entry with updatedAt: 1793303685.5800717. All other members of the entry (appAuthorizationArn, appBundleArn, app=SLACK, tenant, status=PendingConnect) matched.

Model evidence (services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json):
  com.amazonaws.appfabric#DateTime: type timestamp, timestampFormat "date-time".
  AppAuthorization.createdAt / .updatedAt: target DateTime, required, timestampFormat "date-time".
  AppAuthorizationSummary.updatedAt: target DateTime, required, timestampFormat "date-time".

Source evidence: services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs lines 466-491 (to_app_authorization / to_summary) already construct ::aws_smithy_types::DateTime values, so the defect is downstream of the handler, in the member schema / response serializer. The same construction appears at lines 687-698 (Ingestion) and 771-782 (IngestionDestination), so those responses are expected to carry the same defect even though no replayed step pinned them.

Note: the same clusters also report $.appAuthorization.authUrl as mismatched while printing byte-identical expected and actual strings (and, at task#2, against the fixture's unresolved '{{auth_url_create}}' placeholder). That is a fixture-variable binding artifact downstream of the timestamp failure, not a separate emulator requirement, and it is not filed as a cause.

## Steps to Reproduce
1. Boot the emulator and POST /appbundles to create an app bundle.
2. POST /appbundles/{bundle}/appauthorizations with app="SLACK", authType="oauth2", credential.oauth2Credential={clientId, clientSecret}, tenant={tenantIdentifier, tenantDisplayName}.
3. Inspect the raw 201 body: appAuthorization.createdAt and appAuthorization.updatedAt are JSON numbers. AWS emits RFC3339 date-time strings (ending in Z) for both, because AppAuthorization$createdAt/$updatedAt target DateTime with @timestampFormat("date-time").
4. GET /appbundles/{bundle}/appauthorizations/{auth} - same two members, same numeric encoding (cause 2).
5. PATCH/PUT the authorization via UpdateAppAuthorization - same two members numeric, updatedAt advanced (cause 3).
6. GET /appbundles/{bundle}/appauthorizations - appAuthorizationSummaryList[0].updatedAt is numeric (cause 4).
7. Re-replay fixtures appfabric-app-authorization-oauth2-lifecycle steps task#2, task#3, task#4 and task#5 (and appfabric-resource-not-found-and-conflict-errors#task#7) to confirm all four clusters pass.
