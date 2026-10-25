Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.update-rest-api-immutable-field-rejected apigateway.update-integration-type-immutable-rejected apigateway.update-method-response-invalid-status-bad-request apigateway.authorizer-ttl-exceeds-max-rejected apigateway.authorizer-ttl-out-of-range-rejected apigateway.update-authorizer-invalid-ttl-error-contract apigateway.authorizer-token-missing-identitysource-badrequest apigateway.create-usage-plan-negative-quota-limit-rejected apigateway.create-usage-plan-negative-throttle-rate-rejected apigateway.create-documentation-part-duplicate-location-rejected apigateway.documentation-part-duplicate-location-rejected apigateway.create-documentation-version-duplicate-conflict apigateway.documentation-version-duplicate-creation-rejected apigateway.delete-nonexistent-gateway-response-returns-notfound apigateway.import-documentation-parts-malformed-body-rejected apigateway.import-rest-api-malformed-spec-rejected apigateway.op-contract-createRestApi apigateway.op-contract-getRestApis apigateway.put-integration-timeout-out-of-range-rejected apigateway.update-stage-invalid-canary-percent-rejected apigateway.vpc-link-integration-missing-connection-id-rejected apigateway.tag-resource-reserved-prefix-rejected apigateway.create-api-key-limit-exceeded-quota
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.update-rest-api-immutable-field-rejected|run#5|AssertionError|assert err.get("Message") == "Invalid patch path /id", (
Issue-Key: apigateway.update-integration-type-immutable-rejected|run#12|AssertionError|assert err.get("Message") == "Invalid patch path /type", (
Issue-Key: apigateway.update-method-response-invalid-status-bad-request|run#13|AssertionError|assert err.get("Message") == "Invalid patch path /statusCode", (
Issue-Key: apigateway.authorizer-ttl-exceeds-max-rejected|run#12|AssertionError|assert message == "Authorizer result TTL outside allowable range. TTL must be between 0 and 3600 seconds.", (
Issue-Key: apigateway.authorizer-ttl-out-of-range-rejected|run#10|AssertionError|assert message == _TTL_RANGE_MESSAGE, f"expected the 3600s bound named in the message, got: {message!r}"
Issue-Key: apigateway.update-authorizer-invalid-ttl-error-contract|run#12|AssertionError|assert message == "Authorizer result TTL outside allowable range. TTL must be between 0 and 3600 seconds.", (
Issue-Key: apigateway.authorizer-token-missing-identitysource-badrequest|run#8|AssertionError|assert message == "Invalid request input", f"unexpected message {message!r}"
Issue-Key: apigateway.create-usage-plan-negative-quota-limit-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: apigateway.create-usage-plan-negative-throttle-rate-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: apigateway.create-documentation-part-duplicate-location-rejected|run#8|AssertionError|assert message == ("Documentation part already exists for the specified location: type 'API'."), f"unexpected ConflictException message {message!r}"
Issue-Key: apigateway.documentation-part-duplicate-location-rejected|run#8|AssertionError|assert err == expected, f"expected {expected!r}, got {err!r}"
Issue-Key: apigateway.create-documentation-version-duplicate-conflict|run#10|AssertionError|assert message == f"Documentation version {version} already exists.", (
Issue-Key: apigateway.documentation-version-duplicate-creation-rejected|run#11|AssertionError|assert err == expected, f"expected {expected!r}, got {err!r}"
Issue-Key: apigateway.delete-nonexistent-gateway-response-returns-notfound|run#7|AssertionError|assert message1 == "Gateway response type not defined on api", message1
Issue-Key: apigateway.import-documentation-parts-malformed-body-rejected|run#9|AssertionError|assert message == "Unable to build importer with provided input.", f"unexpected message {message!r}"
Issue-Key: apigateway.import-rest-api-malformed-spec-rejected|run#16|AssertionError|assert message == "Invalid OAS input", f"unexpected rejection message {message!r}"
Issue-Key: apigateway.op-contract-createRestApi|run#16|AssertionError|assert err.get("Message") == "Invalid minimum compression size, must be between 0 and 10485760", (
Issue-Key: apigateway.op-contract-getRestApis|run#14|AssertionError|assert message == (
Issue-Key: apigateway.put-integration-timeout-out-of-range-rejected|run#10|AssertionError|assert message == (
Issue-Key: apigateway.update-stage-invalid-canary-percent-rejected|run#15|AssertionError|assert err.get("Message") == "Invalid traffic settings for stage canary. Must be between 0 and 100", (
Issue-Key: apigateway.vpc-link-integration-missing-connection-id-rejected|run#9|AssertionError|assert (err.get("Code"), err.get("Message")) == expected, f"expected {expected!r}, got {err!r}"
Issue-Key: apigateway.tag-resource-reserved-prefix-rejected|run#10|AssertionError|assert err.get("Message") == "Invalid tag key. Tag keys cannot start with 'aws:' (case insensitive)", (
Issue-Key: apigateway.create-api-key-limit-exceeded-quota|run#16|AssertionError|assert message == (
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# API Gateway control plane: 10 causes of invented refusal-message text — the emulator substitutes its own wording for the fixed messages AWS emits for these conditions

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRestApi

