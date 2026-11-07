Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.consumed-capacity-indexes-breakdown-sums-to-total-over-lsi-fanout dynamodb.ddb-eventual-and-strong-read-are-two-different-reads dynamodb.ddbgsi-base-item-delete-removes-the-index-entry-while-a-sibling-survives dynamodb.ddbgsi-index-entries-do-not-cross-into-a-derived-second-region dynamodb.ddbgsi-index-gains-no-entry-when-a-conditional-write-is-rejected dynamodb.ddbgsi-projected-value-converges-in-the-index-after-an-update dynamodb.ddbgsi-query-filter-on-a-non-projected-attribute-rejected-while-scan-matches-nothing dynamodb.gsi-added-to-a-populated-table-backfills-every-preexisting-item dynamodb.gsi-query-omits-attributes-the-projection-excludes dynamodb.gsi-query-paginates-with-last-evaluated-key dynamodb.include-projection-governs-index-reads-on-lsi-and-gsi dynamodb.lsi-projection-include-echoes-non-key-attributes dynamodb.put-item-omitting-a-gsi-key-is-a-sparse-write dynamodb.query-key-condition-expression-third-predicate-rejected dynamodb.ddb-sfa-cli-attribute-exists-scan-derives-the-status-tally dynamodb.ddb-ss-cli-scan-pages-merge-into-the-solve-answer dynamodb.ddb-ss-filter-naming-an-undefined-expression-value-rejected dynamodb.concurrent-table-replica-auto-scaling-race-no-corruption dynamodb.create-global-table-already-exists-rejected dynamodb.global-table-settings-describe-update-roundtrip dynamodb.replica-auto-scaling-describe-update dynamodb.table-replica-auto-scaling-update-reflected dynamodb.ddbglobal-replica-delete-for-absent-replica-rejected dynamodb.ddbglobal-replica-updates-local-region-rejected dynamodb.replica-autoscaling-policy-applied dynamodb.sse-with-a-kms-alias-reports-the-resolved-key-arn dynamodb.update-table-sets-and-clears-the-on-demand-read-ceiling dynamodb.disable-kinesis-streaming-destination-not-enabled-rejected dynamodb.global-table-not-found dynamodb.update-global-table-settings-index-not-found-rejected dynamodb.projection-overlapping-document-paths-rejected dynamodb.select-all-projected-attributes-requires-an-index-rejected dynamodb.deploy-cfn-table-duplicate-name-create-rejected dynamodb.e2e-ddb-gsi-scan-and-fetch-record-lifecycle dynamodb.e2e-ddb-rbp-resource-policy-tables-lifecycle dynamodb.e2e-ddb-ss-scan-dynamodb-table-by-status-lifecycle dynamodb.e2e-ddb-wf-query-workflow-status-analysis-lifecycle
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|Query|NotImplemented
Issue-Key: dynamodb.ddb-sfa-cli-attribute-exists-scan-derives-the-status-tally|run#5|AssertionError|assert proc.returncode == 0, f"`aws dynamodb scan` exited {proc.returncode}: {proc.stderr[-400:]!r}"
Issue-Key: dynamodb.ddb-ss-cli-scan-pages-merge-into-the-solve-answer|cli#9|AssertionError|assert proc.returncode == 0, (
Issue-Key: dynamodb.ddb-ss-filter-naming-an-undefined-expression-value-rejected|run#4|AssertionError|assert code == "ValidationException", f"the request raised {code!r} ({message!r}), expected 'ValidationException'"
Issue-Key: dynamodb|UpdateTable|NotImplemented
Issue-Key: dynamodb.ddbglobal-replica-delete-for-absent-replica-rejected|run#15|AssertionError|assert absent_code == ERR_VALIDATION, (
Issue-Key: dynamodb.ddbglobal-replica-updates-local-region-rejected|run#11|AssertionError|assert code == ERR_VALIDATION, (
Issue-Key: dynamodb.replica-autoscaling-policy-applied|run#19|AssertionError|assert create_result == 'created', create_result
Issue-Key: dynamodb.disable-kinesis-streaming-destination-not-enabled-rejected|run#12|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: dynamodb.global-table-not-found|run#8|AssertionError|assert create_code == 'ValidationException', (
Issue-Key: dynamodb.update-global-table-settings-index-not-found-rejected|run#8|AssertionError|assert code == "GlobalTableNotFoundException", f"expected GlobalTableNotFoundException, got {code}"
Issue-Key: dynamodb.projection-overlapping-document-paths-rejected|run#6|AssertionError|assert code == ERR_VALIDATION, f"'doc, doc' must be rejected with {ERR_VALIDATION}; got {code}"
Issue-Key: dynamodb.select-all-projected-attributes-requires-an-index-rejected|run#10|AssertionError|assert code == ERR_VALIDATION, (
Issue-Key: dynamodb|DescribeTable|ResourceNotFoundException
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# dynamodb: 10 net-new root causes — Query/Scan index reads and four UpdateTable branches answer 501 NotImplemented, and CFN-deployed tables never exist

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
Ten distinct root causes in the DynamoDB emulator, all grounded in failing behavioral checks. Each cause below must be closed before this issue is submitted.

