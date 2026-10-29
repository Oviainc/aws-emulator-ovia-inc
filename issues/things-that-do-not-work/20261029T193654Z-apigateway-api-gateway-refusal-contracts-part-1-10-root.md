Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-authorizer-invalid-type-rejected apigateway.authorizer-missing-uri-fails apigateway.create-deployment-stage-name-conflict-error-contract apigateway.create-duplicate-stage-conflict apigateway.create-domain-name-duplicate-conflict apigateway.create-duplicate-domain-name-conflict apigateway.create-model-invalid-content-type-rejected apigateway.create-usage-plan-key-duplicate-fails apigateway.create-usage-plan-key-invalid-type-rejected apigateway.create-usage-plan-key-wrong-type-fails apigateway.delete-nonexistent-vpc-link-returns-notfound apigateway.get-vpc-link-nonexistent-not-found apigateway.vpc-link-lifecycle apigateway.create-vpc-link-invalid-target-badrequest apigateway.vpc-link-invalid-target-arn-rejected apigateway.create-vpc-link-empty-target-list-rejected apigateway.delete-vpc-link-in-use-conflict apigateway.delete-vpc-link-in-use-rejected apigateway.delete-vpc-link-with-active-integration-fails
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.create-authorizer-invalid-type-rejected|run#8|AssertionError|assert message == "Invalid AuthorizerType specified.", f"unexpected BadRequestException message {message!r}"
Issue-Key: apigateway.authorizer-missing-uri-fails|run#6|AssertionError|assert message == "URI is a required field in an Authorizer", f"unexpected message {message!r}"
Issue-Key: apigateway.create-deployment-stage-name-conflict-error-contract|run#16|AssertionError|assert message == "Stage already exists", f"unexpected ConflictException message {message!r}"
Issue-Key: apigateway.create-duplicate-stage-conflict|run#8|AssertionError|assert message == "Stage already exists", f"unexpected ConflictException message {message!r}"
Issue-Key: apigateway.create-domain-name-duplicate-conflict|run#10|AssertionError|assert code == "BadRequestException", (
Issue-Key: apigateway.create-duplicate-domain-name-conflict|run#9|AssertionError|assert code == "BadRequestException", (
Issue-Key: apigateway.create-model-invalid-content-type-rejected|run#7|AssertionError|assert message == "Invalid content type specified: ", f"unexpected BadRequestException message {message!r}"
Issue-Key: apigateway.create-usage-plan-key-duplicate-fails|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: apigateway.create-usage-plan-key-invalid-type-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: apigateway.create-usage-plan-key-wrong-type-fails|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: apigateway.delete-nonexistent-vpc-link-returns-notfound|run#14|AssertionError|assert get_message == "Invalid Vpc Link identifier specified", get_message
Issue-Key: apigateway.get-vpc-link-nonexistent-not-found|run#4|AssertionError|assert message == "Invalid Vpc Link identifier specified", f"unexpected message {message!r}"
Issue-Key: apigateway.vpc-link-lifecycle|run#24|AssertionError|assert (err.get("Code"), err.get("Message")) == not_found, (
Issue-Key: apigateway.create-vpc-link-invalid-target-badrequest|run#6|AssertionError|assert message == "Invalid ARN. ARN is not well formed not-a-valid-nlb-arn", (
Issue-Key: apigateway.vpc-link-invalid-target-arn-rejected|run#6|AssertionError|assert message == f"Invalid ARN. ARN is not well formed {bad_arn}", (
Issue-Key: apigateway.create-vpc-link-empty-target-list-rejected|run#7|AssertionError|assert message == "Need at least one target arn to create a vpc link", (
Issue-Key: apigateway.delete-vpc-link-in-use-conflict|run#16|AssertionError|assert message == (
Issue-Key: apigateway.delete-vpc-link-in-use-rejected|run#14|AssertionError|assert message == (
Issue-Key: apigateway.delete-vpc-link-with-active-integration-fails|run#15|AssertionError|assert err == expected, (
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# API Gateway refusal contracts, part 1: 10 root causes (authorizer, stage, domain name, model, usage-plan key and VPC link error codes and messages)

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcLink

## Description
This issue bundles 10 NET-NEW root causes where an API Gateway refusal carries a different error code or a different message than AWS's. The status, error code and message are part of the wire contract. A fixer must close all 10 before submitting.

