Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__s3.ddb-table-per-csv-object-enforces-derived-composite-key
Issue-Key: dynamodb__s3.ddb-table-per-csv-object-enforces-derived-composite-key|run#12|AssertionError|assert (code, message) == ("ValidationException", expected_message), (
Issue-Commit: 56c3e2d5db4619f120a986f75a3622f8f9af8979

# dynamodb__s3: PutItem's missing-key ValidationException message is not AWS's — it does not name the missing key attribute

> _Note: this issue was identified against the code base at commit `56c3e2d5db4619f120a986f75a3622f8f9af8979`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
One root cause, grounded in one failing check under `### dynamodb__s3`.

Cause 1 of 1: a PutItem that omits a declared key attribute is refused with the wrong ValidationException message.
AWS: recorded behavior. PutItem against a table with a composite key, where the item omits one of the declared key attributes, is refused HTTP 400 `<Code>ValidationException</Code>` with the message template `One or more parameter values were invalid: Missing the key <attributeName> in the item`. Fixed text: "One or more parameter values were invalid: Missing the key " and " in the item". Variable part: the name of the missing key attribute, taken from the table's KeySchema (here the RANGE key derived from the CSV header). The error code and the 400 status are correct in the emulator; only the message diverges.
Emulator: answers `ValidationException` with the message `One of the required keys was not given a value` — AWS's wording for a different condition, and it names no attribute at all.
Why: a client that reads the message to discover WHICH key attribute it forgot (the common pattern when the key schema is derived at runtime, as here from a CSV header) gets no attribute name, so it cannot repair the item; and tooling that matches AWS's "One or more parameter values were invalid: Missing the key …" text does not match at all, reporting an unrecognised validation failure instead of a missing key.
Fix: the PutItem handler's key-schema validation message (the same template is shared by the other item writes that validate the key schema).
Traces: dynamodb__s3.ddb-table-per-csv-object-enforces-derived-composite-key.

## Evidence
dynamodb__s3.ddb-table-per-csv-object-enforces-derived-composite-key: "AssertionError: <table> accepted an item with no 'warehouse_id' range key (error was 'ValidationException': 'One of the required keys was not given a value') -- the key derived from 'inventory.csv' is not being enforced". The check asserts `(code, message) == ("ValidationException", f"One or more parameter values were invalid: Missing the key {header[1]} in the item")`; the emulator returned the right code and status but the message `One of the required keys was not given a value`.

## Steps to Reproduce
1. CreateTable with AttributeDefinitions/KeySchema declaring a HASH and a RANGE key (e.g. `item_id` HASH, `warehouse_id` RANGE), PAY_PER_REQUEST, and wait for ACTIVE.
2. PutItem with only the HASH attribute set.
3. Expect HTTP 400 ValidationException with `One or more parameter values were invalid: Missing the key warehouse_id in the item`; observe `One of the required keys was not given a value`.
4. Re-replay dynamodb__s3.ddb-table-per-csv-object-enforces-derived-composite-key.
