Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.resolver-kind-pipeline-conflict-rejected appsync.resolver-pipeline-requires-config appsync.unit-resolver-missing-datasource appsync.get-introspection-schema-before-schema-fails
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.resolver-kind-pipeline-conflict-rejected|run#14|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: appsync.resolver-pipeline-requires-config|run#11|AssertionError|assert code == "BadRequestException", (
Issue-Key: appsync.unit-resolver-missing-datasource|run#12|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: appsync.get-introspection-schema-before-schema-fails|run#8|AssertionError|assert code == "GraphQLSchemaException", (
Issue-Commit: 6ffea8718fff09764f6473838301fec30c0f0643

# AppSync: CreateResolver kind/pipelineConfig/dataSourceName validation refusals and GetIntrospectionSchema no-schema refusal are absent — all four answer NotImplemented (4 causes)

> _Note: this issue was identified against the code base at commit `6ffea8718fff09764f6473838301fec30c0f0643`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResolver

## Description
Four net-new refusal semantics are missing on the AppSync control plane. Every one of the four calls below is answered with the error code `NotImplemented` instead of the modeled refusal AWS returns, so the client cannot distinguish "this request is invalid" from "this emulator does not serve the operation". These are NOT the same work item as the open scaffold-stub issues `20261109T003200Z-appsync-createresolver-stub.md` and `20261109T041647Z-appsync-getintrospectionschema-stub.md`: those issues only ask for the happy path of the operation to exist. The four causes below are the specific pre-condition checks and error classes/messages the operations must additionally produce once they exist, and the open issue `20261029T200818Z-appsync-resolver-function-data-source-plane-dependency-refusals.md` (which does cover other CreateResolver/UpdateResolver divergences) does not name any of them. A fix that merely implements the two operations will leave all four of these checks failing.

Cause 1 of 4: CreateResolver does not refuse kind=UNIT carrying a pipelineConfig.
AWS: 400 BadRequestException, x-amzn-errortype BadRequestException, Message exactly "PipelineConfig should not be specified." — the whole message is fixed text with no variable part. A UNIT resolver has no pipeline, so supplying pipelineConfig (even `{"functions": []}`) is an invalid combination and the resolver is NOT created: an independent GetResolver on the same typeName/fieldName afterwards must answer 404 NotFoundException with Message exactly "No resolver found." (also entirely fixed text). Smithy: CreateResolver.kind -> ResolverKind enum {UNIT, PIPELINE}; CreateResolver.pipelineConfig -> PipelineConfig, documented as "The pipeline configuration for a resolver of kind PIPELINE"; CreateResolver errors include BadRequestException (httpError 400) and NotFoundException (httpError 404); GetResolver errors include NotFoundException. Recorded behavior supplies the two message strings.
Emulator: the CreateResolver call is refused with error code `NotImplemented`, not BadRequestException, so the assertion on the refusal class fails before the resolver-absence and positive-control steps are ever reached.
Why: a client that validates its deploy payloads against AppSync's BadRequestException handling (surfacing "PipelineConfig should not be specified." to the operator) sees an unrecognised `NotImplemented` code instead, so it classifies an invalid-input error as a service outage and retries forever instead of fixing the request.
Fix: CreateResolver's input validation in the operation's handler (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_resolver.rs) — reject kind=UNIT with pipelineConfig present, and leave no resolver in state for the rejected call.
Traces: appsync.resolver-kind-pipeline-conflict-rejected.

Cause 2 of 4: CreateResolver does not refuse kind=PIPELINE with no pipelineConfig.
AWS: 400 BadRequestException, x-amzn-errortype BadRequestException, Message exactly "PipelineConfig can't be null." — entirely fixed text. pipelineConfig is mandatory for a PIPELINE resolver, nothing is attached to the field, and an independent GetResolver must then answer 404 NotFoundException with Message "No resolver found.". The identical call WITH a pipelineConfig referencing a real function id must succeed 200 and echo pipelineConfig.functions. Smithy: CreateResolver.pipelineConfig is optional in the model (so this is a cross-member rule the service enforces, not a @required violation); errors BadRequestException 400 / NotFoundException 404. Recorded behavior supplies the message string.
Emulator: CreateResolver answers error code `NotImplemented` rather than BadRequestException.
Why: the single most common pipeline-resolver authoring mistake is silently misreported, and because the refusal class is wrong a client cannot tell that the field was left without a resolver.
Fix: CreateResolver's input validation — require pipelineConfig when kind=PIPELINE (same handler as cause 1).
Traces: appsync.resolver-pipeline-requires-config.

