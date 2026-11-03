Issue-Severity: MEDIUM
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# DynamoDB PutItem missing a range key is refused with "One of the required keys was not given a value" instead of AWS's "Missing the key <name> in the item"

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
One net-new root cause.

Cause 1 of 1: the missing-key refusal on PutItem uses a different sentence from AWS's, and names no attribute.
AWS: a PutItem whose Item omits one of the table's key attributes is refused with ValidationException (HTTP 400) and the message "One or more parameter values were invalid: Missing the key <keyAttributeName> in the item". The fixed text is "One or more parameter values were invalid: Missing the key " ... " in the item"; the single variable part is the name of the key attribute that is absent, taken from the table's KeySchema and the submitted Item.
Emulator: the same call is refused with ValidationException but the message is "One of the required keys was not given a value" — a different sentence that names no attribute at all.
Why: on a composite-key table the caller cannot tell WHICH key it forgot, which is the entire value of AWS's message; and any client, SDK wrapper or test that matches AWS's wording fails. Here the table's composite key is derived per S3 object, so the attribute name is exactly the information the caller needs.
Fix: the key-presence validation in the PutItem handler must render AWS's sentence and substitute the absent key attribute's name.
Traces: ddb-table-per-csv-object-enforces-derived-composite-key. The same missing-/mismatched-key message family is reported separately under the `dynamodb` heading (update-item-key-schema-mismatch, ddb-wf-batch-write-item-missing-the-sort-key-rejected-whole); this entry is the dynamodb__s3 instance and carries its own replay key.

## Evidence
ddb-table-per-csv-object-enforces-derived-composite-key: the check asserts (code, message) == ('ValidationException', "One or more parameter values were invalid: Missing the key warehouse_id in the item") after calling ddb.put_item(TableName=t, Item={<hash key only>}).
AssertionError: sim2real-scn-inventory-476a70cefcb9 accepted an item with no 'warehouse_id' range key (error was 'ValidationException': 'One of the required keys was not given a value') -- the key derived from 'inventory.csv' is not being enforced.
So the code matches (ValidationException) and only the message diverges: expected "One or more parameter values were invalid: Missing the key warehouse_id in the item", got "One of the required keys was not given a value".

## Steps to Reproduce
Replay the behavioral suite and re-run dynamodb__s3.ddb-table-per-csv-object-enforces-derived-composite-key. Minimal reproduction: create a table with KeySchema [{pk, HASH}, {sk, RANGE}], then dynamodb.put_item(TableName=t, Item={'pk': {'S': 'x'}}) and read Error.Message — expect "One or more parameter values were invalid: Missing the key sk in the item", observe "One of the required keys was not given a value".
