Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__cloudformation.deploy-cfn-appconfig-application-name-too-long-rejected
Issue-Key: appconfig__cloudformation.deploy-cfn-appconfig-application-name-too-long-rejected|run#18|AssertionError|assert match is not None, f"CREATE_FAILED reason is not a resource-handler message: {reason!r}"
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# appconfig__cloudformation: the CREATE_FAILED reason for a refused AWS::AppConfig::Application omits AWS's service clause — no (Service: AppConfig, Status Code: 400, Request ID: ...) inside the resource-handler message

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation resource that AppConfig refuses does produce a CREATE_FAILED event, but the ResourceStatusReason is missing the service/status/request-id clause AWS's resource handlers append to the wrapped service error, so a client parsing the reason cannot recover which service refused the call or with what status.
AWS: the underlying refusal is AppConfig's own validation error — Smithy (appconfig) CreateApplication.Name is @required with @length min 1, max 64 and the operation models BadRequestException (client, HTTP 400); AWS's message for an over-length member is '<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must have length less than or equal to <max>' (fixed text; the count, the submitted value, the lowerCamel member name 'name' and the limit 64 come from the request and the model). CloudFormation wraps that service error in a resource-handler reason of the form: Resource handler returned message: "<service message> (Service: <SdkId>, Status Code: <httpStatus>, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: <code>) — for this refusal Service: AppConfig, Status Code: 400, HandlerErrorCode: InvalidRequest. The request id and token are per-run values; the clause itself and its field names are the requirement.
Emulator: the stack does reach CREATE_FAILED and the inner AppConfig validation message is correct, but the reason is emitted as: Resource handler returned message: "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 64" (RequestToken: <token>, HandlerErrorCode: InvalidRequest) — the ' (Service: AppConfig, Status Code: 400, Request ID: <id>)' clause that must close the quoted message is absent entirely.
Why: a client (or a CDK/CloudFormation diagnostic, or a log-scraping runbook) that parses DescribeStackEvents ResourceStatusReason to attribute a failure to a service and an HTTP status finds neither, and any matcher anchored on AWS's reason format fails to match a genuinely failed resource. Sibling issues record the same omission for other services' handlers (e.g. 20261109T185037Z-cloudformation-sqs-5-net-new-causes-resource-import-does-not.md, 20261109T181128Z-cloudformation-codebuild-resources-reportgroup-physical-id-is-the.md); neither carries this check's replay key, so the AppConfig handler is filed here.
Fix: the CloudFormation provisioning path's resource-handler error wrapping — where a refused resource's service error is rendered into the CREATE_FAILED ResourceStatusReason — must append the (Service, Status Code, Request ID) clause taken from the wrapped service response.
Traces: appconfig__cloudformation.deploy-cfn-appconfig-application-name-too-long-rejected.

## Evidence
Expected ResourceStatusReason shape: 'Resource handler returned message: "1 validation error detected: Value '<70-char name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 64 (Service: AppConfig, Status Code: 400, Request ID: <uuid>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'.
Actual: 'Resource handler returned message: "1 validation error detected: Value 'sim2real-behav-cfnbad-9c9524e60997xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' at 'name' failed to satisfy constraint: Member must have length less than or equal to 64" (RequestToken: 9055eedb-d004-9d99-f222-48673b06d14b, HandlerErrorCode: InvalidRequest)' — the quoted message ends at the limit with no (Service: ..., Status Code: ..., Request ID: ...) clause. Suite verdict: 'AssertionError: CREATE_FAILED reason is not a resource-handler message'. The name, request token and request id above are one run's values; the required part is the clause and its field names.

## Steps to Reproduce
1. CreateStack with DisableRollback=true and a template containing a single resource of type AWS::AppConfig::Application whose Name parameter is 70 characters long (longer than AppConfig's 64-character maximum).
2. Poll DescribeStacks until the stack is terminal: it reaches CREATE_FAILED (correct).
3. Page DescribeStackEvents and read the ResourceStatusReason of the single CREATE_FAILED event for the Application's logical id.
4. Observe the quoted resource-handler message carries no ' (Service: AppConfig, Status Code: 400, Request ID: <id>)' clause before the closing quote, so the service name and HTTP status of the wrapped AppConfig BadRequestException are unrecoverable.