Cause 3 of 4: CreateResolver does not refuse kind=UNIT with neither dataSourceName nor pipelineConfig.
AWS: 400 BadRequestException, x-amzn-errortype BadRequestException, Message exactly "Data source name not specified." — entirely fixed text. A UNIT resolver must name a data source; the resolver is not created, and an independent GetResolver answers 404 NotFoundException / "No resolver found.". The same call with dataSourceName set succeeds 200 and echoes kind=UNIT plus that dataSourceName. Smithy: CreateResolver.dataSourceName -> ResourceName, optional in the model, so again a service-enforced cross-member rule; errors BadRequestException 400 / NotFoundException 404.
Emulator: CreateResolver answers error code `NotImplemented` rather than BadRequestException.
Why: an under-specified resolver is reported as an unimplemented operation, so a deploy tool cannot surface the actionable "Data source name not specified." diagnostic and cannot distinguish a bad template from a broken endpoint.
Fix: CreateResolver's input validation — require dataSourceName when kind is UNIT (or defaulted to UNIT) and no pipelineConfig is given (same handler as causes 1-2).
Traces: appsync.unit-resolver-missing-datasource.

Cause 4 of 4: GetIntrospectionSchema on an API that has no schema yet answers the wrong error class.
AWS: 400 GraphQLSchemaException (x-amzn-errortype GraphQLSchemaException), NOT NotFoundException — the API exists, it is its schema that is unparseable because it is empty. The Message is AWS's parser diagnostic for an empty document: fixed text of the form `errors=[InvalidSyntaxError{ message=Invalid Syntax ,locations=[SourceLocation{line=<n>, column=<n>}]}]`, where the line/column are the parse position derived from the submitted (here empty) schema document and everything else is fixed text; for a schema that was never created the position is the document origin. Immediately before this, GetSchemaCreationStatus on a freshly created API with no schema attempt must report status NOT_APPLICABLE, and after a successful StartSchemaCreation the identical GetIntrospectionSchema call must return 200 with the created SDL in the `schema` blob payload. Smithy: GetIntrospectionSchema errors are GraphQLSchemaException (httpError 400, doc "The GraphQL schema is not valid."), NotFoundException (404), UnauthorizedException (401), InternalFailureException (500); output member `schema` is an @httpPayload Blob; http GET /v1/apis/{apiId}/schema.
Emulator: the call is refused with error code `NotImplemented`, so neither the GraphQLSchemaException class nor its parser-shaped Message is produced.
Why: a client distinguishes "the API id is wrong" (NotFoundException) from "this API has no usable schema yet" (GraphQLSchemaException) purely by the error code; getting `NotImplemented` collapses both into an unretryable unknown, and schema-bootstrap tooling that waits for a schema to become introspectable never starts.
Fix: the GetIntrospectionSchema handler (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/get_introspection_schema.rs) — classify the no-schema case as GraphQLSchemaException with the InvalidSyntaxError-shaped message, and serve the stored SDL/JSON blob once StartSchemaCreation has succeeded.
Traces: appsync.get-introspection-schema-before-schema-fails.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout. All four checks are listed under the suite report's `### appsync` heading and none is marked ALREADY FILED or OWNED.

1) appsync.resolver-kind-pipeline-conflict-rejected [mismatch]
   check_appsync_resolver_kind_pipeline_conflict_rejected.py:52
   `assert code == "BadRequestException"` -> AssertionError: expected BadRequestException, got 'NotImplemented'
   Request: CreateResolver(apiId=<api>, typeName="Query", fieldName="behavA", kind="UNIT", dataSourceName=<NONE data source>, pipelineConfig={"functions": []}, requestMappingTemplate="{}", responseMappingTemplate="$util.toJson($ctx.result)").
   Expected: 400 BadRequestException, Message "PipelineConfig should not be specified."; then GetResolver -> 404 NotFoundException, Message "No resolver found."; then the same create without pipelineConfig on field "behavB" -> 200 with kind UNIT and the given dataSourceName.
   Actual: error code NotImplemented.

