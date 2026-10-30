Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-collection-and-face-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-collection-and-face-error-paths rekognition-index-search-and-delete-faces
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|DescribeCollection|4xx->4xx|$.Code
Issue-Key: rekognition|DescribeCollection|4xx->4xx|$.Message
Issue-Key: rekognition|ListFaces|4xx->4xx|$.Code
Issue-Key: rekognition|DeleteFaces|4xx->4xx|$.Code
Issue-Key: rekognition|IndexFaces|4xx->4xx|$.Code
Issue-Key: rekognition|DeleteCollection|4xx->4xx|$.Code
Issue-Key: rekognition|DeleteCollection|4xx->4xx|$.Message
Issue-Key: rekognition|SearchFaces|4xx->4xx|$.Code
Issue-Key: rekognition|SearchFaces|4xx->4xx|$.Message
Issue-Key: rekognition|SearchFaces|4xx->4xx|$.__type
Issue-Commit: 0e13384b9161da1d0d65830e33a6518406bea9f8

# Rekognition error bodies omit the Code member and use non-AWS exception types and messages

> _Note: this issue was identified against the code base at commit `0e13384b9161da1d0d65830e33a6518406bea9f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCollection

## Description
Six causes, all in the Rekognition error path. The service is awsJson1_1 (Smithy: RekognitionService, protocols [awsJson1_1], version 2016-06-27). Every recorded Rekognition error body carries THREE top-level members -- __type, Code and Message -- and the emulator's serializer emits only __type and Message, so the Code member is missing from every modelled error it returns. Two causes additionally get the exception type and/or the message text wrong.

Cause 1 of 6: DescribeCollection on a collection that does not exist.
AWS: Smithy: DescribeCollection errors include ResourceNotFoundException (client, httpError 400). Recorded response: HTTP 400 with body {"__type": "ResourceNotFoundException", "Code": "ResourceNotFoundException", "Message": "Collection:<CollectionId> not found!"}. Message template: fixed text "Collection:" + the CollectionId taken from the request + fixed text " not found!" (no space after the colon, trailing exclamation mark).
Emulator: HTTP 400, __type ResourceNotFoundException, NO Code member, and Message "The collection id: <CollectionId> does not exist" -- wording the service never produces.
Why: SDKs and tools that classify errors from the Code member (and anything matching on the documented message) see an unidentified error; a client that branches on ResourceNotFoundException via Code falls through to its generic/unknown-error path instead of treating the collection as absent.
Fix: the Rekognition awsJson1_1 error serializer (add Code alongside __type for every modelled error) plus the DescribeCollection handler's not-found message text.
Traces: rekognition-collection-and-face-error-paths#task#1; same cause also hit at rekognition-index-search-and-delete-faces#task#9.

Cause 2 of 6: ListFaces against a collection that does not exist.
AWS: Smithy: ListFaces errors include ResourceNotFoundException (client, 400). Recorded response: HTTP 400 carrying Code "ResourceNotFoundException" in the body next to __type and Message.
Emulator: HTTP 400 and the correct __type, but the Code member is absent.
Why: same as cause 1 -- the error cannot be classified from Code.
Fix: the shared Rekognition error serializer.
Traces: rekognition-collection-and-face-error-paths#task#2.

Cause 3 of 6: DeleteFaces against a collection that does not exist.
AWS: Smithy: DeleteFaces errors include ResourceNotFoundException (client, 400). Recorded body carries Code "ResourceNotFoundException".
Emulator: HTTP 400, correct __type, Code member absent.
Why: same as cause 1.
Fix: the shared Rekognition error serializer.
Traces: rekognition-collection-and-face-error-paths#task#3.

Cause 4 of 6: IndexFaces against a collection that does not exist.
AWS: Smithy: IndexFaces errors include ResourceNotFoundException (client, 400). Recorded body carries Code "ResourceNotFoundException".
Emulator: HTTP 400, correct __type, Code member absent.
Why: same as cause 1.
Fix: the shared Rekognition error serializer.
Traces: rekognition-collection-and-face-error-paths#task#4.

Cause 5 of 6: DeleteCollection on a collection that does not exist.
AWS: Smithy: DeleteCollection errors include ResourceNotFoundException (client, 400). Recorded response: HTTP 400 with Code "ResourceNotFoundException" and Message "Collection:<CollectionId> not found!" -- same template as cause 1, variable part is the request's CollectionId.
Emulator: HTTP 400, correct __type, no Code, and Message "The collection id: <CollectionId> does not exist".
Why: same as cause 1, plus the message does not match AWS's wording for the missing-collection class.
Fix: the shared error serializer plus the DeleteCollection handler's not-found message.
Traces: rekognition-collection-and-face-error-paths#task#6.

Cause 6 of 6: SearchFaces with a FaceId that is not present in an existing collection -- wrong exception class entirely.
AWS: Smithy: SearchFaces errors include both InvalidParameterException and ResourceNotFoundException (both client, 400); the recorded response for an unknown FaceId in a collection that DOES exist is HTTP 400 with __type "InvalidParameterException", Code "InvalidParameterException" and Message "faceId was not found in the collection." -- that message is entirely fixed text, with no value interpolated from the request.
Emulator: HTTP 400 with __type "ResourceNotFoundException", no Code, and Message "The face id: <FaceId> does not exist in collection <CollectionId>" (the emulator interpolates the request's FaceId and CollectionId into a message AWS does not use).
Why: AWS distinguishes "the collection is missing" (ResourceNotFoundException) from "the face id you passed is not in this collection" (InvalidParameterException). A client that retries or re-creates the collection on ResourceNotFoundException takes the wrong recovery branch here, and one that catches InvalidParameterException never sees the error at all.
Fix: the SearchFaces handler's face-lookup validation (raise InvalidParameterException with the fixed message for an unknown FaceId in an existing collection), plus the shared error serializer for the missing Code.
Traces: rekognition-index-search-and-delete-faces#task#4.

## Evidence
Replay of simparity-wire-aws-v1, verdict=mismatch on all six, expected status == actual status (400) in every case -- only the body diverges.

Cause 1 (rekognition-collection-and-face-error-paths#task#1, DescribeCollection): $.Code missing (expected 'ResourceNotFoundException', got None); $.Message value_mismatch (expected 'Collection:<id> not found!', got 'The collection id: <id> does not exist'). affected_count=2, also rekognition-index-search-and-delete-faces#task#9.
Cause 2 (...#task#2, ListFaces): $.Code missing (expected 'ResourceNotFoundException', got None).
Cause 3 (...#task#3, DeleteFaces): $.Code missing (expected 'ResourceNotFoundException', got None).
Cause 4 (...#task#4, IndexFaces): $.Code missing (expected 'ResourceNotFoundException', got None).
Cause 5 (...#task#6, DeleteCollection): $.Code missing; $.Message value_mismatch (expected 'Collection:<id> not found!', got 'The collection id: <id> does not exist').
Cause 6 (rekognition-index-search-and-delete-faces#task#4, SearchFaces): $.__type value_mismatch (expected 'InvalidParameterException', got 'ResourceNotFoundException'); $.Code missing (expected 'InvalidParameterException', got None); $.Message value_mismatch (expected 'faceId was not found in the collection.', got 'The face id: <faceId> does not exist in collection <collectionId>').

## Steps to Reproduce
1. Replay fixture rekognition-collection-and-face-error-paths and inspect steps task#1 (DescribeCollection), task#2 (ListFaces), task#3 (DeleteFaces), task#4 (IndexFaces) and task#6 (DeleteCollection): each targets a collection id that was never created; assert the 400 body contains a top-level Code member equal to "ResourceNotFoundException" and, for task#1/task#6, Message == "Collection:<CollectionId> not found!".
2. Replay fixture rekognition-index-search-and-delete-faces step task#4: CreateCollection first, then SearchFaces on that existing collection with FaceId 00000000-0000-0000-0000-000000000000; assert HTTP 400 with __type and Code both "InvalidParameterException" and Message "faceId was not found in the collection.".
3. Also replay rekognition-index-search-and-delete-faces#task#9 for the DescribeCollection variant of cause 1.
