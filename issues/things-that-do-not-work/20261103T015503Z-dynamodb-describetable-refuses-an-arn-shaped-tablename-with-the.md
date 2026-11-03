Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodb|DescribeTable|4xx->4xx|$.message
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# DescribeTable refuses an ARN-shaped TableName with the plain-name pattern clause instead of the ARN-format clause

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
Cause 1 of 1 - DescribeTable given an ARN whose resource segment is a path (table/<name>/stream/<label>) rejects it with the wrong constraint clause.
AWS: Smithy: DescribeTable.TableName is @required and targets TableArn with @length min 1 max 1024 and no modeled pattern - the member accepts either a bare table name or a table ARN, so the ARN-format rule is service-side and is recorded behavior only. The recording shows HTTP 400 with the message "1 validation error detected: Value '<submitted value>' at 'tableName' failed to satisfy constraint: Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<subscriber>:resourceType/resourceName', where Resource name must have length less than or equal to 255, Resource name must have length greater than or equal to 3, Resource name must satisfy regular expression pattern: [a-zA-Z0-9_.-]+". Fixed text: the 'N validation error detected: ... failed to satisfy constraint: ' envelope and the whole 'Valid ARN format is ...' clause, including the three Resource-name sub-clauses and their limits. Variable parts: the wire member name ('tableName') and - for this error class AWS does echo it - the rejected value taken from the request. Note the clause blames the ARN's RESOURCE NAME, not the whole value.
Emulator: HTTP 400 with the correct envelope and value but the plain-table-name clause: "... failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+". It validated the whole ARN string as if it were a bare table name instead of parsing it as an ARN and validating the resource segment.
Why: the status and error class are right, but the caller is told its entire identifier failed a table-name character pattern - which is false for a well-formed ARN - rather than that the ARN's resource name is unacceptable. A client or SDK test matching the documented ARN-format refusal text sees a sentence AWS never emits, and a caller debugging a mis-built ARN gets no indication that ARN parsing is what rejected it.
Fix: DescribeTable's input validation for TableName - when the value is ARN-shaped, parse it and validate the resourceType/resourceName segment, emitting the ARN-format constraint clause; keep the bare-name pattern clause only for non-ARN values.
Traces: dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs#task#2

Related but not covering this: open issue 20261030T060227Z-dynamodb-validationexception-messages-diverge-from-the-wording-aws.md collects other DynamoDB ValidationException wording defects; this is the DescribeTable ARN-vs-name clause selection and is not among them (the replay reports no owning issue for this cluster).

## Evidence
Fixture dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs, step task#2, operation DescribeTable (service dynamodb), status 400 on both sides, trace set simparity-wire-aws-v1.

$.message value_mismatch:
expected "1 validation error detected: Value '<arn with table/<name>/stream/<label> resource path>' at 'tableName' failed to satisfy constraint: Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<subscriber>:resourceType/resourceName', where Resource name must have length less than or equal to 255, Resource name must have length greater than or equal to 3, Resource name must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
got "1 validation error detected: Value '<same value>' at 'tableName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"

Model citation: DescribeTable.TableName @required, target TableArn, @length 1..1024, no @pattern; errors InternalServerError / InvalidEndpointException / ResourceNotFoundException.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Call DynamoDB DescribeTable with TableName set to a stream-style ARN, i.e. an ARN whose resource segment is a path such as arn:aws:dynamodb:<region>:<account>:table/<table>/stream/<label> (fixture dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs, step task#2).
3. Compare $.message: the response must carry the "Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<subscriber>:resourceType/resourceName', where Resource name must ..." constraint clause; the emulator instead returns the bare-name "Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+" clause.
