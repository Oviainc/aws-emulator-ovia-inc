Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-schema-lifecycle-and-validation
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-schema-lifecycle-and-validation
Issue-Shape-Id: com.amazonaws.personalize#AmazonPersonalize
Issue-Key: personalize|CreateSchema|4xx->2xx|$.__type
Issue-Key: personalize|CreateSchema|4xx->2xx|$.message
Issue-Key: personalize|CreateSchema|4xx->2xx|status
Issue-Key: personalize|DescribeSchema|4xx->2xx|$.__type
Issue-Key: personalize|DescribeSchema|4xx->2xx|$.message
Issue-Key: personalize|DescribeSchema|4xx->2xx|status
Issue-Commit: b4a0d642bb551c1144d98d8c876f3d4bc2002490

# Personalize schema operations return 200 where AWS returns 400

> _Note: this issue was identified against the code base at commit `b4a0d642bb551c1144d98d8c876f3d4bc2002490`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchema

## Description
TWO causes, one fault family: in the personalize schema lifecycle the emulator answers HTTP 200 with a success body where the recorded Amazon Personalize wire answers HTTP 400 with a modeled exception — the whole error object ($.__type and $.message) is simply absent from the reply.

Cause 1 of 2: CreateSchema accepts an Avro document that omits required dataset fields.
AWS: recorded behavior — HTTP 400, __type InvalidInputException, message 'Schema is missing required fields: [item_id, timestamp]'. Fixed text is 'Schema is missing required fields: ' followed by a bracketed, comma-separated list; the list contents are variable — they are the required field names for the dataset type the schema is written for that the submitted document does not declare (here item_id and timestamp were absent from an Interactions-style schema). Smithy: CreateSchema.schema is AvroSchema @required @length max 20000 and the operation models InvalidInputException (@httpError 400, 'Provide a valid value for the field or parameter.'); the model does not enumerate the per-dataset-type required fields, so the field set is recorded behavior only.
Emulator: HTTP 200 with a minted schemaArn and the document stored verbatim. check_avro_schema() in src/ops/create_schema.rs validates only that the body parses as JSON, that "type" == "record" and that "fields" is an array — its own comment states 'What each field must contain is per-dataset-type and is not checked here.'
Why: the invalid schema is accepted and persisted, so the failure surfaces much later (or never) instead of at the call that caused it; a client testing its schema-validation error path gets a success and an ARN for a schema AWS would have refused, and any CreateDataset built on it diverges from real behavior too.
Fix: CreateSchema's validation — extend check_avro_schema to require the per-dataset-type mandatory fields and raise InvalidInputException with AWS's 'Schema is missing required fields: [...]' template listing exactly the missing names.

Cause 2 of 2: DescribeSchema still resolves a schema ARN that no longer exists.
AWS: recorded behavior — HTTP 400, __type ResourceNotFoundException, message 'Resource Arn <arn> does not exist.'. Fixed text 'Resource Arn ' + ' does not exist.'; the variable part is the schemaArn from the request. Smithy: DescribeSchema models InvalidInputException (400) and ResourceNotFoundException (annotated @httpError 404 — again not the status the recording shows).
Emulator: HTTP 200 carrying a $.schema object; no __type and no message at all.
Hypothesis: the preceding DeleteSchema step in this fixture is an unimplemented scaffold stub, so the schema is never removed from state and this lookup still hits a live record — src/ops/describe_schema.rs does have a not-found branch, it is just unreachable here. That stub is already filed as 20261029T190605Z-personalize-deleteschema-stub.md, which covers the missing DeleteSchema only; it does not carry this step's replay key and does not cover the two further divergences in this not-found reply: even once deletion works, the handler returns status 404 (modeled) rather than the recorded 400, and the message 'Resource not found: <arn>' rather than AWS's 'Resource Arn <arn> does not exist.'
Why: a client reading back a schema it just deleted is told the schema still exists, so delete-then-verify and idempotent-cleanup flows loop or report success wrongly; when the lookup does fail, its status class and message still do not match AWS.
Fix: the schema state's delete path (so a removed schema leaves state) together with DescribeSchema's not-found construction and the serialized status for ResourceNotFoundException.

Traces: personalize-schema-lifecycle-and-validation#task#3 (CreateSchema), personalize-schema-lifecycle-and-validation#task#5 (DescribeSchema). Source: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/create_schema.rs:215-243 and src/ops/describe_schema.rs:143-154.

## Evidence
Cause 1 — personalize-schema-lifecycle-and-validation#task#3, CreateSchema: status expected 400, got 200; $.__type missing (expected 'InvalidInputException', got None); $.message missing (expected 'Schema is missing required fields: [item_id, timestamp]', got None). Fixed text = 'Schema is missing required fields: [...]'; the bracketed names are the required fields the submitted schema omits, not literals of this run.
Cause 2 — personalize-schema-lifecycle-and-validation#task#5, DescribeSchema: status expected 400, got 200; $.__type missing (expected 'ResourceNotFoundException', got None); $.message missing (expected 'Resource Arn arn:aws:personalize:us-west-2:000000000000:schema/<name> does not exist.', got None) — the emulator returned a populated $.schema instead.
Smithy: CreateSchema -> InvalidInputException @httpError 400; DescribeSchema -> ResourceNotFoundException @httpError 404 while the recording shows 400.

## Steps to Reproduce
1. Replay fixture personalize-schema-lifecycle-and-validation, task#3: CreateSchema with an Avro record whose fields list omits the required item_id and timestamp members -> emulator answers 200 with a schemaArn instead of 400 InvalidInputException 'Schema is missing required fields: [item_id, timestamp]'.
2. Same fixture, task#5: after the DeleteSchema step, DescribeSchema for that schemaArn -> emulator answers 200 with the schema body instead of 400 ResourceNotFoundException 'Resource Arn <arn> does not exist.'