Cause 1 of 10: CreateAuthorizer with a type outside the enum.
AWS: BadRequestException with the message 'Invalid AuthorizerType specified.' (fixed text, no variable part).
Emulator: BadRequestException "1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'createAuthorizerInput.type' failed to satisfy constraint: Member must satisfy enum value set: [TOKEN, REQUEST, COGNITO_USER_POOLS]" — it reports a generic constraint violation instead of the operation's own refusal.
Why: clients matching the documented text cannot classify the refusal.
Fix: CreateAuthorizer's validation/refusal message.
Traces: apigateway.create-authorizer-invalid-type-rejected

Cause 2 of 10: CreateAuthorizer without authorizerUri.
AWS: BadRequestException 'URI is a required field in an Authorizer' (fixed text).
Emulator: BadRequestException 'Invalid request input'.
Why: the refusal does not say which field is missing.
Fix: CreateAuthorizer's required-field validation message.
Traces: apigateway.authorizer-missing-uri-fails

Cause 3 of 10: stage-name collision message.
AWS: ConflictException 'Stage already exists' (fixed text, no variable part), both from CreateDeployment with an existing stageName and from CreateStage.
Emulator: 'Stage already exists for this API: orig' / 'Stage already exists for this API: s412mgputfe' — it appends the stage name.
Why: the message is not AWS's; comparisons on the exact text fail.
Fix: the conflict message in CreateStage / CreateDeployment.
Traces: apigateway.create-deployment-stage-name-conflict-error-contract, apigateway.create-duplicate-stage-conflict

Cause 4 of 10: duplicate CreateDomainName uses the wrong error class.
AWS: BadRequestException (400) with the message 'The domain name you provided already exists.' Smithy: CreateDomainName declares BadRequestException (400) and ConflictException (409); AWS uses the 400 for this condition.
Emulator: ConflictException (409).
Why: a client retrying on 409 and failing fast on 400 takes the wrong branch.
Fix: CreateDomainName's duplicate-name refusal (error class and message).
Traces: apigateway.create-domain-name-duplicate-conflict, apigateway.create-duplicate-domain-name-conflict

Cause 5 of 10: CreateModel with an empty contentType.
AWS: BadRequestException 'Invalid content type specified: ' — fixed text ending in a colon and a space, with the offending content type appended (empty here).
Emulator: 'Invalid content type specified ' (no colon).
Why: byte-exact message comparison fails.
Fix: CreateModel's contentType refusal message.
Traces: apigateway.create-model-invalid-content-type-rejected

Cause 6 of 10: CreateUsagePlanKey refusal messages.
AWS: a key already associated with the plan is refused with ConflictException 'API Key is already subscribed to Usage Plan' (fixed text); an unsupported keyType is refused with BadRequestException 'Invalid Usage Plan key type specified. Must be one of [API_KEY]' (fixed text). Smithy: CreateUsagePlanKey keyType is a required String and the operation declares BadRequestException and ConflictException.
Emulator: 'API Key 7qygd7aaaa already in Usage Plan 8sh3tqaaaa' and 'Invalid key type specified: NOT_A_TYPE, expected API_KEY' — it substitutes ids and the offending value into messages AWS states without them.
Why: the messages are not AWS's and leak ids AWS does not include.
Fix: CreateUsagePlanKey's refusal messages.
Traces: apigateway.create-usage-plan-key-duplicate-fails, apigateway.create-usage-plan-key-invalid-type-rejected, apigateway.create-usage-plan-key-wrong-type-fails

Cause 7 of 10: VPC-link not-found message casing.
AWS: GetVpcLink/DeleteVpcLink on an unknown or deleted id refuse with NotFoundException 'Invalid Vpc Link identifier specified' (fixed text, exactly that casing).
Emulator: 'Invalid VPC link identifier specified'.
Why: a client comparing the message text sees a mismatch for every VPC-link not-found.
Fix: the not-found message in the VPC-link handlers.
Traces: apigateway.delete-nonexistent-vpc-link-returns-notfound, apigateway.get-vpc-link-nonexistent-not-found, apigateway.vpc-link-lifecycle

Cause 8 of 10: malformed VPC-link target ARN message.
AWS: CreateVpcLink with a targetArn that is not a well-formed ARN refuses with BadRequestException 'Invalid ARN. ARN is not well formed <targetArn>' — fixed text 'Invalid ARN. ARN is not well formed ' plus the offending value from the request. Smithy: CreateVpcLink targetArns is a required list of String and declares BadRequestException (400).
Emulator: 'Invalid ARN specified in the request not-a-valid-nlb-arn' / 'Invalid ARN specified in the request not-an-arn-at-all'.
Why: the wording differs from AWS for every malformed-target refusal.
Fix: CreateVpcLink's targetArns validation message.
Traces: apigateway.create-vpc-link-invalid-target-badrequest, apigateway.vpc-link-invalid-target-arn-rejected

