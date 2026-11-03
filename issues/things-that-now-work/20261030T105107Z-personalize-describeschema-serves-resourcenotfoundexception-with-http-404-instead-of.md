Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-schema-lifecycle-and-validation
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-schema-lifecycle-and-validation
Issue-Shape-Id: com.amazonaws.personalize#AmazonPersonalize
Issue-Key: personalize|DescribeSchema|4xx->4xx|status
Issue-Commit: e18b8d48e355f00c62dc3aed2fd43b52117a0dd5

# DescribeSchema serves ResourceNotFoundException with HTTP 404 instead of AWS's 400

> _Note: this issue was identified against the code base at commit `e18b8d48e355f00c62dc3aed2fd43b52117a0dd5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSchema

## Description
ONE cause: the status line of the Personalize DescribeSchema not-found reply.

Cause 1 of 1: DescribeSchema for a schemaArn that holds no schema.
AWS: recorded behavior — HTTP 400 with an awsJson1_1 error body whose $.__type is ResourceNotFoundException and whose $.message is 'Resource Arn <arn> does not exist.' The fixed text is 'Resource Arn ' + ' does not exist.'; the only variable part is the schemaArn copied from the request. Smithy (com-amazonaws-personalize-amazonpersonalize.json): DescribeSchema models InvalidInputException (@httpError 400) and ResourceNotFoundException (@httpError 404) — the recorded wire shows the live service answers this operation's not-found with 400, so the modeled 404 is not the status served, and the recording is the authority.
Emulator: the error code and the message now match exactly; only the HTTP status diverges — the reply is serialized with the modeled 404 instead of 400. The handler raises DescribeSchemaErrorModeled::ResourceNotFoundException with crate::state::schema_not_found_message(..) and the generic modeled-error-to-status mapping stamps 404 onto it.
Why: a client that branches on HTTP class rather than on $.__type (common for awsJson1_1 SDK retry/404-probing wrappers, HTTP proxies and test harnesses asserting on the status line) sees 'not found' where AWS reports a 400 client error, so delete-then-verify and exists-probe flows take the wrong branch even though the body is correct. It also makes the emulator inconsistent with itself once the dataset/dataset-group not-found paths are corrected to 400.
Fix: the serialized status for ResourceNotFoundException on the DescribeSchema path — a per-service override of the modeled httpError for this error shape (the same override the sibling resource-error issues need), not a model edit; the handler's error construction and message are already right.

Related open issues, neither of which covers this divergence: 20261029T204550Z-personalize-schema-operations-return-200-where-aws-returns.md recorded this same step when it still answered HTTP 200 with a live $.schema body (DeleteSchema not removing state, missing error object); that part now behaves and its replay key for this step is the 4xx->2xx shape, not this status-only divergence. 20261029T204550Z-personalize-resource-errors-wrong-http-status-and-non.md reports the identical 404-instead-of-400 mapping for the dataset and dataset-group operations but not for DescribeSchema.

Traces: personalize-schema-lifecycle-and-validation#task#5 (DescribeSchema). Source: services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/describe_schema.rs:144-151.

## Evidence
Replay of personalize-schema-lifecycle-and-validation#task#5, DescribeSchema (divergence key service=personalize|DescribeSchema|status=400->404|keys=status): status expected 400, got 404. No body key diverged — $.__type ('ResourceNotFoundException') and $.message ('Resource Arn <schemaArn> does not exist.', fixed text 'Resource Arn ' / ' does not exist.' with the ARN taken from the request) both matched the recording, so the status line is the sole divergence. Smithy: DescribeSchema errors = InvalidInputException @httpError 400, ResourceNotFoundException @httpError 404; the emulator serves the modeled 404 while the recorded response is 400.

## Steps to Reproduce
1. Replay fixture personalize-schema-lifecycle-and-validation against the emulator.
2. Let it run through CreateSchema and the DeleteSchema step so the schema is gone from state.
3. At task#5, DescribeSchema is called with that now-deleted schemaArn. Observe the reply: body is correct (__type ResourceNotFoundException, message 'Resource Arn <arn> does not exist.') but the HTTP status line is 404 where the recorded AWS response is 400.
