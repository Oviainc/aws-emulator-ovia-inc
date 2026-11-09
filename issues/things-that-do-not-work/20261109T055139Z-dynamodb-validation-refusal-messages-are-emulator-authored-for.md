Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-csvtraj-create-table-enum-capacity-and-projection-mistakes
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-csvtraj-create-table-enum-capacity-and-projection-mistakes dynamodb-csvtraj-every-csv-column-declared-as-attribute dynamodb-csvtraj-single-column-csv-hash-reused-as-range dynamodb-csvtraj-table-name-derived-from-object-key dynamodb-multiple-simultaneous-constraint-violations-rejected dynamodb-vvd-traj-create-table-billing-mode-mistakes dynamodb-vvd-traj-create-table-key-schema-mistakes dynamodb-expression-attribute-value-empty-rejected dynamodb-malformed-n-in-key-top-level-not-cancellation
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateTable|4xx->4xx|$.message
Issue-Key: dynamodb|DeleteItem|4xx->4xx|$.message
Issue-Key: dynamodb|BatchGetItem|4xx->4xx|$.message
Issue-Commit: bdb5184d827fec951c886c257ec0292728205109

# dynamodb validation refusal messages are emulator-authored for three request-shape errors

> _Note: this issue was identified against the code base at commit `bdb5184d827fec951c886c257ec0292728205109`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Three net-new ValidationException wording causes. In all three the HTTP status (400) and the error code (ValidationException) match; only the message diverges, and the message is a requirement. All three must be closed before submitting.

Cause 1 of 3: CreateTable with BillingMode PROVISIONED and no provisioned throughput names the wrong parameters.
AWS: Smithy: CreateTable input has BillingMode (enum BillingMode) and an optional ProvisionedThroughput member, so the "both capacity members are required under PROVISIONED" rule is enforced at runtime, not by the model - the recorded 400 is a ValidationException with message 'One or more parameter values were invalid: ReadCapacityUnits and WriteCapacityUnits must both be specified when BillingMode is PROVISIONED'. The whole message is fixed text; the only variable part of this template is the billing-mode token named at the end.
Emulator: 400 ValidationException with message 'One or more parameter values were invalid: ProvisionedThroughput must be specified when BillingMode is PROVISIONED' - it names the container member instead of the two capacity members AWS names.
Why: a client (or an agent-driven CLI retry loop) that reads the message to learn which members to supply is told to add ProvisionedThroughput rather than ReadCapacityUnits and WriteCapacityUnits, and message-matching negative tests do not recognise the refusal.
Fix: the CreateTable validation component's message for the PROVISIONED billing-mode guard.
Traces: dynamodb-csvtraj-create-table-enum-capacity-and-projection-mistakes#task#3 (cluster also covers #task#4, dynamodb-csvtraj-every-csv-column-declared-as-attribute#task#1, dynamodb-csvtraj-single-column-csv-hash-reused-as-range#task#1, dynamodb-csvtraj-table-name-derived-from-object-key#task#3, dynamodb-multiple-simultaneous-constraint-violations-rejected#task#2, dynamodb-vvd-traj-create-table-billing-mode-mistakes#task#1/#task#2/#task#3, dynamodb-vvd-traj-create-table-key-schema-mistakes#task#1/#task#2/#task#4/#task#5).

Cause 2 of 3: an empty AttributeValue in ExpressionAttributeValues is refused without the ExpressionAttributeValues context or the offending placeholder name.
AWS: recorded behavior only for the wording - the recorded 400 ValidationException message is 'ExpressionAttributeValues contains invalid value: Supplied AttributeValue is empty, must contain exactly one of the supported datatypes for key <placeholder>'. Fixed text: 'ExpressionAttributeValues contains invalid value: Supplied AttributeValue is empty, must contain exactly one of the supported datatypes for key '; the single variable part is the expression-attribute-value placeholder name taken from the request's ExpressionAttributeValues map.
Emulator: 400 ValidationException with message 'Supplied AttributeValue is empty, must contain exactly one of the supported datatypes' - it drops both the ExpressionAttributeValues prefix and the placeholder name, so the message no longer says which value was bad.
Why: a caller with several expression values cannot tell which placeholder to fix, and text-matching clients diverge.
Fix: the expression-attribute-value validation in the item-operation (DeleteItem and the shared expression-value path) validation component - carry the failing placeholder key into the message.
Traces: dynamodb-expression-attribute-value-empty-rejected#task#6 (cluster also covers #task#7).

Cause 3 of 3: BatchGetItem prefixes a numeric-conversion refusal with a validation-error count AWS does not use for this error class.
AWS: recorded behavior only for the wording - the recorded 400 ValidationException message for a key attribute whose N value is not a number is 'The parameter cannot be converted to a numeric value: <value>', with no '1 validation error detected: ' prefix. Fixed text: 'The parameter cannot be converted to a numeric value: '; the variable part is the offending literal echoed from the request key.
Emulator: 400 ValidationException with message '1 validation error detected: The parameter cannot be converted to a numeric value: <value>' - it adds the counted-validation-error prefix AWS reserves for its constraint-violation message class.
Why: clients that match the refusal text, and users reading it, see a message in the wrong class; it also makes the emulator inconsistent between the two message families.
Fix: the BatchGetItem key-validation error formatter - the numeric-conversion refusal must not go through the counted 'N validation error(s) detected' wrapper.
Traces: dynamodb-malformed-n-in-key-top-level-not-cancellation#task#7.

Related open issues, which do NOT carry these replay keys: 20261030T060227Z-dynamodb-validationexception-messages-diverge-from-the-wording-aws.md and 20261108T205321Z-dynamodb-7-net-new-validation-causes-conditionaloperator-refusal.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 (242 fixtures, 2631 steps, score 90.5%); 3 of the 16 net-new divergence clusters, all 400->400 with matching error codes and diverging $.message:

1. dynamodb-csvtraj-create-table-enum-capacity-and-projection-mistakes#task#3 CreateTable - expected 'One or more parameter values were invalid: ReadCapacityUnits and WriteCapacityUnits must both be specified when BillingMode is PROVISIONED', got 'One or more parameter values were invalid: ProvisionedThroughput must be specified when BillingMode is PROVISIONED'. 13 steps affected.
2. dynamodb-expression-attribute-value-empty-rejected#task#6 DeleteItem - expected 'ExpressionAttributeValues contains invalid value: Supplied AttributeValue is empty, must contain exactly one of the supported datatypes for key <placeholder>', got 'Supplied AttributeValue is empty, must contain exactly one of the supported datatypes'. 2 steps affected.
3. dynamodb-malformed-n-in-key-top-level-not-cancellation#task#7 BatchGetItem - expected 'The parameter cannot be converted to a numeric value: <value>', got '1 validation error detected: The parameter cannot be converted to a numeric value: <value>'.

Model evidence: smithy_query operation CreateTable on services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json shows BillingMode as an enum member and ProvisionedThroughput as an optional member with no model-level conditional requirement, so the rule and its message are handler-side.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: fixture dynamodb-csvtraj-create-table-enum-capacity-and-projection-mistakes, step task#3 - CreateTable with BillingMode=PROVISIONED and no ProvisionedThroughput; compare the ValidationException message.
4. Cause 2: fixture dynamodb-expression-attribute-value-empty-rejected, step task#6 - DeleteItem with a ConditionExpression whose ExpressionAttributeValues entry is an empty AttributeValue ({}); compare the message.
5. Cause 3: fixture dynamodb-malformed-n-in-key-top-level-not-cancellation, step task#7 - BatchGetItem whose request key carries an N attribute with a non-numeric literal; compare the message prefix.