=== Cause 1 of 10: Query with IndexName answers 501 NotImplemented ===
AWS: Smithy: QueryInput.IndexName is an OPTIONAL member (target IndexName, @length 3-255, @pattern ^[a-zA-Z0-9_.-]+$). Query's modeled errors are InternalServerError (500), InvalidEndpointException (421), ProvisionedThroughputExceededException (400), RequestLimitExceeded (400), ResourceNotFoundException (400) and ThrottlingException (400) — there is no NotImplemented error and no 501 anywhere in the operation's error set. AWS serves a Query against a GSI or LSI and returns Items/Count/ScannedCount/LastEvaluatedKey/ConsumedCapacity (QueryOutput).
Emulator: every Query that carries IndexName is refused with HTTP 501, error code NotImplemented, example message 'operation Query with IndexName is scaffolded but not implemented in this emulator'.
Why: no client can read a secondary index at all. Index projection, index pagination, sparse-index behavior, GSI backfill and index/base divergence are all unreachable, and a client that models an unmodeled 501/NotImplemented has no retry or error-handling path for it.
Fix: the Query handler's IndexName branch and the secondary-index read path in the table state (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/query.rs plus the index read path in the service's state module).
Traces: dynamodb.consumed-capacity-indexes-breakdown-sums-to-total-over-lsi-fanout, dynamodb.ddb-eventual-and-strong-read-are-two-different-reads, dynamodb.ddbgsi-base-item-delete-removes-the-index-entry-while-a-sibling-survives, dynamodb.ddbgsi-index-entries-do-not-cross-into-a-derived-second-region, dynamodb.ddbgsi-index-gains-no-entry-when-a-conditional-write-is-rejected, dynamodb.ddbgsi-projected-value-converges-in-the-index-after-an-update, dynamodb.ddbgsi-query-filter-on-a-non-projected-attribute-rejected-while-scan-matches-nothing, dynamodb.gsi-added-to-a-populated-table-backfills-every-preexisting-item, dynamodb.gsi-query-omits-attributes-the-projection-excludes, dynamodb.gsi-query-paginates-with-last-evaluated-key, dynamodb.include-projection-governs-index-reads-on-lsi-and-gsi, dynamodb.lsi-projection-include-echoes-non-key-attributes, dynamodb.put-item-omitting-a-gsi-key-is-a-sparse-write, dynamodb.query-key-condition-expression-third-predicate-rejected.

=== Cause 2 of 10: Scan's IndexName / ProjectionExpression / ScanFilter branches answer 501 NotImplemented ===
AWS: Smithy: ScanInput models IndexName, ProjectionExpression, FilterExpression and the legacy ScanFilter as optional members, and Scan's modeled error set contains no NotImplemented and no 501. A Scan naming an index, or projecting attributes, is served; a Scan whose FilterExpression names an undefined ExpressionAttributeValue is refused with ValidationException (400).
Emulator: Scan refuses these inputs with HTTP 501 / NotImplemented, example messages 'operation Scan with ProjectionExpression is scaffolded but not implemented in this emulator' and 'operation Scan with FilterExpression is scaffolded but not implemented in this emulator'. Where AWS answers ValidationException the emulator answers 501/NotImplemented instead (dynamodb.ddb-ss-filter-naming-an-undefined-expression-value-rejected observed "the request raised 'NotImplemented' ..., expected 'ValidationException'"). The AWS CLI surfaces this as exit code 254.
Why: no Scan can project or filter, and a validation refusal that AWS classifies as a client error (400 ValidationException) is reported as an unmodeled server-side 501, which SDK retry policies treat differently.
Fix: the Scan handler's ProjectionExpression/FilterExpression/IndexName/ScanFilter branches and Scan's input validation, which must run before the not-implemented check so a malformed request is still refused with ValidationException (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/scan.rs). The open issue 20261025T002357Z-dynamodb-events-lambda-dynamodb-scan-with-a-filterexpression-is-an-unimplemented.md covers the plain FilterExpression stub for a different service token and does not carry these replay keys.
Traces: dynamodb.ddb-sfa-cli-attribute-exists-scan-derives-the-status-tally, dynamodb.ddb-ss-cli-scan-pages-merge-into-the-solve-answer, dynamodb.ddb-ss-filter-naming-an-undefined-expression-value-rejected.

