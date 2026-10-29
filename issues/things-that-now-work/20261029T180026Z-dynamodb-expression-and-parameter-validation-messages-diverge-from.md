Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.error-query-without-key-condition dynamodb.number-canonicalization-boundaries dynamodb.number-set-add-type-mismatch-rejected dynamodb.set-arithmetic-on-missing-attribute-rejected dynamodb.unused-expression-attribute-value-and-name-rejected dynamodb.update-item-add-type-mismatch dynamodb.put-item-key-type-mismatch-rejected dynamodb.deep-dynamodb-2-overlapping-document-paths-rejected dynamodb.query-limit-zero-rejected dynamodb.update-ttl-invalid-attribute
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.error-query-without-key-condition|run#10|AssertionError|assert message == "Either the KeyConditions or KeyConditionExpression parameter must be specified in the request.", (
Issue-Key: dynamodb.number-canonicalization-boundaries|run#14|AssertionError|assert message == f"1 validation error detected: The parameter cannot be converted to a numeric value: {raw}", (
Issue-Key: dynamodb.number-set-add-type-mismatch-rejected|run#8|AssertionError|assert err.get("Message") == expected, (
Issue-Key: dynamodb.set-arithmetic-on-missing-attribute-rejected|run#7|AssertionError|assert message == "The provided expression refers to an attribute that does not exist in the item", (
Issue-Key: dynamodb.unused-expression-attribute-value-and-name-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: dynamodb.update-item-add-type-mismatch|run#13|AssertionError|assert message == "An operand in the update expression has an incorrect data type", (
Issue-Key: dynamodb.put-item-key-type-mismatch-rejected|run#7|AssertionError|assert err.get("Message") == expected, (
Issue-Key: dynamodb.deep-dynamodb-2-overlapping-document-paths-rejected|run#8|AssertionError|assert msg == MSG_OVERLAP_A_B__A, f'SET a.b REMOVE a: expected {MSG_OVERLAP_A_B__A!r}, got {msg!r}'
Issue-Key: dynamodb.query-limit-zero-rejected|run#15|AssertionError|assert query_message == query_limit_message, f'unexpected Query message {query_message!r}'
Issue-Key: dynamodb|UpdateTimeToLive|ValidationException
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# DynamoDB expression and parameter validation messages diverge from AWS (10 causes)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateItem

## Description
Ten net-new root causes, all in the `dynamodb` service, in which the emulator refuses the call with the right error class but a message AWS never emits — in most cases by dropping AWS's fixed prefix or by substituting its own prose. An HTTP status, an error code and an error message are all requirements, so each of these is a defect. Each cause needs its own fix.

Cause 1 of 10: Query with neither KeyConditions nor KeyConditionExpression.
AWS: `ValidationException` (HTTP 400) with AWS's exact text for the error class. Smithy: Query.KeyConditions and Query.KeyConditionExpression are both optional members, so the requirement lives in the handler.
Emulator: `Either the KeyConditions or KeyConditionExpression parameter must be specified` — not the message AWS returns for this class.
Why: callers keyed on the service's wording mis-classify the failure.
Fix: Query validation message text.
Traces: dynamodb.error-query-without-key-condition

Cause 2 of 10: unparsable N attribute value.
AWS: `ValidationException` with AWS's exact message for a non-numeric N value.
Emulator: `One or more parameter values were invalid: The parameter cannot be converted to a numeric value: e2` (fixed part invented; `e2` is the offending value from the request).
Why: the number-canonicalisation boundary cases all report an emulator-specific message.
Fix: the AttributeValue N parser's validation message.
Traces: dynamodb.number-canonicalization-boundaries

Cause 3 of 10: ADD of a number set onto a string set.
AWS: `An operand in the update expression has an incorrect data type` — fixed text, no variable part.
Emulator: `Type mismatch for attribute to update: an ADD of a NS cannot be applied to a SS`, interpolating the two type tags.
Why: AWS deliberately does not name the types here; the emulator's message is a different error surface.
Fix: UpdateExpression ADD type-check message.
Traces: dynamodb.number-set-add-type-mismatch-rejected

Cause 4 of 10: SET arithmetic over a missing operand.
AWS: names the missing operand as an attribute that does not exist in the item, in AWS's wording for that class.
Emulator: `The provided expression refers to an attribute that does not exist in the item: c` — appends the path, which AWS's message for this class does not.
Why: same error class, different text.
Fix: UpdateExpression operand-resolution message.
Traces: dynamodb.set-arithmetic-on-missing-attribute-rejected

Cause 5 of 10: unused ExpressionAttributeValues/Names loses the validation-error prefix.
AWS: `1 validation error detected: Value provided in ExpressionAttributeValues unused in expressions: keys: {<unused keys>}` — the `1 validation error detected: ` prefix is fixed text; the key list comes from the request.
Emulator: emits the tail only, `Value provided in ExpressionAttributeValues unused in expressions: keys: {:extra}`.
Why: every client that parses the `N validation error(s) detected:` envelope sees a differently-shaped message.
Fix: the shared validation-error envelope used by expression validation.
Traces: dynamodb.unused-expression-attribute-value-and-name-rejected

Cause 6 of 10: ADD onto a non-number, non-set attribute.
AWS: AWS's exact ValidationException message for the class.
Emulator: `One or more parameter values were invalid: ADD only supports Number and set data types, and tag holds a S` — invented prose that interpolates the attribute name and its type tag.
Why: differs from AWS's wording for the error class.
Fix: UpdateExpression ADD validation message.
Traces: dynamodb.update-item-add-type-mismatch

