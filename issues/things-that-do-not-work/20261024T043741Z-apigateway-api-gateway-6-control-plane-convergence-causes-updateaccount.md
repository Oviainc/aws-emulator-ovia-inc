Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.update-account-cloudwatch-role-enforced-for-logging apigateway.update-account-invalid-role-arn-fails apigateway.update-account-remove-cloudwatch-role-patch-rejected apigateway.update-stage-conflict-during-deployment apigateway.delete-nonexistent-basepath-mapping-returns-notfound apigateway.deploy-apigateway-2-update-rollback-keeps-serving-good-deployment apigateway.deep-apigateway-3-apigw-private-vpclink-positive
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|UpdateAccount|BadRequestException
Issue-Key: apigateway.update-account-invalid-role-arn-fails|_expect_rejection#17|AssertionError|raise AssertionError(
Issue-Key: apigateway.update-account-remove-cloudwatch-role-patch-rejected|_error_code#17|AssertionError|raise AssertionError(
Issue-Key: apigateway.update-stage-conflict-during-deployment|run#18|AssertionError|assert err.get("Code") == "BadRequestException", f"expected BadRequestException, got {err.get('Code')!r}"
Issue-Key: apigateway.delete-nonexistent-basepath-mapping-returns-notfound|run#6|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: apigateway.deploy-apigateway-2-update-rollback-keeps-serving-good-deployment|_poll_http#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.deep-apigateway-3-apigw-private-vpclink-positive|_retry_dependency#1|Timeout|time.sleep(15)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# API Gateway: 6 control-plane / convergence causes — UpdateAccount cloudwatchRoleArn patch validation is wrong in three ways, UpdateStage misclassifies a bogus deploymentId, DeleteBasePathMapping is a 501 stub, and two checks never complete

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccount

## Description
Six net-new root causes in the apigateway service, none of them carrying a replay key held by an open issue. Open issues 20261023T125454Z-apigateway-api-gateway-control-plane-accepts-inputs-real-aws.md, 20261023T170254Z-apigateway-api-gateway-nine-control-plane-fidelity-causes-seven.md, 20261023T210711Z-apigateway-api-gateway-control-plane-10-fidelity-causes-context.md and 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md describe the same families of symptom but do not carry these seven check keys.

