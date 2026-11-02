Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-index-search-and-delete-faces
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-index-search-and-delete-faces rekognition-tag-untag-resource-round-trip
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|CreateCollection|2xx->2xx|$.CollectionArn
Issue-Key: rekognition|CreateCollection|2xx->2xx|$.FaceModelVersion
Issue-Key: rekognition|IndexFaces|2xx->2xx|$.FaceModelVersion
Issue-Key: rekognition|IndexFaces|2xx->2xx|$.FaceRecords[*]
Issue-Key: rekognition|ListFaces|2xx->2xx|$.FaceModelVersion
Issue-Key: rekognition|ListFaces|2xx->2xx|$.Faces[*]
Issue-Key: rekognition|DeleteFaces|2xx->2xx|$.DeletedFaces
Issue-Key: rekognition|DeleteFaces|2xx->2xx|$.UnsuccessfulFaceDeletions[*]
Issue-Commit: 0e13384b9161da1d0d65830e33a6518406bea9f8

# Rekognition reports stale FaceModelVersion 6.0 and fabricates face records the recorded service never indexes

> _Note: this issue was identified against the code base at commit `0e13384b9161da1d0d65830e33a6518406bea9f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IndexFaces

## Description
Five causes in the Rekognition collection/face success path. Two faults run through them: (a) every response that carries FaceModelVersion reports "6.0" where the recorded service reports "7.0"; (b) the emulator does not actually perform face detection -- it synthesises a face record for any submitted image, so faces appear that the service never indexed, and the delete path reports the inverse of the recorded result.

Cause 1 of 5: CreateCollection returns a CollectionArn with an "arn:" prefix and the wrong FaceModelVersion.
AWS: Smithy: CreateCollection output members CollectionArn (String) and FaceModelVersion (String); recorded behavior only for their formats. Recorded response: HTTP 200 with CollectionArn in the form "aws:rekognition:<region>:<accountId>:collection/<CollectionId>" -- Rekognition's CreateCollection emits this ARN WITHOUT the leading "arn:" label (unlike DescribeCollection's CollectionARN) -- and FaceModelVersion "7.0", the current face model for a newly created collection.
Emulator: HTTP 200 with CollectionArn "arn:aws:rekognition:<region>:<accountId>:collection/<CollectionId>" (extra "arn:" prefix) and FaceModelVersion "6.0".
Why: a client that stores the returned CollectionArn and feeds it back to TagResource/ListTagsForResource/UntagResource, or compares it against the string a real account returned, gets a value of a different shape; and a client that gates on the face model version (feature availability, index/search compatibility) sees a model generation the service no longer returns.
Fix: the CreateCollection handler's ARN construction and the collection state's face-model-version constant.
Traces: rekognition-index-search-and-delete-faces#task#1.

Cause 2 of 5: IndexFaces invents a FaceRecord for an image the service indexes nothing from.
AWS: Smithy: IndexFaces output = FaceRecords (FaceRecordList), OrientationCorrection, FaceModelVersion, UnindexedFaces. Recorded response: HTTP 200 with FaceModelVersion "7.0" and NO FaceRecords entries -- the probe image yields no indexed face, so the list is empty and index 0 does not exist.
Emulator: HTTP 200 with FaceModelVersion "6.0" and a fabricated FaceRecords[0] containing a Face with a generated FaceId, generated ImageId, the request's ExternalImageId echoed back and IndexFacesModelVersion "6.0".
Why: the caller is told a face was indexed when none was. Downstream logic that counts indexed faces, stores the returned FaceId, or branches on UnindexedFaces proceeds on a face that does not exist in the real service, and the collection's face count diverges from AWS for the rest of the session.
Fix: the IndexFaces handler/state -- do not synthesise a FaceRecord when no face is detected in the submitted image; emit the empty FaceRecords list and the correct FaceModelVersion.
Traces: rekognition-index-search-and-delete-faces#task#2.

Cause 3 of 5: ListFaces returns the fabricated face and the stale model version.
AWS: Smithy: ListFaces output = Faces (FaceList), NextToken, FaceModelVersion. Recorded response for the probe collection: HTTP 200, FaceModelVersion "7.0", Faces empty (no index 0).
Emulator: HTTP 200, FaceModelVersion "6.0", Faces[0] = the face invented by cause 2 (FaceId/ImageId generated, ExternalImageId from the IndexFaces request, IndexFacesModelVersion "6.0").
Why: the invented face leaks out of IndexFaces into collection enumeration, so a client that lists a collection to reconcile its own records finds a face it never created and that no real account would return.
Fix: follows from cause 2 (collection face state), plus the ListFaces serializer's FaceModelVersion.
Traces: rekognition-index-search-and-delete-faces#task#3 (cluster also covers rekognition-index-search-and-delete-faces#task#7, the post-delete listing).

Cause 4 of 5: DeleteFaces of an unknown FaceId returns the wrong response shape.
AWS: Smithy: DeleteFaces output = DeletedFaces (FaceIdList), UnsuccessfulFaceDeletions (UnsuccessfulFaceDeletionsList). Recorded response for a FaceId that is not in an existing collection: HTTP 200 with DeletedFaces present and empty, and UnsuccessfulFaceDeletions absent.
Emulator: HTTP 200 with DeletedFaces absent entirely and UnsuccessfulFaceDeletions[0] = {FaceId: <the requested face id>, Reasons: ["FACE_NOT_FOUND"]}.
Why: a client that reads response.DeletedFaces to learn what was removed dereferences a missing member, and one that treats a non-empty UnsuccessfulFaceDeletions as a partial failure raises an error on a call AWS reports as a clean no-op. Both branches invert against the real service.
Fix: the DeleteFaces handler -- always emit DeletedFaces (empty when nothing matched) and do not report an unknown face id as an unsuccessful deletion.
Traces: rekognition-index-search-and-delete-faces#task#6.

Cause 5 of 5: CreateCollection ARN prefix on the second fixture (same ARN fault as cause 1, separate recorded step).
AWS: recorded response: HTTP 200 with CollectionArn "aws:rekognition:<region>:<accountId>:collection/<CollectionId>".
Emulator: HTTP 200 with CollectionArn "arn:aws:rekognition:<region>:<accountId>:collection/<CollectionId>".
Why: this is the ARN the tagging round-trip then passes to TagResource/ListTagsForResource, so the malformed value propagates into the tag path.
Fix: the CreateCollection handler's ARN construction.
Traces: rekognition-tag-untag-resource-round-trip#task#1.

## Evidence
Replay of simparity-wire-aws-v1, verdict=mismatch on all five; expected status == actual status (200) in every case -- only the body diverges.

Cause 1 (rekognition-index-search-and-delete-faces#task#1, CreateCollection): $.CollectionArn value_mismatch (expected 'aws:rekognition:us-west-2:<acct>:collection/<id>', got 'arn:aws:rekognition:us-west-2:<acct>:collection/<id>'); $.FaceModelVersion value_mismatch (expected '7.0', got '6.0').
Cause 2 (...#task#2, IndexFaces): $.FaceModelVersion value_mismatch (expected '7.0', got '6.0'); $.FaceRecords[0] extra (expected None, got {'Face': {'FaceId': <generated>, 'ImageId': <generated>, 'ExternalImageId': 'probe-face-1', 'IndexFacesModelVersion': '6.0'}}).
Cause 3 (...#task#3, ListFaces): $.FaceModelVersion value_mismatch (expected '7.0', got '6.0'); $.Faces[0] extra (expected None, got the face fabricated by cause 2). affected_count=2, also ...#task#7.
Cause 4 (...#task#6, DeleteFaces): $.DeletedFaces missing (expected [], got None); $.UnsuccessfulFaceDeletions[0] extra (expected None, got {'FaceId': <requested id>, 'Reasons': ['FACE_NOT_FOUND']}).
Cause 5 (rekognition-tag-untag-resource-round-trip#task#1, CreateCollection): $.CollectionArn value_mismatch (expected 'aws:rekognition:us-west-2:<acct>:collection/<id>', got 'arn:aws:rekognition:...').

## Steps to Reproduce
1. Replay fixture rekognition-index-search-and-delete-faces. Step task#1 (CreateCollection): assert CollectionArn starts with "aws:rekognition:" and NOT "arn:aws:rekognition:", and FaceModelVersion == "7.0".
2. Step task#2 (IndexFaces with the fixture's probe image and ExternalImageId probe-face-1): assert FaceModelVersion == "7.0" and FaceRecords is empty -- no FaceRecords[0].
3. Step task#3 (ListFaces on that collection, and task#7 after the delete): assert FaceModelVersion == "7.0" and Faces is empty.
4. Step task#6 (DeleteFaces with FaceIds=[00000000-0000-0000-0000-000000000000]): assert HTTP 200, DeletedFaces present and empty, UnsuccessfulFaceDeletions absent.
5. Replay fixture rekognition-tag-untag-resource-round-trip step task#1 and re-assert the CollectionArn form.
