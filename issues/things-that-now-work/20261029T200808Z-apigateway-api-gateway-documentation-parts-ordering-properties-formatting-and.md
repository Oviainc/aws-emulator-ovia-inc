Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-documentation-parts-and-versions
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-documentation-parts-and-versions apigateway-import-documentation-parts-merge-and-overwrite apigateway-documentation-parts-bulk-import apigateway-import-documentation-parts-merge apigateway-import-documentation-parts-merge-then-overwrite apigateway-import-documentation-parts apigateway-documentation-parts-crud-and-import
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[*]
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[id="6ofuwnbaaa"].properties
Issue-Key: apigateway|GetDocumentationPart|2xx->2xx|$.properties
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[id="4kdlzkcaaa"].properties
Issue-Key: apigateway|ImportDocumentationParts|2xx->2xx|$.ids[*]
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[id="6ofuwnbaaa"]
Issue-Key: apigateway|DeleteDocumentationPart|2xx->4xx|status
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway documentation parts: ordering, properties formatting and import (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDocumentationParts

## Description
The documentation-part surface returns parts in creation order instead of AWS's, hands back the properties document verbatim instead of re-serialising it the way AWS does, and ImportDocumentationParts loses parts. Smithy: DocumentationPart = {id, location (DocumentationPartLocation), properties (String - a JSON document)}; GetDocumentationParts returns items (jsonName 'item') plus position; ImportDocumentationParts returns ids + warnings.

Cause 1 of 10 - collection ordering. AWS orders documentation parts by location (the recorded page is METHOD /toys, RESOURCE /pets, RESOURCE /toys), not by creation time; the emulator returns them in insertion order. Why: a client reading item[0] or paging positionally gets a different part than AWS would return. Fix: the ordering applied in GetDocumentationParts. Trace: apigateway-documentation-parts-and-versions#task#6.

Cause 2 of 10 - the same ordering defect on the later listing in that fixture. Trace: apigateway-documentation-parts-and-versions#task#10 (2 steps).

Causes 3-6 share one fault: the `properties` JSON is echoed exactly as submitted instead of being re-serialised. AWS re-serialises the properties document as pretty-printed JSON - two-space indentation, a space either side of the `:` separator, each key on its own line (e.g. `{\n  "description" : "..."\n}`). The emulator returns the compact form it stored. Why: a client that diffs or hashes the properties document (or reads it from an exported definition) sees different bytes than AWS produces. Fix: the DocumentationPart serialiser's properties rendering.
  Cause 3 of 10 - GetDocumentationParts after a bulk import. Trace: apigateway-documentation-parts-bulk-import#task#3 (3 steps).
  Cause 4 of 10 - GetDocumentationPart (single). Trace: apigateway-import-documentation-parts#task#3 (2 steps).
  Cause 5 of 10 - GetDocumentationParts after mode=merge. Trace: apigateway-import-documentation-parts-merge-and-overwrite#task#4 (2 steps).
  Cause 6 of 10 - GetDocumentationParts after mode=overwrite. Trace: apigateway-import-documentation-parts-merge-and-overwrite#task#7 (2 steps).

Cause 7 of 10 - an imported RESPONSE-location part is missing from the listing (alongside the properties formatting above). AWS lists the part created for location {type: RESPONSE, method, path, statusCode}; the emulator never creates it. Fix: ImportDocumentationParts' handling of RESPONSE locations. Trace: apigateway-documentation-parts-crud-and-import#task#8.

Cause 8 of 10 - ImportDocumentationParts returns one id per part it created; the recording imports two parts and AWS returns two ids, the emulator one. Why: a client that records the returned ids can only delete/patch half of what it imported. Fix: ImportDocumentationParts' id collection. Trace: apigateway-documentation-parts-crud-and-import#task#7.

Cause 9 of 10 - ImportDocumentationParts with mode=overwrite drops the part it was supposed to replace: AWS lists the API-location part with the overwritten properties, the emulator lists nothing for that id. Fix: the overwrite path in ImportDocumentationParts. Trace: apigateway-import-documentation-parts-merge-then-overwrite#task#5 (2 steps).

Cause 10 of 10 - consequence in the same fixture: DeleteDocumentationPart for that part answers 404 instead of 202 (Smithy: DeleteDocumentationPart succeeds with 202). Fix: closes with cause 9. Trace: apigateway-import-documentation-parts-merge-then-overwrite#teardown#7.

No open issue owns these clusters.

## Evidence
Causes 1/2: $.item[0]/$.item[1]/$.item[2] occupied by different parts than expected - expected order METHOD /toys, RESOURCE /pets, RESOURCE /toys; got creation order.
Causes 3-6: $.properties value_mismatch - expected '{\n  "description" : "..."\n}', got '{"description":"..."}'.
Cause 7: $.item[2] missing (expected the RESPONSE-location part) and $.item[1].properties compact.
Cause 8: $.ids[1] missing (expected a second imported id).
Cause 9: $.item[id=...] missing (expected the overwritten API-location part).
Cause 10: status expected 202, got 404.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Causes 1/2: create METHOD and RESOURCE documentation parts, then GET /restapis/{id}/documentation/parts and compare item order (apigateway-documentation-parts-and-versions#task#6).
3. Causes 3-6: GET a documentation part and compare the properties string byte for byte.
4. Causes 7-10: PUT /restapis/{id}/documentation/parts with mode=merge then mode=overwrite (apigateway-documentation-parts-crud-and-import, apigateway-import-documentation-parts-merge-then-overwrite) and compare ids and the resulting listing.