## Description
Ten causes, each a family of refusals whose message AWS emits as a fixed constant for the named condition (confirmed by the blessed replay that produced these checks) and that the emulator replaces with its own request-echoing wording. The HTTP status and error code match in every case below; only the message diverges, and here the rule producing the message IS known — AWS's text is a constant tied to the condition, not derived from the request — so it is part of the contract a client's log/assertion tooling reads. Each cause is numbered with its own failing check ids; all ten must be closed before this issue is submitted.

Cause 1 of 10: an invalid JSON-Patch path is reported with a verbose generated listing instead of AWS's constant.
AWS: for UpdateRestApi/UpdateIntegration/UpdateMethodResponse, a patch naming an unsupported path is refused with BadRequestException whose message is exactly "Invalid patch path <path>" (e.g. "Invalid patch path /id"). The rule is known: the literal prefix plus the offending path from the request, nothing else.
Emulator: emits a long message that also enumerates every allowed path, e.g. "Invalid patch path  '/id' specified for op 'replace'. Must be one of: [/apiKeySource, /binaryMediaTypes/<media-type>, ...]" (note also the doubled space). Same shape for '/type' on UpdateIntegration and '/statusCode' on UpdateMethodResponse.
Why: tooling that matches on the refusal text to decide which field to drop from a patch set does not match, and the emulator's enumeration advertises an allowed-path set that is not AWS's.
Fix: the shared JSON-Patch path validator used by the Update* handlers.
Checks: apigateway.update-rest-api-immutable-field-rejected, apigateway.update-integration-type-immutable-rejected, apigateway.update-method-response-invalid-status-bad-request

Cause 2 of 10: authorizer validation messages.
AWS: an authorizerResultTtlInSeconds outside 0..3600 is refused with the constant "Authorizer result TTL outside allowable range. TTL must be between 0 and 3600 seconds." (on both CreateAuthorizer and UpdateAuthorizer); a TOKEN authorizer created without identitySource is refused with the constant "Invalid request input".
Emulator: emits request-derived wording instead — 'authorizerResultTtlInSeconds must be between 0 and 3600, got 3601', '/authorizerResultTtlInSeconds must be between 0 and 3600, inclusive: 3601', 'identitySource is required for an authorizer of type TOKEN'.
Why: the refusals are not attributable by text, and the emulator's TOKEN message asserts a specific cause where AWS deliberately says only "Invalid request input".
Fix: CreateAuthorizer/UpdateAuthorizer validation.
Checks: apigateway.authorizer-ttl-exceeds-max-rejected, apigateway.authorizer-ttl-out-of-range-rejected, apigateway.update-authorizer-invalid-ttl-error-contract, apigateway.authorizer-token-missing-identitysource-badrequest

Cause 3 of 10: usage-plan quota/throttle validation messages.
AWS: a negative quota limit is refused with the constant "Usage Plan quota limit must be a non-negative numeric"; a negative throttle rate with "Usage Plan throttle rate limit and burst limit must be numeric and greater than zero".
Emulator: 'quota.limit must not be negative, got -5' and 'throttle.rateLimit must be a non-negative number, got -5'.
Fix: CreateUsagePlan validation.
Checks: apigateway.create-usage-plan-negative-quota-limit-rejected, apigateway.create-usage-plan-negative-throttle-rate-rejected

