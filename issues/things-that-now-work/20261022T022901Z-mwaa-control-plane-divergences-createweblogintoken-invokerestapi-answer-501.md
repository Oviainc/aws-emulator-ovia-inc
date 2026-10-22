Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mwaa.create-web-login-token-nonexistent-environment-not-found mwaa.invoke-rest-api-nonexistent-environment-not-found mwaa.get-environment-nonexistent-not-found mwaa.update-environment-nonexistent-not-found mwaa.tag-resource-nonexistent-arn-not-found mwaa.create-environment-invalid-execution-role-arn-rejected
Issue-Shape-Id: com.amazonaws.mwaa#AmazonMWAA
Issue-Key: mwaa.create-web-login-token-nonexistent-environment-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, actual_code={code!r}"
Issue-Key: mwaa.invoke-rest-api-nonexistent-environment-not-found|run#4|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: mwaa.get-environment-nonexistent-not-found|run#5|AssertionError|assert message == f"Environment {name} not found.", f"got message {message!r}"
Issue-Key: mwaa.update-environment-nonexistent-not-found|run#5|AssertionError|assert message == f"Environment {name} not found.", f"actual_message={message!r}"
Issue-Key: mwaa.tag-resource-nonexistent-arn-not-found|run#7|AssertionError|assert code == "NotFoundException", f"code={code!r}"
Issue-Key: mwaa.create-environment-invalid-execution-role-arn-rejected|run#5|AssertionError|assert message == (
Issue-Commit: 8b2e309fd4cecf10969d46f8d97ffd56bcdb7486

# MWAA control-plane divergences: CreateWebLoginToken/InvokeRestApi answer 501, not-found message rule, TagResource wire error code, and pattern-anchor leak in ValidationException (5 root causes)

> _Note: this issue was identified against the code base at commit `8b2e309fd4cecf10969d46f8d97ffd56bcdb7486`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebLoginToken, InvokeRestApi, GetEnvironment, UpdateEnvironment, TagResource, CreateEnvironment

## Description
The MWAA (AmazonMWAA, restJson1) emulator is bootstrapped and answers, but five distinct root causes make its control-plane rejection paths diverge from AWS. Each cause below is independent and must be closed separately.

Cause 1 of 5: CreateWebLoginToken is an unimplemented scaffold stub, so a well-formed call against a nonexistent environment answers 501 NotImplemented instead of the operation's modeled rejection.
AWS: Smithy (services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json) binds CreateWebLoginToken POST /webtoken/{Name} with errors AccessDeniedException, InternalServerException, ResourceNotFoundException (errorKind client, httpError 404), ValidationException. A Name matching no environment must be refused with HTTP 404 and error code ResourceNotFoundException (recorded message example: "Environment not found").
Emulator: HTTP 501, Error.Code 'NotImplemented' ("operation CreateWebLoginToken is scaffolded but not implemented in this emulator").
Why: a client that catches ResourceNotFoundException to distinguish "no such environment" from a transport/server fault sees an unmodeled 501 instead, which botocore surfaces as a retryable server error, so its not-found branch never runs.
Fix: the operation's handler — implement the environment lookup and the modeled 404 rejection.
Traces: mwaa.create-web-login-token-nonexistent-environment-not-found (file cited under Traces below).

Cause 2 of 5: InvokeRestApi is an unimplemented scaffold stub, so the Airflow REST data plane answers 501 NotImplemented instead of the modeled rejection.
AWS: Smithy binds InvokeRestApi POST /restapi/{Name} with errors AccessDeniedException, InternalServerException, ResourceNotFoundException (404), RestApiClientException, RestApiServerException, ValidationException. Invoking it for a Name matching no environment must answer HTTP 404 with error code ResourceNotFoundException (recorded message example: "Environment not found").
Emulator: HTTP 501, Error.Code 'NotImplemented' ("operation InvokeRestApi is scaffolded but not implemented in this emulator").
Why: the whole Airflow REST data plane is unusable and its not-found class is unreachable; clients cannot tell an absent environment from an emulator fault.
Fix: the operation's handler — implement the environment lookup, the modeled 404, and the REST-API invocation response.
Traces: mwaa.invoke-rest-api-nonexistent-environment-not-found.

Cause 3 of 5: the environment not-found message is built with the wrong wording. GetEnvironment and UpdateEnvironment do return 404 ResourceNotFoundException, but the message text diverges.
AWS: recorded behavior only — the message is built from the request and names the requested environment: "Environment <Name> not found." (note the trailing period). The rule is request-derived, so the format, not one recording's name, is the requirement.
Emulator: "Environment <Name> could not be found" (no trailing period, different phrasing). Observed: 'Environment sim2real-mwaa-missing-ff42f547d3b6 could not be found'.
Why: clients and operator tooling that match or display AWS's not-found text for environments see a different sentence for the same condition; the two operations share one message builder so both diverge identically.
Fix: the not-found message construction in the GetEnvironment and UpdateEnvironment handlers (shared with the other environment lookups).
Traces: mwaa.get-environment-nonexistent-not-found, mwaa.update-environment-nonexistent-not-found; services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/get_environment.rs:152-156 (and the matching block in src/ops/update_environment.rs).

