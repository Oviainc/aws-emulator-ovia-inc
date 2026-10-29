Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.get-graphql-api-not-found appsync.op-contract-listApiKeys appsync.op-contract-listDataSources appsync.op-contract-listSourceApiAssociations appsync.op-contract-get-data-source appsync.create-graphql-api-invalid-visibility-value-rejected appsync.op-contract-listTagsForResource appsync.put-environment-variables-size-limit-rejected appsync.get-domain-name-malformed-input-rejected appsync.get-domain-name-malformed-name-rejected appsync.graphql-api-unicode-name-round-trip appsync.associate-merged-graphql-api-invalid-source-rejected appsync.op-contract-associateSourceGraphqlApi appsync.disassociate-merged-graphql-api-without-association-rejected appsync.op-contract-disassociate-source-graphql-api appsync.delete-nonexistent-api-key-not-found appsync.op-contract-deleteApiKey appsync.get-nonexistent-function-not-found appsync.op-contract-createFunction appsync.op-contract-update-function appsync.op-contract-list-tags-for-resource appsync.op-contract-tagResource appsync.op-contract-untag-resource appsync.op-contract-untagResource appsync.op-contract-tag-resource appsync.tag-resource-reserved-prefix-rejected appsync.put-environment-variables-invalid-key-rejected appsync.op-contract-putGraphqlApiEnvironmentVariables appsync.api-key-limit-exceeded appsync.create-graphql-api-limit-exceeded appsync.create-data-source-type-config-mismatch-rejected appsync.data-source-type-config-mismatch-rejected appsync.data-source-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.get-graphql-api-not-found|run#10|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: appsync.op-contract-listApiKeys|run#14|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: appsync.op-contract-listDataSources|run#16|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: appsync.op-contract-listSourceApiAssociations|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: appsync.op-contract-get-data-source|run#17|AssertionError|assert code2 == "BadRequestException", f"expected BadRequestException, got {code2}"
Issue-Key: appsync.create-graphql-api-invalid-visibility-value-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: appsync.op-contract-listTagsForResource|run#10|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: appsync.put-environment-variables-size-limit-rejected|run#13|AssertionError|assert rejected_code == "BadRequestException", (
Issue-Key: appsync.get-domain-name-malformed-input-rejected|run#4|AssertionError|assert malformed_code == "NotFoundException", (
Issue-Key: appsync.get-domain-name-malformed-name-rejected|run#4|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: appsync.graphql-api-unicode-name-round-trip|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appsync.associate-merged-graphql-api-invalid-source-rejected|run#14|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: appsync.op-contract-associateSourceGraphqlApi|run#21|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: appsync.disassociate-merged-graphql-api-without-association-rejected|run#18|AssertionError|assert err == ("NotFoundException", "No source API association was found"), (
Issue-Key: appsync.op-contract-disassociate-source-graphql-api|run#10|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: appsync.delete-nonexistent-api-key-not-found|run#13|AssertionError|assert err == ("NotFoundException", f"API key not found: {key_id}"), (
Issue-Key: appsync.op-contract-deleteApiKey|run#16|AssertionError|assert msg == f"API key not found: {key_id}", f"unexpected message: {msg!r}"
Issue-Key: appsync.get-nonexistent-function-not-found|run#17|AssertionError|assert delete_message == "Function not found.", (
Issue-Key: appsync.op-contract-createFunction|run#25|AssertionError|assert msg == "Data source not found.", f"unexpected message: {msg!r}"
Issue-Key: appsync.op-contract-update-function|run#26|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: appsync.op-contract-list-tags-for-resource|run#11|AssertionError|assert msg == "Resource was not found", f"unexpected message: {msg!r}"
Issue-Key: appsync.op-contract-tagResource|run#13|AssertionError|assert message == "Resource was not found", message
Issue-Key: appsync.op-contract-untag-resource|run#20|AssertionError|assert message == "Resource was not found", message
Issue-Key: appsync.op-contract-untagResource|run#13|AssertionError|assert message == "Resource was not found", message
Issue-Key: appsync.op-contract-tag-resource|run#11|AssertionError|assert message == "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)", message
Issue-Key: appsync.tag-resource-reserved-prefix-rejected|run#10|AssertionError|assert msg == "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)", (
Issue-Key: appsync.put-environment-variables-invalid-key-rejected|run#10|AssertionError|assert message == (
Issue-Key: appsync.op-contract-putGraphqlApiEnvironmentVariables|run#9|AssertionError|assert message == (
Issue-Key: appsync.api-key-limit-exceeded|run#13|AssertionError|assert limit_message == f"API key limit ({created}) exceeded.", (
Issue-Key: appsync.create-graphql-api-limit-exceeded|run#6|AssertionError|assert limit_error == expected, (
Issue-Key: appsync.create-data-source-type-config-mismatch-rejected|run#7|AssertionError|assert message == "Missing DynamoDB data source config", f"expected the exact refusal message, got {message!r}"
Issue-Key: appsync.data-source-type-config-mismatch-rejected|run#6|AssertionError|assert error == expected, f"expected {expected}, got {error!r}"
Issue-Key: appsync.data-source-type-mismatch-rejected|run#7|AssertionError|assert error == expected, f"expected {expected}, got {error}"
Issue-Commit: 910698ab7b22be6d08fbb15c4abd76821047ddd2

