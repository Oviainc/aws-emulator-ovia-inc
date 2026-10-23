Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appfabric.create-ingestion-destination-short-bucket-name-rejected appfabric.list-ingestion-destinations-reflects-created appfabric.stop-already-disabled-ingestion-rejected appfabric.create-ingestion-without-app-authorization-rejected appfabric.create-app-bundle-and-get-roundtrip appfabric.create-app-bundle-with-customer-managed-key appfabric.list-app-authorizations-maxresults-out-of-range-rejected appfabric.create-app-authorization-invalid-app-name-rejected appfabric.concurrent-create-app-authorization-duplicate-tenant-single-winner
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric.create-ingestion-destination-short-bucket-name-rejected|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: appfabric.list-ingestion-destinations-reflects-created|run#8|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: appfabric.stop-already-disabled-ingestion-rejected|run#7|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: appfabric.create-ingestion-without-app-authorization-rejected|run#7|AssertionError|raise AssertionError("expected a rejection, but CreateIngestion succeeded with no app authorization")
Issue-Key: appfabric.create-app-bundle-and-get-roundtrip|run#14|AssertionError|assert read_message == "The app bundle ID was not found.", (
Issue-Key: appfabric.create-app-bundle-with-customer-managed-key|run#19|AssertionError|assert message == "The app bundle ID was not found.", (
Issue-Key: appfabric.list-app-authorizations-maxresults-out-of-range-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appfabric.create-app-authorization-invalid-app-name-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException for app={bad_app!r}, got {code!r}"
Issue-Key: appfabric.concurrent-create-app-authorization-duplicate-tenant-single-winner|run#12|AssertionError|assert len(winners) == 1, f"expected exactly one winner, got {winners}"
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# AppFabric: 8 behavioral divergences — three unimplemented ingestion operations answer 501, CreateIngestion ignores the app-authorization dependency, and three error-message/validation rules diverge

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIngestionDestination

## Description
This issue bundles EIGHT distinct root causes in the AppFabric (FabricFrontendService) emulator, all observed in the behavioral replay. They must ALL be closed before this issue is submitted. Related-but-separate: the suite auto-filed scaffold-stub issues for CreateAppAuthorization, DeleteIngestion, DeleteIngestionDestination, GetIngestionDestination, UpdateIngestionDestination, TagResource and StartUserAccessTasks; those issues cover standing those operations up. Causes 1-3 below name THREE FURTHER stubs that no issue covers, and causes 7-8 name semantics that go beyond merely standing CreateAppAuthorization up.

