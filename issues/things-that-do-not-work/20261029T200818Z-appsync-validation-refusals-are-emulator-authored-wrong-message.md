Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-cfg-create-datasource-rejections
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-cfg-create-datasource-rejections appsync-cfg-update-datasource-rules appsync-cfg-resolver-batch-and-caching-rules appsync-datasource-pagination-page-boundary appsync-datasource-function-pipeline-resolver appsync-datasource-function-resolver-pipeline appsync-error-list-pagination-invalid-token appsync-list-apis-pagination-token-error appsync-eval-schema-creation-introspection appsync-event-api-channel-namespace appsync-event-api-crud-lifecycle appsync-event-api-full-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateDataSource|4xx->4xx|$.message
Issue-Key: appsync|CreateDataSource|4xx->2xx|$.message
Issue-Key: appsync|CreateDataSource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateDataSource|4xx->2xx|status
Issue-Key: appsync|UpdateDataSource|4xx->4xx|$.message
Issue-Key: appsync|CreateResolver|4xx->4xx|$.message
Issue-Key: appsync|ListDataSources|4xx->4xx|$.message
Issue-Key: appsync|CreateFunction|4xx->4xx|$.message
Issue-Key: appsync|CreateFunction|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|ListApis|4xx->4xx|$.message
Issue-Key: appsync|GetIntrospectionSchema|4xx->4xx|$.message
Issue-Key: appsync|GetIntrospectionSchema|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateApi|4xx->4xx|$.message
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# AppSync validation refusals are emulator-authored: wrong message text, wrong error class, and one check missing (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
Across the AppSync control plane the request-validation layer answers its own sentences rather than AWS's, in three recurring forms: a diagnostic that names the wrong rule, the generic Smithy constraint renderer (ValidationException + "N validation errors detected: ...") where AWS raises the operation's modeled BadRequestException, and one rule that is not checked at all. 10 net-new causes, all in the operations' validation stage.

Cause 1 of 10 — CreateDataSource with two type configs diagnoses a missing config instead of multiple configs.
AWS: 400 BadRequestException with the template "Multiple configs given for data source of type <type>." — fixed text "Multiple configs given for data source of type " plus the requested data-source type, lowercased as AWS renders it (recorded behavior; Smithy: CreateDataSource errors include BadRequestException httpError 400).
Emulator: 'Missing Amazon DynamoDB data source config' — it reports the opposite fault, i.e. the validator checks only for the config matching the declared type and never notices the extra one.
Why: the diagnostic sends the caller to add a config when the fix is to remove one.
Fix: CreateDataSource validation — count the supplied *Config members and refuse more than one with AWS's template.
Traces: appsync-cfg-create-datasource-rejections#task#3, #task#4, #task#5, #task#8.

Cause 2 of 10 — CreateDataSource does not refuse a RELATIONAL_DATABASE data source with no source type.
AWS: 400 BadRequestException with the fixed sentence "Missing relational database source type" (recorded behavior).
Emulator: 200, no error body, no x-amzn-errortype — the data source is created.
Why: an unusable data source is accepted, and the failure surfaces later (or never) instead of at create time.
Fix: add the missing relationalDatabaseConfig.relationalDatabaseSourceType check to CreateDataSource validation.
Traces: appsync-cfg-create-datasource-rejections#task#9, #task#10, #task#11.

Cause 3 of 10 — UpdateDataSource has the same wrong multiple-configs diagnostic.
AWS: 400 BadRequestException "Multiple configs given for data source of type <type>."
Emulator: 'Missing Amazon DynamoDB data source config'.
Fix: the same multiple-config check in UpdateDataSource validation.
Traces: appsync-cfg-update-datasource-rules#task#4, #task#5.

Cause 4 of 10 — CreateResolver's maxBatchSize refusal is emulator-authored.
AWS: 400 BadRequestException with the template "Invalid value for maxBatchSize. Must be between 0 and 2000 (inclusive)" — all fixed text, with the bounds as AWS words them (unspaced "2000"), and the member name maxBatchSize (recorded behavior).
Emulator: 'Invalid maxBatchSize given. Must fall within range 0 to 2,000' — different sentence, and it renders the bound with a thousands separator.
Why: text-asserting clients diverge and the message no longer matches AWS documentation.
Fix: CreateResolver validation's maxBatchSize refusal message.
Traces: appsync-cfg-resolver-batch-and-caching-rules#task#6.