2) appsync.resolver-pipeline-requires-config [mismatch]
   check_appsync_resolver_pipeline_requires_config.py:71
   AssertionError: expected BadRequestException for PIPELINE resolver without pipelineConfig, got NotImplemented
   Request: CreateResolver(apiId=<api>, typeName="Query", fieldName="echo", kind="PIPELINE", requestMappingTemplate="{}", responseMappingTemplate="$util.toJson($ctx.result)") — no pipelineConfig.
   Expected: 400 BadRequestException, Message "PipelineConfig can't be null."; GetResolver -> 404 NotFoundException / "No resolver found."; the same call with pipelineConfig={"functions":[<functionId>]} -> 200 echoing that functions list, and an independent GetResolver confirming kind PIPELINE.
   Actual: error code NotImplemented.

3) appsync.unit-resolver-missing-datasource [mismatch]
   check_appsync_unit_resolver_missing_datasource.py:80
   AssertionError: expected BadRequestException, got NotImplemented
   Request: CreateResolver(apiId=<api>, typeName="Query", fieldName="getWidget", kind="UNIT", requestMappingTemplate="{}", responseMappingTemplate="$util.toJson($ctx.result)") — no dataSourceName, no pipelineConfig.
   Expected: 400 BadRequestException, Message "Data source name not specified."; GetResolver -> 404 NotFoundException / "No resolver found."; the same create on field "getWidget2" with dataSourceName set -> 200 with kind UNIT and that dataSourceName.
   Actual: error code NotImplemented.

4) appsync.get-introspection-schema-before-schema-fails [mismatch]
   check_appsync_get_introspection_schema_before_schema_fails.py:54
   AssertionError: expected GraphQLSchemaException before any schema is created, got NotImplemented
   Request: CreateGraphqlApi(authenticationType=API_KEY) then GetSchemaCreationStatus (expected status NOT_APPLICABLE) then GetIntrospectionSchema(apiId=<api>, format="SDL").
   Expected: 400 GraphQLSchemaException with Message of the form `errors=[InvalidSyntaxError{ message=Invalid Syntax ,locations=[SourceLocation{line=<n>, column=<n>}]}]`; then after StartSchemaCreation(definition=b"schema { query: Query }\ntype Query { hello: String }\n") reaches SUCCESS/ACTIVE, the identical GetIntrospectionSchema -> 200 whose `schema` payload contains the created field.
   Actual: error code NotImplemented.

## Steps to Reproduce
1. Clone the repository and boot the emulator (the behavioral suite builds and boots it from the checkout).
2. Replay simparity-behavioral-aws-v1; the four checks under `### appsync` listed in trace_ids mismatch.
3. By hand, against the emulator's appsync endpoint:
   a. CreateGraphqlApi(name=<any>, authenticationType=API_KEY); StartSchemaCreation with `type Query { behavA: String behavB: String }` and poll GetSchemaCreationStatus to SUCCESS; CreateDataSource(name="BehavNoneDS", type="NONE").
   b. Cause 1: CreateResolver(typeName="Query", fieldName="behavA", kind="UNIT", dataSourceName="BehavNoneDS", pipelineConfig={"functions": []}, requestMappingTemplate="{}", responseMappingTemplate="$util.toJson($ctx.result)") -> expect 400 BadRequestException "PipelineConfig should not be specified."; then GetResolver("Query","behavA") -> expect 404 NotFoundException "No resolver found.".
   c. Cause 2: CreateResolver(typeName="Query", fieldName="behavB", kind="PIPELINE") with no pipelineConfig -> expect 400 BadRequestException "PipelineConfig can't be null."; repeat with pipelineConfig={"functions":[<id from CreateFunction>]} -> expect 200.
   d. Cause 3: CreateResolver(typeName="Query", fieldName="behavB", kind="UNIT") with neither dataSourceName nor pipelineConfig -> expect 400 BadRequestException "Data source name not specified."; repeat with dataSourceName="BehavNoneDS" -> expect 200.
   e. Cause 4: on a NEW API with no schema, GetSchemaCreationStatus -> expect NOT_APPLICABLE, then GetIntrospectionSchema(format="SDL") -> expect 400 GraphQLSchemaException with the InvalidSyntaxError-shaped Message; after StartSchemaCreation succeeds, the same call -> expect 200 with the SDL in the `schema` payload.
4. Each call above currently answers error code NotImplemented.
