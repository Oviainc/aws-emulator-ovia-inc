Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-query-scan-and-batch-reads
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-query-scan-and-batch-reads ddbdepth-query-pagination-boundary ddbdepth-scan-pagination-boundary
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|Scan|2xx->2xx|$.Count
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*]
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].sk.N
Issue-Key: dynamodb|Scan|2xx->2xx|$.Items[*].val.S
Issue-Key: dynamodb|Scan|2xx->2xx|$.ScannedCount
Issue-Key: dynamodb|Query|4xx->4xx|$.message
Issue-Key: dynamodb|Scan|4xx->4xx|$.message
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# DynamoDB Scan returns the wrong item set and starting-key refusals lose their reason

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Scan's result computation is wrong in both directions — it drops items that match and returns items that do not — and the two refusals that guard ExclusiveStartKey report a different reason than AWS. 4 causes, all dynamodb.

Cause 1 of 4: Scan returns a subset of the table's items, and the items it does return are not the ones at those positions.
AWS: Scan with no filter returns every item in the table (up to Limit / the 1 MB page) with Count and ScannedCount equal to the number of items examined. The recording shows Count 5 and ScannedCount 5 over the five items the fixture wrote, in key order.
Emulator: Count 3 and ScannedCount 3; the items at index 1 and 2 are the wrong items (sort keys 2 and 4 where 1 and 2 are expected) and indexes 3 and 4 are absent entirely.
Why: a full scan is the primitive behind table exports, migrations and reconciliation; silently returning a short, differently-ordered result set means a client concludes items do not exist. Because ScannedCount matches the short Count, there is no signal that anything was skipped.
Fix: the Scan handler's item enumeration over table state — iterate the full item set for the segment/page rather than a partial view.
Traces: dynamodb-query-scan-and-batch-reads#task#12.

Cause 2 of 4: Scan with a FilterExpression that matches nothing returns items anyway.
AWS: FilterExpression is applied after the scan and removes non-matching items from Items, with Count reflecting the items returned. The recording shows Count 0, ScannedCount 0 and no Items for this filter.
Emulator: Count 2 and ScannedCount 2, returning two items that the filter excludes.
Why: the filter is not being evaluated (or is evaluated inverted), so a client reading filtered data gets records it explicitly asked to exclude — a correctness and, where filters carry tenant or visibility predicates, a disclosure problem.
Fix: the Scan handler's filter evaluation — apply the FilterExpression to each candidate item before adding it to Items and counting it.
Traces: dynamodb-query-scan-and-batch-reads#task#13.

Cause 3 of 4: Query's out-of-boundary ExclusiveStartKey refusal names the wrong reason.
AWS: the recording shows HTTP 400 with the message 'The provided starting key is outside query boundaries based on provided conditions' — the whole sentence is fixed text, nothing in it is request-derived.
Emulator: 400 with "The provided starting key does not match the key condition's partition key".
Why: the two conditions are different: AWS's message says the key is a valid key that falls outside the range the KeyConditionExpression selects, while the emulator's asserts a partition-key mismatch. A client that branches on the message (reset the paginator vs. fix the key condition) takes the wrong branch.
Fix: the Query ExclusiveStartKey validation — distinguish the out-of-boundary case and emit AWS's sentence for it.
Traces: ddbdepth-query-pagination-boundary#task#11.

Cause 4 of 4: Scan's invalid-ExclusiveStartKey refusal truncates the message, dropping the schema explanation.
AWS: the recording shows 'The provided starting key is invalid: The provided key element does not match the schema' — a fixed prefix plus the fixed explanatory clause after the colon.
Emulator: 'The provided starting key is invalid' with the explanation omitted.
Why: the clause after the colon is what tells the caller the key's attributes do not match the table's key schema, as opposed to being malformed or expired; without it the refusal is not actionable.
Fix: the Scan ExclusiveStartKey validation — append the schema-mismatch clause.
Traces: ddbdepth-scan-pagination-boundary#task#11.

## Evidence
Divergence 31 (Scan, 200->200): $.Count expected 5 got 3; $.ScannedCount expected 5 got 3; $.Items[1].sk.N expected '1' got '2'; $.Items[1].val.S expected 'v1' got 'v2'; $.Items[2].sk.N expected '2' got '4'; $.Items[3] and $.Items[4] missing (expected the sk=3 and sk=4 items).
Divergence 32 (Scan, 200->200): $.Count expected 0 got 2; $.ScannedCount expected 0 got 2; $.Items[0] and $.Items[1] extra (two items the filter excludes).
Divergence 13 (Query, 400->400): $.message expected 'The provided starting key is outside query boundaries based on provided conditions', got "The provided starting key does not match the key condition's partition key".
Divergence 14 (Scan, 400->400): $.message expected 'The provided starting key is invalid: The provided key element does not match the schema', got 'The provided starting key is invalid'.

## Steps to Reproduce
1. Replay dynamodb-query-scan-and-batch-reads#task#12: write five items then Scan -> Count/ScannedCount 3 instead of 5, with the wrong items at indexes 1 and 2.
2. Replay dynamodb-query-scan-and-batch-reads#task#13: Scan with a FilterExpression that matches nothing -> 2 items and Count 2 instead of 0.
3. Replay ddbdepth-query-pagination-boundary#task#11: Query with an ExclusiveStartKey outside the key condition's range -> partition-key-mismatch message instead of the out-of-boundary message.
4. Replay ddbdepth-scan-pagination-boundary#task#11: Scan with an ExclusiveStartKey that does not match the key schema -> message truncated before ': The provided key element does not match the schema'.
