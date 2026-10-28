Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.ddb-frc-absent-null-and-string-typed-values-never-satisfy-the-filter dynamodb.ddb-sfa-misspelled-attribute-exists-function-rejected dynamodb.ddb-sfa-unused-expression-attribute-name-in-scan-rejected dynamodb.partiql-partition-key-predicate-is-planned-as-a-query-not-a-scan dynamodb.hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.ddb-frc-absent-null-and-string-typed-values-never-satisfy-the-filter|run#9|AssertionError|assert _ids(ne["items"]) == ("item3", "near-absent", "near-null"), (
Issue-Key: dynamodb.ddb-sfa-misspelled-attribute-exists-function-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: dynamodb.ddb-sfa-unused-expression-attribute-name-in-scan-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: dynamodb.partiql-partition-key-predicate-is-planned-as-a-query-not-a-scan|run#27|AssertionError|assert pq_nonkey_cap == filtered_scan_cap, (
Issue-Key: dynamodb.hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected|_paced_arm#16|Timeout|f.result()
Issue-Commit: 2524c81a99ad2e7d923e1349dfe7c153509bdc71

# dynamodb: 5 net-new root causes — Scan `<>` skips items missing the attribute, two FilterExpression validation defects, PartiQL non-key SELECT bills as a Query, and no per-partition-key write limiter

> _Note: this issue was identified against the code base at commit `2524c81a99ad2e7d923e1349dfe7c153509bdc71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Five distinct, independently fixable root causes in the DynamoDB read/validation/throughput paths. All five must be closed before this issue is submitted.

=== Cause 1 of 5: Scan FilterExpression `a <> :v` drops items that do not carry `a` ===
AWS: Smithy: Scan.FilterExpression -> ConditionExpression; Scan output Count/ScannedCount/Items. The model does not state comparator semantics for an absent attribute, so this is recorded behavior only plus the documented FilterExpression rule: for a not-equal comparison the attribute need not be present — an item that has no `a` at all, and an item whose `a` is the NULL type, both satisfy `a <> :v` for a non-null `:v`. Only `=` requires the attribute to be present and equal. So over a table holding one item with `tag = "active"`, one item with NO `tag` member, and one item with `tag = {"NULL": true}`, a strongly consistent Scan with FilterExpression `tag <> :e`, `:e = {"S": ""}` returns all three.
Emulator: the same Scan returns only the two items that HAVE a `tag` member (the `"active"` item and the NULL-typed item). Every item where the attribute is absent is filtered out, as if `<>` were evaluated as `attribute_exists(tag) AND tag <> :e`.
Why: a client using `<>` as the complement of `=` silently loses every sparse-attribute item. Backfill/reconciliation scans that select "everything not yet tagged X" come back short and report success, so rows are skipped with no error to detect.
Fix: the Scan/Query handler's FilterExpression comparator evaluation — the not-equal (and by symmetry every negated) comparator must treat a missing attribute as a value that is not equal, rather than short-circuiting to false when the path does not resolve.
Traces: dynamodb.ddb-frc-absent-null-and-string-typed-values-never-satisfy-the-filter.

=== Cause 2 of 5: the unknown-function ValidationException message is emulator-authored ===
AWS: Smithy: Scan declares no ValidationException (it is DynamoDB's service-common client validation error, HTTP 400, code `ValidationException`). For a FilterExpression naming a function DynamoDB does not have, AWS refuses with 400 ValidationException and message `Invalid FilterExpression: Invalid function name; function: <name>`. Fixed text: `Invalid FilterExpression: Invalid function name; function: ` — the `<name>` suffix is the offending identifier taken verbatim from the request's expression. (The same rule holds per expression member, i.e. `Invalid ConditionExpression: ...`, `Invalid UpdateExpression: ...`.)
Emulator: the code is right (`ValidationException`) but the message is `Invalid FilterExpression: unknown function attribute_exist` — the fixed text `Invalid function name; function: ` is replaced by the emulator's own phrase `unknown function `.
Why: the error code is correct, so the request is refused, but the message is a requirement. Clients, CLI output and test suites that match on AWS's message text (and any human reading the diagnosis) see wording that does not exist in AWS.
Fix: the Scan/Query expression parser's error construction (the FilterExpression validation layer that builds the ValidationException message) — emit the member-qualified `Invalid <Member>: Invalid function name; function: <name>` template.
Traces: dynamodb.ddb-sfa-misspelled-attribute-exists-function-rejected.

=== Cause 3 of 5: an ExpressionAttributeNames entry no expression references is accepted instead of refused ===
AWS: Smithy: Scan.ExpressionAttributeNames -> ExpressionAttributeNameMap (no modeled constraint on usage), so this is the documented whole-request validation rule plus recorded behavior: every key supplied in ExpressionAttributeNames must be referenced by at least one of FilterExpression/ProjectionExpression/KeyConditionExpression. An unused key makes the whole request invalid — AWS answers 400 `ValidationException` with message `Value provided in ExpressionAttributeNames unused in expressions: keys: {<key>}`. Fixed text: `Value provided in ExpressionAttributeNames unused in expressions: keys: {` ... `}`; the variable part is the set of unreferenced alias keys from the request (e.g. `#b`), comma-separated when there is more than one.
Emulator: a Scan with `FilterExpression="attribute_exists(#a)"` and `ExpressionAttributeNames={"#a": "status", "#b": "category"}` SUCCEEDS with 200 and returns items. The unused-alias check does not exist, so no error is raised at all.
Why: this is a request AWS always refuses. A caller whose expression-building code drops a predicate but keeps its alias (a very common bug) is told everything is fine against the emulator and then fails in production with a 400. The emulator is more permissive than AWS, which is the failure mode an emulator must not have.
Fix: the Scan/Query/UpdateItem/PutItem/DeleteItem shared expression-validation component — after parsing every expression member, diff the referenced alias set against the supplied ExpressionAttributeNames (and, symmetrically, ExpressionAttributeValues) keys and refuse the request before it touches state.
Traces: dynamodb.ddb-sfa-unused-expression-attribute-name-in-scan-rejected.

