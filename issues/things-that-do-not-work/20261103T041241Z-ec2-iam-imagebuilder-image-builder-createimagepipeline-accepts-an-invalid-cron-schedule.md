Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-an-invalid-cron-schedule
Issue-Key: ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-an-invalid-cron-schedule|_error#7|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it. Response: {resp!r}")
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# Image Builder CreateImagePipeline accepts an invalid cron schedule expression instead of refusing it with InvalidParameterValueException

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateImagePipeline

## Description
One net-new root cause, grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED. It is adjacent to the open issue 20261031T221631Z-ec2-iam-imagebuilder-image-builder-createimagepipeline-refuses-imagetestsconfiguration-timeoutminutes-above-the.md (which covers the imageTestsConfiguration.timeoutMinutes bound and does not carry this check's replay key).

Cause 1 of 1 — `schedule.scheduleExpression` is stored without being parsed, so a syntactically invalid cron expression creates a pipeline.
AWS: Smithy (imagebuilder, PUT /CreateImagePipeline): `schedule` targets the `Schedule` structure (`scheduleExpression`), and the operation models client-side refusals (ClientException, InvalidRequestException — HTTP 400). Recorded behavior: a schedule expression of the form `cron(<garbage>)` is refused with HTTP 400, error code `InvalidParameterValueException` and the message `The value supplied for parameter 'schedule.scheduleExpression' is not valid. Invalid schedule expression. Details: Invalid CRON expression: error at position <n>, details: Illegal characters for this position: '<TOKEN>'`. Fixed text: `The value supplied for parameter 'schedule.scheduleExpression' is not valid. Invalid schedule expression. Details: Invalid CRON expression: error at position `, `, details: Illegal characters for this position: '`, `'`. Variable parts come from the submitted expression: the position of the offending token and that token upper-cased. No pipeline is created. A valid expression such as `cron(0 3 ? * SUN *)` round-trips verbatim through GetImagePipeline, and an omitted start condition defaults to `EXPRESSION_MATCH_AND_DEPENDENCY_UPDATES_AVAILABLE`.
Emulator: the call SUCCEEDS with HTTP 200 and returns an imagePipelineArn — the invalid expression is persisted as a live pipeline schedule.
Why: this is an over-permissive accept. A client that relies on AWS to validate the cron syntax gets a 200 and believes it has a scheduled pipeline; nothing ever fires, and the defect surfaces only as a pipeline that silently never builds. Code paths that test their validation by submitting a bad expression are told the bad expression is legal.
Fix: input validation in the imagebuilder CreateImagePipeline handler — parse schedule.scheduleExpression (and the same member on UpdateImagePipeline) and refuse an unparseable cron with HTTP 400 / InvalidParameterValueException and the message above, before any pipeline is persisted.
Traces: ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-an-invalid-cron-schedule.

## Evidence
Expected: (code, message) == ('InvalidParameterValueException', "The value supplied for parameter 'schedule.scheduleExpression' is not valid. Invalid schedule expression. Details: Invalid CRON expression: error at position 0, details: Illegal characters for this position: 'NOT'") for schedule={'scheduleExpression': 'cron(not valid)'}.
Actual: the call succeeded — HTTPStatusCode 200, response body carried requestId, clientToken and imagePipelineArn 'arn:aws:imagebuilder:<region>:<account>:image-pipeline/<name>' (AssertionError: the call succeeded; real AWS rejects it).

## Steps to Reproduce
Replay ec2__iam__imagebuilder.ibp-create-image-pipeline-rejects-an-invalid-cron-schedule against the emulator. Equivalently: create an imagebuilder component, image recipe and infrastructure configuration, then call CreateImagePipeline(name=<n>, imageRecipeArn=<recipe>, infrastructureConfigurationArn=<infra>, schedule={'scheduleExpression': 'cron(not valid)'}). The emulator answers 200 with an imagePipelineArn instead of a 400 InvalidParameterValueException.
