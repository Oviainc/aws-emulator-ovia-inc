Issue-Source: trace-sweep-divergence
Issue-Trace: athena-aorigin-athena-metadata-table-vs-view
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-aorigin-athena-metadata-table-vs-view athena-cflog-athena-catalog-lookup-mistakes athena-batch-query-submission-and-pagination athena-cats-traj-output-location-mistakes athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket athena-get-prepared-statement-contract-and-invalid-name-rejected athena-named-query-crud-and-batch-retrieval
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|ListTableMetadata|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|ListTableMetadata|4xx->4xx|$.ErrorCode
Issue-Key: athena|ListTableMetadata|4xx->4xx|$.Message
Issue-Key: athena|ListTableMetadata|4xx->4xx|$.__type
Issue-Key: athena|GetTableMetadata|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|GetTableMetadata|4xx->4xx|$.ErrorCode
Issue-Key: athena|GetTableMetadata|4xx->4xx|$.Message
Issue-Key: athena|GetTableMetadata|4xx->4xx|$.__type
Issue-Key: athena|BatchGetQueryExecution|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|BatchGetQueryExecution|4xx->4xx|$.ErrorCode
Issue-Key: athena|BatchGetQueryExecution|4xx->4xx|$.Message
Issue-Key: athena|BatchGetQueryExecution|4xx->4xx|$.__type
Issue-Key: athena|StartQueryExecution|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|StartQueryExecution|4xx->4xx|$.ErrorCode
Issue-Key: athena|StartQueryExecution|4xx->4xx|$.Message
Issue-Key: athena|StartQueryExecution|4xx->4xx|$.__type
Issue-Key: athena|GetPreparedStatement|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|GetPreparedStatement|4xx->4xx|$.ErrorCode
Issue-Key: athena|GetPreparedStatement|4xx->4xx|$.Message
Issue-Key: athena|GetPreparedStatement|4xx->4xx|$.__type
Issue-Key: athena|BatchGetNamedQuery|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|BatchGetNamedQuery|4xx->4xx|$.ErrorCode
Issue-Key: athena|BatchGetNamedQuery|4xx->4xx|$.Message
Issue-Key: athena|BatchGetNamedQuery|4xx->4xx|$.__type
Issue-Commit: fb394b2a45eb301a39bf46320599094d7846a70d

# Athena input validation returns ValidationException without AthenaErrorCode or AWS's validation message (6 causes)