=== Cause 1 of 8: CreateIngestionDestination is an unimplemented scaffold stub ===
Failing check: appfabric.create-ingestion-destination-short-bucket-name-rejected
AWS: Smithy model (com.amazonaws.appfabric#CreateIngestionDestination) binds POST /appbundles/{appBundleIdentifier}/ingestions/{ingestionIdentifier}/ingestiondestinations, success 201, and models ValidationException (HTTP 400) among its errors. Recorded behavior: against an ingestionIdentifier that does not exist, AppFabric reports the missing parent as a VALIDATION failure, not a not-found — HTTP 400, error code `ValidationException`, message (example, recorded) "The specified ingestion does not exist". Note the model's `ingestionIdentifier` is a required @httpLabel Identifier (len 1..1011), so the identifier itself is well-formed; the refusal is server-side existence checking.
Emulator: answers HTTP 501 with error code `NotImplemented` ("operation CreateIngestionDestination is scaffolded but not implemented in this emulator"). The handler body is still the generated stub, returning `crate::scaffolded::Scaffolded::not_implemented("CreateIngestionDestination")`.
Why: a client cannot create an ingestion destination at all, and a client probing for a bad parent gets a non-modeled 501 instead of the 400 `ValidationException` the SDK knows how to classify — 501 is not in the operation's modeled error set, so error-specific handling and retry classification both break.
Fix: the operation's handler, plus the existence/validation check against the parent ingestion in this service's state.
Traces: appfabric.create-ingestion-destination-short-bucket-name-rejected (services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_ingestion_destination.rs, SCAFFOLDED marker at line 4, stub return at line 186).

=== Cause 2 of 8: ListIngestionDestinations is an unimplemented scaffold stub ===
Failing check: appfabric.list-ingestion-destinations-reflects-created
AWS: Smithy model (com.amazonaws.appfabric#ListIngestionDestinations) binds GET /appbundles/{appBundleIdentifier}/ingestions/{ingestionIdentifier}/ingestiondestinations, success 200, @readonly, @paginated(items=ingestionDestinations), and models ResourceNotFoundException (HTTP 404) — documented in the model as "The specified resource does not exist." Recorded behavior: listing destinations of an ingestion identifier that does not exist inside a real app bundle returns HTTP 404 with error code `ResourceNotFoundException`, message (example, recorded) "The specified ingestion does not exist".
Emulator: answers HTTP 501 with error code `NotImplemented`. The handler body is still the generated stub.
Why: the read side of the ingestion-destination resource is unusable, and the not-found probe clients use to confirm a resource is absent returns an unmodeled 501 instead of 404 `ResourceNotFoundException`.
Fix: the operation's handler, its pagination, and the parent-ingestion existence check in this service's state.
Traces: appfabric.list-ingestion-destinations-reflects-created (services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/list_ingestion_destinations.rs, SCAFFOLDED marker at line 4, stub return at line 180).

=== Cause 3 of 8: StopIngestion is an unimplemented scaffold stub ===
Failing check: appfabric.stop-already-disabled-ingestion-rejected
AWS: Smithy model (com.amazonaws.appfabric#StopIngestion) binds POST /appbundles/{appBundleIdentifier}/ingestions/{ingestionIdentifier}/stop, success 200 with an empty output, and models ResourceNotFoundException (HTTP 404) and ConflictException (HTTP 409). Recorded behavior: StopIngestion against an ingestion identifier that does not exist in an existing app bundle returns HTTP 404 with error code `ResourceNotFoundException`, message (example, recorded) "The specified ingestion does not exist"; the app bundle itself is left untouched.
Emulator: answers HTTP 501 with error code `NotImplemented`. The handler body is still the generated stub.
Why: ingestions can never be disabled, so the enabled→disabled lifecycle is unreachable, and the not-found path returns an unmodeled 501 instead of 404.
Fix: the operation's handler, plus the ingestion lookup and state transition in this service's state.
Traces: appfabric.stop-already-disabled-ingestion-rejected (services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/stop_ingestion.rs, SCAFFOLDED marker at line 4, stub return at line 164).

=== Cause 4 of 8: CreateIngestion is over-permissive — it ignores the app-authorization dependency ===
Failing check: appfabric.create-ingestion-without-app-authorization-rejected
AWS: Smithy model (com.amazonaws.appfabric#CreateIngestion) models ValidationException (HTTP 400) and takes required `app` (String255) and `tenantId` (TenantIdentifier). Recorded behavior: AppFabric will not create an ingestion for an (app, tenantId) pair unless that pair has an app authorization in the bundle that has completed the connect handshake; creating an ingestion for a pair with NO app authorization at all is refused with HTTP 400, error code `ValidationException` (the same dependency AppFabric enforces when an authorization exists but is not Connected). No ingestion is created, so a following ListIngestions returns an empty list.
Emulator: CreateIngestion succeeds (HTTP 201) and persists an Ingestion record for an app/tenant pair with no app authorization whatsoever. The handler validates only the parent app bundle's existence, the idempotency token and the per-bundle ingestion quota; it never consults the bundle's app authorizations.
Why: a client that relies on AWS refusing this gets a phantom ingestion that could never exist on real AWS. Tests of the real dependency-ordering contract (authorize → connect → ingest) pass against the emulator and fail against AWS, and the emulator's state diverges permanently from any AWS-reachable state.
Fix: the operation's handler validation — add the app-authorization dependency check on (app, tenantId) within the resolved bundle before the record is inserted, returning the modeled ValidationException.
Traces: appfabric.create-ingestion-without-app-authorization-rejected (services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_ingestion.rs, handler lines 174-324; the bundle lookup at lines 196-211 is the only parent check performed).

=== Cause 5 of 8: GetAppBundle's ResourceNotFoundException message echoes the identifier instead of AWS's fixed read-path message ===
Failing checks: appfabric.create-app-bundle-and-get-roundtrip, appfabric.create-app-bundle-with-customer-managed-key
AWS: Smithy model (com.amazonaws.appfabric#GetAppBundle) models ResourceNotFoundException (HTTP 404). Recorded behavior only, but the rule is pinned by two independent recordings of two different bundles: GetAppBundle on a deleted/unknown bundle returns a CONSTANT message that does NOT contain the identifier — "The app bundle ID was not found." The same recordings show AppFabric uses a DIFFERENT constant on the mutation path for the same dead bundle (CreateAppAuthorization → `ResourceNotFoundException` "The specified app bundle ID is not valid"), so the two paths are not interchangeable and neither interpolates the caller's identifier. Status (404) and code (`ResourceNotFoundException`) already match.
Emulator: returns the correct 404/`ResourceNotFoundException` but a single interpolated message for every path, of the form "App bundle <arn> was not found." (recorded examples: "App bundle arn:aws:appfabric:us-east-1:000000000000:appbundle/8c00abef-02a6-4a61-86c5-0caf9eab7bed was not found." and the same text for a second bundle id). Both failing checks diverge ONLY on this message.
Why: a client or test that matches AppFabric's documented not-found text, or that distinguishes the read-path refusal from the mutation-path refusal by its message, never matches; and the emulator additionally echoes a resource ARN that AWS deliberately does not reflect back.
Fix: this service's shared not-found error construction (`crate::state::app_bundle_not_found`) must emit the fixed, non-interpolated read-path text, with a separate constant for the mutation path.
Traces: appfabric.create-app-bundle-and-get-roundtrip and appfabric.create-app-bundle-with-customer-managed-key (both assert the GetAppBundle-after-delete message; services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs, `app_bundle_not_found`, as called from src/ops/create_ingestion.rs lines 198/208/258).

=== Cause 6 of 8: constraint-violation message for maxResults echoes the offending value, which AWS omits ===
Failing check: appfabric.list-app-authorizations-maxresults-out-of-range-rejected
AWS: Smithy model (com.amazonaws.appfabric#ListAppAuthorizations) binds `maxResults` as an @httpQuery integer targeting MaxResults with @range{min:1,max:100}; the operation models ValidationException (HTTP 400). The message here IS a requirement because it is built from the request — it names the member and the violated constraint. Recorded behavior: maxResults=101 returns HTTP 400 `ValidationException` with "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100" — the member path and the bound are present, the offending VALUE is not.
Emulator: same status and same code, but the formatter inserts the rejected value: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Why: a client that parses or asserts on the constraint-violation text (a common pattern for surfacing field errors) sees an extra quoted token in a fixed-format message; and for this member AWS does not reflect the submitted value back, so the emulator over-discloses relative to the contract.
Fix: the boundary range-constraint message formatter used by this service's deserialize→validate walk (the serializer/validation layer, not the handler) must omit the offending value for this member class.
Traces: appfabric.list-app-authorizations-maxresults-out-of-range-rejected.

=== Cause 7 of 8: CreateAppAuthorization does not refuse an app name outside AppFabric's supported-application set ===
Failing check: appfabric.create-app-authorization-invalid-app-name-rejected
AWS: Smithy model (com.amazonaws.appfabric#CreateAppAuthorization) types `app` as String255 with @length{min:1,max:255} and NO enum trait, so the supported-application set is NOT enforced by boundary validation — it is a server-side check, and the operation models ValidationException (HTTP 400) for it. Recorded behavior: app="NOT_A_REAL_SUPPORTED_APP" is refused with HTTP 400, error code `ValidationException`, message (example, recorded) "app NOT_A_REAL_SUPPORTED_APP is not supported"; the message is built from the request, so naming the rejected app value is the rule. Nothing is created, so a following ListAppAuthorizations on the bundle returns an empty list, while app="SLACK" with the same arguments succeeds.
Emulator: answers HTTP 501 with error code `NotImplemented` for BOTH the unsupported and the supported app name, because the operation is still a scaffold stub. Standing the operation up (covered by the auto-filed CreateAppAuthorization stub issue) is necessary but NOT sufficient: this cause is the supported-application validation the handler must additionally perform, which no stub issue describes.
Why: without it the emulator accepts authorizations for apps AppFabric does not support, so a client's app-name validation path is never exercised and a typo'd app silently creates a resource that cannot exist on AWS.
Fix: the operation's handler validation — reject an `app` outside the supported-application set with the modeled ValidationException, before any state is written.
Traces: appfabric.create-app-authorization-invalid-app-name-rejected (services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_app_authorization.rs).

=== Cause 8 of 8: CreateAppAuthorization does not enforce single-winner uniqueness per (bundle, app, tenant) under concurrency ===
Failing check: appfabric.concurrent-create-app-authorization-duplicate-tenant-single-winner
AWS: Smithy model (com.amazonaws.appfabric#CreateAppAuthorization) models ConflictException (HTTP 409) — "The request has created a conflict" — and ValidationException (HTTP 400). Recorded behavior: when several concurrent CreateAppAuthorization calls race for the SAME (appBundle, app, tenantIdentifier) triple, AT MOST ONE succeeds; every loser is refused with HTTP 409 `ConflictException` or HTTP 400 `ValidationException` (AppFabric's tie-break is not a single code), and the bundle afterwards holds exactly ONE authorization for that tenant, whose ARN is the winner's.
Emulator: zero racers succeed — all of them get HTTP 501 `NotImplemented`, so the suite observes no winner at all ("expected exactly one winner, got []"). As with cause 7, implementing the operation is not enough: the uniqueness/dedup invariant on (bundle, app, tenantIdentifier) must hold across concurrent requests, which no stub issue states.
Why: a client relying on AppFabric's dedup to make authorization creation safe to retry or to run from multiple workers would, once the operation is naively implemented without the invariant, get several duplicate authorizations for one tenant — an unreachable AWS state — instead of one winner and modeled refusals.
Fix: the operation's handler plus this service's state — a uniqueness check on (bundle, app, tenantIdentifier) taken under the same state lock as the insert, returning the modeled ConflictException to losers.
Traces: appfabric.concurrent-create-app-authorization-duplicate-tenant-single-winner (services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_app_authorization.rs).

## Evidence
Suite: simparity-behavioral-aws-v1. appfabric 1/24 passed (4%), 23 failed. 14 of the failures are marked ALREADY FILED / stub-blocked and are excluded here; the 9 below are this issue's grounding.

Cause 1 — appfabric.create-ingestion-destination-short-bucket-name-rejected [mismatch]
  expected: HTTP 400, code ValidationException (message example "The specified ingestion does not exist")
  actual:   HTTP 501, code NotImplemented
  "AssertionError: expected ValidationException, got 'NotImplemented'"

Cause 2 — appfabric.list-ingestion-destinations-reflects-created [mismatch]
  expected: HTTP 404, code ResourceNotFoundException (message example "The specified ingestion does not exist")
  actual:   HTTP 501, code NotImplemented
  "AssertionError: expected ResourceNotFoundException, got 'NotImplemented'"

Cause 3 — appfabric.stop-already-disabled-ingestion-rejected [mismatch]
  expected: HTTP 404, code ResourceNotFoundException (message example "The specified ingestion does not exist")
  actual:   HTTP 501, code NotImplemented
  "AssertionError: expected ResourceNotFoundException, got 'NotImplemented'"

Cause 4 — appfabric.create-ingestion-without-app-authorization-rejected [mismatch]
  expected: HTTP 400, code ValidationException; no ingestion persisted
  actual:   HTTP 201 success, ingestion persisted
  "AssertionError: expected a rejection, but CreateIngestion succeeded with no app authorization"

Cause 5 — appfabric.create-app-bundle-and-get-roundtrip [mismatch] and appfabric.create-app-bundle-with-customer-managed-key [mismatch]
  status/code agree (404 / ResourceNotFoundException); message diverges.
  expected (constant, no identifier): "The app bundle ID was not found."
  actual (identifier interpolated, examples):
    "App bundle arn:aws:appfabric:us-east-1:000000000000:appbundle/8c00abef-02a6-4a61-86c5-0caf9eab7bed was not found."
    "App bundle arn:aws:appfabric:us-east-1:000000000000:appbundle/5386dd81-ec1f-4f4a-a2c5-51867abc2d28 was not found."
  Two independent recordings of two different bundles yield the SAME expected constant, which is what pins the rule.

Cause 6 — appfabric.list-app-authorizations-maxresults-out-of-range-rejected [mismatch]
  status/code agree (400 / ValidationException); request-built message diverges.
  expected: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
  actual:   "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"

Cause 7 — appfabric.create-app-authorization-invalid-app-name-rejected [mismatch]
  expected: HTTP 400, code ValidationException for app='NOT_A_REAL_SUPPORTED_APP' (message built from request, example "app NOT_A_REAL_SUPPORTED_APP is not supported"); then app='SLACK' succeeds
  actual:   HTTP 501, code NotImplemented
  "AssertionError: expected ValidationException for app='NOT_A_REAL_SUPPORTED_APP', got 'NotImplemented'"

Cause 8 — appfabric.concurrent-create-app-authorization-duplicate-tenant-single-winner [mismatch]
  expected: exactly 1 of N concurrent racers succeeds; losers HTTP 409 ConflictException or HTTP 400 ValidationException; bundle then holds exactly one authorization for the tenant
  actual:   0 winners (every racer refused HTTP 501 NotImplemented)
  "AssertionError: expected exactly one winner, got []"

Source grounding (handler stubs, all returning Scaffolded::not_implemented):
  src/ops/create_ingestion_destination.rs:186
  src/ops/list_ingestion_destinations.rs:180
  src/ops/stop_ingestion.rs:164
Over-permissive handler: src/ops/create_ingestion.rs:174-324 (bundle-existence check at 196-211; no app-authorization check anywhere).

## Steps to Reproduce
All calls against the emulator with a client pinned to region us-east-1 (AppFabric is not served in every region), AppFabric enforces one app bundle per account/region so clear any existing bundle first.

Cause 1:
  arn = create_app_bundle(clientToken=<uuid>)['appBundle']['arn']
  create_ingestion_destination(appBundleIdentifier=arn, ingestionIdentifier=<random uuid>,
      processingConfiguration={'auditLog':{'format':'json','schema':'raw'}},
      destinationConfiguration={'auditLog':{'destination':{'s3Bucket':{'bucketName':'some-bucket'}}}})
  -> observe 501 NotImplemented; AWS gives 400 ValidationException.

Cause 2:
  arn = create_app_bundle(clientToken=<uuid>)['appBundle']['arn']
  list_ingestion_destinations(appBundleIdentifier=arn, ingestionIdentifier=<random uuid>)
  -> observe 501 NotImplemented; AWS gives 404 ResourceNotFoundException.

Cause 3:
  arn = create_app_bundle(clientToken=<uuid>)['appBundle']['arn']
  stop_ingestion(appBundleIdentifier=arn, ingestionIdentifier=<random uuid>)
  -> observe 501 NotImplemented; AWS gives 404 ResourceNotFoundException.

Cause 4:
  arn = create_app_bundle(clientToken=<uuid>)['appBundle']['arn']
  create_ingestion(appBundleIdentifier=arn, app='SLACK', tenantId=<fresh tenant id>, ingestionType='auditLog')
  -> observe 201 success and the ingestion present in list_ingestions; AWS refuses with 400 ValidationException and list_ingestions stays empty.

Cause 5:
  arn = create_app_bundle(clientToken=<uuid>)['appBundle']['arn']
  delete_app_bundle(appBundleIdentifier=arn)
  get_app_bundle(appBundleIdentifier=arn)
  -> read Error.Message: emulator interpolates the bundle ARN; AWS returns the constant "The app bundle ID was not found."
  (Repeat with a second bundle to see the identifier change while AWS's text does not.)

Cause 6:
  arn = create_app_bundle(clientToken=<uuid>)['appBundle']['arn']
  list_app_authorizations(appBundleIdentifier=arn, maxResults=101)
  -> read Error.Message: emulator includes "Value '101' at 'maxResults'"; AWS omits the value ("Value at 'maxResults'").
  (maxResults=0 cannot be used: botocore refuses it client-side from the model's min=1, so the request never reaches the wire.)

Cause 7:
  arn = create_app_bundle(clientToken=<uuid>)['appBundle']['arn']
  create_app_authorization(appBundleIdentifier=arn, app='NOT_A_REAL_SUPPORTED_APP', authType='oauth2',
      credential={'oauth2Credential':{'clientId':'placeholder-client-id','clientSecret':'placeholder-client-secret'}},
      tenant={'tenantIdentifier':<fresh>,'tenantDisplayName':'t'})
  -> observe 501 NotImplemented; AWS gives 400 ValidationException naming the app. Then repeat with app='SLACK' and confirm it succeeds.

Cause 8:
  arn = create_app_bundle(clientToken=<uuid>)['appBundle']['arn']
  From a thread pool, fire N concurrent create_app_authorization calls with IDENTICAL
  appBundleIdentifier, app='SLACK' and tenant.tenantIdentifier.
  -> count successes: emulator yields 0 (all 501). AWS yields exactly 1, with every loser refused
     409 ConflictException or 400 ValidationException, and list_app_authorizations then showing a
     single authorization for that tenant whose ARN is the winner's.