=== Cause 3 of 10: UpdateTable with ReplicaUpdates answers 501 NotImplemented ===
AWS: Smithy: UpdateTableInput.ReplicaUpdates is an optional list (target ReplicationGroupUpdateList, @length min 1); UpdateTable's modeled errors are InternalServerError (500), InvalidEndpointException (421), LimitExceededException (400), ResourceInUseException (400) and ResourceNotFoundException (400), plus DynamoDB's common ValidationException (400). AWS applies a replica create/delete and returns TableDescription; a ReplicaUpdates Delete naming an absent replica, or a Create naming the table's own region, is refused with ValidationException (400).
Emulator: every UpdateTable carrying ReplicaUpdates answers HTTP 501 with code NotImplemented and the example message 'operation UpdateTable with ReplicaUpdates is scaffolded but not implemented in this emulator' — both where AWS succeeds and where AWS refuses with ValidationException.
Why: global tables, replica auto scaling and global-table settings are entirely unavailable, and the negative cases report an unmodeled 501 instead of the modeled 400 client error.
Fix: UpdateTable's ReplicaUpdates branch and the replica/global-table state it needs (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/update_table.rs), with ReplicaUpdates input validation ahead of it.
Traces: dynamodb.concurrent-table-replica-auto-scaling-race-no-corruption, dynamodb.create-global-table-already-exists-rejected, dynamodb.global-table-settings-describe-update-roundtrip, dynamodb.replica-auto-scaling-describe-update, dynamodb.table-replica-auto-scaling-update-reflected, dynamodb.ddbglobal-replica-delete-for-absent-replica-rejected, dynamodb.ddbglobal-replica-updates-local-region-rejected, dynamodb.replica-autoscaling-policy-applied.

=== Cause 4 of 10: UpdateTable with SSESpecification answers 501 NotImplemented ===
AWS: Smithy: UpdateTableInput.SSESpecification is an optional member; UpdateTable models no NotImplemented error. AWS accepts an SSESpecification naming a KMS alias and DescribeTable then reports SSEDescription with the RESOLVED key ARN.
Emulator: HTTP 501 / NotImplemented, example message 'operation UpdateTable with SSESpecification is scaffolded but not implemented in this emulator'.
Why: a table's encryption cannot be changed, and the alias-to-key-ARN resolution a client reads back from DescribeTable never exists.
Fix: UpdateTable's SSESpecification branch and the table's SSE state (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/update_table.rs). Related but distinct from the open CreateTable-side issue 20261024T102105Z-dynamodb-kms-dynamodb-createtable-with-an-ssespecification-is-an-unimplemented.md, which is the CreateTable branch.
Traces: dynamodb.sse-with-a-kms-alias-reports-the-resolved-key-arn.

=== Cause 5 of 10: UpdateTable with OnDemandThroughput answers 501 NotImplemented ===
AWS: Smithy: UpdateTableInput.OnDemandThroughput is an optional member (target OnDemandThroughput); no NotImplemented error is modeled. AWS sets and clears MaxReadRequestUnits/MaxWriteRequestUnits on an on-demand table and reflects them in TableDescription.
Emulator: HTTP 501 / NotImplemented, example message 'operation UpdateTable with OnDemandThroughput is scaffolded but not implemented in this emulator'.
Why: the on-demand read/write ceiling cannot be set or cleared.
Fix: UpdateTable's OnDemandThroughput branch and the table throughput state (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/update_table.rs).
Traces: dynamodb.update-table-sets-and-clears-the-on-demand-read-ceiling.

