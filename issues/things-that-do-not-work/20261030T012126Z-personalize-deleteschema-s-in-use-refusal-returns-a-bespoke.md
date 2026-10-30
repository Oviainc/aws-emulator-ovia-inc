Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize.schema-delete-blocked-while-dataset-references-it
Issue-Shape-Id: com.amazonaws.personalize#AmazonPersonalize
Issue-Key: personalize.schema-delete-blocked-while-dataset-references-it|run#15|AssertionError|assert message == expected_message, f"unexpected message {message!r}"
Issue-Commit: 53eb816e99061cb37fa36e4196f468308f70e8b6

# DeleteSchema's in-use refusal returns a bespoke ResourceInUseException message instead of AWS's fixed wording

> _Note: this issue was identified against the code base at commit `53eb816e99061cb37fa36e4196f468308f70e8b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSchema

## Description
REOPENS 20261029T190605Z-personalize-deleteschema-stub.md: its fix left the call refused. DeleteSchema is now implemented and the refusal CLASS it produces is right (ResourceInUseException / HTTP 409, which is what AWS also answers at this point of the flow), but the message it carries is invented, so the check that exercises the refusal still fails on that reply and never reaches its positive control (delete the dataset, then DeleteSchema succeeds and DescribeSchema answers ResourceNotFoundException).

Cause 1 of 1: DeleteSchema on a schema that is still referenced by a dataset is refused with a locally composed diagnostic sentence rather than the message AWS returns for this error class. Failing check: personalize.schema-delete-blocked-while-dataset-references-it.

AWS: Smithy (services/service-com-amazonaws-personalize-amazonpersonalize/smithy-model/com-amazonaws-personalize-amazonpersonalize.json) models DeleteSchema with input schemaArn (Arn, @required, @length 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$) and errors InvalidInputException (@httpError 400), ResourceInUseException (@httpError 409, errorKind client, member `message` -> ErrorMessage, doc "The specified resource is in use."), ResourceNotFoundException (@httpError 404). The operation doc states "Deletes a schema. Before deleting a schema, you must delete all datasets referencing the schema." The model does not state the message text, so for the text this is recorded behavior only: Amazon Personalize answers this refusal with the FIXED message "The provided schema is being referenced by one or more datasets" — the ENTIRE string is fixed text, with NO variable parts: no schema ARN, no count and no list of referencing dataset ARNs is interpolated. Status 409 and code ResourceInUseException, which the emulator already returns, are correct.

Emulator: Error.Code is ResourceInUseException as required, but Error.Message is built per request as "Schema <schemaArn> is in use by dataset(s) <comma-joined dataset ARNs>. Before deleting a schema, you must delete all datasets referencing the schema." In the replay that came out as 'Schema arn:aws:personalize:us-west-2:000000000000:schema/<schema-name> is in use by dataset(s) arn:aws:personalize:us-west-2:000000000000:dataset/<group-name>/INTERACTIONS. Before deleting a schema, you must delete all datasets referencing the schema.' — the ARNs and names there are per-run values filling that template, not part of the requirement. None of that wording is AWS's: the fixed text differs entirely, and the emulator adds request/state-derived variables to a message AWS keeps constant.

Why: a client that branches on, matches or logs the refusal text — an SDK wrapper, a cleanup-ordering routine, an operator runbook or a message-equality contract test written against Personalize's documented sentence — cannot match the string, so a correctly-refused delete is treated as an unrecognized failure. It also discloses the referencing dataset ARNs that real Personalize does not reveal in this error. The sibling DeleteDatasetGroup defect is the same fault family and is already filed as 20261029T190605Z-personalize-deletedatasetgroup-returns-a-bespoke-resourceinuseexception-message-instead-of.md (its Fix section explicitly flagged this schema refusal as still to come); that issue carries only the dataset-group check's replay key and does not cover this one.

Fix: the DeleteSchema handler's in-use rejection path — the branch that collects the datasets whose schema_arn matches the input and constructs ResourceInUseException { message }. It must emit AWS's constant string with no interpolation. The ResourceNotFoundException path in the same handler composes "Resource not found: <arn>" and should be reviewed for the same class of bespoke wording (AWS's recorded not-found wording for schema lookups is "Resource Arn <arn> does not exist.", per the open issue 20261029T204550Z-personalize-schema-operations-return-200-where-aws-returns.md).

Traces: personalize.schema-delete-blocked-while-dataset-references-it. Handler: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/delete_schema.rs lines 168-193 (message built with format! at lines 181-186); not-found wording at lines 156-166.

## Evidence
Expected (AWS): HTTP 409, Error.Code = "ResourceInUseException", Error.Message = "The provided schema is being referenced by one or more datasets" (entire string is fixed text; no variable parts).

Actual (emulator): HTTP 409, Error.Code = "ResourceInUseException" (correct), Error.Message = 'Schema arn:aws:personalize:us-west-2:000000000000:schema/sim2real-behav-personalize-sch-125619b2ff9d is in use by dataset(s) arn:aws:personalize:us-west-2:000000000000:dataset/sim2real-behav-personalize-dg-dd7d3da4c5ed/INTERACTIONS. Before deleting a schema, you must delete all datasets referencing the schema.'

Template breakdown of the actual message: fixed text "Schema ", " is in use by dataset(s) ", ". Before deleting a schema, you must delete all datasets referencing the schema."; variable parts taken from the request and the store: the schemaArn from input, and the comma-joined ARNs of datasets whose schema_arn matches it. AWS interpolates none of these.

Replay assertion:
  AssertionError: unexpected message 'Schema arn:aws:personalize:...:schema/... is in use by dataset(s) arn:aws:personalize:...:dataset/.../INTERACTIONS. Before deleting a schema, you must delete all datasets referencing the schema.'
  at check_personalize_schema_delete_blocked_while_dataset_references_it.py:104
    assert message == expected_message

Smithy: DeleteSchema errors = InvalidInputException (400), ResourceInUseException (409, doc "The specified resource is in use."), ResourceNotFoundException (404); message member is ErrorMessage. Text is recorded behavior only.

Source of the divergence: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/delete_schema.rs:181-186.

The check's positive control (after DeleteDataset, the same DeleteSchema succeeds and DescribeSchema raises ResourceNotFoundException) was never reached because the message assertion aborts first.

## Steps to Reproduce
1. Boot the emulator and point a botocore `personalize` client at it.
2. group_arn = create_dataset_group(name=<unique>)["datasetGroupArn"]; poll describe_dataset_group until datasetGroup.status == "ACTIVE".
3. schema_arn = create_schema(name=<unique>, schema=<Interactions Avro record with USER_ID string, ITEM_ID string, TIMESTAMP long>)["schemaArn"].
4. dataset_arn = create_dataset(name=<unique>, datasetGroupArn=group_arn, schemaArn=schema_arn, datasetType="Interactions")["datasetArn"].
5. Call delete_schema(schemaArn=schema_arn) and capture the ClientError.
6. Observe response["Error"]["Code"] == "ResourceInUseException" (correct) but response["Error"]["Message"] is the bespoke "Schema <arn> is in use by dataset(s) <arns>. Before deleting a schema, you must delete all datasets referencing the schema." instead of AWS's fixed "The provided schema is being referenced by one or more datasets".
7. Re-replay personalize.schema-delete-blocked-while-dataset-references-it to confirm the fix; the check then proceeds to its positive control (delete the dataset, then the same delete_schema succeeds and describe_schema raises ResourceNotFoundException).