=== Cause 4 of 5: ExecuteStatement bills a non-key WHERE predicate like a single-partition Query ===
AWS: Smithy: ExecuteStatement.Statement -> PartiQLStatement, output ConsumedCapacity; reference https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/ql-reference.select.html. A PartiQL SELECT is planned to one of the low-level plans: a WHERE clause that pins the partition key with an equality is planned as a Query and bills the Query's capacity; a WHERE clause on a non-key attribute with no partition-key equality is planned as a FULL TABLE SCAN with the predicate applied after the read, and therefore bills what the equivalent `Scan(FilterExpression=...)` bills — which in turn equals the unfiltered Scan of the same table, because a FilterExpression never reduces capacity. Both statements return the identical item set; only the cost differs.
Emulator: on a table of 10 partitions x 30 items of ~1 KB, with ConsistentRead=true and ReturnConsumedCapacity=TOTAL: the native Query on `pk` bills 7.0, the native filtered Scan bills 68.0 (equal to the unfiltered Scan, correctly), and `SELECT * FROM "<table>" WHERE pk = ?` bills 7.0 (correct) — but `SELECT * FROM "<table>" WHERE tag = ?`, where `tag` is a plain non-key attribute of no index, also bills 7.0 instead of 68.0. The numbers above are one run's values; the requirement is the relation: non-key-predicate ExecuteStatement capacity == equivalent filtered-Scan capacity == unfiltered-Scan capacity, and strictly greater than the single-partition Query capacity.
Why: ConsumedCapacity is the only signal a client has for whether its PartiQL statement is a Query or a Scan. Reporting Query-shaped capacity for a statement AWS plans as a full Scan makes a table-scanning query look cheap; capacity planning, throttling budgets and cost regression tests built on the emulator are all wrong, and the statement that looks fine at 7.0 units consumes ten times that on AWS.
Fix: the ExecuteStatement handler's planner/capacity accounting — the plan choice must be derived from whether the WHERE clause supplies a partition-key equality (and only then a sort-key/index narrowing), and the reported ConsumedCapacity must be computed from the bytes the chosen plan actually read, not from the post-filter result set.
Traces: dynamodb.partiql-partition-key-predicate-is-planned-as-a-query-not-a-scan.