=== Cause 6 of 10: DisableKinesisStreamingDestination answers 501 instead of ValidationException ===
AWS: DisableKinesisStreamingDestination on a table that never had a Kinesis streaming destination enabled is refused with ValidationException, HTTP 400.
Emulator: HTTP 501 with code NotImplemented (the check observed 'expected ValidationException, got NotImplemented').
Why: the negative path of the Kinesis streaming surface reports an unmodeled server error rather than the modeled client error, so a client cannot distinguish 'not enabled' from an outage.
Fix: the DisableKinesisStreamingDestination handler and its input validation (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/disable_kinesis_streaming_destination.rs); the refusal must be produced before the scaffold check. The filed EnableKinesisStreamingDestination stub issue covers the enable side only.
Traces: dynamodb.disable-kinesis-streaming-destination-not-enabled-rejected.

=== Cause 7 of 10: the legacy (2017) global-table APIs answer 501 instead of the withdrawal ValidationException ===
AWS: DescribeGlobalTable/CreateGlobalTable for the withdrawn 2017.11.29 Global Tables version answer ValidationException, HTTP 400 — the version is no longer offered, so the call is a client error, not an unimplemented feature.
Emulator: HTTP 501 / NotImplemented (observed 'expected ValidationException (legacy Global Tables version withdrawn), got NotImplemented').
Why: a client probing which global-table version a region offers reads a server failure instead of the documented refusal.
Fix: the DescribeGlobalTable handler's validation (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/describe_global_table.rs).
Traces: dynamodb.global-table-not-found.

=== Cause 8 of 10: UpdateGlobalTableSettings answers 501 instead of GlobalTableNotFoundException ===
AWS: UpdateGlobalTableSettings naming an index that does not exist on the global table is refused with GlobalTableNotFoundException, HTTP 400 (a modeled error of the operation).
Emulator: HTTP 501 / NotImplemented (observed 'expected GlobalTableNotFoundException, got NotImplemented').
Why: the modeled error a client catches by name is never raised.
Fix: the UpdateGlobalTableSettings handler and its validation (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/update_global_table_settings.rs).
Traces: dynamodb.update-global-table-settings-index-not-found-rejected.

=== Cause 9 of 10: ProjectionExpression / Select validation refusals answer 501 instead of ValidationException ===
AWS: a ProjectionExpression whose paths overlap ('doc, doc') is refused with ValidationException, HTTP 400, and Select=ALL_PROJECTED_ATTRIBUTES without an accompanying IndexName is likewise refused with ValidationException, HTTP 400. These are input-validation outcomes, reached before any read.
Emulator: both answer HTTP 501 with code NotImplemented, because the scaffold check for the ProjectionExpression/Select branch runs ahead of validation.
Why: a malformed request is reported as an unimplemented feature; a client sees a retryable-looking server error for an error it can never fix by retrying.
Fix: order the GetItem/Query/Scan input validation ahead of the not-implemented branch check in the expression handling path (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/get_item.rs and the shared projection-expression validator). The open issue 20261024T065715Z-dynamodb-iam-sts-dynamodb-data-plane-reads-are-never-authorized-identity.md carries the GetItem-with-ProjectionExpression stub for legal expressions; it does not carry these validation keys.
Traces: dynamodb.projection-overlapping-document-paths-rejected, dynamodb.select-all-projected-attributes-requires-an-index-rejected.

