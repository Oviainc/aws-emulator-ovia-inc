Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-dataset-group-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-dataset-group-lifecycle personalize-dataset-lifecycle-and-validation
Issue-Shape-Id: com.amazonaws.personalize#AmazonPersonalize
Issue-Key: personalize|CreateDatasetGroup|4xx->4xx|$.message
Issue-Key: personalize|CreateDatasetGroup|4xx->4xx|status
Issue-Key: personalize|DescribeDatasetGroup|4xx->4xx|$.message
Issue-Key: personalize|DescribeDatasetGroup|4xx->4xx|status
Issue-Key: personalize|CreateDataset|4xx->4xx|$.message
Issue-Key: personalize|CreateDataset|4xx->4xx|status
Issue-Key: personalize|DescribeDataset|4xx->4xx|$.message
Issue-Key: personalize|DescribeDataset|4xx->4xx|status
Issue-Commit: b4a0d642bb551c1144d98d8c876f3d4bc2002490

# Personalize resource errors: wrong HTTP status and non-AWS messages

> _Note: this issue was identified against the code base at commit `b4a0d642bb551c1144d98d8c876f3d4bc2002490`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDatasetGroup

## Description
FOUR causes, all one fault family: the personalize dataset/dataset-group handlers build their resource errors with the Smithy-modeled httpError status (403 for ResourceAlreadyExistsException, 404 for ResourceNotFoundException) and with emulator-invented message text, whereas the recorded Amazon Personalize wire responses carry HTTP 400 for these same awsJson1_1 errors and a fixed AWS message template. Every cause below diverges on BOTH the status line and $.message; the error code ($.__type) is already correct in all four.

Cause 1 of 4: CreateDatasetGroup, duplicate dataset group.
AWS: recorded behavior — HTTP 400 with __type ResourceAlreadyExistsException and message 'Another resource with Arn <arn> already exists.'. Fixed text is 'Another resource with Arn ' + ' already exists.'; the only variable part is the ARN of the colliding dataset group, taken from the request's name resolved against the caller's account/region. Smithy: CreateDatasetGroup errors InvalidInputException/LimitExceededException/ResourceAlreadyExistsException/TooManyTagsException; the model annotates ResourceAlreadyExistsException @httpError 403, which the recording shows is NOT the status the live service serves for this operation.
Emulator: HTTP 403 with message 'Dataset group <arn> already exists.' — a bespoke sentence that names the resource kind instead of AWS's Arn-generic wording.
Why: a client that branches on HTTP class (or on a 403/AccessDenied-style path) treats a benign name collision as an authorization failure and stops retrying under a new name; a client that matches the message to report 'already exists' sees text AWS never emits.
Fix: the CreateDatasetGroup handler's ResourceAlreadyExistsException construction plus the service's modeled-error-to-HTTP-status mapping for this error shape.

Cause 2 of 4: DescribeDatasetGroup, ARN with no dataset group behind it.
AWS: recorded behavior — HTTP 400, __type ResourceNotFoundException, message 'The given dataset group does not exist: <arn>'. Fixed text is 'The given dataset group does not exist: '; the variable part is the datasetGroupArn copied from the request. Smithy: DescribeDatasetGroup errors InvalidInputException (400) and ResourceNotFoundException (modeled @httpError 404, again not the recorded status).
Emulator: HTTP 404 with the generic message 'Resource not found: <arn>', which does not say which kind of resource was looked up.
Why: client code distinguishing 'bad request' from 'not found' by status takes the wrong branch, and operators lose the resource-kind wording AWS uses to disambiguate a missing group from a missing dataset or schema.
Fix: the DescribeDatasetGroup handler's not-found path and the shared status mapping.