Cause 5 of 10 — ListDataSources' maxResults refusal is emulator-authored.
AWS: 400 BadRequestException with the fixed sentence "maxResults must be greater than 0 and less than or equal to 25." (recorded behavior).
Emulator: 'Invalid maxResults given. Must fall within range 0 to 25' — a different sentence that also states the wrong lower bound (0 rather than "greater than 0").
Fix: ListDataSources validation's maxResults refusal message.
Traces: appsync-datasource-pagination-page-boundary#task#10.

Cause 6 of 10 — CreateFunction's unknown-data-source refusal leaks the name AWS withholds.
AWS: 404 NotFoundException with the fixed sentence "Data source not found." — no variable part (recorded behavior; Smithy: CreateFunction errors include NotFoundException httpError 404).
Emulator: 'Data source <name> not found' — a different sentence that interpolates the requested name and drops the period.
Fix: CreateFunction's data-source lookup refusal message.
Traces: appsync-datasource-function-pipeline-resolver#task#15.

Cause 7 of 10 — CreateFunction's invalid-name refusal answers ValidationException and a constraint dump instead of BadRequestException.
AWS: 400 BadRequestException, x-amzn-errortype BadRequestException, fixed sentence "The Function name is invalid. Please refer to the documentation for more details." (recorded behavior; Smithy: CreateFunction errors are ApiKey/BadRequestException(400)/ConcurrentModificationException(409)/InternalFailureException/NotFoundException/UnauthorizedException — ValidationException is not modeled on this operation).
Emulator: 400 with x-amzn-errortype ValidationException and "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[_A-Za-z][_0-9A-Za-z]*$".
Why: the error code is not one the operation models, so a client's typed-error dispatch misses it entirely.
Fix: CreateFunction validation must raise the modeled BadRequestException with AWS's sentence instead of the generic constraint renderer.
Traces: appsync-datasource-function-resolver-pipeline#task#7.

Cause 8 of 10 — ListApis' invalid-pagination-token refusal is emulator-authored and drops the diagnostic.
AWS: 400 BadRequestException with the template "Pagination token exception in operation '<OperationName>': <decoder diagnostic>" — fixed text "Pagination token exception in operation '", the operation name taken from the request, and the token decoder's own diagnostic (e.g. an invalid payload-wrapping schema report) as the variable tail (recorded behavior).
Emulator: 'Invalid pagination token given.' — a single fixed sentence naming neither the operation nor the fault.
Why: a client paginating several collections cannot tell which call's token was rejected or why.
Fix: the shared pagination-token decoder's refusal message, and the ListApis handler that surfaces it.
Traces: appsync-error-list-pagination-invalid-token#task#3, appsync-list-apis-pagination-token-error#task#1.

Cause 9 of 10 — GetIntrospectionSchema's invalid-format refusal answers ValidationException and a constraint dump.
AWS: 400 BadRequestException with the template "Invalid format <value> given" — fixed text "Invalid format " / " given" with the rejected format value from the request (recorded behavior; Smithy: GetIntrospectionSchema errors include BadRequestException httpError 400; OutputType/format is an enum, and ValidationException is not modeled here).
Emulator: x-amzn-errortype ValidationException with "1 validation error detected: Value at 'format' failed to satisfy constraint: Member must satisfy enum value set: [SDL, JSON]".
Fix: GetIntrospectionSchema validation must raise the modeled BadRequestException with AWS's template.
Traces: appsync-eval-schema-creation-introspection#task#8.

Cause 10 of 10 — CreateApi's multi-error validation message lists the enum value set in the wrong order and the errors in the wrong order.
AWS: when several eventConfig auth-mode members carry an unmodeled authType, the message is "<N> validation errors detected: " followed by one clause per member, and each clause's enum set is rendered in the model's declaration order: [AWS_LAMBDA, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_IAM, API_KEY] as AWS renders it for this member; the clause order follows AWS's traversal (defaultPublishAuthModes, connectionAuthModes, defaultSubscribeAuthModes). The fixed text is the "N validation errors detected: " prefix and each clause's "Value at '<path>' failed to satisfy constraint: Member must satisfy enum value set: [...]"; the variable parts are the member paths and N (Smithy: AuthenticationType values API_KEY, AWS_IAM, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_LAMBDA).
Emulator: emits the clauses in a different order (connectionAuthModes first) and renders the enum set in the Smithy declaration order [API_KEY, AWS_IAM, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_LAMBDA].
Why: the message is a requirement; a client or test matching AWS's rendering of the allowed-value list does not match.
Fix: the constraint-message renderer's enum-set ordering and clause ordering for CreateApi's eventConfig members.
Traces: appsync-event-api-channel-namespace#task#1, appsync-event-api-crud-lifecycle#task#5, appsync-event-api-full-lifecycle#task#5.

