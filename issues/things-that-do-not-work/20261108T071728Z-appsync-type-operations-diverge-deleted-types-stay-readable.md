Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-graphql-api-and-type-lifecycle
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-graphql-api-and-type-lifecycle appsync-schema-type-management appsync-type-sdl-json-format-fidelity
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|GetType|4xx->2xx|$
Issue-Key: appsync|GetType|4xx->2xx|$.message
Issue-Key: appsync|GetType|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|GetType|4xx->2xx|status
Issue-Key: appsync|GetType|2xx->2xx|$.type.definition
Issue-Key: appsync|GetType|4xx->4xx|$.message
Issue-Key: appsync|CreateType|4xx->4xx|$.message
Issue-Commit: 03f5ae02c344a1919f5f0e30cbc48d62675aa871

# AppSync type operations diverge: deleted types stay readable, definition not re-rendered, refusal wording emulator-authored (4 causes)

> _Note: this issue was identified against the code base at commit `03f5ae02c344a1919f5f0e30cbc48d62675aa871`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetType

## Description
The emulator's AppSync GraphQL *type* sub-resource (CreateType / GetType / DeleteType on /v1/apis/{apiId}/types) diverges from AWS in four independent ways: a deleted type remains readable, the returned type definition is the client's raw input rather than AWS's re-rendered SDL, and both refusal messages are emulator-authored instead of AWS's wording. All four are appsync and must all be closed before this issue is submitted. The open issue 20261029T200818Z-appsync-validation-refusals-are-emulator-authored-wrong-message.md covers refusal wording on other appsync operations (data sources, resolvers, functions) and does NOT cover the type operations below.

Cause 1 of 4 - DeleteType does not remove the type; GetType keeps serving it (fixture appsync-graphql-api-and-type-lifecycle, step teardown#7).
AWS: Smithy: DeleteType = DELETE /v1/apis/{apiId}/types/{typeName}, output empty, http code 200; GetType = GET /v1/apis/{apiId}/types/{typeName}, errors include NotFoundException (errorKind client, httpError 404). After a successful DeleteType the type no longer exists, so GetType for that typeName answers 404 NotFoundException with message "Type not found." (recorded behavior for the message text; the status and error class are modeled).
Emulator: GetType continues to answer 200 with a full Type body for the just-deleted type - the replay polled the step 30 times and got 200 every time, body {"type": {"name": <typeName>, "arn": <type ARN>, "definition": <submitted SDL>, "format": "SDL"}} instead of the expected 404 {"message": "Type not found."}. The names/ARNs/ids in that body are per-run values, not requirements; the requirement is that the type is gone.
Why: a client that treats 404 as proof of deletion never observes the delete taking effect - idempotent teardown loops never terminate, and re-creating the same type name hits a stale type that AWS would not have. The delete is reported successful while state is unchanged, so the emulator's type inventory silently drifts from what the client believes.
Fix: the DeleteType handler's state mutation - the type must be removed from the API's type collection in appsync state, so GetType/UpdateType/DeleteType on it subsequently raise NotFoundException.
Traces: appsync-graphql-api-and-type-lifecycle#teardown#7, appsync-schema-type-management#teardown#10.

Cause 2 of 4 - GetType returns the submitted definition verbatim instead of AWS's re-rendered SDL (fixture appsync-schema-type-management, step task#3).
AWS: recorded behavior only - Smithy models Type$definition merely as String ("The type definition") and TypeDefinitionFormat as the enum SDL|JSON, with no formatting trait. The recording shows AWS does not store the client's text: it parses the submitted SDL and returns a re-printed document in which the type name, the opening brace, each field and the closing brace are on their own lines and the document ends with a newline. A single-line definition such as "type <Name> { <field>: <Type>! <field2>: <Type2> }" comes back as "type <Name>\n\n{\n<field>: <Type>!\n\n\n<field2>: <Type2>\n\n}\n" - the layout is the requirement; the particular type and field names in the recording are request values.
Emulator: echoes the request's definition string byte-for-byte (expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }'), status 200 and all other Type members correct.
Why: any client that diffs, hashes or line-parses the stored schema - schema-diff tooling, SDL/JSON round-trip through the format query parameter, CI drift detection - sees a document AWS would never return, so comparisons against a real account fail even though nothing changed.
Fix: the Type response projection shared by CreateType/UpdateType/GetType - render Type$definition from the parsed type model instead of storing and echoing the raw request string.
Traces: appsync-schema-type-management#task#3, appsync-type-sdl-json-format-fidelity#task#8 through #task#17 (11 steps in the cluster).

