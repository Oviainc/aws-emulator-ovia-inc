Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.recommender-schema-lifecycle
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles.recommender-schema-lifecycle|run#11|AssertionError|assert message == _NO_DATA_TABLE, f"expected the missing-backing-data-table message, got {message!r}"
Issue-Commit: 5710a44b9d7c49e8e76b586d9d6c4a9e7e3ce0f5

# customer-profiles CreateRecommenderSchema emits an emulator-authored data-table refusal instead of AWS's fixed missing-data-table sentence (1 cause)

> _Note: this issue was identified against the code base at commit `5710a44b9d7c49e8e76b586d9d6c4a9e7e3ce0f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRecommenderSchema

## Description
REOPENS 20261029T183204Z-customer-profiles-createrecommenderschema-stub.md: its fix left the call refused. The refusal CLASS is correct — real AWS also refuses this request, with the same error code and the same HTTP status — so this is not a new defect in the control flow; what the fix left behind is a hand-written English sentence where AWS returns a fixed, documented one.

Cause 1 of 1: CreateRecommenderSchema's domain-readiness (missing backing data table) refusal message is composed by the emulator rather than being AWS's fixed sentence.

AWS: Smithy: CreateRecommenderSchema (POST /domains/{DomainName}/recommender-schemas/{RecommenderSchemaName}, success 200) models BadRequestException with httpError 400; CreateRecommenderSchemaRequest$Fields is a required map (@length 1..2) documented as "A map of dataset type to column definitions that specifies which data columns to include in the schema. The _webAnalytics and _catalogItem keys are supported." The model does not state the readiness message, so for the message itself this is recorded behavior only. The recording shows: HTTP 400, error code BadRequestException, and Message exactly

  "Unable to create recommender: Required data table for _webAnalytics is missing. Before creating a recommender, please ensure you have: 1. Ingested user interaction data (such as clicks, purchases, or views) and 2. Uploaded your product/item catalog data. The system will automatically create necessary tables when you import this data. Refer documentation for details: https://docs.aws.amazon.com/connect/latest/adminguide/predictive-insights-get-started.html"

That message is ENTIRELY fixed text — it has NO variable parts. In particular the token `_webAnalytics` is literal: the recording shows AWS naming `_webAnalytics` for a request whose only Fields key was `_catalogItem`, i.e. the sentence is a domain-level readiness statement and does not interpolate the dataset-type key from the request. Nothing from the request (DomainName, RecommenderSchemaName, the Fields keys) and nothing from emulator state appears in it.

Emulator: HTTP 400 and BadRequestException (both correct), but Message is 'DataTable _catalogItem not found' — a short emulator-authored sentence that additionally interpolates the request's Fields key, which AWS never does. Source: services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/create_recommender_schema.rs line 214 returns `bad_request(crate::state::data_table_not_found(absent))`, where `absent` is the per-request dataset-type key found by the `holds_data_table` scan at lines 208-215.

Why: this is the FIRST refusal any client hits when it tries to define a recommender schema on a domain that has not ingested Predictive Insights source data — the normal state of a freshly created domain. AWS's sentence is the operator-facing remediation text (ingest interaction data, upload the catalog, tables are created automatically, plus the doc link); the emulator's sentence carries none of it. A client, console surface, or test harness that matches, logs, or displays AWS's sentence sees a string it cannot match and cannot surface remediation, and a caller that keys off the dataset-type token in the message will key off a token AWS does not put there (AWS always says `_webAnalytics`, the emulator echoes whichever key was sent). Because the dataset type is interpolated, the emulator's body is also request-dependent where AWS's is constant, so success/failure body comparisons are non-deterministic across requests.

Fix: the data-table readiness refusal behind CreateRecommenderSchema — the shared message builder `crate::state::data_table_not_found` as used by the create_recommender_schema handler's readiness gate. It must emit the fixed sentence verbatim, with no interpolation of the dataset-type key or any other request/state value; the surrounding gate (400 + BadRequestException, nothing persisted) is already right.

Note on neighbouring filings: open issue 20261103T055557Z-customer-profiles-resource-not-found-and-deleteprofile-success.md and open issue 20261102T122410Z-customer-profiles-10-net-new-behavioral-causes-refusal.md both record other emulator-authored customer-profiles messages, and neither names CreateRecommenderSchema or the recommender data-table readiness branch, so neither covers this cause.

Traces: behavioral check customer-profiles.recommender-schema-lifecycle (verdict mismatch).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false; customer-profiles 17/45 passed).

Failing check customer-profiles.recommender-schema-lifecycle, verdict=mismatch, at check_customer_profiles_recommender_schema_lifecycle.py line 63:

  assert message == _NO_DATA_TABLE, f"expected the missing-backing-data-table message, got {message!r}"
  AssertionError: expected the missing-backing-data-table message, got 'DataTable _catalogItem not found'

The preceding assertion on the error CODE passed: the emulator answered BadRequestException (HTTP 400), which matches AWS. The only divergence is the Message member of the error body:

  expected (AWS, fixed text, no variable parts):
    'Unable to create recommender: Required data table for _webAnalytics is missing. Before creating a recommender, please ensure you have: 1. Ingested user interaction data (such as clicks, purchases, or views) and 2. Uploaded your product/item catalog data. The system will automatically create necessary tables when you import this data. Refer documentation for details: https://docs.aws.amazon.com/connect/latest/adminguide/predictive-insights-get-started.html'
  actual (emulator):
    'DataTable _catalogItem not found'

The check documents the invariant in its own source comment: "The domain-level readiness refusal names `_webAnalytics` whichever dataset-type key the request carries." The request under test carried Fields={'_catalogItem': [...]}, and the emulator echoed `_catalogItem` back; AWS still says `_webAnalytics`. So `_webAnalytics` is fixed text in the template, not a value taken from the request.

Emulator code path: services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/create_recommender_schema.rs lines 204-215 (sorted scan for a dataset type the domain holds no data table for) -> line 214 `return Err(bad_request(crate::state::data_table_not_found(absent)));`.

The check aborted at this first assertion, so its later assertions (GetRecommenderSchema answering ResourceNotFoundException 'RecommenderSchema <name> is not found' for the refused schema, and ListRecommenderSchemas omitting it) were never exercised and are NOT claimed as divergences here. The domain name and schema name in the run are per-run generated values and are not part of any requirement.

## Steps to Reproduce
1. Create a KMS key and take its ARN.
2. CreateDomain(DomainName=<fresh name>, DefaultExpirationDays=30, DefaultEncryptionKey=<key ARN>, DataStore={'Enabled': True}) — a brand-new domain with no Predictive Insights source data ingested, so no backing data table exists for any dataset type.
3. CreateRecommenderSchema(DomainName=<that domain>, RecommenderSchemaName=<fresh name>, Fields={'_catalogItem': [{'TargetFieldName': 'Id', 'ContentType': 'STRING'}, {'TargetFieldName': 'Name', 'ContentType': 'STRING'}]}).
4. Expect: HTTP 400, error code BadRequestException, Message exactly the fixed sentence 'Unable to create recommender: Required data table for _webAnalytics is missing. Before creating a recommender, please ensure you have: 1. Ingested user interaction data (such as clicks, purchases, or views) and 2. Uploaded your product/item catalog data. The system will automatically create necessary tables when you import this data. Refer documentation for details: https://docs.aws.amazon.com/connect/latest/adminguide/predictive-insights-get-started.html' — with NO substitution of the dataset-type key sent in Fields.
5. Observe: HTTP 400 and BadRequestException are correct, but Message is 'DataTable _catalogItem not found'.
6. Repeat step 3 with Fields={'_webAnalytics': [...]} instead: AWS returns the identical fixed sentence; the emulator returns 'DataTable _webAnalytics not found', confirming the dataset-type key is being interpolated where AWS emits constant text.

Re-replay behavioral check customer-profiles.recommender-schema-lifecycle to confirm the fix; it also goes on to assert GetRecommenderSchema/ListRecommenderSchemas behaviour for the refused schema, which this check never reached.