=== Cause 5 of 5: no per-partition-key write limiter, and the paced hot-key load never completes ===
AWS: Smithy: PutItem/BatchWriteItem declare `ProvisionedThroughputExceededException` (client, HTTP 400), whose members are `message` and `ThrottlingReasons` -> ThrottlingReasonList of ThrottlingReason{reason, resource}. The model documents the reason format explicitly as `ResourceType+OperationType+LimitType`, with `KeyRangeThroughputExceeded` defined as "the request rate directed at a specific partition key value has exceeded the internal partition-level throughput limits" — so a write-side hot-key throttle on a table is reported as reason `TableWriteKeyRangeThroughputExceeded` with `resource` set to the table ARN. On a PROVISIONED table (4000 WCU / 3000 RCU) an offered 2500 writes/s of 1 KB items spread over 50 partition keys is inside the table budget and must NOT be throttled, while the same 2500/s aimed at one partition key must be throttled at the per-partition-key ceiling; four keys must land strictly between the two. Throttled writes must leave nothing behind: items reported in BatchWriteItem UnprocessedItems must be absent on a later consistent read.
Emulator: the check does not finish — the harness cut it at its 60 s cap, so none of the arms' results were observed. Hypothesis: the paced driver issues three 10 s arms of 25-item BatchWriteItem calls at 2500 items/s (plus a 4 s warm-up, 20 s quiesce gaps and consistent read-back), and the emulator's write path does not sustain the offered rate, so the arms over-run their pacing budget and the check exceeds the bound. The fidelity gap it exists to grade is the same one: there is no partition-key-level write limiter and no `ThrottlingReasons` on the throttle error.
Why: hot-partition throttling is the single most common production DynamoDB failure, and `ThrottlingReasons` is how a client tells a key-design problem from an under-provisioned table. An emulator that accepts an unbounded hot-key write stream (or simply stalls under it) lets a schema with a hot partition key pass every pre-production test, and a client keyed on `TableWriteKeyRangeThroughputExceeded` never takes its hot-key branch. Separately, a write path that cannot carry a few thousand 1 KB writes per second makes every load-shaped check unrunnable.
Fix: the write-path admission/throughput component shared by PutItem and BatchWriteItem — add a per-partition-key rate limiter beneath the existing table-level accounting, populate ProvisionedThroughputExceededException.ThrottlingReasons with {reason: "TableWriteKeyRangeThroughputExceeded", resource: <table ARN>} when it is the binding constraint, keep throttled writes out of state (reported via BatchWriteItem UnprocessedItems), and profile the BatchWriteItem hot path so a paced multi-thread writer is not the bottleneck. Related but distinct from open issue 20261025T024002Z-dynamodb-7-net-new-root-causes-consumedcapacity-breakdown.md, which covers table-level throughput enforcement; that issue does not cover the per-partition-key limiter or the ThrottlingReasons member.
Traces: dynamodb.hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected.

## Evidence
Cause 1 — dynamodb.ddb-frc-absent-null-and-string-typed-values-never-satisfy-the-filter:
  AssertionError: tag <> '' returned ('item3', 'near-null'); a missing or NULL tag is not equal to ''
  Expected ('item3', 'near-absent', 'near-null') — 'near-absent' is the item with NO `tag` member.

Cause 2 — dynamodb.ddb-sfa-misspelled-attribute-exists-function-rejected:
  AssertionError: expected message 'Invalid FilterExpression: Invalid function name; function: attribute_exist',
                  got 'Invalid FilterExpression: unknown function attribute_exist'
  Code was correct: ValidationException / HTTP 400.

Cause 3 — dynamodb.ddb-sfa-unused-expression-attribute-name-in-scan-rejected:
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code
  Request: Scan(TableName=<t>, ConsistentRead=True, FilterExpression="attribute_exists(#a)",
                ExpressionAttributeNames={"#a": "status", "#b": "category"})
  Expected: 400 ValidationException, message
            'Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}'
  Actual:   HTTP 200 with Items.

