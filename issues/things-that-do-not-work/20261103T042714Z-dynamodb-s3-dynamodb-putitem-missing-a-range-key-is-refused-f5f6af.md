Issue-Severity: MEDIUM
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# DynamoDB PutItem missing a range key is refused with "One of the required keys was not given a value" instead of AWS's "Missing the key <name> in the item" (1 cause)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
One net-new root cause. Open issue 20261103T022510Z-dynamodb-s3-dynamodb-putitem-missing-a-range-key-is-refused.md names the same divergence but the suite did not mark this check as owned by it, so this replay key is not carried anywhere; close them together.

=== Cause 1 of 1: the missing-key refusal does not name the key ===
AWS: PutItem against a table with a composite key, where the Item omits the RANGE key attribute, is refused with ValidationException (HTTP 400) and the fixed sentence "One or more parameter values were invalid: Missing the key <rangeKeyAttributeName> in the item". The fixed text is "One or more parameter values were invalid: Missing the key " ... " in the item"; the only variable part is the key attribute's name, taken from the table's KeySchema. (PutItem's Smithy error set models ConditionalCheckFailedException, ResourceNotFoundException and the throughput errors; the key defect is the service-common ValidationException.)
Emulator: it answers ValidationException with 'One of the required keys was not given a value' — the correct error code and status, but a message that names neither the key nor the concept AWS names.
Why: the caller is told a key is missing but not WHICH one, which on a wide item with several candidate attributes is the whole diagnostic; and a client or tool matching AWS's sentence (or extracting the key name from it) gets nothing. The check also shows the positive control passing, so this is purely the refusal's message, not the key enforcement itself.
Fix: the key-validation step in the PutItem handler must render AWS's sentence with the missing key attribute's name interpolated from the table's KeySchema.
Traces: ddb-table-per-csv-object-enforces-derived-composite-key.

## Evidence
ddb-table-per-csv-object-enforces-derived-composite-key: AssertionError: sim2real-scn-inventory-<id> accepted an item with no 'warehouse_id' range key (error was 'ValidationException': 'One of the required keys was not given a value') -- the key derived from 'inventory.csv' is not being enforced. The check asserts (code, message) == ('ValidationException', "One or more parameter values were invalid: Missing the key warehouse_id in the item"); the code matches, the message does not. The preceding positive control (an item carrying both derived key attributes) round-tripped successfully, so only the refusal text diverges.

## Steps to Reproduce
1. Create a table whose KeySchema is HASH <h> (S) and RANGE <r> (S) — e.g. derived from a CSV header's first two columns.
2. ddb.put_item(TableName=t, Item={'<h>': {'S': 'ctl'}, '<r>': {'S': 'ctl'}}) — succeeds (positive control).
3. ddb.put_item(TableName=t, Item={'<h>': {'S': 'no-range-key'}}) and read Error.Code / Error.Message.
4. Expect ('ValidationException', "One or more parameter values were invalid: Missing the key <r> in the item"); observe ('ValidationException', 'One of the required keys was not given a value').
Then replay the dynamodb__s3 check named in trace_ids.
