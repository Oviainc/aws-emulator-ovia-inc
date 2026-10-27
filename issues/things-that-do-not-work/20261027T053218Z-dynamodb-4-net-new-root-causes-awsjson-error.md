Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.awsjson-error-code-carrier-is-body-only-rejected dynamodb.condition-size-counts-string-length-not-bytes dynamodb.table-arn-addresses-a-table-wherever-a-name-does dynamodb.update-item-mixing-attribute-updates-and-expression-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.awsjson-error-code-carrier-is-body-only-rejected|run#29|AssertionError|assert headers.get("x-amz-crc32"), (
Issue-Key: dynamodb|PutItem|ConditionalCheckFailedException
Issue-Key: dynamodb.table-arn-addresses-a-table-wherever-a-name-does|run#24|AssertionError|assert message == expected_message, (
Issue-Key: dynamodb.update-item-mixing-attribute-updates-and-expression-rejected|run#9|AssertionError|assert err.get('Message') == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Commit: a55c69b9a776d6a2a8a52ff1d4d449819274859a

# dynamodb: 4 net-new root causes — awsJson error responses carry no x-amz-crc32, size() on a String misjudges a condition, and two ValidationException messages are not AWS's text

> _Note: this issue was identified against the code base at commit `a55c69b9a776d6a2a8a52ff1d4d449819274859a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
REOPENS 20261025T024002Z-dynamodb-10-net-new-root-causes-validation-that.md: its fix left the call refused. That resolved issue carried dynamodb.awsjson-error-code-carrier-is-body-only-rejected, dynamodb.condition-size-counts-string-length-not-bytes and dynamodb.table-arn-addresses-a-table-wherever-a-name-does; all three still fail, each now one assertion further on. This issue bundles 4 root causes in the dynamodb data plane. A fixer must close ALL FOUR before submitting.

=== Cause 1 of 4: awsJson 1.0 error responses carry no x-amz-crc32 header ===
AWS: recorded behavior only — the Smithy model describes no HTTP response headers for awsJson1_0 (DescribeTable's ResourceNotFoundException is httpError 400). Measured against real DynamoDB, EVERY response on the awsJson1_0 route — success and error alike — carries an x-amz-crc32 header whose value is the CRC32 of the response body, and botocore validates it. The recorded refusal is HTTP 400 with body {"__type":"com.amazonaws.dynamodb.v20120810#ResourceNotFoundException","message":...} AND an x-amz-crc32 header; it carries NO x-amzn-errortype header.
Emulator: the 400 refusal body and __type are now correct and no x-amzn-errortype is added (the parts the earlier fix did land), but the response headers are only content-type, content-length and date — x-amz-crc32 is absent on the error path.
Why: a client configured to validate response checksums (botocore's default on this service) raises a checksum/parse failure instead of surfacing the modeled ResourceNotFoundException, so negative-path handling breaks on every refusal; a proxy that forwards only checksummed responses drops them.
Fix: the service's awsJson error-response writer (the shared HTTP error serializer for the dynamodb crate, not any single operation handler) must emit x-amz-crc32 over the serialized error body exactly as the success path does.
Traces: dynamodb.awsjson-error-code-carrier-is-body-only-rejected.

=== Cause 2 of 4: size() on a String attribute in a ConditionExpression measures the wrong unit, so a satisfied condition is refused ===
AWS: recorded behavior only for the unit — Smithy models PutItem.ConditionExpression and PutItem's ConditionalCheckFailedException (client error, httpError 400) but says nothing about size() semantics. Measured against real DynamoDB, size() applied to a String counts the string's CHARACTERS: for the 3-character, 7-UTF-8-byte value U+00E9 U+03B1 U+4E2D, ConditionExpression 'size(s) = :n' with :n = {"N":"3"} is SATISFIED and PutItem answers HTTP 200. (Note: the resolved issue above described this as UTF-8 bytes; the recording says characters, and the recording is the requirement.)
Emulator: the same PutItem is refused HTTP 400 ConditionalCheckFailedException, message 'The conditional request failed' — the comparison evaluated to the opposite truth value, i.e. the implementation is still measuring a unit other than characters.
Why: a correct conditional write guarded by size() on any non-ASCII string never lands and the client sees a spurious conditional failure; optimistic-concurrency and length-guard patterns silently stop working for multi-byte data.
Fix: the size() function in the condition-expression evaluator (dynamodb crate's expression/condition function implementation) must return the character count for S-typed attributes.
Traces: dynamodb.condition-size-counts-string-length-not-bytes.

=== Cause 3 of 4: the malformed-ARN TableName ValidationException message is truncated — it omits AWS's resource-name constraint clause and renames two ARN segments ===
AWS: Smithy — GetItem/PutItem/UpdateItem.TableName targets TableArn, required, @length 1..1024, so an ARN is a legal spelling of TableName; the refusal is the service-common ValidationException at HTTP 400, whose message text is not modeled (recorded behavior only). Recorded, an ARN with an empty vendor segment is refused with the message: "1 validation error detected: Value '<the TableName exactly as sent>' at 'tableName' failed to satisfy constraint: Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<subscriber>:resourceType/resourceName', where Resource name must have length less than or equal to 255, Resource name must have length greater than or equal to 3, Resource name must satisfy regular expression pattern: [a-zA-Z0-9_.-]+". Fixed text: everything except the quoted value; variable: only the offending TableName copied verbatim from the request.
Emulator: the code is ValidationException (correct) but the message stops at the format template and uses different segment names: "1 validation error detected: Value '<value>' at 'tableName' failed to satisfy constraint: Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<awsAccountId>:<resourceType>/<resourceId>'" — '<subscriber>' is rendered '<awsAccountId>', 'resourceType/resourceName' is rendered '<resourceType>/<resourceId>', and the three 'where Resource name must ...' clauses are missing entirely.
Why: clients and test suites that match on AWS's validation text (and humans reading it) get a message that neither names the real constraint nor tells them the 3..255 / [a-zA-Z0-9_.-]+ rule the resource name must satisfy; the two plane spellings of the same refusal diverge from AWS's wording contract.
Fix: the TableName/ARN parser's message template in the shared item-operation request validation of the dynamodb crate (it must be used identically by GetItem and PutItem, both of which the check exercises).
Traces: dynamodb.table-arn-addresses-a-table-wherever-a-name-does.

=== Cause 4 of 4: the expression/non-expression parameter-collision ValidationException message omits AWS's '1 validation error detected: ' prefix ===
AWS: Smithy — UpdateItem accepts both the legacy AttributeUpdates map and UpdateExpression as optional members, and the refusal for sending both is the service-common ValidationException at HTTP 400 (message text not modeled; recorded behavior only). Recorded message: "1 validation error detected: Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {AttributeUpdates} Expression parameters: {UpdateExpression}". Fixed text: the whole sentence including the 'N validation error(s) detected: ' prefix; variable: only the parameter names listed in the two brace groups, taken from the request.
Emulator: the code is ValidationException (correct) and the body text matches from 'Can not use both ...' onward, but the leading '1 validation error detected: ' is missing: "Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {AttributeUpdates} Expression parameters: {UpdateExpression}".
Why: every DynamoDB validation refusal is prefixed with the detected-error count; dropping it on this class makes the message inconsistent with the service's other refusals and breaks clients/tests that parse or match the prefix.
Fix: the mutually-exclusive expression/non-expression parameter validator on the UpdateItem path (dynamodb crate) must emit the message through the same validation-error formatter that prefixes the count, as the other validation refusals do.
Traces: dynamodb.update-item-mixing-attribute-updates-and-expression-rejected.

## Evidence
Cause 1 — dynamodb.awsjson-error-code-carrier-is-body-only-rejected, raw signed POST of DynamoDB_20120810.DescribeTable for an absent table: status 400 and body __type/message asserted OK, then AssertionError: "the error response carried no x-amz-crc32 header: {'content-type': 'application/x-amz-json-1.0', 'content-length': '171', 'date': '...'} -- DynamoDB checksums error bodies too, and botocore validates it".

Cause 2 — dynamodb.condition-size-counts-string-length-not-bytes: put_item(Item={'pk':{'S':'a'},'s':{'S':'\u00e9\u03b1\u4e2d'},'ok':{'N':'1'}}, ConditionExpression='size(s) = :n', ExpressionAttributeValues={':n':{'N':'3'}}) raised botocore.errorfactory.ConditionalCheckFailedException: An error occurred (ConditionalCheckFailedException) when calling the PutItem operation: The conditional request failed. Expected: HTTP 200 and the item carrying ok={'N':'1'}.

Cause 3 — dynamodb.table-arn-addresses-a-table-wherever-a-name-does: GetItem with TableName = the table ARN with an empty vendor segment.
expected message: "1 validation error detected: Value '<arn>' at 'tableName' failed to satisfy constraint: Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<subscriber>:resourceType/resourceName', where Resource name must have length less than or equal to 255, Resource name must have length greater than or equal to 3, Resource name must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
got message:      "1 validation error detected: Value '<arn>' at 'tableName' failed to satisfy constraint: Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<awsAccountId>:<resourceType>/<resourceId>'"
(The same message is asserted for PutItem with the same TableName. The earlier legs of the check — item written by name and read by ARN, Query/DescribeTable/DeleteItem by ARN — now pass.)

Cause 4 — dynamodb.update-item-mixing-attribute-updates-and-expression-rejected: update_item(UpdateExpression='SET x = :v', AttributeUpdates={'y': {...}}) → code ValidationException (correct).
expected message: '1 validation error detected: Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {AttributeUpdates} Expression parameters: {UpdateExpression}'
got message:      'Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {AttributeUpdates} Expression parameters: {UpdateExpression}'

## Steps to Reproduce
Replay the four behavioral checks listed in trace_ids against the emulator:
1. dynamodb.awsjson-error-code-carrier-is-body-only-rejected — create a table, then send a raw SigV4-signed POST with x-amz-target DynamoDB_20120810.DescribeTable for an absent table name and inspect the response headers for x-amz-crc32.
2. dynamodb.condition-size-counts-string-length-not-bytes — create a PAY_PER_REQUEST table with hash key pk (S); put {'pk':'a','s':'\u00e9\u03b1\u4e2d'}; put again with ConditionExpression 'size(s) = :n', :n = 3; expect HTTP 200 and read back ok=1.
3. dynamodb.table-arn-addresses-a-table-wherever-a-name-does — describe the table to get its ARN, blank out the vendor segment, and call GetItem and PutItem with that string as TableName; compare the ValidationException message to AWS's text.
4. dynamodb.update-item-mixing-attribute-updates-and-expression-rejected — call UpdateItem with both UpdateExpression and AttributeUpdates and compare the ValidationException message to AWS's text.
