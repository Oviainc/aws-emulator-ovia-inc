Issue-Severity: HIGH
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# DynamoDB index-targeted and projection-carrying Query/Scan answer 501 NotImplemented, so request-validation refusals AWS models are never served

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
One net-new root cause.

Cause 1 of 1: a scaffold guard on Query/Scan fires before input validation, so every refusal AWS models for an index-targeted or projection-carrying read is replaced by a 501 NotImplemented.
AWS: Query and Scan are fully modeled (Smithy: Query/Scan errors = InternalServerError, InvalidEndpointException, ProvisionedThroughputExceededException, RequestLimitExceeded, ResourceNotFoundException, ThrottlingException, plus the service-common ValidationException) and every one of the following is a 400 ValidationException with a fixed message and request-supplied variables: naming an index the table does not have — "The table does not have the specified index: <IndexName>"; a key condition that omits the index partition key, or supplies a value whose type does not match the index key's declared type; ConsistentRead=true against a GSI; Select=COUNT together with a ProjectionExpression; Select=ALL_ATTRIBUTES together with a ProjectionExpression; a ProjectionExpression naming an undefined placeholder or a bare reserved word; and a ProjectionExpression path into an attribute on Scan is legal and must be SERVED.
Emulator: each of these answers HTTP 501 with Code NotImplemented and the message 'operation Query with IndexName is scaffolded but not implemented in this emulator' / 'operation Query with ProjectionExpression is scaffolded but not implemented in this emulator' / 'operation Scan with a ProjectionExpression path into an attribute is scaffolded but not implemented in this emulator'. The guard is evaluated ahead of validation, so even a request whose defect AWS reports without ever touching an index is answered 501.
Why: 501 is not a modeled DynamoDB error — botocore raises an unmodeled ClientError, no retry/branch logic matches, and a client cannot tell "your request is invalid" from "this endpoint cannot do indexes". A legal Scan projection is refused outright, so working code fails.
Fix: the Query and Scan handlers must run input validation (index existence, key-condition coverage and operand typing, Select/ProjectionExpression consistency, ConsistentRead-on-GSI) before any scaffold guard, and must serve ProjectionExpression paths; the 501 guard must not pre-empt a modeled 400.
Traces: the eight checks in trace_ids. The open issue 20261025T024002Z-dynamodb-10-net-new-root-causes-query-scan.md and 20261029T180026Z-dynamodb-global-tables-replica-auto-scaling-indexed-query.md own a different, overlapping set of indexed-read checks; the eight listed here are not carried by either and are reported unowned by the replay.

## Evidence
ddb-gsi-query-naming-an-absent-index-rejected: AssertionError: a query against an absent index raised ('NotImplemented', 'operation Query with IndexName is scaffolded but not implemented in this emulator'), expected ('ValidationException', 'The table does not have the specified index: TaskStatus').
ddb-gsi-query-omitting-the-index-partition-key-rejected: a sort-key-only condition on an index raised 'NotImplemented', expected 'ValidationException'.
ddb-gsi-query-on-numeric-index-key-with-a-string-value-rejected: a string value for the numeric index key raised 'NotImplemented', expected 'ValidationException'.
query-capacity-gsi-rejects-consistent-read-lsi-accepts-it: a strongly consistent Query against a GSI must be refused with ValidationException; got NotImplemented.
ddb-wf-select-count-with-projection-expression-rejected: Select=COUNT with a ProjectionExpression on Query raised 'NotImplemented', expected 'ValidationException'.
select-non-specific-values-reject-projection-expression: Query Select=ALL_ATTRIBUTES with a ProjectionExpression must be refused with ValidationException; got NotImplemented.
query-and-put-item-placeholder-and-reserved-word-validation: zero-match Query projecting a.#u undefined gave 'NotImplemented': 'operation Query with ProjectionExpression is scaffolded but not implemented in this emulator'.
ddb-ss-aliased-dotted-attribute-name-projects-the-top-level-attribute: botocore ClientError (NotImplemented) when calling Scan: 'operation Scan with a ProjectionExpression path into an attribute is scaffolded but not implemented in this emulator' — a legal projection that AWS serves.

## Steps to Reproduce
Replay the behavioral suite and re-run the dynamodb checks in trace_ids. Minimal reproductions against the emulator: (1) ddb.query(TableName=t, IndexName='no-such-index', KeyConditionExpression='pk = :p', ExpressionAttributeValues={':p':{'S':'a'}}) — expect 400 ValidationException 'The table does not have the specified index: no-such-index', observe 501 NotImplemented; (2) ddb.query(TableName=t, IndexName=gsi, KeyConditionExpression='pk = :p', ConsistentRead=True) — expect 400 ValidationException, observe 501; (3) ddb.query(TableName=t, Select='COUNT', ProjectionExpression='a', KeyConditionExpression='pk = :p') — expect 400 ValidationException, observe 501; (4) ddb.scan(TableName=t, ProjectionExpression='#p.#c', ExpressionAttributeNames={'#p':'parent','#c':'child'}) — expect the projected items, observe 501.
