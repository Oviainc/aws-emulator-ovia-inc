Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-data-exports-export-resource-not-found-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-data-exports-export-resource-not-found-errors bcm-data-exports-table-catalog-inspection
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports|TagResource|4xx->4xx|$.Message
Issue-Key: bcm-data-exports|TagResource|4xx->4xx|status
Issue-Key: bcm-data-exports|GetTable|4xx->4xx|$.Message
Issue-Commit: 8beaefc9c507e60dc8fbd7bca4267c7f5166e2dd

# TagResource not-found answers 404 and GetTable unknown-table message diverges

> _Note: this issue was identified against the code base at commit `8beaefc9c507e60dc8fbd7bca4267c7f5166e2dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two recorded client-error paths that the existing open issue 20261022T061241Z-bcm-data-exports-export-lifecycle-errors-validation-and-tag-state-diverge.md does NOT cover (its replay keys name GetExport, DeleteExport, UpdateExport, ListTagsForResource and CreateExport only, and the replay reports no owner for either cluster below). 2 causes.

Cause 1 of 2: TagResource against an export ARN that does not exist answers HTTP 404 instead of 400.
AWS: recorded behavior - the recording asserts HTTP 400 for tagging a non-existent export ARN. Smithy TagResource (input ResourceArn required, @pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):...$, ResourceTags required) models errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(@httpError 404), ThrottlingException(429) and ValidationException(@httpError 400, Message required). Model and recorded wire disagree on the status, and the recorded wire is the contract being replayed: this service collapses the not-found condition onto 400, exactly as the sibling operations in the open lifecycle issue do.
Emulator: HTTP 404. The body text also differs (recorded Message 'Resource not found' vs emulator 'No matching definition found'); the status is the requirement, the message text is quoted only as an example since no rule for it is documented.
Why: SDKs, CLIs and gateways branch on the HTTP status before the __type, so a 404 where AWS sends 400 makes a deterministic client error look like a routing/404 failure and can change retry behavior. A client tagging an export it just failed to create takes the wrong branch.
Fix: the same error-to-status mapping in this service's error serializer, on the TagResource path (the lifecycle issue fixes GetExport/DeleteExport/UpdateExport/ListTagsForResource; TagResource is a separate recorded step and must be re-replayed on its own).
Traces: bcm-data-exports-export-resource-not-found-errors#task#1.

Cause 2 of 2: GetTable for an unknown table name returns the right status but a message built to a different rule.
AWS: Smithy GetTable errors = InternalServerException(500), ThrottlingException(429), ValidationException(@httpError 400, Message required). The recording shows AWS building the Message from the request's TableName with the rule '<TableName> not found' - recorded example 'Table NOT_A_REAL_TABLE_XYZ not found'. The request-derived rule is the requirement, not the recorded sentence.
Emulator: same HTTP 400, but Message 'Table NOT_A_REAL_TABLE_XYZ is not supported. Call ListTables for the tables available in data exports.' - a not-supported/advice form rather than the not-found form AWS derives.
Why: callers and test suites that classify a ValidationException by matching the not-found phrasing of the message (the only discriminator here, since the __type and status are shared with every other validation failure) mis-classify an unknown table as something else.
Fix: the GetTable handler's unknown-table validation message (render '<Table> not found' from the requested TableName); keep the 400 ValidationException.
Traces: bcm-data-exports-table-catalog-inspection#task#3.

## Evidence
Replayed trace set simparity-wire-aws-v1 against the emulator booted from the mainline checkout: emulator_up=true, state_reset_failures=0, bootstrap_signal=false, 10 fixtures replayed, 49 steps (18 passed, 24 mismatched, 7 blocked), score 36.7%.

Cause 1 - bcm-data-exports-export-resource-not-found-errors#task#1, TagResource: expected_status 400, actual_status 404. detail: 'status: expected 400, got 404' and '$.Message: value_mismatch (expected \'Resource not found\', got \'No matching definition found\')'. divergence_key service=bcm-data-exports|TagResource|status=400->404|keys=$.Message,status; owned_by=[] and stub_path='' (not covered by the open lifecycle issue, whose Issue-Keys list GetExport/DeleteExport/UpdateExport/ListTagsForResource/CreateExport only).
Cause 2 - bcm-data-exports-table-catalog-inspection#task#3, GetTable: expected_status 400, actual_status 400. detail: '$.Message: value_mismatch (expected \'Table NOT_A_REAL_TABLE_XYZ not found\', got \'Table NOT_A_REAL_TABLE_XYZ is not supported. Call ListTables for the tables available in data exports.\')'. divergence_key service=bcm-data-exports|GetTable|status=400->400|keys=$.Message; owned_by=[] and stub_path=''.

Model consulted: services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/smithy-model - TagResource errors {AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400}, ResourceArn @pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$; GetTable errors {InternalServerException 500, ThrottlingException 429, ValidationException 400}; ValidationException{Message required, Reason, Fields} @httpError 400.

## Steps to Reproduce
1. git clone the mainline checkout and boot the emulator from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture bcm-data-exports-export-resource-not-found-errors step task#1 - TagResource against an export ARN that was never created; observe HTTP 404 where the recording expects 400.
4. Cause 2: replay fixture bcm-data-exports-table-catalog-inspection step task#3 - GetTable with TableName NOT_A_REAL_TABLE_XYZ; the status is 400 as expected but $.Message is the 'is not supported. Call ListTables...' sentence instead of the request-derived '<Table> not found' form.