=== Cause 10 of 10: a CloudFormation-deployed AWS::DynamoDB::Table does not exist in DynamoDB afterwards ===
AWS: a stack declaring AWS::DynamoDB::Table reaches CREATE_COMPLETE only once the table exists; DescribeTable on the table the stack declares then returns a TableDescription (and the table is ACTIVE). A second stack declaring the same TableName is refused and the stack fails.
Emulator: after the deploy phase reports success, DescribeTable on the stack's table answers ResourceNotFoundException, HTTP 400, example message 'Requested resource not found: Table: <name> not found'. Every scenario whose first phase is a CFN deploy of a table aborts in phase 1.
Why: no infrastructure-as-code path to a DynamoDB table works; the stack claims success for a resource that was never created, so every downstream assertion is unreachable.
Fix: the CloudFormation resource provisioner for AWS::DynamoDB::Table must create the table in the DynamoDB service state (and fail the resource when it cannot) rather than only recording it in the stack. Related open issues that do not carry these replay keys: 20261023T000000Z-cloudformation-stacks-report-create-complete-but-provision-no-resources.md and 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md.
Traces: dynamodb.deploy-cfn-table-duplicate-name-create-rejected, dynamodb.e2e-ddb-gsi-scan-and-fetch-record-lifecycle, dynamodb.e2e-ddb-rbp-resource-policy-tables-lifecycle, dynamodb.e2e-ddb-ss-scan-dynamodb-table-by-status-lifecycle, dynamodb.e2e-ddb-wf-query-workflow-status-analysis-lifecycle.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Query operation: operation Query with IndexName is scaffolded but not implemented in this emulator. Expected: HTTP 200 with QueryOutput (Items/Count/ScannedCount/LastEvaluatedKey/ConsumedCapacity). Smithy models no NotImplemented error for Query.
Cause 2: ClientError (NotImplemented) when calling the Scan operation: 'operation Scan with ProjectionExpression ...' and '... with FilterExpression ...'; AWS CLI exits 254. dynamodb.ddb-ss-filter-naming-an-undefined-expression-value-rejected: "the request raised 'NotImplemented' ('operation Scan with FilterExpression is scaffolded but not implemented in this emulator'), expected 'ValidationException'".
Cause 3: ClientError (NotImplemented) when calling UpdateTable: 'operation UpdateTable with ReplicaUpdates is scaffolded but not implemented in this emulator'; raw response showed HTTPStatusCode 501. Negative cases: "ReplicaUpdates Delete for the absent replica in 'us-east-1' returned 'NotImplemented', expected 'ValidationException'" and "ReplicaUpdates Create naming the table's own region 'us-west-2' returned 'NotImplemented', expected 'ValidationException'".
Cause 4: ClientError (NotImplemented) when calling UpdateTable: 'operation UpdateTable with SSESpecification is scaffolded but not implemented in this emulator'. Expected: 200 + TableDescription whose SSEDescription carries the resolved key ARN.
Cause 5: ClientError (NotImplemented) when calling UpdateTable: 'operation UpdateTable with OnDemandThroughput is scaffolded but not implemented in this emulator'. Expected: 200 + TableDescription with the on-demand ceiling set, then cleared.
Cause 6: AssertionError: expected ValidationException, got NotImplemented.
Cause 7: AssertionError: expected ValidationException (legacy Global Tables version withdrawn), got NotImplemented.
Cause 8: AssertionError: expected GlobalTableNotFoundException, got NotImplemented.
Cause 9: AssertionError: 'doc, doc' must be rejected with ValidationException; got NotImplemented. AssertionError: Select=ALL_PROJECTED_ATTRIBUTES refers to an index projection and must be refused with ValidationException when no IndexName accompanies it; got NotImplemented.
Cause 10: _PhaseError: [phase 1/5 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: <stack's table> not found. Expected: the table exists and DescribeTable returns it.

## Steps to Reproduce
1. Build and boot the emulator, then replay the checks listed in trace_ids.
2. Cause 1: CreateTable with a GSI, PutItem, then Query with IndexName set -> observe 501/NotImplemented instead of QueryOutput.
3. Cause 2: Scan with ProjectionExpression (or FilterExpression, including via `aws dynamodb scan --projection-expression`) -> observe 501/NotImplemented; Scan with a FilterExpression naming an undefined :value -> observe 501 instead of 400 ValidationException.
4. Cause 3: UpdateTable with ReplicaUpdates (Create in the table's own region, and Delete of an absent replica) -> observe 501/NotImplemented instead of 400 ValidationException; a valid replica create also 501s.
5. Cause 4: UpdateTable with SSESpecification naming a KMS alias -> 501.
6. Cause 5: UpdateTable with OnDemandThroughput -> 501.
7. Cause 6: DisableKinesisStreamingDestination on a table with no destination -> 501 instead of 400 ValidationException.
8. Cause 7: DescribeGlobalTable for an unknown legacy global table -> 501 instead of 400 ValidationException.
9. Cause 8: UpdateGlobalTableSettings naming a nonexistent index -> 501 instead of GlobalTableNotFoundException.
10. Cause 9: GetItem with ProjectionExpression 'doc, doc'; Query/Scan with Select=ALL_PROJECTED_ATTRIBUTES and no IndexName -> 501 instead of 400 ValidationException.
11. Cause 10: Deploy a CloudFormation template declaring AWS::DynamoDB::Table, wait for stack success, then call DescribeTable on the declared table -> ResourceNotFoundException.
