Issue-Source: trace-sweep-divergence
Issue-Trace: b2bi-list-operations-and-pagination-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b2bi-list-operations-and-pagination-errors b2bi-mapping-template-and-test-mapping
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|ListProfiles|4xx->4xx|$.Message
Issue-Key: b2bi|CreateStarterMappingTemplate|2xx->2xx|$.mappingTemplate
Issue-Key: b2bi|TestMapping|2xx->2xx|$.mappedFileContent
Issue-Key: b2bi|TestMapping|2xx->2xx|status
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# b2bi response text diverges: pagination refusal wording, starter mapping template and TestMapping output

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestMapping

## Description
Three net-new causes, one shared fault: the emulator serves b2bi payload TEXT it composed itself rather than the text AWS returns — one error message and two document bodies. Status and shape are otherwise right in all three.

Cause 1 of 3: ListProfiles refusal wording for a malformed nextToken.
AWS: Smithy ListProfilesRequest.nextToken targets PageToken (@length 1..2048); an unusable token is refused with the common ValidationException (member `Message`), and the recorded message is exactly "Invalid pagination token" — entirely fixed text, with no echo of the offending token and no member name in it. Status 400 matches.
Emulator: 400 with Message "nextToken is not a valid pagination token" — the emulator's own wording, naming the member and reordering the sentence.
Why: a client (or a test) that keys off AWS's refusal text to distinguish an expired/garbage cursor from another ValidationException does not match.
Fix: ListProfiles' pagination-token validation message wording.
Trace: b2bi-list-operations-and-pagination-errors#task#3.

Cause 2 of 3: CreateStarterMappingTemplate returns the wrong template document.
AWS: recorded behavior only (the model types mappingTemplate as a plain string). The recorded template is a pretty-printed JSON skeleton of the requested EDI structure: a top-level "interchanges" array whose objects carry the named ISA elements (ISA_01_AuthorizationQualifier ... ISA_15_TestIndicator) with the values a sample interchange of the requested X12 transaction set uses, a nested "functional_groups" array carrying the named GS elements, a nested "transactions" array carrying ST_01_TransactionSetIdentifierCode/ST_02_TransactionSetControlNumber for the requested transaction set, and a "segments" array in which every element of every segment of that transaction set (BEG_01..BEG_12, CUR_01..CUR_09, ...) is present and set to the literal "PLACEHOLDER". Two-space indentation and the "key" : value spacing are part of the recorded text.
Emulator: a different mappingTemplate body for the same request; the recorded skeleton is not reproduced.
Why: the whole point of this operation is to hand the caller a template they then edit and pass to CreateTransformer/TestMapping; a template of a different shape does not name the elements a client's downstream mapping expression references, so the generated mapping fails against AWS's.
Fix: CreateStarterMappingTemplate's template generator.
Trace: b2bi-mapping-template-and-test-mapping#task#1 (also affects #task#2).

Cause 3 of 3: TestMapping answers 201 and returns the mapped document unencoded.
AWS: protocol awsJson1_0 has no 201 — success is 200. TestMappingResponse.mappedFileContent is modeled as a required String, and the recorded value is the mapped document's TEXT carried in that string: a JSON string whose content is the pretty-printed output document (newlines and two-space indentation preserved, members in the template's order), hence doubly-escaped on the wire.
Emulator: status 201, and mappedFileContent is a compact JSON object serialized directly into the member (keys reordered alphabetically, no pretty-printing), i.e. a structure where AWS sends a string.
Why: a client reads mappedFileContent as a string and parses or displays it; receiving an object (or a reordered, unindented rendering) breaks the parse and makes a byte comparison of the mapping result against AWS impossible, while 201 breaks status handling on a protocol whose success is always 200.
Fix: TestMapping's handler/serializer — render the mapped document to text, place it in the String member, and answer 200.
Trace: b2bi-mapping-template-and-test-mapping#task#3. (The order ids and item values in the recorded document are request values, not part of the requirement.)

## Evidence
Replay of simparity-wire-aws-v1. b2bi-list-operations-and-pagination-errors#task#3 ListProfiles (400/400): $.Message expected 'Invalid pagination token', got 'nextToken is not a valid pagination token'. b2bi-mapping-template-and-test-mapping#task#1 CreateStarterMappingTemplate (200/200): $.mappingTemplate value_mismatch — expected the pretty-printed interchanges/functional_groups/transactions/segments skeleton with named ISA_*/GS_*/ST_* elements and "PLACEHOLDER" segment element values; got a different document (also affects #task#2). b2bi-mapping-template-and-test-mapping#task#3 TestMapping: status expected 200 got 201; $.mappedFileContent expected a JSON string containing the pretty-printed mapped document ('"{\n  \"orderId\": ... }"'), got the object {"item":"widget","orderId":"12345","quantity":3}.

## Steps to Reproduce
1. Boot the emulator from mainline. 2. Replay simparity-wire-aws-v1. 3. Replay b2bi-list-operations-and-pagination-errors#task#3, b2bi-mapping-template-and-test-mapping#task#1 and b2bi-mapping-template-and-test-mapping#task#3. 4. Observe the emulator's own pagination-refusal wording, a starter mapping template unlike the recorded skeleton, and TestMapping answering 201 with mappedFileContent as an object instead of 200 with the mapped document as a string.