Cause 4 — dynamodb.partiql-partition-key-predicate-is-planned-as-a-query-not-a-scan:
  AssertionError: a SELECT with no partition-key equality must be planned as a full table Scan and
  bill what the equivalent filtered Scan billed: PartiQL 7.0 vs Scan 68.0 (a single-partition Query
  bills 7.0, so a PartiQL number near that means the non-key predicate was resolved through an index
  that does not exist)
  Same run: Query(pk = :p) 7.0; Scan(FilterExpression tag = :p) 68.0 == unfiltered Scan 68.0;
  ExecuteStatement WHERE pk = ? 7.0 (correct); ExecuteStatement WHERE tag = ? 7.0 (wrong).
  Both statements returned the identical 30 items.

Cause 5 — dynamodb.hot-partition-key-throttled-while-distributed-keys-at-same-rate-rejected:
  Error: Check timed out after 60.0s — no arm result observed.
  Setup: PROVISIONED table, ReadCapacityUnits 3000 / WriteCapacityUnits 4000; 1 KB items; offered
  2500 writes/s per arm via 25-item BatchWriteItem with single-PutItem probes; arms = 50 keys,
  1 key, 4 keys.
  Expected on AWS: 50-key arm 0 unprocessed and no error codes; 1-key arm unprocessed > 0 with
  codes == {ProvisionedThroughputExceededException} and ThrottlingReasons reason set ==
  {'TableWriteKeyRangeThroughputExceeded'} and resource set == {table ARN}; 4-key arm strictly
  between; items reported unprocessed absent on a consistent BatchGetItem.

## Steps to Reproduce
All five run against the emulator's DynamoDB endpoint; replay the five check ids in trace_ids.

Cause 1: CreateTable on-demand, hash key `id` (S). BatchWriteItem three items: {id:"item3", tag:{S:"active"}}, {id:"near-absent", ResolvedAt:{N:"5"}} (no `tag` member), {id:"near-null", tag:{NULL:true}}. Scan(ConsistentRead=True, FilterExpression="tag <> :e", ExpressionAttributeValues={":e":{"S":""}}). Observe only item3 and near-null come back; near-absent must be returned too.

Cause 2: on the same style of table, Scan(ConsistentRead=True, FilterExpression="attribute_exist(#a)", ExpressionAttributeNames={"#a":"status"}) — note the missing trailing `s`. Read Error.Message; compare to 'Invalid FilterExpression: Invalid function name; function: attribute_exist'.

Cause 3: Scan(ConsistentRead=True, FilterExpression="attribute_exists(#a)", ExpressionAttributeNames={"#a":"status", "#b":"category"}). The call must be refused 400 ValidationException with 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#b}'; today it returns 200.

Cause 4: CreateTable PAY_PER_REQUEST, keys pk (S) HASH + sk (N) RANGE. Write 10 partitions x 30 items, each ~1 KB, with a non-key attribute `tag` whose value mirrors `pk`. Then with ConsistentRead=True and ReturnConsumedCapacity=TOTAL collect: Query(KeyConditionExpression='pk = :p'), Scan(FilterExpression='tag = :p'), Scan() unfiltered, ExecuteStatement('SELECT * FROM "<t>" WHERE pk = ?'), ExecuteStatement('SELECT * FROM "<t>" WHERE tag = ?'). Compare CapacityUnits: the last must equal the filtered Scan, not the Query.

Cause 5: CreateTable PROVISIONED with ReadCapacityUnits=3000, WriteCapacityUnits=4000, keys pk (S) + sk (S). With a client configured retries total_max_attempts=1, drive 2500 1 KB writes/s for 10 s in 25-item BatchWriteItem calls, once over 50 distinct pk values and once over the single pk "hot", with single PutItem probes alongside to capture the exception. Inspect UnprocessedItems counts, the probe error codes, and response['ThrottlingReasons'].