# AppSync control-plane error semantics: not-found/bad-request codes inverted, ValidationException for modeled BadRequestException/NotFoundException cases, and ten non-AWS refusal messages

> _Note: this issue was identified against the code base at commit `910698ab7b22be6d08fbb15c4abd76821047ddd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGraphqlApi

## Description
Ten distinct net-new root causes in how the AppSync emulator classifies and words its refusals. 10 causes in this issue; all ten must be closed before submitting. The open issue 20261024T032443Z-appsync-control-plane-fidelity-domain-name-operations-unimplemented.md names a ValidationException-for-BadRequestException defect, but it carries none of the replay keys below (its own checks are a different set), so Cause 2 here is not covered by it.

=== Cause 1 of 10: the missing-apiId error class is inverted per operation ===
AWS: Smithy (appsync) GetGraphqlApi (GET /v1/apis/{apiId}) models AccessDeniedException|403, BadRequestException|400, InternalFailureException|500, NotFoundException|404, UnauthorizedException|401; recorded behavior for an apiId that was never created is 404 NotFoundException with the fixed message 'API not found.'. Conversely ListApiKeys (GET /v1/apis/{apiId}/apikeys), ListDataSources, ListSourceApiAssociations and GetDataSource (GET /v1/apis/{apiId}/datasources/{name}) answer 400 BadRequestException for an apiId that does not exist — the identifier is treated as malformed/unresolvable input, not as a missing resource.
Emulator: exactly the opposite in both directions. GetGraphqlApi on an unknown apiId returns BadRequestException; ListApiKeys, ListDataSources, ListSourceApiAssociations and GetDataSource on an unknown apiId return NotFoundException.
Why: both HTTP status (400 vs 404) and error code differ from AWS, so client retry/branching logic inverts: SDK code treating NotFoundException as 'create it' loops forever on GetGraphqlApi, and code treating BadRequestException as non-retryable input error now sees it on list calls that AWS classifies that way only for bad input.
Fix: the per-operation api-lookup failure mapping shared by these handlers (the GraphQL-API lookup helper used by GetGraphqlApi vs the api-scoped list/get handlers) — the class must be chosen per operation as AWS does, not from one shared helper.
Traces: appsync.get-graphql-api-not-found, appsync.op-contract-listApiKeys, appsync.op-contract-listDataSources, appsync.op-contract-listSourceApiAssociations, appsync.op-contract-get-data-source.

=== Cause 2 of 10: constraint violations are refused with ValidationException, which AppSync does not model ===
AWS: the appsync Smithy model binds NO ValidationException to any of these operations. CreateGraphqlApi models ApiLimitExceededException|400, BadRequestException|400, ConcurrentModificationException|409, InternalFailureException|500, LimitExceededException|429, UnauthorizedException|401. PutGraphqlApiEnvironmentVariables models AccessDeniedException|403, BadRequestException|400, ConcurrentModificationException|409, NotFoundException|404, UnauthorizedException|401 and bounds environmentVariables with @length max 50. ListTagsForResource models BadRequestException|400 (resourceArn @length 70-75, @pattern ^arn:aws:appsync:...:apis/[0-9A-Za-z_-]{26}$). GetDomainName performs no server-side format/length validation at all: a malformed/overlong domainName is answered 404 NotFoundException.
Emulator: constraint failures surface as ValidationException — an invalid `visibility` value on CreateGraphqlApi, a malformed resourceArn on ListTagsForResource, a 51-entry environmentVariables map on PutGraphqlApiEnvironmentVariables, and a malformed domainName on GetDomainName (where AWS returns NotFoundException).
Why: ValidationException is not in these operations' error sets, so no generated SDK client models it; `except BadRequestException`/`NotFoundException` handlers never match and the call surfaces as an unmodeled error. The status code differs too for the GetDomainName case (400 instead of 404).
Fix: the shared request-constraint validation layer must map AppSync constraint failures to the operation's modeled BadRequestException, and the GetDomainName handler must skip format validation and fall through to its not-found path.
Traces: appsync.create-graphql-api-invalid-visibility-value-rejected, appsync.op-contract-listTagsForResource, appsync.put-environment-variables-size-limit-rejected, appsync.get-domain-name-malformed-input-rejected, appsync.get-domain-name-malformed-name-rejected.

=== Cause 3 of 10: validation-message format leaks the offending value and the regex anchors ===
AWS: recorded behavior for a CreateGraphqlApi `name` violating AppSync's name charset: the message is '1 validation error detected: Value at \'name\' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_\\-\\ ]+'. Fixed text: '1 validation error detected: Value at ... failed to satisfy constraint: Member must satisfy regular expression pattern: ...'; the variable parts are the member name ('name') and the pattern as AWS publishes it — WITHOUT the submitted value and WITHOUT the ^...$ anchors.
Emulator: '1 validation error detected: Value \'sim2real-uni-...-café-测试-😀\' at \'name\' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_\\-\\ ]+$' — it interpolates the rejected value and prints the anchored pattern.
Why: the rejection is right but the message is not AWS's; worse, echoing the submitted value into an error message leaks caller input into logs for every validation failure, which AWS deliberately does not do for this class.
Fix: the validation-message formatter for AppSync constraint failures (omit the value, publish the unanchored pattern).
Traces: appsync.graphql-api-unicode-name-round-trip.

=== Cause 4 of 10: source-API association refusals use each other's error class ===
AWS: Smithy (appsync) AssociateSourceGraphqlApi — POST /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations, models BadRequestException|400, ConcurrentModificationException|409, LimitExceededException|429, NotFoundException|404, UnauthorizedException|401; recorded behavior for a sourceApiIdentifier that is not a valid/eligible source api is 400 BadRequestException. DisassociateMergedGraphqlApi — DELETE /v1/sourceApis/{sourceApiIdentifier}/mergedApiAssociations/{associationId} (and DisassociateSourceGraphqlApi) answer 404 NotFoundException 'No source API association was found' when no association with that associationId exists.
Emulator: the two are swapped — Associate returns NotFoundException, and Disassociate returns 400 BadRequestException with the message 'Invalid SourceApi Association ID <associationId>' (the id is a request value; AWS's message has no id in it).
Why: merged-API orchestration branches on these codes to decide whether to create or to ignore; inverted classes turn 'already gone' into a hard input error and 'bad request' into a retry loop.
Fix: the AssociateSourceGraphqlApi / DisassociateMergedGraphqlApi / DisassociateSourceGraphqlApi handlers' refusal mapping and messages.
Traces: appsync.associate-merged-graphql-api-invalid-source-rejected, appsync.op-contract-associateSourceGraphqlApi, appsync.disassociate-merged-graphql-api-without-association-rejected, appsync.op-contract-disassociate-source-graphql-api.

=== Cause 5 of 10: NotFoundException messages for api keys, functions and data sources are not AWS's ===
AWS: Smithy (appsync) DeleteApiKey (DELETE /v1/apis/{apiId}/apikeys/{id}) models NotFoundException|404; recorded behavior for an id that no longer exists is the message 'API key not found: <id>' — fixed text 'API key not found: ' plus the key id taken from the request. For a missing function the recorded message is the fixed 'Function not found.' (no identifier). For CreateFunction/UpdateFunction naming a dataSourceName that does not exist, the recorded message is the fixed 'Data source not found.' (no identifier).
Emulator: inverted in all three places — DeleteApiKey drops the id ('API key not found.'), the function lookup injects one ('function id c5b6hgzkkunrbdkyo4wojl6fsi not found'), and the data-source lookup injects one ('Data source sim2real_does_not_exist_ds not found').
Why: these messages are the operation's contract for the not-found class; tooling that parses the key id out of the DeleteApiKey message gets nothing, and tooling matching 'Function not found.'/'Data source not found.' never matches.
Fix: the not-found message construction in the DeleteApiKey handler, the function lookup used by GetFunction/DeleteFunction, and the data-source resolution in CreateFunction/UpdateFunction.
Traces: appsync.delete-nonexistent-api-key-not-found, appsync.op-contract-deleteApiKey, appsync.get-nonexistent-function-not-found, appsync.op-contract-createFunction, appsync.op-contract-update-function.

=== Cause 6 of 10: tagging operations on an unknown resource ARN report 'API not found.' instead of AWS's resource wording ===
AWS: Smithy (appsync) TagResource/UntagResource/ListTagsForResource take resourceArn (@length 70-75, @pattern ^arn:aws:appsync:[A-Za-z0-9_/.-]{0,63}:\\d{12}:apis/[0-9A-Za-z_-]{26}$) and model NotFoundException|404; recorded behavior for a well-formed ARN naming no existing resource is NotFoundException with the fixed message 'Resource was not found' (these operations are ARN-generic, not api-specific).
Emulator: NotFoundException 'API not found.' — the GetGraphqlApi message reused for the tagging path.
Why: the tagging APIs are the generic resource-tagging surface; a client tagging any AppSync resource ARN receives a message about an 'API', so generic tagging tooling (and resourcegroupstaggingapi-style wrappers) cannot match or surface the correct diagnosis.
Fix: the resource-ARN resolution shared by the TagResource/UntagResource/ListTagsForResource handlers must emit its own not-found message.
Traces: appsync.op-contract-list-tags-for-resource, appsync.op-contract-tagResource, appsync.op-contract-untag-resource, appsync.op-contract-untagResource.

=== Cause 7 of 10: the reserved aws: tag-key refusal message is not AWS's ===
AWS: recorded behavior for TagResource with a key beginning 'aws:' is 400 BadRequestException with the fixed message 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)' — entirely fixed text, no request values.
Emulator: BadRequestException 'Invalid tag key aws:reserved: a key must be between 1 and 128 characters long, cannot begin with aws:, and can only contain letters, numbers, spaces and the characters + - = . _ : /' — it echoes the submitted key and recites the generic charset rule instead of the reserved-prefix rule.
Why: the reserved-prefix rejection is a documented, separately-worded class; clients that distinguish it from a charset/length violation (to tell the user their key is reserved rather than malformed) cannot.
Fix: the tag-key validation in the TagResource handler — the reserved-prefix branch must precede the charset/length branch and carry its own message.
Traces: appsync.op-contract-tag-resource, appsync.tag-resource-reserved-prefix-rejected.

=== Cause 8 of 10: PutGraphqlApiEnvironmentVariables key rejection uses prose instead of AWS's constraint message ===
AWS: Smithy (appsync) PutGraphqlApiEnvironmentVariables — PUT /v1/apis/{apiId}/environmentVariables, environmentVariables @length max 50 with keys constrained to length 2-64 and pattern ^[A-Za-z]+\\w*$, modeled BadRequestException|400. Recorded behavior for a key violating the pattern: 400 BadRequestException with '1 validation error detected: Value \'{<key>=<value>}\' at \'environmentVariables\' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 2, Member must satisfy regular expression pattern: ^[A-Za-z]+\\w*$]'. Fixed text is the whole frame and the bracketed constraint list; the variable part is the rejected map entry rendered as {key=value}.
Emulator: 400 BadRequestException with a hand-written sentence: 'Invalid environment variable key 1_INVALID: a key must be between 2 and 64 characters long, must begin with a letter, and can only contain letters, numbers, and the underscore character (_).'
Why: the code is right but the message is not AWS's for this class, so clients and gates that match the documented constraint text do not recognize the refusal.
Fix: the environment-variable key validation in the PutGraphqlApiEnvironmentVariables handler must emit the modeled constraint message.
Traces: appsync.put-environment-variables-invalid-key-rejected, appsync.op-contract-putGraphqlApiEnvironmentVariables.

=== Cause 9 of 10: limit-exceeded messages are not AWS's ceiling messages ===
AWS: Smithy (appsync) CreateApiKey models ApiKeyLimitExceededException|400; recorded behavior once an api holds the maximum number of API keys is that error with the message 'API key limit (<limit>) exceeded.' — fixed text 'API key limit (' + the enforced limit from service state + ') exceeded.'. CreateGraphqlApi models ApiLimitExceededException|400; recorded behavior once the account/Region GraphQL-API ceiling is reached is the fixed message 'The service limit for number of GraphQL APIs has been reached.'
Emulator: the correct codes but invented sentences — 'an API may hold at most 50 API keys' and 'an account may hold at most 25 GraphQL APIs in a Region'.
Why: the message is the only human-readable signal of which quota was hit; quota tooling matching AWS's text (or parsing the numeric limit out of the API-key message) breaks.
Fix: the limit-check branches of the CreateApiKey and CreateGraphqlApi handlers (message text only; codes are already right).
Traces: appsync.api-key-limit-exceeded, appsync.create-graphql-api-limit-exceeded.

=== Cause 10 of 10: the missing type-specific data-source config message names the type wrongly ===
AWS: recorded behavior for CreateDataSource/UpdateDataSource with type=AMAZON_DYNAMODB and no dynamodbConfig is 400 BadRequestException with the message 'Missing DynamoDB data source config' — the variable part is the data-source type's short name as AWS renders it in this message ('DynamoDB', 'Lambda', ...), not the full service name.
Emulator: 400 BadRequestException 'Missing Amazon DynamoDB data source config' — it renders the type's full display name.
Why: same refusal class, wrong message; clients that assert the per-type required-config diagnosis (the only way to tell which config block was missing) do not match.
Fix: the data-source type/config validation message in the CreateDataSource (and UpdateDataSource) handler.
Traces: appsync.create-data-source-type-config-mismatch-rejected, appsync.data-source-type-config-mismatch-rejected, appsync.data-source-type-mismatch-rejected.

## Evidence
Cause 1 — GetGraphqlApi(apiId='nonexistent-...'): expected ('NotFoundException', 'API not found.'), actual 'BadRequestException' (check_appsync_get_graphql_api_not_found.py line 40). ListApiKeys/ListDataSources/ListSourceApiAssociations/GetDataSource on an unknown apiId: 'expected BadRequestException, got NotFoundException'.
Cause 2 — 'expected BadRequestException, got ValidationException' (create-graphql-api-invalid-visibility-value-rejected; op-contract-listTagsForResource); 'expected BadRequestException for a 51-entry environment variable map, got ValidationException'; 'expected NotFoundException (real AWS performs no server-side format/length validation for this lookup), got \'ValidationException\'' and 'expected NotFoundException, got ValidationException' (get-domain-name-malformed-input-rejected, get-domain-name-malformed-name-rejected).
Cause 3 — expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_\\-\\ ]+", got "1 validation error detected: Value 'sim2real-uni-295ee8031d81-café-测试-😀' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9_\\-\\ ]+$".
Cause 4 — 'expected BadRequestException, got NotFoundException' (associate-merged-graphql-api-invalid-source-rejected, op-contract-associateSourceGraphqlApi); expected ('NotFoundException', 'No source API association was found'), got ('BadRequestException', 'Invalid SourceApi Association ID 828b2e6e-f458-46e7-aa7d-4c64f8052d38') (disassociate-merged-graphql-api-without-association-rejected); 'expected NotFoundException, got BadRequestException' (op-contract-disassociate-source-graphql-api).
Cause 5 — expected 'API key not found: <keyId>', got 'API key not found.'; expected 'Function not found.', got 'function id c5b6hgzkkunrbdkyo4wojl6fsi not found'; expected 'Data source not found.', got 'Data source sim2real_does_not_exist_ds not found' / 'Data source sim2real_missing_3b1ac1570ae1 not found'.
Cause 6 — expected 'Resource was not found', got 'API not found.' (op-contract-list-tags-for-resource, op-contract-tagResource, op-contract-untag-resource, op-contract-untagResource).
Cause 7 — expected 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)', got 'Invalid tag key aws:reserved: a key must be between 1 and 128 characters long, cannot begin with aws:, and can only contain letters, numbers, spaces and the characters + - = . _ : /'.
Cause 8 — expected "1 validation error detected: Value '{1_INVALID=x}' at 'environmentVariables' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 2, Member must satisfy regular expression pattern: ^[A-Za-z]+\\w*$]", got 'Invalid environment variable key 1_INVALID: a key must be between 2 and 64 characters long, must begin with a letter, and can only contain letters, numbers, and the underscore character (_).' (identical shape for key 1_BAD_KEY in op-contract-putGraphqlApiEnvironmentVariables).
Cause 9 — expected 'API key limit (50) exceeded.', got 'an API may hold at most 50 API keys'; expected ('ApiLimitExceededException', 'The service limit for number of GraphQL APIs has been reached.'), got ('ApiLimitExceededException', 'an account may hold at most 25 GraphQL APIs in a Region').
Cause 10 — expected ('BadRequestException', 'Missing DynamoDB data source config'), got ('BadRequestException', 'Missing Amazon DynamoDB data source config') (3 checks).

## Steps to Reproduce
Cause 1: CreateGraphqlApi (positive control), then GetGraphqlApi(apiId='nonexistent-x') -> must be 404 NotFoundException 'API not found.'; ListApiKeys / ListDataSources / ListSourceApiAssociations / GetDataSource with a nonexistent apiId -> must be 400 BadRequestException.
Cause 2: CreateGraphqlApi(visibility='NOT_A_VISIBILITY') -> 400 BadRequestException; ListTagsForResource(resourceArn='not-an-arn') -> 400 BadRequestException; PutGraphqlApiEnvironmentVariables with 51 entries -> 400 BadRequestException; GetDomainName(domainName='!!! not a domain !!!') -> 404 NotFoundException.
Cause 3: CreateGraphqlApi(name='sim2real-uni-café-测试-😀', authenticationType='API_KEY') -> BadRequestException whose message omits the submitted value and the pattern anchors.
Cause 4: CreateGraphqlApi (non-merged) then AssociateSourceGraphqlApi(mergedApiIdentifier=<that api>, sourceApiIdentifier=<ineligible/unknown>) -> 400 BadRequestException; DisassociateMergedGraphqlApi(sourceApiIdentifier=<api>, associationId=<random uuid>) -> 404 NotFoundException 'No source API association was found'.
Cause 5: CreateApiKey then DeleteApiKey twice -> second must say 'API key not found: <id>'; GetFunction(apiId, functionId=<unknown>) -> 'Function not found.'; CreateFunction/UpdateFunction with dataSourceName=<unknown> -> 'Data source not found.'
Cause 6: ListTagsForResource / TagResource / UntagResource with a well-formed appsync apis/ ARN for a deleted api -> NotFoundException 'Resource was not found'.
Cause 7: TagResource(resourceArn=<existing api arn>, tags={'aws:reserved': 'x'}) -> BadRequestException 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'.
Cause 8: PutGraphqlApiEnvironmentVariables(apiId, environmentVariables={'1_INVALID': 'x'}) -> BadRequestException with the modeled constraint message; the previously stored map must be unchanged.
Cause 9: CreateApiKey until the api-key ceiling -> ApiKeyLimitExceededException 'API key limit (50) exceeded.'; CreateGraphqlApi until the Region ceiling -> ApiLimitExceededException 'The service limit for number of GraphQL APIs has been reached.'
Cause 10: CreateDataSource(apiId, name, type='AMAZON_DYNAMODB') with no dynamodbConfig -> BadRequestException 'Missing DynamoDB data source config'.