Related open issue that does NOT cover these: "AppSync control-plane error semantics: not-found/bad-request codes inverted, ValidationException for modeled BadRequestException/NotFoundException cases..." (20261029T174359Z) — it carries none of these replay keys.

## Evidence
Replay of simparity-wire-aws-v1.

1) appsync-cfg-create-datasource-rejections#task#3 CreateDataSource 400: expected 'Multiple configs given for data source of type dynamo.', got 'Missing Amazon DynamoDB data source config' (4 steps in cluster).
2) appsync-cfg-create-datasource-rejections#task#9 CreateDataSource: status expected 400 got 200; expected 'Missing relational database source type', got None (3 steps).
3) appsync-cfg-update-datasource-rules#task#4 UpdateDataSource 400: expected 'Multiple configs given for data source of type dynamo.', got 'Missing Amazon DynamoDB data source config'.
4) appsync-cfg-resolver-batch-and-caching-rules#task#6 CreateResolver 400: expected 'Invalid value for maxBatchSize. Must be between 0 and 2000 (inclusive)', got 'Invalid maxBatchSize given. Must fall within range 0 to 2,000'.
5) appsync-datasource-pagination-page-boundary#task#10 ListDataSources 400: expected 'maxResults must be greater than 0 and less than or equal to 25.', got 'Invalid maxResults given. Must fall within range 0 to 25'.
6) appsync-datasource-function-pipeline-resolver#task#15 CreateFunction 404: expected 'Data source not found.', got 'Data source nonexistent_ds not found'.
7) appsync-datasource-function-resolver-pipeline#task#7 CreateFunction 400: expected 'The Function name is invalid. Please refer to the documentation for more details.' with errortype 'BadRequestException', got "1 validation error detected: Value '...' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[_A-Za-z][_0-9A-Za-z]*$" with errortype 'ValidationException'.
8) appsync-error-list-pagination-invalid-token#task#3 ListApis 400: expected "Pagination token exception in operation 'ListApis': invalid payload wrapping schema -24949", got 'Invalid pagination token given.'
9) appsync-eval-schema-creation-introspection#task#8 GetIntrospectionSchema 400: expected 'Invalid format YAML given', got "1 validation error detected: Value at 'format' failed to satisfy constraint: Member must satisfy enum value set: [SDL, JSON]" with errortype 'ValidationException'.
10) appsync-event-api-channel-namespace#task#1 CreateApi 400: expected clause order defaultPublishAuthModes/connectionAuthModes/defaultSubscribeAuthModes with enum set [AWS_LAMBDA, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_IAM, API_KEY]; got connectionAuthModes first with enum set [API_KEY, AWS_IAM, AMAZON_COGNITO_USER_POOLS, OPENID_CONNECT, AWS_LAMBDA].

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay simparity-wire-aws-v1.
2. The steps under Traces mismatch in fixtures appsync-cfg-create-datasource-rejections, appsync-cfg-update-datasource-rules, appsync-cfg-resolver-batch-and-caching-rules, appsync-datasource-pagination-page-boundary, appsync-datasource-function-pipeline-resolver, appsync-datasource-function-resolver-pipeline, appsync-error-list-pagination-invalid-token, appsync-eval-schema-creation-introspection and appsync-event-api-channel-namespace.
3. By hand: Create/UpdateDataSource with type AMAZON_DYNAMODB plus a second *Config member (causes 1, 3); CreateDataSource type RELATIONAL_DATABASE with no relationalDatabaseSourceType (cause 2); CreateResolver maxBatchSize out of 0..2000 (cause 4); ListDataSources maxResults=0 or >25 (cause 5); CreateFunction naming a nonexistent data source (cause 6) and with a hyphenated name (cause 7); ListApis with a corrupt nextToken (cause 8); GetIntrospectionSchema?format=YAML (cause 9); CreateApi with an unmodeled authType in three eventConfig auth-mode lists (cause 10).