Cause 4 of 10: documentation part/version conflict messages.
AWS: a duplicate documentation-part location is refused with ConflictException "Documentation part already exists for the specified location: type 'API'."; a duplicate documentation version with ConflictException "Documentation version <version> already exists."
Emulator: 'Documentation part already exists for the specified location: type=API' and 'Documentation version already exists: 1.0.0' — same code, re-ordered/re-punctuated text.
Fix: CreateDocumentationPart / CreateDocumentationVersion conflict reporting.
Checks: apigateway.create-documentation-part-duplicate-location-rejected, apigateway.documentation-part-duplicate-location-rejected, apigateway.create-documentation-version-duplicate-conflict, apigateway.documentation-version-duplicate-creation-rejected

Cause 5 of 10: deleting a gateway response type that was never customized.
AWS: refuses with the constant "Gateway response type not defined on api".
Emulator: 'No customized gateway response specified for response type DEFAULT_4XX'.
Fix: DeleteGatewayResponse's not-found path.
Checks: apigateway.delete-nonexistent-gateway-response-returns-notfound

Cause 6 of 10: importer rejection messages.
AWS: a malformed ImportDocumentationParts body is refused with the constant "Unable to build importer with provided input."; a malformed OpenAPI document on ImportRestApi with the constant "Invalid OAS input".
Emulator: 'body must be a UTF-8 encoded JSON object carrying the documentation parts to import' and 'Invalid OpenAPI input.'
Fix: the ImportDocumentationParts and ImportRestApi body parsers' refusal paths.
Checks: apigateway.import-documentation-parts-malformed-body-rejected, apigateway.import-rest-api-malformed-spec-rejected

Cause 7 of 10: CreateRestApi minimumCompressionSize and GetRestApis position messages.
AWS: an out-of-range minimumCompressionSize is refused with the constant "Invalid minimum compression size, must be between 0 and 10485760"; GetRestApis with an unparsable position is refused with AWS's constant position wording.
Emulator: 'minimumCompressionSize must be between 0 and 10485760, inclusive: -1' and 'Invalid position: not-a-real-pagination-token'.
Fix: CreateRestApi validation and the GetRestApis pagination-token decoder.
Checks: apigateway.op-contract-createRestApi, apigateway.op-contract-getRestApis

Cause 8 of 10: numeric-range refusals on PutIntegration, UpdateStage canary and the VPC_LINK connectionId rule.
AWS: PutIntegration timeoutInMillis out of range, UpdateStage /canarySettings/percentTraffic out of range, and a VPC_LINK integration missing connectionId are each refused with a fixed message; for the last the recorded text is "ConnectionId should be set to the vpcLinkId or stage variable on connection type VPC_LINK".
Emulator: 'timeoutInMillis must be between 50 and 29000 milliseconds, got 30001', '/canarySettings/percentTraffic must be between 0 and 100, inclusive: 150', and 'connectionId is required when connectionType is VPC_LINK'.
Fix: PutIntegration / UpdateStage validation.
Checks: apigateway.put-integration-timeout-out-of-range-rejected, apigateway.update-stage-invalid-canary-percent-rejected, apigateway.vpc-link-integration-missing-connection-id-rejected

Cause 9 of 10: TagResource reserved-prefix refusal message.
AWS: refuses a tag key beginning with the reserved "aws:" prefix with its own fixed wording.
Emulator: 'a tag key must not start with the reserved prefix aws:, got aws:Reserved'.
Fix: TagResource key validation.
Checks: apigateway.tag-resource-reserved-prefix-rejected

Cause 10 of 10: LimitExceededException stage-quota message.
AWS: exceeding the per-API stage quota is refused with LimitExceededException whose message is the constant "Maximum number of Stages for this API has been reached.  Please contact AWS if you need additional Stages." (two spaces after the period, as AWS emits it).
Emulator: 'Maximum number of stages exceeded for this API: 10' — correct code and status, invented text.
Fix: the per-API stage quota check's error construction.
Checks: apigateway.create-api-key-limit-exceeded-quota

## Evidence
All of the below match AWS on status and error code; the message is what diverges.

