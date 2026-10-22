Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-list-api-keys-contract
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-list-api-keys-contract appsync-cfg-graphql-api-limits-and-metrics appsync-error-list-pagination-invalid-token appsync-event-api-channel-namespace appsync-event-api-crud-lifecycle appsync-event-api-full-lifecycle appsync-graphql-api-core-lifecycle appsync-graphql-api-create-delete-not-found
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|4xx->4xx|$.message
Issue-Key: appsync|ListApiKeys|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|ListApiKeys|4xx->4xx|status
Issue-Key: appsync|CreateGraphqlApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|ListGraphqlApis|4xx->4xx|$.message
Issue-Key: appsync|CreateApi|4xx->4xx|$.message
Issue-Key: appsync|GetGraphqlApi|4xx->4xx|$.message
Issue-Key: appsync|DeleteGraphqlApi|4xx->4xx|$.message
Issue-Related: 20261022T112243Z-appsync-creategraphqlapi-rejects-valid-api-names-and-omits-dns.md
Issue-Commit: 6f223cb07d8f19a9e48b2b4f7aac2665ac919e98

# AppSync error responses carry wrong error type, status and message

> _Note: this issue was identified against the code base at commit `6f223cb07d8f19a9e48b2b4f7aac2665ac919e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
AppSync's error path diverges from the recorded contract: a malformed identifier is classified as not-found instead of a bad request, a modeled range violation is emitted as a generic ValidationException rather than the service's own BadRequestException, and the service's fixed error texts are replaced by the emulator's own wording. This bundle carries 6 causes; all 6 must be closed.

Cause 1 of 6: ListApiKeys returns 404/NotFoundException for a malformed apiId instead of 400/BadRequestException (fixture appsync-list-api-keys-contract, step task#4).
AWS: Smithy: ListApiKeys http {GET /v1/apis/{apiId}/apikeys}; errors include BadRequestException (httpError 400) and NotFoundException (httpError 404). The recording shows AWS answering 400 for the syntactically invalid apiId 'f3a91c07e6b5d0a48c2e1976badcafe0' — an id that is not a well-formed AppSync API id is a bad request, not a missing resource. Recorded message 'Invalid API ID f3a91c07e6b5d0a48c2e1976badcafe0' is an example of the rule: name the rejected apiId.
Emulator: 404 with x-amzn-errortype NotFoundException and 'No GraphQL API found with ID f3a91c07e6b5d0a48c2e1976badcafe0'.
Why: the status class and error code are what SDKs branch on. A client that retries or creates on NotFoundException, and surfaces a validation failure on BadRequestException, takes the wrong branch for a malformed id — and a 404 is commonly treated as benign/absent rather than as caller error.
Fix: ListApiKeys' input validation — validate apiId shape before the state lookup and raise BadRequestException (400); reserve NotFoundException for a well-formed id that is absent.
Traces: appsync-list-api-keys-contract#task#4.

Cause 2 of 6: CreateGraphqlApi's queryDepthLimit range violation is emitted as a generic ValidationException instead of AppSync's BadRequestException (fixture appsync-cfg-graphql-api-limits-and-metrics, step task#1; 4 steps in cluster).
AWS: Smithy: CreateGraphqlApiRequest.queryDepthLimit targets QueryDepthLimit with @range min 0 max 75; the modeled error for a malformed request is BadRequestException (httpError 400) — CreateGraphqlApi does not model ValidationException at all. Recorded message 'Invalid queryDepthLimit given. Must fall within range 0 to 75' is an example of the rule: AppSync reports the offending member and its modeled range.
Emulator: 400 with x-amzn-errortype: ValidationException and the Smithy-generic '1 validation error detected: Value '76' at 'queryDepthLimit' failed to satisfy constraint: Member must have value less than or equal to 75'.
Why: the status matches but the error code does not. SDK error unions for AppSync model BadRequestException (with its reason/detail members); an unmodeled ValidationException deserializes as an unknown/service error, so client code matching on BadRequestException never fires and the typed reason/detail are unavailable.
Fix: CreateGraphqlApi's validation — raise the service's BadRequestException for range violations on queryDepthLimit and resolverCountLimit instead of the framework's generic constraint error.
Traces: appsync-cfg-graphql-api-limits-and-metrics#task#1 (+3 affected: task#2, task#3, task#4).

Cause 3 of 6: ListGraphqlApis rejects a bad nextToken with its own wording (fixture appsync-error-list-pagination-invalid-token, step task#2).
AWS: recorded behavior only — Smithy models nextToken as PaginationToken (@length 1-65536, @pattern ^[\S]+$) and ListGraphqlApis' BadRequestException (httpError 400), but not the text. The recording shows 400 with the fixed, request-independent text 'Invalid pagination token given.'
Emulator: 400 with 'nextToken does not name a page of this list'.
Why: status and class agree, so this is the lowest-value cause in the bundle; it matters only for clients and test suites that assert AppSync's constant text. Hypothesis: the pagination layer surfaces its own generic message rather than the service's.
Fix: ListGraphqlApis' pagination-token handling — emit the service's constant invalid-token text.
Traces: appsync-error-list-pagination-invalid-token#task#2.

Cause 4 of 6: CreateApi's enum validation message does not follow AppSync's recorded construction (fixture appsync-event-api-channel-namespace, step task#1; 3 steps in cluster).
AWS: Smithy: CreateApi http {POST /v2/apis}, eventConfig required; AuthenticationType is an enum with values [API_KEY, AWS_IAM, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_LAMBDA]. The recording shows 400 aggregating all three offending auth-mode lists into one '3 validation errors detected: ...' message, each clause naming the member path and the permitted enum set WITHOUT echoing the rejected value, in the order defaultPublishAuthModes, connectionAuthModes, defaultSubscribeAuthModes and with the enum set rendered [AWS_LAMBDA, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_IAM, API_KEY]. The rule is request-derived, so it is a requirement: one clause per offending member path, no echoed value.
Emulator: 400 with the right count and member paths, but each clause echoes "Value 'INVALID_AUTH' at ..." and renders the enum set and the clause order differently.
Why: clients that parse the aggregated validation message per member path get an extra echoed value they do not expect; the status and the set of flagged members are otherwise correct, so impact is limited to message-level parsing.
Fix: CreateApi's validation message builder — omit the rejected value from enum clauses and order the clauses/enum set as AppSync does.
Traces: appsync-event-api-channel-namespace#task#1 (+2 affected: appsync-event-api-crud-lifecycle#task#5, appsync-event-api-full-lifecycle#task#5).

Cause 5 of 6: GetGraphqlApi's not-found text is the emulator's own (fixture appsync-graphql-api-core-lifecycle, step task#6).
AWS: Smithy: GetGraphqlApi errors include NotFoundException (httpError 404), documented 'The resource specified in the request was not found.' Recorded behavior only for the text: 404 with the fixed, request-independent 'API not found.'
Emulator: 404 with 'No GraphQL API found with ID nonexistent0000000000' — status and class are right, the constant text is not.
Why: low impact — SDK branching is unaffected; only assertions on AppSync's constant text break.
Fix: GetGraphqlApi's handler/error mapping — emit the service's constant not-found text.
Traces: appsync-graphql-api-core-lifecycle#task#6.

Cause 6 of 6: DeleteGraphqlApi's not-found text is the emulator's own (fixture appsync-graphql-api-create-delete-not-found, step task#3).
AWS: Smithy: DeleteGraphqlApi errors include NotFoundException (httpError 404). Recorded behavior only for the text: 404 with 'API not found.'
Emulator: 404 with 'No GraphQL API found with ID aiutd6lgacjniavmoda09c2mh8'. Same mapping defect as Cause 5 on a different operation, so both call sites must be changed.
Why: low impact, as Cause 5.
Fix: DeleteGraphqlApi's handler/error mapping — share one not-found error constructor with GetGraphqlApi.
Traces: appsync-graphql-api-create-delete-not-found#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (91 fixtures, 736 steps, 45 passed, 182 mismatched, score 6.1%).

Cause 1 — appsync-list-api-keys-contract#task#4, ListApiKeys: status: expected 400, got 404; $headers.x-amzn-errortype: expected <shaped-not-asserted>, got 'NotFoundException'; $.message: expected 'Invalid API ID f3a91c07e6b5d0a48c2e1976badcafe0', got 'No GraphQL API found with ID f3a91c07e6b5d0a48c2e1976badcafe0'.
Cause 2 — appsync-cfg-graphql-api-limits-and-metrics#task#1, CreateGraphqlApi: expected 400 / got 400; $headers.x-amzn-errortype: expected <shaped-not-asserted>, got 'ValidationException'; $.message: expected 'Invalid queryDepthLimit given. Must fall within range 0 to 75', got "1 validation error detected: Value '76' at 'queryDepthLimit' failed to satisfy constraint: Member must have value less than or equal to 75". affected_count 4.
Cause 3 — appsync-error-list-pagination-invalid-token#task#2, ListGraphqlApis: expected 400 / got 400; $.message: expected 'Invalid pagination token given.', got 'nextToken does not name a page of this list'.
Cause 4 — appsync-event-api-channel-namespace#task#1, CreateApi: expected 400 / got 400; $.message mismatch — expected clauses of the form "Value at 'eventConfig.defaultPublishAuthModes.1.member.authType' failed to satisfy constraint: Member must satisfy enum value set: [AWS_LAMBDA, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_IAM, API_KEY]", got clauses of the form "Value 'INVALID_AUTH' at 'eventConfig.connectionAuthModes.1.member.authType' failed to satisfy constraint: Member must satisfy enum value set: [API_KEY, AWS_IAM, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_LAMBDA]". affected_count 3.
Cause 5 — appsync-graphql-api-core-lifecycle#task#6, GetGraphqlApi: expected 404 / got 404; $.message: expected 'API not found.', got 'No GraphQL API found with ID nonexistent0000000000'.
Cause 6 — appsync-graphql-api-create-delete-not-found#task#3, DeleteGraphqlApi: expected 404 / got 404; $.message: expected 'API not found.', got 'No GraphQL API found with ID aiutd6lgacjniavmoda09c2mh8'.

Model evidence: ListApiKeys errors {BadRequestException 400, NotFoundException 404, InternalFailureException 500, UnauthorizedException 401} — no ValidationException on any of these operations; CreateGraphqlApiRequest.queryDepthLimit -> QueryDepthLimit @range 0..75; AuthenticationType enum values [API_KEY, AWS_IAM, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_LAMBDA]; BadRequestException httpError 400 with members message/reason/detail; NotFoundException httpError 404.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: GET /v1/apis/f3a91c07e6b5d0a48c2e1976badcafe0/apikeys (fixture appsync-list-api-keys-contract, step task#4). Expect 400 with BadRequestException naming the invalid API ID; observe 404 with x-amzn-errortype NotFoundException.
3. Cause 2: POST /v1/apis with queryDepthLimit 76 (fixture appsync-cfg-graphql-api-limits-and-metrics, step task#1). Expect 400 with AppSync's BadRequestException reporting the 0..75 range; observe 400 with x-amzn-errortype ValidationException and the generic constraint text.
4. Cause 3: GET /v1/apis?nextToken=<bogus> (fixture appsync-error-list-pagination-invalid-token, step task#2). Expect 400 'Invalid pagination token given.'; observe 'nextToken does not name a page of this list'.
5. Cause 4: POST /v2/apis with INVALID_AUTH in eventConfig.connectionAuthModes, defaultPublishAuthModes and defaultSubscribeAuthModes (fixture appsync-event-api-channel-namespace, step task#1). Expect a 3-error aggregated message whose clauses do not echo the rejected value; observe clauses echoing "Value 'INVALID_AUTH'".
6. Cause 5: GET /v1/apis/nonexistent0000000000 (fixture appsync-graphql-api-core-lifecycle, step task#6). Expect 404 'API not found.'; observe 'No GraphQL API found with ID ...'.
7. Cause 6: DELETE /v1/apis/<already-deleted-id> (fixture appsync-graphql-api-create-delete-not-found, step task#3). Expect 404 'API not found.'; observe 'No GraphQL API found with ID ...'.
