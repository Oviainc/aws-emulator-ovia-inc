Issue-Source: trace-sweep-divergence
Issue-Trace: cleanroomsml-get-operations-not-found
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanroomsml-get-operations-not-found cleanroomsml-training-dataset-lifecycle cleanroomsml-training-dataset-validation-errors cleanroomsml-training-dataset-create-and-delete-errors
Issue-Shape-Id: com.amazonaws.cleanroomsml#AWSStarkControlService
Issue-Key: cleanroomsml|GetTrainingDataset|4xx->4xx|$.message
Issue-Key: cleanroomsml|GetAudienceModel|4xx->4xx|$.message
Issue-Key: cleanroomsml|CreateTrainingDataset|4xx->4xx|$.message
Issue-Key: cleanroomsml|CreateTrainingDataset|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cleanroomsml|CreateTrainingDataset|4xx->4xx|status
Issue-Key: cleanroomsml|CreateTrainingDataset|4xx->2xx|$.message
Issue-Key: cleanroomsml|CreateTrainingDataset|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cleanroomsml|CreateTrainingDataset|4xx->2xx|status
Issue-Key: cleanroomsml|DeleteTrainingDataset|4xx->4xx|$.message
Issue-Commit: c204ffd13545bba78014bc80d4ce821b8ce22cdd

# cleanroomsml error responses diverge from AWS: not-found message wording, missing role-ARN authorization, assume-role failure, and validation wording

