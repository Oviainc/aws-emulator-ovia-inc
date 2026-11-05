Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rekognition.index-faces-success-face-record rekognition.delete-faces-partial-failure-and-state rekognition.search-faces-round-trip-by-id-and-image
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition.index-faces-success-face-record|run#7|AssertionError|assert len(result["FaceRecords"]) == 1, f"expected exactly 1 FaceRecord, got {len(result['FaceRecords'])}: {result['FaceRecords']}"
Issue-Key: rekognition.delete-faces-partial-failure-and-state|run#7|AssertionError|assert len(indexed["FaceRecords"]) == 1, f"expected 1 indexed face, got {indexed['FaceRecords']!r}"
Issue-Key: rekognition.search-faces-round-trip-by-id-and-image|run#8|AssertionError|assert len(first["FaceRecords"]) == 1, f"expected exactly 1 face indexed from copy-one, got {len(first['FaceRecords'])}"
Issue-Commit: 38a893cb906a213bf779a9e5f05354914555db8b

# Rekognition IndexFaces enrols several fabricated faces for an image containing one face (a full-frame record plus synthetic sub-boxes), so FaceRecords, FaceCount, DeleteFaces and SearchFaces* are all off

> _Note: this issue was identified against the code base at commit `38a893cb906a213bf779a9e5f05354914555db8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IndexFaces

## Description
One net-new root cause, reached over the live emulator (rekognition is bootstrapped and answering).

Related, named for the fixer: open issue 20261029T194740Z-rekognition-collection-face-fidelity-wrong-not-found-message.md carries these two check ids under its causes 2/3/4, but its Issue-Keys bind them to the assertions `result["FaceRecords"] == []` (no-face image) and `face["Confidence"]` / `bogus_face_id not in unsuccessful_ids`; all three checks now fail EARLIER, on the number of records returned for a real single-face image, so the replay reports them as unowned. The fixed issue 20261029T191740Z-rekognition-reports-stale-facemodelversion-6-0-and-fabricates.md claimed the fabrication fault from the wire corpus; this is not a refusal (the call answers HTTP 200), so it is filed as its own cause rather than as a reopen.

Cause 1 of 1: IndexFaces returns more FaceRecords than there are faces in the submitted image.
AWS: Smithy: IndexFaces input CollectionId (required), Image (required), MaxFaces (@range min 1), QualityFilter; output FaceRecords (FaceRecordList), UnindexedFaces, FaceModelVersion, OrientationCorrection. The model does not bound the list, so recorded behavior only: IndexFaces emits exactly one FaceRecord per face actually DETECTED in the image (plus an UnindexedFaces entry per detected-but-rejected face), and each Face.BoundingBox describes that face's region of the image. For a photograph containing a single human face the recorded response is HTTP 200 with exactly one FaceRecord, and DescribeCollection.FaceCount is then 1.
Emulator: the same single-face JPEG yields 5 FaceRecords in rekognition.index-faces-success-face-record and in rekognition.search-faces-round-trip-by-id-and-image, and 2 FaceRecords in rekognition.delete-faces-partial-failure-and-state — i.e. the count is synthesised, not derived from the image, and it is not even stable across calls on the same bytes. The first record always carries the degenerate whole-image BoundingBox {Width: 1.0, Height: 1.0, Left: 0.0, Top: 0.0}, which is not a face region at all; the remaining records carry further fabricated boxes/landmarks. Each fabricated record is also ENROLLED, so the collection holds several faces for one real face.
Why: every client that indexes an upload and stores "the" FaceId takes the wrong branch — single-face enrolment flows (identity enrolment, profile photos) see a multi-face image where AWS sees one; FaceCount, ListFaces and DeleteFaces reconcile against phantom ids; SearchFaces/SearchFacesByImage match the phantom records; and a whole-image bounding box fed into a crop/blur pipeline returns the entire picture. Because the count varies between calls on identical bytes, tests built on the emulator are also nondeterministic.
Fix: the face-detection step of the IndexFaces handler and the collection face state it writes — derive one record per detected face from the submitted image (empty FaceRecords when none is detected) with a bounding box for that face's region, instead of emitting a fixed/synthetic set of records per request.
Traces: rekognition.index-faces-success-face-record, rekognition.delete-faces-partial-failure-and-state, rekognition.search-faces-round-trip-by-id-and-image.

## Evidence
Cause 1 — [mismatch] rekognition.index-faces-success-face-record: 'AssertionError: expected exactly 1 FaceRecord, got 5: [{'Face': {'FaceId': ..., 'BoundingBox': {'Width': 1.0, 'Height': 1.0, 'Left': 0.0, 'Top': 0.0}, 'ImageId': ..., 'Confidence': 81.05..., 'IndexFacesModelVersion': '7.0'}, ...}, {... 'BoundingBox': {'Width': 0.601..., 'Height': 0.700..., 'Left': 0.033..., 'Top': 0.145...} ...}, ...]' after IndexFaces(CollectionId=<new collection>, Image={'Bytes': <320px JPEG of one human face shipped with the check>}). The FaceIds, ImageId and the exact float values are per-run generated values; the requirement is one record per detected face with that face's bounding box.
Cause 1 — [mismatch] rekognition.delete-faces-partial-failure-and-state: 'AssertionError: expected 1 indexed face, got [ ...2 FaceRecords... ]' on the same image, the first again carrying the whole-image BoundingBox {Width: 1.0, Height: 1.0, Left: 0.0, Top: 0.0}. Note the differing count (2 here vs 5 above) on identical bytes.
Cause 1 — [mismatch] rekognition.search-faces-round-trip-by-id-and-image: 'AssertionError: expected exactly 1 face indexed from copy-one, got 5' after IndexFaces(..., ExternalImageId='copy-one').
All three calls answered HTTP 200; the divergence is the content of FaceRecords, not a status or error code.

## Steps to Reproduce
1. rek.create_collection(CollectionId=Z).
2. rek.index_faces(CollectionId=Z, Image={'Bytes': <JPEG photograph containing exactly one human face>}).
3. Assert len(FaceRecords) == 1 and that FaceRecords[0].Face.BoundingBox is the face's region, not {Width:1.0, Height:1.0, Left:0.0, Top:0.0}; observe 5 (or 2) records whose first entry is the whole-image box.
4. rek.describe_collection(CollectionId=Z) — assert FaceCount == 1; observe the fabricated count.
5. Repeat step 2 and compare the counts between calls on identical bytes.
Replay checks rekognition.index-faces-success-face-record, rekognition.delete-faces-partial-failure-and-state and rekognition.search-faces-round-trip-by-id-and-image.
