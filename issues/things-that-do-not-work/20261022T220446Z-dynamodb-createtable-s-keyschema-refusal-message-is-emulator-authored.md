Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__dynamodb.obs-dynamodb-1-cloudtrail-dynamodb-create-table-success-and-rejected-calls-are-audited
Issue-Key: cloudtrail__dynamodb.obs-dynamodb-1-cloudtrail-dynamodb-create-table-success-and-rejected-calls-are-audited|run#20|AssertionError|assert message == "Invalid KeySchema: The second KeySchemaElement is not a RANGE key type", (
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# CreateTable's KeySchema refusal message is emulator-authored instead of naming the offending KeySchemaElement

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Cause 1 of 1: CreateTable refuses a KeySchema that names two HASH keys with its own summary text rather than with DynamoDB's per-element validation message.

AWS: recorded behavior (the check's measured oracle, graded against real AWS): a CreateTable whose KeySchema lists two HASH elements is refused by the SERVICE (botocore accepts it client-side, since both elements are well-typed) with `ValidationException` and the message `Invalid KeySchema: The second KeySchemaElement is not a RANGE key type`. The rule behind that message is known and request-derived — DynamoDB validates the key schema element-by-element and names the ordinal position of the element that broke the rule and the key type it should have had — so naming the offending element is the requirement, not the literal string alone. (Smithy: CreateTable.KeySchema is a KeySchema list of KeySchemaElement{AttributeName, KeyType: HASH|RANGE} with length 1..2; ValidationException is the service-common client-input error.)
Emulator: the error code is right but the message is a hand-written summary of the whole rule, `Invalid KeySchema: a table's primary key must be one HASH attribute and at most one RANGE attribute, got 2 element(s)`, which names no element position and misreports the fault (the element count IS legal — it is the second element's key type that is not).
Why: a caller debugging a key schema is told the wrong thing — that it sent too many elements rather than that element 2 must be RANGE — so the message cannot be acted on the way AWS's can; and a client or test that asserts on DynamoDB's documented validation text (or parses it to point at an element) diverges here even though the status and code match.
Fix: the CreateTable handler's KeySchema validation in the dynamodb service crate — validate element-by-element (first element must be HASH, second, if present, must be RANGE) and word the ValidationException after the element that failed.
Traces: cloudtrail__dynamodb.obs-dynamodb-1-cloudtrail-dynamodb-create-table-success-and-rejected-calls-are-audited.

## Evidence
`AssertionError: unexpected two-HASH-keys refusal message: "Invalid KeySchema: a table's primary key must be one HASH attribute and at most one RANGE attribute, got 2 element(s)"`, raised by `assert message == "Invalid KeySchema: The second KeySchemaElement is not a RANGE key type"` (cloudtrail__dynamodb.obs-dynamodb-1-cloudtrail-dynamodb-create-table-success-and-rejected-calls-are-audited, line 226).

The check's hypothesis H3, graded against real AWS: "A CreateTable whose KeySchema names two HASH keys is refused by the SERVICE (botocore's client-side validation accepts it: both elements are well-typed) with ValidationException" and the message above. The error CODE matched; only the message diverged.

## Steps to Reproduce
1. Call `create_table(TableName='t-1', AttributeDefinitions=[{'AttributeName':'a','AttributeType':'S'},{'AttributeName':'b','AttributeType':'S'}], KeySchema=[{'AttributeName':'a','KeyType':'HASH'},{'AttributeName':'b','KeyType':'HASH'}], BillingMode='PAY_PER_REQUEST')`.
   Expect: HTTP 400 `ValidationException`, message `Invalid KeySchema: The second KeySchemaElement is not a RANGE key type`.
   Observe: HTTP 400 `ValidationException` with `Invalid KeySchema: a table's primary key must be one HASH attribute and at most one RANGE attribute, got 2 element(s)`.
