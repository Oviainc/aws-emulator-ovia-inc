Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.deep-dynamodb-1-ddbvec-hash-schema-search-without-condition-rejected dynamodb.deep-dynamodb-1-ddbvec-include-projection-hides-unprojected-attributes dynamodb.deep-dynamodb-1-ddbvec-inline-filter-range-comparator-rejected dynamodb.deep-dynamodb-1-ddbvec-search-condition-filters-below-topk-with-projection dynamodb.contributor-insights-enable-describe-list
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateTable|ValidationException
Issue-Key: dynamodb.contributor-insights-enable-describe-list|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 8872a34b239da6f35a39fa56ec21084188bccf39

# dynamodb: 2 net-new root causes — CreateTable refuses attribute definitions consumed by a VectorIndexes SearchSchema, and a Contributor Insights enable never converges for Describe/ListContributorInsights

> _Note: this issue was identified against the code base at commit `8872a34b239da6f35a39fa56ec21084188bccf39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
This issue bundles 2 distinct net-new root causes in the dynamodb service. A fixer must close BOTH before submitting.

=== Cause 1 of 2: CreateTable's attribute-definition usage check does not count attributes consumed by a VectorIndexes SearchSchema (or VectorAttribute), so a well-formed vector-index CreateTable is refused ValidationException ===

AWS: Smithy model, CreateTable.AttributeDefinitions — "An array of attributes that describe the key schema for the table and indexes." CreateTable.VectorIndexes (VectorIndexList) is a modeled input member whose element shape com.amazonaws.dynamodb#VectorIndex carries a required VectorAttribute (VectorAttributeDefinition) and a SearchSchema (list of com.amazonaws.dynamodb#SearchSchemaElement, @length min 1), each element being {AttributeName (required), SearchSchemaElementType (required, enum HASH | INLINE_FILTER)}. The SearchSchemaElementType docs state HASH is "A partition key that partitions the vector index for independent scaling. When specified, you must provide this attribute's value in the SearchConditionExpression" and INLINE_FILTER is "An attribute projected into the vector index for filtering at the storage layer during search." An attribute named by a vector index's SearchSchema therefore has to be declared in AttributeDefinitions (its type is needed), and AWS accepts such a CreateTable: in all four recordings the request is accepted and the check proceeds to DescribeTable, PutItem and SearchVectors. AWS's "Some AttributeDefinitions are not used in any KeySchema: <names>" ValidationException exists only for an attribute definition that NO key schema and no index consumes; a vector-index search-schema attribute is consumed, so the refusal must not fire. Recorded behavior only for the acceptance itself (the model does not spell out the usage rule).

Emulator: CreateTable answers HTTP 400 ValidationException with message "Some AttributeDefinitions are not used in any KeySchema: <attributeName>" for every one of these requests. Observed verbatim across the four checks, each naming the attribute its vector index's SearchSchema declares:
  - "Some AttributeDefinitions are not used in any KeySchema: tenant" (SearchSchema [{tenant, HASH}])
  - "Some AttributeDefinitions are not used in any KeySchema: tier" (SearchSchema element on `tier`)
  - "Some AttributeDefinitions are not used in any KeySchema: price" (SearchSchema [{price, INLINE_FILTER}])
  - "Some AttributeDefinitions are not used in any KeySchema: cat" (SearchSchema element on `cat`)
Message anatomy: the fixed text is "Some AttributeDefinitions are not used in any KeySchema: "; the trailing part is a value taken from the request (the AttributeDefinitions entry the check declared for its vector index). The fixed text itself is AWS's wording for the genuine orphan-definition case — the defect is that the check fires at all here, i.e. the emulator refuses a request AWS accepts. The refusal also pre-empts the slice's own VectorIndexes handling: the request never reaches the point where the vector index would be stored or reported, so the error class the caller sees is ValidationException rather than anything about vector support.

Why: a client that creates a vector-search table exactly as the model allows — AttributeDefinitions declaring the search-schema attributes, KeySchema declaring only the table key — cannot create the table at all. CreateTable fails at step one with a validation error that blames the client's own, correct, request, so every downstream behavior (HASH-partitioned search isolation, INLINE_FILTER equality vs. range comparators, INCLUDE projection on a vector index, SearchConditionExpression narrowing below TopK) is unreachable and the client has no way to work around it: dropping the AttributeDefinition would make the vector index's attribute untyped.

Fix: CreateTable's input validation — the attribute-definition usage check in the dynamodb CreateTable handler. It must treat an AttributeDefinitions entry as USED when its name appears in the table KeySchema, in any LocalSecondaryIndexes/GlobalSecondaryIndexes KeySchema, OR in any VectorIndexes[].SearchSchema[].AttributeName / VectorIndexes[].VectorAttribute.AttributeName, and only raise "Some AttributeDefinitions are not used in any KeySchema: <name>" for a definition no schema consumes.

Traces: dynamodb.deep-dynamodb-1-ddbvec-hash-schema-search-without-condition-rejected, dynamodb.deep-dynamodb-1-ddbvec-include-projection-hides-unprojected-attributes, dynamodb.deep-dynamodb-1-ddbvec-inline-filter-range-comparator-rejected, dynamodb.deep-dynamodb-1-ddbvec-search-condition-filters-below-topk-with-projection. The validation site is services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/create_table.rs lines 807-828, which builds its used-attribute set from the table KeySchema plus GlobalSecondaryIndexes/LocalSecondaryIndexes key schemas only; VectorIndexes is handled separately at lines 593-600 of the same file, after this check has already returned the error.

=== Cause 2 of 2: enabling Contributor Insights never converges to a state the DescribeContributorInsights / ListContributorInsights pair reports, so the enable→describe→list→disable lifecycle never completes ===

AWS: Smithy model — UpdateContributorInsights{TableName (required), IndexName, ContributorInsightsAction (required), ContributorInsightsMode} returns ContributorInsightsStatus (enum com.amazonaws.dynamodb#ContributorInsightsStatus = ENABLING | ENABLED | DISABLING | DISABLED | FAILED). DescribeContributorInsights returns the same ContributorInsightsStatus plus ContributorInsightsRuleList and LastUpdateDateTime; ListContributorInsights returns ContributorInsightsSummaries, each a com.amazonaws.dynamodb#ContributorInsightsSummary{TableName, IndexName, ContributorInsightsStatus, ContributorInsightsMode}. The modeled lifecycle is therefore: a fresh table describes DISABLED; ENABLE answers ENABLING (or ENABLED) and settles to ENABLED on both DescribeContributorInsights and the table's ListContributorInsights summary; DISABLE answers DISABLING (or DISABLED), settles to DISABLED on Describe, and the table drops out of ListContributorInsights.

Emulator: the lifecycle never completes — the check is killed by the harness with "Check timed out after 60.0s" while polling for convergence, so no terminal state is ever observed. Grounded facts from this run: the emulator does answer UpdateContributorInsights and DescribeContributorInsights (the cloudwatch__dynamodb twin checks get ENABLING echoed back from UpdateContributorInsights with ContributorInsightsMode echoed, and reach ENABLED on DescribeContributorInsights), so this is not a 501 refusal. Hypothesis: the stage that does not converge is the ListContributorInsights projection of the status — the status is tracked where Describe reads it but the table's ContributorInsightsSummaries never reports ENABLED (and/or never drops out again after DISABLE), so the poll spins until the cap. Related but NOT covering issues already open: 20261025T020208Z-dynamodb-updatecontributorinsights-stub.md and 20261025T024002Z-dynamodb-describecontributorinsights-stub.md describe those two operations as unimplemented scaffold stubs; this failure is not a stub refusal (the operations answer) and neither issue mentions ListContributorInsights or the status-convergence lifecycle.

Why: a client that enables Contributor Insights and then polls for readiness — the documented way to know the feature is live, and what the CLI/console do — never sees ENABLED through ListContributorInsights and loops forever. A client that enumerates which of its tables have Contributor Insights on cannot discover any of them, and after DISABLE cannot tell that it is off.

Fix: the dynamodb Contributor Insights state (UpdateContributorInsights handler and the service state it writes) plus the ListContributorInsights handler, so one store drives all three operations: UpdateContributorInsights records action+mode and the ENABLING→ENABLED / DISABLING→DISABLED transition, DescribeContributorInsights reports it, and ListContributorInsights emits a ContributorInsightsSummary for the table while enabled and omits it once DISABLED.

Traces: dynamodb.contributor-insights-enable-describe-list.

## Evidence
Cause 1 — four CreateTable calls AWS accepts are refused 400 ValidationException.
  Expected (AWS): CreateTable succeeds (TableDescription, TableStatus CREATING), e.g. for
    KeySchema=[{pk, HASH}],
    AttributeDefinitions=[{pk, S}, {tenant, S}],
    VectorIndexes=[{IndexName: idx-tenant, VectorAttribute: {AttributeName: emb}, Dimensions: 4, DistanceFunction: EUCLIDEAN, Projection: {ProjectionType: ALL}, SearchSchema: [{AttributeName: tenant, SearchSchemaElementType: HASH}]}]
  Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the CreateTable operation: Some AttributeDefinitions are not used in any KeySchema: tenant
  Same refusal, with the attribute name taken from the request, for `tier`, `price` and `cat`:
    ... (ValidationException) when calling the CreateTable operation: Some AttributeDefinitions are not used in any KeySchema: tier
    ... (ValidationException) when calling the CreateTable operation: Some AttributeDefinitions are not used in any KeySchema: price
    ... (ValidationException) when calling the CreateTable operation: Some AttributeDefinitions are not used in any KeySchema: cat
  In each of the four checks the attribute named is exactly the one the request's VectorIndexes[].SearchSchema declares, and it is NOT in any KeySchema — which is legal, because a vector index consumes it.

Cause 2 — Contributor Insights lifecycle does not settle.
  Expected (AWS): describe DISABLED -> UpdateContributorInsights(ENABLE) answers ENABLING/ENABLED -> DescribeContributorInsights settles ENABLED -> ListContributorInsights(TableName=t) reports a summary with ContributorInsightsStatus ENABLED -> UpdateContributorInsights(DISABLE) answers DISABLING/DISABLED -> Describe settles DISABLED -> the table is absent from ListContributorInsights.
  Actual (emulator): "Error: Check timed out after 60.0s" — the lifecycle never reaches a terminal state within the harness cap; no ENABLED/absent observation is ever made.
  Corroboration that the operations themselves answer (so this is convergence, not a stub): the cloudwatch__dynamodb Contributor Insights checks get ContributorInsightsStatus == "ENABLING" back from UpdateContributorInsights with ContributorInsightsMode echoed, and their DescribeContributorInsights poll reaches "ENABLED" before failing on a later assertion.

## Steps to Reproduce
Cause 1:
1. dynamodb CreateTable with TableName=<new>, BillingMode=PAY_PER_REQUEST, KeySchema=[{AttributeName: pk, KeyType: HASH}], AttributeDefinitions=[{pk, S}, {tenant, S}], and VectorIndexes=[{IndexName: idx-tenant, VectorAttribute: {AttributeName: emb}, Dimensions: 4, DistanceFunction: EUCLIDEAN, Projection: {ProjectionType: ALL}, SearchSchema: [{AttributeName: tenant, SearchSchemaElementType: HASH}]}].
2. Observe HTTP 400 ValidationException "Some AttributeDefinitions are not used in any KeySchema: tenant" instead of a TableDescription.
3. Repeat with the SearchSchema element typed INLINE_FILTER on a numeric attribute (AttributeDefinitions [{pk,S},{price,N}], SearchSchema [{price, INLINE_FILTER}]) — same refusal naming `price`.
4. Re-replay: dynamodb.deep-dynamodb-1-ddbvec-hash-schema-search-without-condition-rejected, dynamodb.deep-dynamodb-1-ddbvec-include-projection-hides-unprojected-attributes, dynamodb.deep-dynamodb-1-ddbvec-inline-filter-range-comparator-rejected, dynamodb.deep-dynamodb-1-ddbvec-search-condition-filters-below-topk-with-projection.

Cause 2:
1. CreateTable a hash-only PAY_PER_REQUEST table and wait for TableStatus ACTIVE.
2. DescribeContributorInsights(TableName=t) — expect ContributorInsightsStatus DISABLED.
3. UpdateContributorInsights(TableName=t, ContributorInsightsAction=ENABLE) — expect ENABLING or ENABLED.
4. Poll DescribeContributorInsights(TableName=t) for ENABLED, then poll ListContributorInsights(TableName=t) for a ContributorInsightsSummaries entry whose TableName is t with ContributorInsightsStatus ENABLED. Observe the poll never satisfies and the check is cut off at the 60 s cap.
5. UpdateContributorInsights(TableName=t, ContributorInsightsAction=DISABLE); poll DescribeContributorInsights for DISABLED and ListContributorInsights for the table's absence.
6. Re-replay: dynamodb.contributor-insights-enable-describe-list.