Cause 9 of 10: CreateVpcLink with an empty targetArns list.
AWS: BadRequestException 'Need at least one target arn to create a vpc link' (fixed text).
Emulator: 'targetArns must name at least one target'.
Why: the documented message is not produced.
Fix: CreateVpcLink's targetArns validation message.
Traces: apigateway.create-vpc-link-empty-target-list-rejected

Cause 10 of 10: DeleteVpcLink while an integration references the link.
AWS: BadRequestException "Cannot delete vpc link. Vpc link '<vpcLinkId>', is referenced in [<restApiId>:<httpMethod>:<resourceId>] in format of [RESTApiId:Method:Resource]." — fixed text as shown, with the VPC link ID and the referencing api/method/resource IDS taken from state. Smithy: DeleteVpcLink declares BadRequestException (400) and ConflictException (409).
Emulator: "Cannot delete VPC link '<vpcLinkName>', is referenced in integration: GET//" — it names the link by NAME, not id, and reports the reference as 'GET//' instead of the [RESTApiId:Method:Resource] triple.
Why: a client cannot locate the blocking integration from the message.
Fix: DeleteVpcLink's in-use refusal message.
Traces: apigateway.delete-vpc-link-in-use-conflict, apigateway.delete-vpc-link-in-use-rejected, apigateway.delete-vpc-link-with-active-integration-fails

## Evidence
Representative replay output (verbatim):
- 'AssertionError: unexpected BadRequestException message "1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'createAuthorizerInput.type' failed to satisfy constraint: Member must satisfy enum value set: [TOKEN, REQUEST, COGNITO_USER_POOLS]"' against the asserted 'Invalid AuthorizerType specified.' (Cause 1)
- 'AssertionError: unexpected message 'Invalid request input'' against 'URI is a required field in an Authorizer' (Cause 2)
- 'AssertionError: unexpected ConflictException message 'Stage already exists for this API: orig'' against 'Stage already exists' (Cause 3)
- 'AssertionError: expected BadRequestException on duplicate CreateDomainName, got 'ConflictException'' and 'expected BadRequestException creating a DomainName that already exists, got ConflictException'; asserted message 'The domain name you provided already exists.' (Cause 4)
- 'AssertionError: unexpected BadRequestException message 'Invalid content type specified '' against 'Invalid content type specified: ' (Cause 5)
- 'AssertionError: expected message 'API Key is already subscribed to Usage Plan', got 'API Key 7qygd7aaaa already in Usage Plan 8sh3tqaaaa'' and 'expected message 'Invalid Usage Plan key type specified. Must be one of [API_KEY]', got 'Invalid key type specified: NOT_A_TYPE, expected API_KEY'' (Cause 6)
- 'AssertionError: unexpected message 'Invalid VPC link identifier specified'' against 'Invalid Vpc Link identifier specified' (Cause 7)
- 'AssertionError: unexpected BadRequestException message 'Invalid ARN specified in the request not-a-valid-nlb-arn'' against 'Invalid ARN. ARN is not well formed not-a-valid-nlb-arn' (Cause 8)
- 'AssertionError: unexpected BadRequestException message 'targetArns must name at least one target'' against 'Need at least one target arn to create a vpc link' (Cause 9)
- 'AssertionError: expected ('BadRequestException', "Cannot delete vpc link. Vpc link '8sh3tqaaaa', is referenced in [7qygd7aaaa:GET:6ofuwnbaaa] in format of [RESTApiId:Method:Resource].") deleting a VPC link referenced by an active integration, got ('BadRequestException', "Cannot delete VPC link 'sim2real-vpclink-29e631991eeb', is referenced in integration: GET//")' (Cause 10)

## Steps to Reproduce
1. CreateAuthorizer with type='NOT_A_REAL_TYPE' (Cause 1) and with type='TOKEN' but no authorizerUri (Cause 2).
2. CreateStage twice with the same stageName, and CreateDeployment naming an existing stageName (Cause 3).
3. CreateDomainName twice with the same domainName (Cause 4). CreateModel with contentType='' (Cause 5).
4. CreateUsagePlanKey twice for the same key, and once with keyType='NOT_A_TYPE' (Cause 6).
5. GetVpcLink/DeleteVpcLink on an unknown id (Cause 7); CreateVpcLink with targetArns=['not-a-valid-nlb-arn'] (Cause 8) and targetArns=[] (Cause 9); attach a VPC link to an integration and DeleteVpcLink (Cause 10).
6. Compare each refusal's Code and Message with the values quoted above.