Cause 1: expected 'Invalid patch path /id'; got "Invalid patch path  '/id' specified for op 'replace'. Must be one of: [/apiKeySource, /binaryMediaTypes/<media-type>, /description, /disableExecuteApiEndpoint, /endpointAccessMode, /endpointConfiguration/ipAddressType, /endpointConfiguration/types/<endpoint-type>, /endpointConfiguration/vpcEndpointIds/<vpc-endpoint-id>, /minimumCompressionSize, /name, /policy, /securityPolicy, /version]". Same for '/type' (UpdateIntegration) and '/statusCode' (UpdateMethodResponse).
Cause 2: expected 'Authorizer result TTL outside allowable range. TTL must be between 0 and 3600 seconds.'; got 'authorizerResultTtlInSeconds must be between 0 and 3600, got 3601' / '/authorizerResultTtlInSeconds must be between 0 and 3600, inclusive: 3601'. Expected 'Invalid request input'; got 'identitySource is required for an authorizer of type TOKEN'.
Cause 3: expected 'Usage Plan quota limit must be a non-negative numeric'; got 'quota.limit must not be negative, got -5'. Expected 'Usage Plan throttle rate limit and burst limit must be numeric and greater than zero'; got 'throttle.rateLimit must be a non-negative number, got -5'.
Cause 4: expected ('ConflictException', "Documentation part already exists for the specified location: type 'API'."); got ('ConflictException', 'Documentation part already exists for the specified location: type=API'). Expected ('ConflictException', 'Documentation version 1.0.0 already exists.'); got ('ConflictException', 'Documentation version already exists: 1.0.0').
Cause 5: expected 'Gateway response type not defined on api'; got 'No customized gateway response specified for response type DEFAULT_4XX'.
Cause 6: expected 'Unable to build importer with provided input.'; got 'body must be a UTF-8 encoded JSON object carrying the documentation parts to import'. Expected 'Invalid OAS input'; got 'Invalid OpenAPI input.'
Cause 7: expected 'Invalid minimum compression size, must be between 0 and 10485760'; got 'minimumCompressionSize must be between 0 and 10485760, inclusive: -1'. GetRestApis: got 'Invalid position: not-a-real-pagination-token'.
Cause 8: got 'timeoutInMillis must be between 50 and 29000 milliseconds, got 30001'; got '/canarySettings/percentTraffic must be between 0 and 100, inclusive: 150'; expected ('BadRequestException', 'ConnectionId should be set to the vpcLinkId or stage variable on connection type VPC_LINK'), got {'Code': 'BadRequestException', 'Message': 'connectionId is required when connectionType is VPC_LINK'}.
Cause 9: got 'a tag key must not start with the reserved prefix aws:, got aws:Reserved'.
Cause 10: expected 'Maximum number of Stages for this API has been reached.  Please contact AWS if you need additional Stages.'; got 'Maximum number of stages exceeded for this API: 10'.

## Steps to Reproduce
For each cause, issue the named call against the emulator and read the Message of the returned error:
1. UpdateRestApi patchOperations=[{op:replace, path:/id, value:x}] → message should be exactly 'Invalid patch path /id'. Likewise UpdateIntegration /type and UpdateMethodResponse /statusCode.
2. CreateAuthorizer/UpdateAuthorizer with authorizerResultTtlInSeconds=3601; CreateAuthorizer type=TOKEN with no identitySource.
3. CreateUsagePlan with quota.limit=-5; CreateUsagePlan with throttle.rateLimit=-5.
4. CreateDocumentationPart twice for the same type=API location; CreateDocumentationVersion twice for '1.0.0'.
5. DeleteGatewayResponse responseType=DEFAULT_4XX on an API with no customized gateway response.
6. ImportDocumentationParts with a non-JSON body; ImportRestApi with a malformed OpenAPI document.
7. CreateRestApi minimumCompressionSize=-1; GetRestApis position='not-a-real-pagination-token'.
8. PutIntegration timeoutInMillis=30001; UpdateStage /canarySettings/percentTraffic=150; PutIntegration connectionType=VPC_LINK with no connectionId.
9. TagResource with tag key 'aws:Reserved'.
10. Create stages until the per-API stage quota is exceeded.
Then replay the check ids in trace_ids.
