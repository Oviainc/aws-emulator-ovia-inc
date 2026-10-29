Issue-Source: trace-sweep-divergence
Issue-Trace: b2bi-mapping-template-and-test-mapping
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2bi-mapping-template-and-test-mapping
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|CreateStarterMappingTemplate|2xx->2xx|$.mappingTemplate
Issue-Key: b2bi|TestMapping|2xx->2xx|$.mappedFileContent
Issue-Commit: a1f92506050befa9acc0ff29de706b69960d0a20

# b2bi mapping data plane returns synthetic string payloads: mappingTemplate skeleton and mappedFileContent encoding both diverge

> _Note: this issue was identified against the code base at commit `a1f92506050befa9acc0ff29de706b69960d0a20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStarterMappingTemplate

## Description
TWO net-new causes, both in b2bi's mapping data plane, both on operations whose entire response is a single required String member that must be DERIVED from the request. In both the emulator answers HTTP 200 with a body it synthesized itself, so the required member's content is unrelated to what AWS would return. Both causes are wire divergences from simparity-wire-aws-v1 and neither is owned by an open issue: the related open issue 20261029T193110Z-b2bi-mapping-data-plane-is-synthetic-createstartermappingtemplate-ignores.md is a BEHAVIORAL issue (Issue-Corpus: simparity-behavioral-aws-v1) carrying only check ids and no Issue-Trace/Issue-Step, so it does not gate either fixture step below; its cause 1 is about outputSampleLocation being ignored (these recordings pass no sample at all) and its cause 2 is about the JSONata template never being evaluated, whereas cause 2 below is about how the evaluated result is ENCODED into the member.

Cause 1 of 2: CreateStarterMappingTemplate returns a mappingTemplate that is not the X12 starter skeleton for the requested template details.
AWS: Smithy CreateStarterMappingTemplate takes outputSampleLocation (S3Location), mappingType (required enum) and templateDetails (required), and returns CreateStarterMappingTemplateResponse.mappingTemplate as a required String; errors are AccessDeniedException(403), ResourceNotFoundException(404), ValidationException(400), InternalServerException(500). The model says nothing about the template's content, so for the content this is recorded behavior only: the member holds a pretty-printed JSON document (two-space indent, " : " between key and value) whose root object carries an "interchanges" array; each interchange names the X12 envelope elements in positional order (ISA_01_AuthorizationQualifier through ISA_15_TestIndicator), then nests "functional_groups" naming GS_01_FunctionalIdentifierCode through GS_08_Version, then "transactions" naming ST_01_TransactionSetIdentifierCode and ST_02_TransactionSetControlNumber, then a "segments" array with one object per segment of the transaction set named in templateDetails, every business element keyed <SEGMENT>_<nn> (BEG_01..BEG_12, CUR_01..CUR_09, ...) and valued with the literal string "PLACEHOLDER". The fixed part of the requirement is the member NAMES, their positional order, the nesting (interchanges -> functional_groups -> transactions -> segments), and the "PLACEHOLDER" literal for business elements; the envelope values and which segments appear are functions of the requested transactionSet/version and are not themselves a fixed requirement.
Emulator: answers 200, but $.mappingTemplate is a value_mismatch against that skeleton on every replay of the operation (both steps of the fixture), i.e. the returned document is a fixed synthetic string rather than a skeleton generated for the requested templateDetails.
Why: this operation exists only to hand the caller a template it then edits and feeds to TestMapping and CreateTransformer. A template that does not name the envelope and segment elements of the requested transaction set means every mapping expression a client derives from it addresses members that do not exist in AWS's canonicalized document, so the mapping silently maps nothing once pointed at the real service.
Fix: the CreateStarterMappingTemplate handler's starter-template generator — build the skeleton from templateDetails (transaction set and version) with the named ISA/GS/ST envelope members and one object per segment whose business elements are the "PLACEHOLDER" literal, and serialize it with the recorded pretty-printing.
Traces: b2bi-mapping-template-and-test-mapping#task#1 (representative; task#2 is the same cluster).

Cause 2 of 2: TestMapping emits mappedFileContent as a bare JSON object instead of the JSON-string-encoded mapped document, and re-orders its members.
AWS: Smithy TestMapping requires inputFileContent (@length 0..5000000), mappingTemplate (@length 0..350000) and fileFormat (enum), and returns TestMappingResponse.mappedFileContent as a required String. Because the member is typed String, the mapped document travels as TEXT inside it: recorded behavior shows the value is a JSON string literal whose characters are the mapped document, so one parse of the member yields a string and parsing that string yields the document. The recording also preserves the member order of the input document rather than sorting it.
Emulator: emits the mapped document's members directly as a JSON object value for $.mappedFileContent (value_mismatch: expected a quoted string literal holding the document text, got the object), and the members come back in sorted order (item, orderId, quantity) instead of the input's order (orderId, item, quantity). The member names and values in the recording are the fixture's own request payload and are not part of the requirement; the requirement is the string-encoding layer and the preserved ordering.
Why: a typed SDK deserializes mappedFileContent into a string field. Against the emulator that field is either a deserialization failure or an object-shaped value, so a client that parses the mapped document — the only reason to call TestMapping — cannot read it at all, and a client that diffs the mapped output against an expected document sees spurious differences from the re-ordering.
Fix: the TestMapping response serializer — place the mapped document's serialized TEXT into the required String member mappedFileContent (string-encoded, not inlined as an object), and build it from an order-preserving map so input member order survives.
Traces: b2bi-mapping-template-and-test-mapping#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline: 56 steps, 21 passed, 35 mismatched, score 37.5%, 26 divergence clusters, bootstrap_signal=false, emulator_up=true. 24 of the 26 clusters carry owned_by; the two below carry an empty owned_by and an empty stub_path.

Cause 1 — fixture b2bi-mapping-template-and-test-mapping, step task#1 (cluster also covers task#2), CreateStarterMappingTemplate, expected status 200, actual status 200, verdict mismatch:
  $.mappingTemplate: value_mismatch. Expected a pretty-printed JSON document of the form {"interchanges": [ { "ISA_01_AuthorizationQualifier": ..., "ISA_02_AuthorizationInformation": ..., ... "ISA_15_TestIndicator": ..., "functional_groups": [ { "GS_01_FunctionalIdentifierCode": ..., ... "GS_08_Version": ..., "transactions": [ { "ST_01_TransactionSetIdentifierCode": ..., "ST_02_TransactionSetControlNumber": ..., "segments": [ {"BEG_01": "PLACEHOLDER", ... "BEG_12": "PLACEHOLDER"}, {"CUR_01": "PLACEHOLDER", ...}, ... ] } ] } ] } ]}; the emulator's value does not match that skeleton.

Cause 2 — fixture b2bi-mapping-template-and-test-mapping, step task#3, TestMapping, expected status 200, actual status 200, verdict mismatch:
  $.mappedFileContent: value_mismatch. Expected the member to hold a JSON string literal whose content is the mapped document text (the recording's value is "{\n  \"orderId\": \"12345\",\n  \"item\": \"widget\",\n  \"quantity\": 3\n}" — note the enclosing quotes and the input's member order); the emulator returned the un-encoded object {"item": "widget", "orderId": "12345", "quantity": 3}, dropping the string-encoding layer and sorting the members. The specific keys/values are the fixture's request payload, not a requirement.

Smithy consulted (services/service-com-amazonaws-b2bi-b2bi/smithy-model/com-amazonaws-b2bi-b2bi.json): CreateStarterMappingTemplate input {outputSampleLocation: S3Location, mappingType required, templateDetails required} / output {mappingTemplate: String, required}; TestMapping input {inputFileContent required @length 0..5000000, mappingTemplate required @length 0..350000, fileFormat required} / output {mappedFileContent: String, required}. Both operations are RPC-dispatched (httpBindingsInert).

## Steps to Reproduce
1. git clone the repo and boot the emulator from mainline (the data listener answers on the configured port; /__admin/health returns 200).
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Cause 1: re-replay fixture b2bi-mapping-template-and-test-mapping step task#1 (and task#2) — CreateStarterMappingTemplate with mappingType=JSONATA and templateDetails naming an X12 transaction set; parse the returned mappingTemplate and observe that it is not the interchanges/functional_groups/transactions/segments skeleton with the named ISA_*/GS_*/ST_* members and "PLACEHOLDER" business elements for the requested transaction set.
4. Cause 2: re-replay fixture b2bi-mapping-template-and-test-mapping step task#3 — TestMapping(fileFormat=JSON, inputFileContent=<a JSON document>, mappingTemplate=<a JSONata template>) and inspect the raw response body: observe mappedFileContent is a JSON object rather than a JSON string holding the mapped document's text, and that its members are sorted instead of keeping the input document's order.
5. Both steps must pass before the issue is closed; each is a separate cause.