Cause 3 of 4 - GetType's not-found message is emulator-authored (fixture appsync-schema-type-management, step task#7).
AWS: Smithy: GetType errors include NotFoundException (client, httpError 404, member message). For an unknown typeName AWS answers 404 with the message "Type not found." - that sentence is entirely fixed text: it interpolates nothing from the request and ends with a period (recorded behavior for the exact wording).
Emulator: answers the correct 404/NotFoundException but with "Type <typeName> not found" - it splices the requested type name into the sentence and drops the trailing period (recorded example: expected 'Type not found.', got 'Type NoSuchType not found'; 'NoSuchType' is the request's typeName, not part of the template).
Why: clients and SDK test suites that match on the service's error message (or assert the message is request-independent) mis-classify the failure, and the message leaks the requested name where AWS deliberately does not.
Fix: the GetType handler's not-found error construction - emit NotFoundException with the fixed message "Type not found." and no request interpolation.
Traces: appsync-schema-type-management#task#7, appsync-type-sdl-json-format-fidelity#task#20.

Cause 4 of 4 - CreateType's parse-failure message leaks internal parser tokens (fixture appsync-type-sdl-json-format-fidelity, step task#19).
AWS: Smithy: CreateType errors include BadRequestException (client, httpError 400, members message/reason/detail). A definition that is not valid for the declared TypeDefinitionFormat is refused 400 BadRequestException with the message "Unable to parse provided type definition." - again entirely fixed text with no variable parts; AWS does not report which token or position failed (recorded behavior for the wording).
Emulator: correct status 400 but message "Invalid type definition: expected R_CURLY, got EOF" - emulator-authored prose that exposes internal grammar symbol names and the parser's position state.
Why: a client asserting on AWS's refusal sentence sees an unknown message; worse, the text varies with the shape of the malformed input, so a message AWS keeps constant becomes input-dependent and un-matchable.
Fix: the CreateType definition-validation path - map any parse failure to BadRequestException with the fixed message "Unable to parse provided type definition.", keeping the parser's diagnostic internal.
Traces: appsync-type-sdl-json-format-fidelity#task#19.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). Four net-new divergence clusters, all service=appsync, none stub-blocked, none owned by an open issue:

1) appsync-graphql-api-and-type-lifecycle#teardown#7 - GetType, expected 404, actual 200. detail: status: expected 404, got 200; $.message: missing (expected 'Type not found.', got None); $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None); $: poll: status 200 not in [404] after 30 attempt(s) (expected {'message': 'Type not found.'}, got {'type': {'name': 'Blog', 'arn': 'arn:aws:appsync:us-west-2:000000000000:apis/aniuv4luaxz4uc3wkvyhfyla76/types/Blog', 'definition': 'type Blog { id: ID! title: String }\n', 'format': 'SDL'}}). Also affected: appsync-schema-type-management#teardown#10.

2) appsync-schema-type-management#task#3 - GetType, 200 vs 200. detail: $.type.definition: value_mismatch (expected 'type Blog\n\n{\nid: ID!\n\n\ntitle: String\n\n}\n', got 'type Blog { id: ID! title: String }'). Cluster of 11 steps, also appsync-type-sdl-json-format-fidelity#task#8..#task#17.

3) appsync-schema-type-management#task#7 - GetType, 404 vs 404. detail: $.message: value_mismatch (expected 'Type not found.', got 'Type NoSuchType not found'). Also affected: appsync-type-sdl-json-format-fidelity#task#20.

4) appsync-type-sdl-json-format-fidelity#task#19 - CreateType, 400 vs 400. detail: $.message: value_mismatch (expected 'Unable to parse provided type definition.', got 'Invalid type definition: expected R_CURLY, got EOF').

Model evidence (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model): GetType GET /v1/apis/{apiId}/types/{typeName} with required format query, errors BadRequestException(400)/NotFoundException(404)/ConcurrentModificationException(409)/UnauthorizedException(401)/InternalFailureException(500); CreateType POST /v1/apis/{apiId}/types, required definition + format, same error set; DeleteType DELETE /v1/apis/{apiId}/types/{typeName}, empty output, code 200. Type members: name, description, arn, definition, format (TypeDefinitionFormat = SDL|JSON). NotFoundException httpError 404 with member message; BadRequestException httpError 400 with message/reason/detail. Nothing in the model constrains definition formatting, so Cause 2's requirement is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay appsync-graphql-api-and-type-lifecycle step teardown#7 (or appsync-schema-type-management#teardown#10): CreateGraphqlApi, CreateType with an SDL type, DeleteType on that typeName, then GetType for it with format=SDL. Expect 404 NotFoundException {"message": "Type not found."}; the emulator answers 200 with the type body indefinitely (30 polls).
3. Cause 2: replay appsync-schema-type-management step task#3: CreateType with a single-line SDL definition, then GetType format=SDL. Expect the definition re-rendered multi-line and newline-terminated; the emulator returns the submitted string verbatim.
4. Cause 3: replay appsync-schema-type-management step task#7: GetType with a typeName that does not exist. Expect 404 NotFoundException with the fixed message "Type not found."; the emulator returns "Type <typeName> not found".
5. Cause 4: replay appsync-type-sdl-json-format-fidelity step task#19: CreateType with an SDL definition that is truncated (unclosed brace). Expect 400 BadRequestException "Unable to parse provided type definition."; the emulator returns "Invalid type definition: expected R_CURLY, got EOF".
6. All four steps must pass before this issue is closed.