> _Note: this issue was identified against the code base at commit `c204ffd13545bba78014bc80d4ce821b8ce22cdd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingDataset

## Description
The Clean Rooms ML emulator's error paths diverge from AWS in six distinct ways. All six are error-response-fidelity defects in the cleanroomsml handlers: three operations word ResourceNotFoundException wrongly, CreateTrainingDataset mis-classifies two distinct authorization failures, and the input-validation serializer words ValidationException wrongly. This issue bundles SIX causes; all six must be fixed before it is closed.

Cause 1 of 6 - GetTrainingDataset not-found message wording.
AWS: Smithy: GetTrainingDataset errors include ResourceNotFoundException (httpError 404, required member `message`, doc "The resource you are requesting does not exist."). The message text itself is recorded behavior only: AWS returns HTTP 404 with a `message` of the fixed form "Training Dataset <arn> does not exist." - the fixed text is "Training Dataset ", then a space, then "does not exist."; the only variable part is the training-dataset ARN taken verbatim from the request's `trainingDatasetArn` httpLabel.
Emulator: returns the correct 404 but words the message "Training Dataset: <arn> not found." - it inserts a colon after the resource-type name and substitutes "not found." for "does not exist.".
Why: clients and test suites that match on AWS's ResourceNotFoundException text (log scrapers, retry/branching logic, recorded-contract assertions) do not recognise the emulator's message, so a correctly-signalled missing resource reads as an unknown failure.
Fix: the ResourceNotFoundException construction in the GetTrainingDataset handler for the cleanroomsml service (the shared not-found message formatter it calls, if the three operations share one).
Traces: cleanroomsml-get-operations-not-found#task#2 (representative), plus cleanroomsml-training-dataset-lifecycle#task#10 and cleanroomsml-training-dataset-validation-errors#task#5 in the same cluster.

Cause 2 of 6 - GetAudienceModel not-found message wording.
AWS: Smithy: GetAudienceModel errors include ResourceNotFoundException (httpError 404). Message text is recorded behavior only: HTTP 404 with `message` of the fixed form "Audience Model <arn> does not exist.", the only variable part being the audience-model ARN from the request's `audienceModelArn` httpLabel.
Emulator: returns 404 but with "Audience Model: <arn> not found." - same colon insertion and same "not found." substitution as Cause 1.
Why: same as Cause 1, for the audience-model resource.
Fix: the ResourceNotFoundException construction in the GetAudienceModel handler.
Traces: cleanroomsml-get-operations-not-found#task#3.

Cause 3 of 6 - DeleteTrainingDataset not-found message wording.
AWS: Smithy: DeleteTrainingDataset errors include ResourceNotFoundException (httpError 404). Message text is recorded behavior only: HTTP 404 with the same fixed form "Training Dataset <arn> does not exist." as Cause 1.
Emulator: returns 404 with "Training Dataset: <arn> not found.".
Why: same as Cause 1; a delete of an already-absent dataset is not recognisable as a not-found by message-matching clients.
Fix: the ResourceNotFoundException construction in the DeleteTrainingDataset handler.
Traces: cleanroomsml-training-dataset-create-and-delete-errors#task#3, plus cleanroomsml-training-dataset-validation-errors#task#6 in the same cluster.

Cause 4 of 6 - CreateTrainingDataset with no roleArn is answered as a 400 ValidationException instead of a 403 AccessDeniedException.
AWS: Smithy: CreateTrainingDataset.roleArn is @required (IamRoleArn, @length 20..2048, pattern ^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$) and the operation models AccessDeniedException (httpError 403) alongside ValidationException (httpError 400). Which one a missing roleArn produces is recorded behavior only: AWS answers HTTP 403 AccessDeniedException whose `message` has the fixed text "Role Arn not present in request. Please provide a valid role arn in the path: roleArn. because no resource-based policy allows the cleanrooms-ml:CreateTrainingDataset action" - the variable parts are the member path ("roleArn") and the action name ("cleanrooms-ml:CreateTrainingDataset"), both derived from the request; there are no per-run values in it. Hypothesis: the service's authorization layer runs ahead of request validation and cannot resolve a principal policy without the role, so the absent member surfaces as an authorization failure rather than a validation failure.
Emulator: answers HTTP 400 with x-amzn-errortype: ValidationException and message "1 validation error detected: Value null at 'roleArn' failed to satisfy constraint: Member must not be null".
Why: a client gets the wrong error class and the wrong retry/handling decision - SDK error unions surface ValidationException where AWS surfaces AccessDeniedException, so code that catches AccessDeniedException to prompt for IAM setup never fires, and the HTTP status a caller branches on is 400 rather than 403.
Fix: the authorization/precondition ordering in the CreateTrainingDataset handler - the roleArn presence check must raise AccessDeniedException (403) before generic required-member validation runs.
Traces: cleanroomsml-training-dataset-create-and-delete-errors#task#1.

Cause 5 of 6 - CreateTrainingDataset succeeds with a role it cannot assume, instead of returning 403 AccessDeniedException.
AWS: Smithy: CreateTrainingDataset models AccessDeniedException (httpError 403, required member `message`, doc "You do not have sufficient access to perform this action."). Behavior is recorded behavior only: when the supplied roleArn cannot be assumed by the service, AWS returns HTTP 403 AccessDeniedException with a message of the fixed form "Service was not able to assume role <roleArn>." - the only variable part is the roleArn echoed from the request; no training dataset is created.
Emulator: returns HTTP 200 with a CreateTrainingDatasetResponse (no `message`, no x-amzn-errortype header) and creates the dataset, i.e. it never checks role assumability at all.
Why: this is the most damaging cause in the bundle: the emulator reports success for a request AWS rejects, so a client's negative-path and IAM-misconfiguration tests pass against the emulator and fail in production, and the emulator's state then contains a training dataset that AWS would never have created, skewing every subsequent List/Get.
Fix: the CreateTrainingDataset handler's pre-create validation - it must verify the roleArn resolves to an assumable role in the emulator's IAM state and raise AccessDeniedException (403) when it does not, before any state mutation.
Traces: cleanroomsml-training-dataset-create-and-delete-errors#task#2, plus cleanroomsml-training-dataset-validation-errors#task#4 in the same cluster.

Cause 6 of 6 - ValidationException message inserts a spurious "null" into the AWS required-member template.
AWS: Smithy: CreateTrainingDataset models ValidationException (httpError 400, required member `message`); `name` is @required (NameString). Message text is recorded behavior only: HTTP 400 with the fixed template "<N> validation error detected: Value at '<member>' failed to satisfy constraint: Member must not be null" - for an absent required member AWS writes "Value at '<member>'" with NO value token between "Value" and "at". The variable parts are the error count N and the member path, both derived from the request.
Emulator: emits "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null", inserting the literal "null" as if a value had been supplied.
Why: the message is a documented, client-parsed contract; the extra token breaks exact-match assertions and any client that extracts the offending value from the message, and it misreports an omitted member as an explicitly-null one.
Fix: the required-member branch of the cleanroomsml input-validation message formatter - it must omit the value token entirely when the member is absent (the same formatter also produces the wrong text seen in Cause 4).
Traces: cleanroomsml-training-dataset-validation-errors#task#2, plus cleanroomsml-training-dataset-validation-errors#task#3 in the same cluster.

Note: the three cleanroomsml issues currently open (20261109T115700Z-cleanrooms-cleanroomsml-cloudformation-createstack-refuses-aws-cleanroomsml..., 20261109T115857Z-... and 20261109T123922Z-...) are all about CloudFormation resource-type registration for AWS::CleanRoomsML::TrainingDataset and cover none of the six causes above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (5 fixtures, 33 steps, 20 passed, score 60.6%).

Cause 1 - cleanroomsml-get-operations-not-found#task#2, GetTrainingDataset, status 404 expected and 404 got:
  $.message expected 'Training Dataset arn:aws:cleanrooms-ml:us-west-2:000000000000:training-dataset/00000000-0000-0000-0000-000000000000 does not exist.'
  $.message got      'Training Dataset: arn:aws:cleanrooms-ml:us-west-2:000000000000:training-dataset/00000000-0000-0000-0000-000000000000 not found.'
  (cluster also covers cleanroomsml-training-dataset-lifecycle#task#10, cleanroomsml-training-dataset-validation-errors#task#5)

Cause 2 - cleanroomsml-get-operations-not-found#task#3, GetAudienceModel, 404 vs 404:
  expected 'Audience Model <audience-model-arn> does not exist.'
  got      'Audience Model: <audience-model-arn> not found.'

Cause 3 - cleanroomsml-training-dataset-create-and-delete-errors#task#3, DeleteTrainingDataset, 404 vs 404:
  expected 'Training Dataset <arn> does not exist.'
  got      'Training Dataset: <arn> not found.'
  (cluster also covers cleanroomsml-training-dataset-validation-errors#task#6)

Cause 4 - cleanroomsml-training-dataset-create-and-delete-errors#task#1, CreateTrainingDataset:
  status: expected 403, got 400
  $.message expected 'Role Arn not present in request. Please provide a valid role arn in the path: roleArn. because no resource-based policy allows the cleanrooms-ml:CreateTrainingDataset action'
  $.message got      "1 validation error detected: Value null at 'roleArn' failed to satisfy constraint: Member must not be null"
  $headers.x-amzn-errortype: expected the AccessDeniedException shape, got 'ValidationException'

Cause 5 - cleanroomsml-training-dataset-create-and-delete-errors#task#2, CreateTrainingDataset:
  status: expected 403, got 200
  $.message expected 'Service was not able to assume role <role-arn-that-does-not-exist>.', got None (no error body at all)
  $headers.x-amzn-errortype: expected an error shape, got None
  (cluster also covers cleanroomsml-training-dataset-validation-errors#task#4)

Cause 6 - cleanroomsml-training-dataset-validation-errors#task#2, CreateTrainingDataset, 400 vs 400:
  $.message expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must not be null"
  $.message got      "1 validation error detected: Value null at 'name' failed to satisfy constraint: Member must not be null"
  (cluster also covers cleanroomsml-training-dataset-validation-errors#task#3)

ARNs, account ids and role names above are the recorded run's values filling the message templates; only the fixed text and the request-derived substitution points are the requirement.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus), which exercises fixtures cleanroomsml-get-operations-not-found, cleanroomsml-training-dataset-create-and-delete-errors, cleanroomsml-training-dataset-validation-errors and cleanroomsml-training-dataset-lifecycle.
3. Cause 1/2/3: GET /training-dataset/{arn} and GET /audience-model/{arn} for an ARN that was never created, and DELETE /training-dataset/{arn} likewise -> 404 arrives, but the message reads 'Training Dataset: <arn> not found.' / 'Audience Model: <arn> not found.' instead of '<Type> <arn> does not exist.' (fixture cleanroomsml-get-operations-not-found steps task#2 and task#3; cleanroomsml-training-dataset-create-and-delete-errors step task#3).
4. Cause 4: POST /training-dataset with a body omitting roleArn -> emulator answers 400 ValidationException; AWS answers 403 AccessDeniedException (fixture cleanroomsml-training-dataset-create-and-delete-errors step task#1).
5. Cause 5: POST /training-dataset with a well-formed roleArn naming a role that does not exist -> emulator answers 200 and creates the dataset; AWS answers 403 AccessDeniedException 'Service was not able to assume role <roleArn>.' (same fixture, step task#2).
6. Cause 6: POST /training-dataset with a body omitting `name` -> emulator message contains the extra token "null" ("Value null at 'name'") where AWS writes "Value at 'name'" (fixture cleanroomsml-training-dataset-validation-errors steps task#2 and task#3).
