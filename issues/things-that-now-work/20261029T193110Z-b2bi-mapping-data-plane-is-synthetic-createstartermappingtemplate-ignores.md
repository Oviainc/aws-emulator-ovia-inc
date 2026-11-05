Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.create-starter-mapping-template-with-output-sample b2bi.test-mapping-jsonata-identity-roundtrip
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi.create-starter-mapping-template-with-output-sample|_assert_template_derived_from_sample#8|KeyError|assert interchange["ISA_06_SenderId"].strip() == "SENDER", (
Issue-Key: b2bi.test-mapping-jsonata-identity-roundtrip|_load_mapped#8|JSONDecodeError|parsed = json.loads(parsed)
Issue-Commit: 24dcbcf55b45bb9d0573a38e9c06262a825c087d

# b2bi mapping data plane is synthetic: CreateStarterMappingTemplate ignores outputSampleLocation and TestMapping does not evaluate the JSONata template

> _Note: this issue was identified against the code base at commit `24dcbcf55b45bb9d0573a38e9c06262a825c087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestMapping

## Description
TWO net-new causes: the two data-plane operations that are supposed to read the caller's input and produce a derived document instead return a body unrelated to that input. Related open issue 20261029T171003Z-b2bi-response-text-diverges-pagination-refusal-wording-starter.md records, from the WIRE corpus, that CreateStarterMappingTemplate's template body and TestMapping's mappedFileContent encoding diverge; neither cause below is covered by it — cause 1 is about outputSampleLocation being ignored (that issue's recording passes no sample), and cause 2 is about the template never being evaluated (that issue's cause 3 is only about the member being an object instead of a string, and about status 201).

Cause 1 of 2: CreateStarterMappingTemplate does not derive the template from outputSampleLocation.
AWS: Smithy CreateStarterMappingTemplateRequest takes outputSampleLocation (S3Location), mappingType (required) and templateDetails (required); the response member mappingTemplate is a required String. Recorded behavior only (the model types mappingTemplate as a plain string): when outputSampleLocation points at a real sample EDI document in S3, the returned template is the canonicalized EDI-as-JSON skeleton of THAT document — a top-level "interchanges" array whose first object carries the named ISA elements with the sample's own values echoed verbatim (ISA_06_SenderId, ISA_13_InterchangeControlNumber, ...), a nested "functional_groups" -> "transactions" -> "segments" tree for the requested transaction set, and each transaction-set business element rendered as the literal "PLACEHOLDER: <that element's value in the sample>" (e.g. BEG_03 and PO1_09). The envelope member NAMES and the "PLACEHOLDER: " prefix are the fixed part of the requirement; the values after the prefix come from whatever sample the request points at.
Emulator: returns a non-empty mappingTemplate that parses as JSON but whose interchanges[0] does not even carry the named ISA elements — reading ISA_06_SenderId raises KeyError — so the sample at outputSampleLocation was never fetched or parsed and the template is a fixed synthetic skeleton.
Why: the entire purpose of this operation is to hand back a template the caller then edits and feeds to CreateTransformer/TestMapping. A template that does not name the elements of the caller's own sample means every mapping expression a client derives from it references members that do not exist in AWS's document, so the mapping silently produces nothing in production.
Fix: CreateStarterMappingTemplate's template generator — fetch the object named by outputSampleLocation, parse the EDI interchange, and emit the canonicalized skeleton derived from it (envelope values verbatim, business elements as "PLACEHOLDER: <sample value>").
Traces: b2bi.create-starter-mapping-template-with-output-sample.

Cause 2 of 2: TestMapping does not run the JSONata template over inputFileContent, and mappedFileContent is not parseable as the mapped document.
AWS: Smithy TestMappingRequest requires inputFileContent (@length 0..5000000), mappingTemplate (@length 0..350000) and fileFormat; TestMappingResponse.mappedFileContent is a required String carrying the mapped file's TEXT (observed JSON-string-encoded, so one parse yields a string holding the document). The template is genuinely evaluated: the documented identity expression "$" returns the input document unchanged, and a projecting expression such as "orderId" returns just that member's value — a different, narrower document for the identical input.
Emulator: mappedFileContent cannot be parsed as the mapped document at all — json parsing of the returned member fails immediately at character 0 ("Expecting value: line 1 column 1"), i.e. the member holds no JSON document, so neither the identity nor the projecting result is produced. The same operation's output is also wrong when invoked with a template obtained from CreateStarterMappingTemplate (cause 1's check reaches TestMapping too).
Why: TestMapping is how a client validates a mapping before creating a transformer. If the template is not executed and the returned member is not the mapped document, every mapping validates identically and a caller cannot tell a correct mapping from a broken one.
Fix: TestMapping's handler — evaluate mappingTemplate with a real JSONata engine over the parsed inputFileContent and serialize the result to TEXT in the required String member mappedFileContent.
Traces: b2bi.test-mapping-jsonata-identity-roundtrip, b2bi.create-starter-mapping-template-with-output-sample.

## Evidence
Replay of simparity-behavioral-aws-v1 (23/42 passed; all 19 failures in b2bi).
Cause 1 — b2bi.create-starter-mapping-template-with-output-sample: a sample X12 850 interchange (ISA sender 'SENDER', interchange control number '000000001', BEG*00*SA*PO123456, PO1 line item 'WIDGET-A') was PUT to S3 and passed as outputSampleLocation; CreateStarterMappingTemplate returned a non-empty JSON template, but asserting on it raised KeyError: 'ISA_06_SenderId' at interchanges[0] — the named ISA envelope members derived from the sample are absent entirely.
Cause 2 — b2bi.test-mapping-jsonata-identity-roundtrip: TestMapping(fileFormat='JSON', inputFileContent='{"orderId": "PO-10042", "amount": 42, "lineItems": [...]}', mappingTemplate='$') returned a mappedFileContent that cannot be decoded: json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0). The expected result of the identity template is the input document unchanged, and of mappingTemplate='orderId' just the value 'PO-10042' (the document values are the check's request values, not part of the requirement).

## Steps to Reproduce
1. Clone the repo and boot the emulator from mainline. 2. Run the simparity-behavioral-aws-v1 behavioral suite. 3. Re-replay b2bi.create-starter-mapping-template-with-output-sample and b2bi.test-mapping-jsonata-identity-roundtrip. 4. Cause 1: PutObject a sample X12 850 EDI interchange into a bucket, then call CreateStarterMappingTemplate(mappingType='JSONATA', templateDetails={x12:{transactionSet:'X12_850', version:'VERSION_4010'}}, outputSampleLocation={bucketName, key}) and parse mappingTemplate — observe that interchanges[0] carries none of the named ISA_*/GS_*/ST_* members from the sample and no 'PLACEHOLDER: <sample value>' business elements. 5. Cause 2: call TestMapping(fileFormat='JSON', inputFileContent=<a JSON document>, mappingTemplate='$') and then with mappingTemplate='orderId' — observe mappedFileContent is not the evaluated document (it does not parse as JSON) and does not differ between the identity and the projecting template.