Cause 4 of 5: TagResource reports the modeled shape name as the wire error code for a nonexistent environment ARN.
AWS: recorded behavior only — for a well-formed environment ARN (correct partition, region and account) that resolves to no environment, TagResource answers HTTP 404 with wire error code NotFoundException, even though the modeled error shape in the Smithy model is ResourceNotFoundException (httpError 404). Recorded message example: "Resource was not found".
Emulator: HTTP 404 with Error.Code 'ResourceNotFoundException', and a message that echoes the ARN ("Environment <arn> could not be found").
Why: error-code dispatch is by wire code, so a client branching on NotFoundException for the tagging operations never matches; the same divergence will affect UntagResource/ListTagsForResource, which share this lookup.
Fix: the error serialization/classification for the tagging operations' not-found path (the wire code the error shape is rendered with), in the TagResource handler.
Traces: mwaa.tag-resource-nonexistent-arn-not-found; services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/tag_resource.rs:151-158.

Cause 5 of 5: constraint-violation messages quote the raw Smithy regex including its ^...$ anchors.
AWS: Smithy models CreateEnvironment.ExecutionRoleArn -> IamRoleArn with pattern ^arn:aws(-[a-z]+)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$ and CreateEnvironment errors include ValidationException (httpError 400). The message rule is known — it is built from the request, naming the rejected value, the member path and the violated constraint: "1 validation error detected: Value '<value>' at 'executionRoleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" where <pattern> is the modeled pattern with the leading ^ and trailing $ removed.
Emulator: the status (400) and code (ValidationException) are right, but the rendered pattern keeps the anchors: "... regular expression pattern: ^arn:aws(-[a-z]+)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$".
Why: any client, test or UI that reconstructs the advertised pattern from the message gets a pattern AWS never emits; this is the generic constraint-message renderer, so every pattern-constrained member of this service is affected. Same cause as the open issue 20261022T012742Z-osis-pipelinename-validationexception-quotes-the-raw-smithy-regex (OSIS) — that issue does not cover mwaa.
Fix: the shared constraint-violation message renderer used by this service's request validation (strip the anchors when quoting a @pattern).
Traces: mwaa.create-environment-invalid-execution-role-arn-rejected.

## Evidence
Suite: mwaa 2/15 passed, 13 failed.

Cause 1 — mwaa.create-web-login-token-nonexistent-environment-not-found [mismatch]: AssertionError: expected ResourceNotFoundException, actual_code='NotImplemented'. Expected HTTP 404 / ResourceNotFoundException; got HTTP 501 / NotImplemented. services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/create_web_login_token.rs:4 carries the 'SCAFFOLDED, NOT IMPLEMENTED' marker and line 169 returns Scaffolded::not_implemented("CreateWebLoginToken").

Cause 2 — mwaa.invoke-rest-api-nonexistent-environment-not-found [mismatch]: AssertionError: expected ResourceNotFoundException, got 'NotImplemented'. src/ops/invoke_rest_api.rs:4 carries the same SCAFFOLDED marker.

Cause 3 — mwaa.get-environment-nonexistent-not-found [mismatch]: AssertionError: got message 'Environment sim2real-mwaa-missing-ff42f547d3b6 could not be found'; expected 'Environment <name> not found.'. mwaa.update-environment-nonexistent-not-found [mismatch]: actual_message='Environment sim2real-mwaa-missing-2ab89075672e could not be found'. Code and status (404 ResourceNotFoundException) matched in both.

Cause 4 — mwaa.tag-resource-nonexistent-arn-not-found [mismatch]: AssertionError: code='ResourceNotFoundException'; expected wire code 'NotFoundException' (message example 'Resource was not found'). Request: TagResource(ResourceArn=arn:aws:airflow:<region>:<account>:environment/<never-created-name>, Tags={'k':'v'}).

Cause 5 — mwaa.create-environment-invalid-execution-role-arn-rejected [mismatch]: AssertionError on message: emulator returned '1 validation error detected: Value 'not-a-valid-role-arn' at 'executionRoleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(-[a-z]+)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$' while AWS reports the same sentence with the pattern unanchored (no leading ^, no trailing $).

## Steps to Reproduce
Boot the emulator and point a botocore mwaa client at it.
1. Cause 1: mwaa.create_web_login_token(Name='sim2real-mwaa-missing-x') -> expect ClientError 404 ResourceNotFoundException; observe 501 NotImplemented.
2. Cause 2: mwaa.invoke_rest_api(Name='sim2real-mwaa-missing-x', Path='/dags', Method='GET') -> expect 404 ResourceNotFoundException; observe 501 NotImplemented.
3. Cause 3: mwaa.get_environment(Name='sim2real-mwaa-missing-x') and mwaa.update_environment(Name='sim2real-mwaa-missing-x', MaxWorkers=2) -> expect Message 'Environment sim2real-mwaa-missing-x not found.'; observe '... could not be found'.
4. Cause 4: mwaa.tag_resource(ResourceArn='arn:aws:airflow:<region>:<account>:environment/sim2real-mwaa-missing-x', Tags={'k':'v'}) -> expect Error.Code 'NotFoundException'; observe 'ResourceNotFoundException'.
5. Cause 5: mwaa.create_environment(Name='sim2real-mwaa-badarn-x', ExecutionRoleArn='not-a-valid-role-arn', SourceBucketArn='arn:aws:s3:::some-bucket-name', DagS3Path='dags', NetworkConfiguration={'SubnetIds':['subnet-00000000000000001','subnet-00000000000000002'],'SecurityGroupIds':['sg-00000000000000001']}) -> expect 400 ValidationException whose message quotes the pattern without ^/$; observe the anchored regex.