> _Note: this issue was identified against the code base at commit `fb394b2a45eb301a39bf46320599094d7846a70d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTableMetadata

## Description
Every Athena request that violates a modeled input constraint is refused by a generic cross-service constraint validator instead of by Athena's own error contract: the status is right (400) but the error body is a bare ValidationException with no Message at all, where AWS returns InvalidRequestException carrying AthenaErrorCode, ErrorCode and the standard AWS validation sentence. 6 causes, one per constrained member.

Shared AWS contract (Smithy: InvalidRequestException {AthenaErrorCode: ErrorCode @length 1..256, Message}, httpError 400, bound to all six operations below): AWS answers __type 'InvalidRequestException' with AthenaErrorCode='INVALID_INPUT', ErrorCode='INVALID_INPUT' and a Message of the fixed AWS validation form "<n> validation error detected: Value <value-clause>at '<memberName>' failed to satisfy constraint: <rule>", where n, the member name and the rule are derived from the request and the model's constraint (for a null member AWS writes "Value null at '<member>' ... Member must not be null"; for a bound it writes the bound). The emulator instead answers __type 'ValidationException' with no AthenaErrorCode, no ErrorCode and no Message member at all. Fix for all six: the athena crate's shared request-validation layer must raise Athena's InvalidRequestException with the AthenaErrorCode/ErrorCode pair and render AWS's validation sentence, rather than delegating to the generic ValidationException shape.

Cause 1 of 6: ListTableMetadata MaxResults above its bound. Smithy: ListTableMetadata.MaxResults targets MaxTableMetadataCount @range 1..50. AWS Message: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50". Why: a client paging metadata cannot tell a page-size violation from any other validation failure. Traces: athena-aorigin-athena-metadata-table-vs-view#task#8.

Cause 2 of 6: GetTableMetadata with a null DatabaseName. Smithy: GetTableMetadata.DatabaseName required (NameString @length 1..128). AWS Message: "1 validation error detected: Value null at 'databaseName' failed to satisfy constraint: Member must not be null". Why: the missing member is never named, so a client cannot repair the request. Traces: athena-aorigin-athena-metadata-table-vs-view#task#9, athena-cflog-athena-catalog-lookup-mistakes#task#9.

Cause 3 of 6: BatchGetQueryExecution with more than 50 ids. Smithy: BatchGetQueryExecution.QueryExecutionIds required, QueryExecutionIdList @length 1..50. AWS Message: "1 validation error detected: Value at 'queryExecutionIds' failed to satisfy constraint: Member must have length less than or equal to 50". Why: a client that chunks a batch on the service's stated limit gets no limit back to chunk on. Traces: athena-batch-query-submission-and-pagination#task#6.

Cause 4 of 6: StartQueryExecution with a null QueryString. Smithy: StartQueryExecution.QueryString required (@length 1..262144). AWS Message: "1 validation error detected: Value null at 'queryString' failed to satisfy constraint: Member must not be null". Why: the most common submission mistake is reported without naming the member. Traces: athena-cats-traj-output-location-mistakes#task#5, athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#13.

Cause 5 of 6: GetPreparedStatement with a StatementName that violates its pattern. Smithy: GetPreparedStatement.StatementName required, pattern ^[a-zA-Z_][a-zA-Z0-9_@:]{1,256}$. AWS Message: "1 validation error detected: Value at 'statementName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z_][a-zA-Z0-9_@:]{1,256}". Why: the pattern a client must satisfy is never returned. Traces: athena-get-prepared-statement-contract-and-invalid-name-rejected#task#4.

Cause 6 of 6: BatchGetNamedQuery with more than 50 ids. Smithy: BatchGetNamedQuery.NamedQueryIds required, NamedQueryIdList @length 1..50. AWS Message: "1 validation error detected: Value at 'namedQueryIds' failed to satisfy constraint: Member must have length less than or equal to 50". Why: as cause 3, on the named-query batch path. Traces: athena-named-query-crud-and-batch-retrieval#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Every cause reports status 400 both sides; the divergence is entirely in the error body.
Cause 1 — athena-aorigin-athena-metadata-table-vs-view#task#8, ListTableMetadata: expected $.__type='InvalidRequestException', $.AthenaErrorCode='INVALID_INPUT', $.ErrorCode='INVALID_INPUT', $.Message="1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50"; actual $.__type='ValidationException' and all three other members absent.
Cause 2 — athena-aorigin-athena-metadata-table-vs-view#task#9, GetTableMetadata: expected the same envelope with Message="1 validation error detected: Value null at 'databaseName' failed to satisfy constraint: Member must not be null"; actual 'ValidationException', members absent (2 steps).
Cause 3 — athena-batch-query-submission-and-pagination#task#6, BatchGetQueryExecution: expected Message="1 validation error detected: Value at 'queryExecutionIds' failed to satisfy constraint: Member must have length less than or equal to 50"; actual 'ValidationException', members absent.
Cause 4 — athena-cats-traj-output-location-mistakes#task#5, StartQueryExecution: expected Message="1 validation error detected: Value null at 'queryString' failed to satisfy constraint: Member must not be null"; actual 'ValidationException', members absent (2 steps).
Cause 5 — athena-get-prepared-statement-contract-and-invalid-name-rejected#task#4, GetPreparedStatement: expected Message="1 validation error detected: Value at 'statementName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z_][a-zA-Z0-9_@:]{1,256}"; actual 'ValidationException', members absent.
Cause 6 — athena-named-query-crud-and-batch-retrieval#task#11, BatchGetNamedQuery: expected Message="1 validation error detected: Value at 'namedQueryIds' failed to satisfy constraint: Member must have length less than or equal to 50"; actual 'ValidationException', members absent.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Inspect the listed fixture#step pairs, representative athena-aorigin-athena-metadata-table-vs-view#task#8.
4. Minimal manual repro: call ListTableMetadata with MaxResults=51, GetTableMetadata with DatabaseName omitted, BatchGetQueryExecution/BatchGetNamedQuery with 51 ids, StartQueryExecution with QueryString omitted, and GetPreparedStatement with StatementName='1bad-name'. Each must answer 400 with __type InvalidRequestException, AthenaErrorCode=ErrorCode=INVALID_INPUT and the AWS validation sentence naming the member and the rule.
