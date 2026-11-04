Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize.dataset-group-delete-blocked-while-dataset-active
Issue-Shape-Id: com.amazonaws.personalize#AmazonPersonalize
Issue-Key: personalize.dataset-group-delete-blocked-while-dataset-active|run#15|AssertionError|assert message == expected_message, f"unexpected message {message!r}"
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# DeleteDatasetGroup returns a bespoke ResourceInUseException message instead of AWS's fixed wording

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDatasetGroup

## Description
Cause 1 of 1: DeleteDatasetGroup on a dataset group that still holds datasets is correctly refused with ResourceInUseException / HTTP 409, but the emulator synthesizes its own diagnostic message rather than the message AWS returns for this error class. Failing check: personalize.dataset-group-delete-blocked-while-dataset-active.

AWS: Smithy model services/service-com-amazonaws-personalize-amazonpersonalize/smithy-model/com-amazonaws-personalize-amazonpersonalize.json declares DeleteDatasetGroup with errors InvalidInputException (400), ResourceInUseException (409, errorKind client, member `message` -> ErrorMessage, doc "The specified resource is in use."), ResourceNotFoundException (404); the operation doc states "Before you delete a dataset group, you must delete the following: All associated event trackers. All associated solutions. All datasets in the dataset group." The model does not state the message text, so for the text this is recorded behavior only: Amazon Personalize answers this refusal with the FIXED message "The provided dataset-group is being referenced by one or more datasets" — the whole string is fixed text, with NO variable parts: it interpolates no ARN, no count and no list of referencing dataset ARNs. The status (409) and code (ResourceInUseException) the emulator already returns are correct.

Emulator: Error.Code is ResourceInUseException as required, but Error.Message is a locally composed sentence of the form "Dataset group <datasetGroupArn> still holds <n> dataset(s) (<comma-joined dataset ARNs>); delete them first." — in the replay, 'Dataset group arn:aws:personalize:us-west-2:000000000000:dataset-group/<group-name> still holds 1 dataset(s) (arn:aws:personalize:us-west-2:000000000000:dataset/<group-name>/INTERACTIONS); delete them first.' (the ARNs, name and count there are per-run values filling that template, not part of the requirement). None of that wording is AWS's: the fixed text differs entirely, and the emulator adds request/state-derived variables to a message AWS keeps constant.

Why: a client that branches on, matches or logs the refusal text — or an assertion/test harness, SDK retry wrapper or operator runbook keyed to Personalize's documented sentence — sees a string it cannot match, so it treats a correctly-refused delete as an unrecognized failure. It also leaks the referencing dataset ARNs and a count that real Personalize does not disclose in this error, so message-equality contract tests written against AWS fail against the emulator even though the status and code agree.

Fix: the DeleteDatasetGroup handler's in-use rejection path — the branch that builds `ResourceInUseException { message }` after collecting the datasets whose dataset_group_arn matches the input. It must emit the constant AWS string with no interpolation; the ResourceNotFoundException path in the same handler ("Resource not found: <arn>") should be reviewed for the same class of bespoke wording, and the identical fixed-string requirement applies to the sibling schema refusal ("The provided schema is being referenced by one or more datasets") once DeleteSchema is implemented.

Traces: personalize.dataset-group-delete-blocked-while-dataset-active — creates a dataset group, polls it to ACTIVE, creates a schema and an Interactions dataset in it, then calls DeleteDatasetGroup and asserts (code, message) == ("ResourceInUseException", "The provided dataset-group is being referenced by one or more datasets"); the code matches and the message assertion fails. Handler: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/delete_dataset_group.rs lines 180-193 (message built at lines 181-186). Not covered by any open personalize issue (the open ones concern the events registry, ListSchemas/CreateDataset registration, and personalize-runtime inference).

## Evidence
Expected (AWS): HTTP 409, Error.Code = "ResourceInUseException", Error.Message = "The provided dataset-group is being referenced by one or more datasets" (entire string fixed text; no variable parts).

Actual (emulator): HTTP 409, Error.Code = "ResourceInUseException" (correct), Error.Message = "Dataset group arn:aws:personalize:us-west-2:000000000000:dataset-group/sim2real-behav-personalize-dg-7682d8684bf2 still holds 1 dataset(s) (arn:aws:personalize:us-west-2:000000000000:dataset/sim2real-behav-personalize-dg-7682d8684bf2/INTERACTIONS); delete them first."

Template breakdown of the actual message: fixed text "Dataset group ", " still holds ", " dataset(s) (", "); delete them first."; variable parts taken from the request/state: the dataset group ARN (from input.datasetGroupArn), the count of held datasets, and the comma-joined held dataset ARNs. AWS interpolates none of these.

Replay assertion:
  AssertionError: unexpected message 'Dataset group arn:aws:personalize:...:dataset-group/... still holds 1 dataset(s) (...); delete them first.'
  at check_personalize_dataset_group_delete_blocked_while_dataset_active.py:96
    assert message == expected_message

Source of the divergence: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/delete_dataset_group.rs:181-186 composes the message with format! over the ARN, count and held list.

Positive control in the same check (delete the dataset, then DeleteDatasetGroup succeeds and the group becomes ResourceNotFoundException) was never reached because the message assertion aborts first.

## Steps to Reproduce
1. Boot the emulator and point a botocore `personalize` client at it.
2. group_arn = create_dataset_group(name=<unique>)["datasetGroupArn"]; poll describe_dataset_group until datasetGroup.status == "ACTIVE".
3. schema_arn = create_schema(name=<unique>, schema=<Interactions Avro record with USER_ID string, ITEM_ID string, TIMESTAMP long>)["schemaArn"].
4. dataset_arn = create_dataset(name=<unique>, datasetGroupArn=group_arn, schemaArn=schema_arn, datasetType="Interactions")["datasetArn"].
5. Call delete_dataset_group(datasetGroupArn=group_arn) and capture the ClientError.
6. Observe response["Error"]["Code"] == "ResourceInUseException" (correct) but response["Error"]["Message"] is the bespoke "Dataset group <arn> still holds N dataset(s) (<arns>); delete them first." instead of AWS's fixed "The provided dataset-group is being referenced by one or more datasets".
7. Re-replay personalize.dataset-group-delete-blocked-while-dataset-active to confirm the fix; the check then proceeds to its positive control (delete the dataset, then the same DeleteDatasetGroup succeeds and describe_dataset_group raises ResourceNotFoundException).
