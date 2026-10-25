Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.create-data-source-missing-iam-role-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.create-data-source-missing-iam-role-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# AppSync CreateDataSource accepts a role-requiring data source type (AWS_LAMBDA) with no serviceRoleArn and returns 200 instead of refusing with BadRequestException

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
One net-new root cause in the appsync service: the newly implemented CreateDataSource handler performs no per-type required-field validation, so a request real AWS refuses now SUCCEEDS.

Cause 1 of 1: CreateDataSource does not reject a data source type that requires a service role when serviceRoleArn is omitted.

AWS: Smithy CreateDataSource (POST /v1/apis/{apiId}/datasources, success code 200) declares `type` as required (DataSourceType enum) and `serviceRoleArn` as an OPTIONAL top-level member; its modeled errors include `BadRequestException` with httpError 400 and `NotFoundException` with httpError 404. Because serviceRoleArn is optional in the model, the per-type requirement is enforced by the operation's handler: for a data source type AppSync must assume a role to invoke (recorded: type=AWS_LAMBDA with lambdaConfig and no serviceRoleArn) the create is REFUSED with HTTP 400 / error code `BadRequestException` rather than creating a data source AppSync could never invoke. Recorded refusal message, as an example of the rule "the refusal states that the service role arn was not supplied": "Service role arn not specified." The refusal is effect-free: the name does not appear in ListDataSources and GetDataSource for it answers HTTP 404 / `NotFoundException` (recorded message example: "Data source <name> not found"). The identical configuration WITH a valid serviceRoleArn succeeds and GetDataSource reports back the same serviceRoleArn.

Emulator: the call returns HTTP 200 with a fully-formed dataSource object (dataSourceArn/name/type/lambdaConfig populated, no serviceRoleArn), i.e. it creates the resource instead of refusing it. The check asserts the call is rejected and fails with "expected the call to be rejected, but it SUCCEEDED and returned {... 'HTTPStatusCode': 200, 'dataSource': {... 'type': 'AWS_LAMBDA', 'lambdaConfig': {...}}}".

Note on already-filed work: open issue 20261024T165547Z-appsync-validation-rejections-answer-notimplemented-501-instead-of.md lists this check id, but it describes the PREVIOUS failure mode (the handler was a scaffold stub answering NotImplemented/501) and its recorded replay key for this check no longer matches. The stub has since been implemented (see fixed issue 20261024T032443Z-appsync-createdatasource-stub.md) and the operation now answers 200; the surviving defect is the MISSING validation described here, which that issue's description does not cover.

Why: a client wiring up a Lambda (or other role-requiring) data source gets no validation signal at create time. On real AWS the misconfiguration is caught synchronously with a 400 BadRequestException the caller can branch on to correct operator input; here the create appears to succeed and the breakage only surfaces later as an un-invokable resolver. IaC/CLI flows that treat a 200 as "configured correctly" proceed with a data source AppSync can never invoke, and the "a rejected create leaves no resource behind" invariant (absent from ListDataSources, 404 NotFoundException from GetDataSource) is silently violated because the resource IS created.

Fix: the CreateDataSource handler and its per-type required-field validation in the appsync service crate (ops/create_data_source.rs) — reject role-requiring types with no serviceRoleArn as BadRequestException (400) and write nothing to the DataSource state; GetDataSource/ListDataSources must then show no trace of the name.

Traces: appsync.create-data-source-missing-iam-role-rejected.

## Evidence
Check appsync.create-data-source-missing-iam-role-rejected (verdict: mismatch).

expected: create_data_source(apiId=<api>, name=<ds>, type="AWS_LAMBDA", lambdaConfig={"lambdaFunctionArn": "arn:aws:lambda:us-east-1:123456789012:function:sim2real-nonexistent"}) with serviceRoleArn OMITTED is refused with HTTP 400, error code "BadRequestException" (recorded message example: "Service role arn not specified."); then the name is absent from list_data_sources and get_data_source(name=<ds>) answers HTTP 404 "NotFoundException" (recorded message example: "Data source <ds> not found").

actual:   HTTP 200 and the data source is created:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'dataSource': {'dataSourceArn': 'arn:aws:appsync:<region>:<account>:apis/<apiId>/datasources/<ds>', 'name': '<ds>', 'type': 'AWS_LAMBDA', 'lambdaConfig': {'lambdaFunctionArn': 'arn:aws:lambda:us-east-1:123456789012:function:sim2real-nonexistent'}}}
  raised at check_appsync_create_data_source_missing_iam_role_rejected.py:40 (via _error() at :115)

Smithy (com-amazonaws-appsync-awsdeepdishcontrolplaneservice): CreateDataSource POST /v1/apis/{apiId}/datasources, code 200; input members apiId (required, httpLabel), name (required, ResourceName, pattern ^[_A-Za-z][_0-9A-Za-z]*$), type (required, DataSourceType enum), serviceRoleArn (optional String), lambdaConfig (LambdaDataSourceConfig); errors BadRequestException (400), ConcurrentModificationException (409), NotFoundException (404), UnauthorizedException (401), InternalFailureException (500). GetDataSource GET /v1/apis/{apiId}/datasources/{name} models NotFoundException (404).

## Steps to Reproduce
1. Boot the emulator and point an AppSync client at it.
2. appsync.create_graphql_api(name=<unique>, authenticationType="API_KEY") and take apiId from the response.
3. appsync.create_data_source(apiId=<apiId>, name="bad_ds", type="AWS_LAMBDA", lambdaConfig={"lambdaFunctionArn": "arn:aws:lambda:us-east-1:123456789012:function:does-not-exist"}) with serviceRoleArn OMITTED.
4. Observe HTTP 200 and a created dataSource, where real AWS answers HTTP 400 with error code BadRequestException (message example "Service role arn not specified.").
5. appsync.list_data_sources(apiId=<apiId>) and appsync.get_data_source(apiId=<apiId>, name="bad_ds"): the name is present / GetDataSource returns 200, where AWS leaves no resource and GetDataSource answers 404 NotFoundException.
6. Positive control (must keep working): create an IAM role trusted by appsync.amazonaws.com and repeat step 3 WITH serviceRoleArn=<roleArn> — it succeeds and GetDataSource reports the same serviceRoleArn.
7. Replay appsync.create-data-source-missing-iam-role-rejected.