Cause 7 of 10: key type mismatch on PutItem loses the parameter-values prefix.
AWS: `One or more parameter values were invalid: Type mismatch for key <key name> expected: <schema type> actual: <supplied type>` — the prefix and the `Type mismatch for key ... expected: ... actual: ...` frame are fixed; the key name and the two type tags come from the request and the table's key schema.
Emulator: emits only `Type mismatch for key pk expected: S actual: N`, dropping the `One or more parameter values were invalid: ` prefix.
Why: message-shape divergence for one of the most common write refusals.
Fix: PutItem key-validation message (reuse the parameter-values envelope).
Traces: dynamodb.put-item-key-type-mismatch-rejected

Cause 8 of 10: overlapping document paths are reported in the wrong order.
AWS: `1 validation error detected: Invalid UpdateExpression: Two document paths overlap with each other; must remove or rewrite one of these paths; path one: [a, b], path two: [a]` — AWS names the *deeper* path as `path one` and the shallower as `path two` (the path lists themselves come from the request).
Emulator: emits `path one: [a], path two: [a, b]`, the two swapped.
Why: the message is the only thing telling a caller which path to rewrite; swapping them points at the wrong path.
Fix: UpdateExpression overlapping-path detector's message construction.
Traces: dynamodb.deep-dynamodb-2-overlapping-document-paths-rejected

Cause 9 of 10: Query Limit=0 refusal message.
AWS: refuses Limit=0 with AWS's exact message for the class. Smithy: Query.Limit -> PositiveIntegerObject with @range min 1.
Emulator: `1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1` — not the text AWS uses for this member.
Why: differs from AWS for a plain range violation.
Fix: Query input-constraint message (member path/rendering).
Traces: dynamodb.query-limit-zero-rejected

Cause 10 of 10: UpdateTimeToLive empty AttributeName message uses the wrong member path and omits the value.
AWS: `1 validation error detected: Value '' at 'timeToLiveSpecification.attributeName' failed to satisfy constraint: Member must have length greater than or equal to 1` — the member path is lower-camel (`timeToLiveSpecification.attributeName`) and the offending value is quoted. Smithy: UpdateTimeToLive.TimeToLiveSpecification is required.
Emulator: `1 validation error detected: Value at 'TimeToLiveSpecification.AttributeName' failed to satisfy constraint: Member must have length greater than or equal to 1` — upper-camel path and no `'<value>'`.
Why: constraint messages are produced from the modeled member path; getting the casing and the value rendering wrong affects every such message on this operation.
Fix: UpdateTimeToLive validation / the constraint-message serializer's member-path rendering.
Traces: dynamodb.update-ttl-invalid-attribute

## Evidence
Replay details, one per cause:
- error-query-without-key-condition: AssertionError: expected the exact ValidationException message AWS returns, got 'Either the KeyConditions or KeyConditionExpression parameter must be specified'.
- number-canonicalization-boundaries: expected AWS's exact ValidationException message, got 'One or more parameter values were invalid: The parameter cannot be converted to a numeric value: e2'.
- number-set-add-type-mismatch-rejected: expected 'An operand in the update expression has an incorrect data type', got 'Type mismatch for attribute to update: an ADD of a NS cannot be applied to a SS'.
- set-arithmetic-on-missing-attribute-rejected: the refusal's message is 'The provided expression refers to an attribute that does not exist in the item: c'; real AWS names the missing operand as an attribute that does not exist in the item.
- unused-expression-attribute-value-and-name-rejected: expected '1 validation error detected: Value provided in ExpressionAttributeValues unused in expressions: keys: {:extra}', got 'Value provided in ExpressionAttributeValues unused in expressions: keys: {:extra}'.
- update-item-add-type-mismatch: expected AWS's exact ValidationException message, got 'One or more parameter values were invalid: ADD only supports Number and set data types, and tag holds a S'.
- put-item-key-type-mismatch-rejected: expected 'One or more parameter values were invalid: Type mismatch for key pk expected: S actual: N', got 'Type mismatch for key pk expected: S actual: N'.
- deep-dynamodb-2-overlapping-document-paths-rejected: SET a.b REMOVE a: expected '... path one: [a, b], path two: [a]', got '... path one: [a], path two: [a, b]'.
- query-limit-zero-rejected: unexpected Query message "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1".
- update-ttl-invalid-attribute: expected "1 validation error detected: Value '' at 'timeToLiveSpecification.attributeName' failed to satisfy constraint: Member must have length greater than or equal to 1", got "1 validation error detected: Value at 'TimeToLiveSpecification.AttributeName' failed to satisfy constraint: Member must have length greater than or equal to 1".

## Steps to Reproduce
Boot the emulator and replay: dynamodb.error-query-without-key-condition, dynamodb.number-canonicalization-boundaries, dynamodb.number-set-add-type-mismatch-rejected, dynamodb.set-arithmetic-on-missing-attribute-rejected, dynamodb.unused-expression-attribute-value-and-name-rejected, dynamodb.update-item-add-type-mismatch, dynamodb.put-item-key-type-mismatch-rejected, dynamodb.deep-dynamodb-2-overlapping-document-paths-rejected, dynamodb.query-limit-zero-rejected, dynamodb.update-ttl-invalid-attribute. Each drives one malformed request and compares the returned Error.Code and Error.Message with AWS's.