Cause 3 of 4: CreateDataset naming a datasetGroupArn that does not exist.
AWS: recorded behavior — HTTP 400, __type ResourceNotFoundException, message 'The given dataset group does not exist: <arn>' (same template as cause 2, variable part = the request's datasetGroupArn). Smithy: CreateDataset models ResourceNotFoundException (@httpError 404) among its errors.
Emulator: HTTP 404 with 'Resource not found: <arn>' — services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_dataset.rs has one shared not_found(arn) helper that emits that one generic sentence for BOTH an unknown dataset group and an unknown schema ARN, so neither matches AWS and the two cases are indistinguishable.
Why: a client cannot tell from the reply whether its schemaArn or its datasetGroupArn was the unresolvable one, and the status class is wrong.
Fix: the CreateDataset handler's not-found construction — one message per looked-up resource kind, in AWS's wording — plus the status mapping.

Cause 4 of 4: DescribeDataset on a dataset ARN that does not exist.
AWS: recorded behavior — HTTP 400, __type ResourceNotFoundException, message 'The given dataset does not exist: <arn>'. Fixed text 'The given dataset does not exist: '; variable part = the request's datasetArn. Smithy: DescribeDataset errors InvalidInputException (400), ResourceNotFoundException (@httpError 404).
Emulator: HTTP 404 with 'Resource not found: <arn>'.
Why: same as cause 2 — wrong status class and a message that is not AWS's, so not-found handling keyed on either one misfires.
Fix: the DescribeDataset handler's not-found path and the shared status mapping.

Note the modeled @httpError values are what the emulator is faithfully reproducing; the recorded wire is the authority here, so the fix is a per-service override of the serialized status for these error shapes, not a model edit. An open issue, 20261029T190605Z-personalize-deletedatasetgroup-returns-a-bespoke-resourceinuseexception-message-instead-of.md, reports the same bespoke-message habit for DeleteDatasetGroup/ResourceInUseException; it does not cover any of the four causes here.

Traces: personalize-dataset-group-lifecycle#task#3 (CreateDatasetGroup), personalize-dataset-group-lifecycle#task#5 (DescribeDatasetGroup), personalize-dataset-lifecycle-and-validation#task#6 (CreateDataset), personalize-dataset-lifecycle-and-validation#task#8 (DescribeDataset). Source: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_dataset_group.rs:209-218, src/ops/create_dataset.rs:189-191, 210-212, 277-284.

## Evidence
Cause 1 — personalize-dataset-group-lifecycle#task#3, CreateDatasetGroup: expected status 400, got 403; $.message expected 'Another resource with Arn arn:aws:personalize:us-west-2:000000000000:dataset-group/<name> already exists.', got 'Dataset group arn:aws:personalize:us-west-2:000000000000:dataset-group/<name> already exists.'
Cause 2 — personalize-dataset-group-lifecycle#task#5, DescribeDatasetGroup: expected 400, got 404; $.message expected 'The given dataset group does not exist: <datasetGroupArn>', got 'Resource not found: <datasetGroupArn>'.
Cause 3 — personalize-dataset-lifecycle-and-validation#task#6, CreateDataset: expected 400, got 404; $.message expected 'The given dataset group does not exist: <datasetGroupArn>', got 'Resource not found: <datasetGroupArn>'.
Cause 4 — personalize-dataset-lifecycle-and-validation#task#8, DescribeDataset: expected 400, got 404; $.message expected 'The given dataset does not exist: <datasetArn>', got 'Resource not found: <datasetArn>'.
In all four the ARNs matched exactly and $.__type matched; only the status line and the message text diverged. Smithy (com-amazonaws-personalize-amazonpersonalize.json): ResourceAlreadyExistsException @httpError 403, ResourceNotFoundException @httpError 404, InvalidInputException @httpError 400 — the emulator serves the modeled numbers, the recording shows 400.

## Steps to Reproduce
1. Replay fixture personalize-dataset-group-lifecycle: CreateDatasetGroup with a name, then CreateDatasetGroup again with the SAME name -> step task#3 answers 403 'Dataset group ... already exists.' instead of 400 'Another resource with Arn ... already exists.'
2. Same fixture, task#5: DescribeDatasetGroup for a dataset-group ARN that was deleted/never created -> 404 'Resource not found: ...' instead of 400 'The given dataset group does not exist: ...'
3. Replay fixture personalize-dataset-lifecycle-and-validation, task#6: CreateDataset whose datasetGroupArn names no group -> 404 'Resource not found: ...' instead of 400 'The given dataset group does not exist: ...'
4. Same fixture, task#8: DescribeDataset for a dataset ARN that holds no dataset -> 404 'Resource not found: ...' instead of 400 'The given dataset does not exist: ...'