Cause 1 of 6: UpdateAccount refuses to CLEAR /cloudwatchRoleArn.
AWS: Smithy: UpdateAccount (PATCH /account) takes patchOperations: ListOfPatchOperation and returns the Account shape (cloudwatchRoleArn, throttleSettings, features, apiKeyVersion); modeled errors are BadRequestException 400 / ConflictException 409 / LimitExceededException 429 / NotFoundException 404 / TooManyRequestsException 429 / UnauthorizedException 401. Recorded behavior: a `replace` of /cloudwatchRoleArn with an EMPTY string value is accepted and clears the account setting (this is the only documented way to unset it — see cause 3), so GetAccount afterwards reports no cloudwatchRoleArn.
Emulator: that same call is refused with HTTP 400 BadRequestException, example message 'Invalid role ARN specified: /cloudwatchRoleArn takes an IAM role ARN of the form arn:{partition}:iam::{account}:role/{name}'. The emulator applies an ARN-shape check to every value, including the empty string, so the setting can be set but never unset. A `replace` with a real role ARN does succeed (observed in the cause-3 check's positive control), which localises the defect to the empty/clearing value.
Why: any client or test harness that saves the account's CloudWatch role, changes it and restores/clears it cannot complete the round trip; the account is left permanently pointed at a role, and the baseline 'no account role' state that gates stage execution logging is unreachable.
Fix: the UpdateAccount handler's patch validation for /cloudwatchRoleArn (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/, update_account.rs) — the empty string must be accepted as a clear.
Traces: apigateway.update-account-cloudwatch-role-enforced-for-logging.

Cause 2 of 6: UpdateAccount accepts a cloudwatchRoleArn naming a role that does not exist.
AWS: API Gateway validates the role before storing it — a well-formed but non-existent IAM role ARN is rejected with BadRequestException 400 (recorded behavior; Smithy lists BadRequestException in UpdateAccount's error set). The check's positive control proves a real role that trusts apigateway.amazonaws.com is accepted, so the refusal is attributable to the bogus ARN.
Emulator: the call SUCCEEDS (HTTP 200, Account returned) and the bogus ARN is stored on the account.
Why: the write is laundered — stage execution/access logging is then configured against a role that cannot be assumed, and the failure surfaces later (or never) instead of at the call that set it. Clients relying on UpdateAccount as the validation point get no signal.
Fix: the UpdateAccount handler's /cloudwatchRoleArn validation must resolve the role in IAM state and refuse with BadRequestException when it does not exist.
Traces: apigateway.update-account-invalid-role-arn-fails.

Cause 3 of 6: UpdateAccount accepts patch op `remove` on /cloudwatchRoleArn.
AWS: /cloudwatchRoleArn supports only the `replace` op; `remove` on that path is rejected with BadRequestException 400 and the role is left in place (recorded behavior; BadRequestException is modeled on UpdateAccount).
Emulator: the call SUCCEEDS with HTTP 200 and returns the Account body.
Why: an over-permissive patch surface — a client that sends the wrong op gets a success and an account state AWS would never have produced, and the op/path matrix that API Gateway enforces cannot be discovered from the emulator.
Fix: the UpdateAccount handler's patch-op/path matrix (which ops are legal per path), not the value validator.
Traces: apigateway.update-account-remove-cloudwatch-role-patch-rejected.

Cause 4 of 6: UpdateStage misclassifies a /deploymentId patch that names a non-existent deployment.
AWS: Smithy: UpdateStage (PATCH /restapis/{restApiId}/stages/{stageName}) models BadRequestException 400, ConflictException 409, LimitExceededException 429, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401. Recorded behavior: repointing a stage at a deployment id that does not exist is refused with BadRequestException 400 (the stage and the api in the request DO exist); the check's positive control repoints the same stage at a second real deployment successfully, so the refusal is attributable to the bogus id.
Emulator: HTTP 404 with error code NotFoundException.
Why: the status class is wrong (404 vs 400) and the error code is a different modeled error. A client branching on NotFoundException concludes the STAGE or API is gone and may recreate it, where AWS told it only that one patch value was invalid.
Fix: the UpdateStage handler's /deploymentId resolution (services/.../src/ops/update_stage.rs) — an unresolvable deployment id in a patch value is a bad request, not a missing resource.
Traces: apigateway.update-stage-conflict-during-deployment.

Cause 5 of 6: DeleteBasePathMapping answers 501 NotImplemented.
AWS: Smithy: DeleteBasePathMapping (DELETE /domainnames/{domainName}/basepathmappings/{basePath}, success code 202, output Unit) models BadRequestException 400, ConflictException 409, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401. For a domain name that does not exist it answers NotFoundException 404 (example message 'Invalid domain name identifier specified'), as does GetBasePathMapping on the same input.
Emulator: HTTP 501 with error code NotImplemented and the message 'operation DeleteBasePathMapping is scaffolded but not implemented in this emulator'. NotImplemented is not in the operation's modeled error set, and GetDomainNames in the same check answers normally, so this is this one operation's handler.
Why: a client cannot distinguish 'no such mapping' from 'service broken', and no cleanup path that deletes a base path mapping can run at all.
Fix: implement the DeleteBasePathMapping handler in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/ (delete_base_path_mapping.rs) over the domain-name/base-path-mapping state.
Traces: apigateway.delete-nonexistent-basepath-mapping-returns-notfound.

Cause 6 of 6: two more checks never produce a response and hang to the harness cap.
AWS: (a) a CloudFormation UPDATE that fails and rolls back leaves the stage serving the previous, good deployment, and every request to the stage invoke URL gets an HTTP response. (b) A VpcLink is observed PENDING and then reaches AVAILABLE, after which a VPC_LINK integration serves the private backend's bytes at the public stage. Recorded behavior only for the per-case bodies; what is structural is that each polled state transition and each stage request terminates.
Emulator: both checks report exactly 'Check timed out after 60.0s' — no HTTP status and no state transition ever arrives, so the assertions are never reached. Hypothesis: the same dead execute-api data plane and non-advancing resource state machines described in 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md, which does not carry these two keys.
Why: a hang stalls the caller instead of failing it; rollback safety and VPC_LINK delivery are both unobservable.
Fix: the execute-api data-plane request path (stage/route resolution and integration dispatch) and the VpcLink status state machine; every request and every polled status must terminate.
Traces: apigateway.deploy-apigateway-2-update-rollback-keeps-serving-good-deployment, apigateway.deep-apigateway-3-apigw-private-vpclink-positive.

## Evidence
Cause 1: botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the UpdateAccount operation: Invalid role ARN specified: /cloudwatchRoleArn takes an IAM role ARN of the form arn:{partition}:iam::{account}:role/{name} — raised while the check was establishing its baseline with patchOperations=[{'op':'replace','path':'/cloudwatchRoleArn','value':''}].
Cause 2: AssertionError: real AWS rejects UpdateAccount with a non-existent cloudwatchRoleArn, but the update SUCCEEDED.
Cause 3: AssertionError: the call was expected to be REJECTED but it succeeded, returning {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'throttleSettings': {'burstLimit': 5000, 'rateLimit': 10000.0}, 'features': ['UsagePlans'], 'apiKeyVersion': '4'} for patchOperations=[{'op':'remove','path':'/cloudwatchRoleArn'}].
Cause 4: AssertionError: expected BadRequestException, got 'NotFoundException'.
Cause 5: AssertionError: expected NotFoundException, got NotImplemented (delete_base_path_mapping(domainName=<random>.example.org, basePath='unmapped')).
Cause 6: 'Error: Check timed out after 60.0s' for both checks — no status line, no body, no state transition.

## Steps to Reproduce
1. Cause 1: apigw.get_account(); then apigw.update_account(patchOperations=[{'op':'replace','path':'/cloudwatchRoleArn','value':''}]) -> observe BadRequestException instead of a 200 Account with no cloudwatchRoleArn.
2. Cause 2: apigw.update_account(patchOperations=[{'op':'replace','path':'/cloudwatchRoleArn','value':'arn:aws:iam::<account>:role/does-not-exist'}]) -> observe HTTP 200 instead of BadRequestException.
3. Cause 3: after setting a real role via replace, apigw.update_account(patchOperations=[{'op':'remove','path':'/cloudwatchRoleArn'}]) -> observe HTTP 200 instead of BadRequestException.
4. Cause 4: create_rest_api -> put_method/put_integration -> create_deployment -> create_stage; then update_stage(patchOperations=[{'op':'replace','path':'/deploymentId','value':'<bogus>'}]) -> observe 404 NotFoundException instead of 400 BadRequestException.
5. Cause 5: apigw.delete_base_path_mapping(domainName='<random>.example.org', basePath='unmapped') -> observe 501 NotImplemented instead of 404 NotFoundException.
6. Cause 6: replay apigateway.deploy-apigateway-2-update-rollback-keeps-serving-good-deployment and apigateway.deep-apigateway-3-apigw-private-vpclink-positive and observe the 60 s timeout.
