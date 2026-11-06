Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rekognition.collection-lifecycle-create-describe-delete rekognition.create-collection-duplicate-id-rejected rekognition.list-collections-reflects-created-and-deleted rekognition.index-faces-no-face-image-returns-empty rekognition.index-faces-success-face-record rekognition.delete-faces-partial-failure-and-state
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition.collection-lifecycle-create-describe-delete|run#19|AssertionError|assert message == not_found_message, f"describe after delete message: {message!r}"
Issue-Key: rekognition.create-collection-duplicate-id-rejected|run#20|AssertionError|assert gone_message == f"Collection:{collection_id} not found!", (
Issue-Key: rekognition.list-collections-reflects-created-and-deleted|run#18|AssertionError|assert message == f"Collection:{id_a} not found!", f"re-deleting {id_a} returned message {message!r}"
Issue-Key: rekognition.index-faces-no-face-image-returns-empty|run#7|AssertionError|assert result["FaceRecords"] == []
Issue-Key: rekognition.index-faces-success-face-record|run#9|KeyError|assert face["Confidence"] > 50, f"expected face Confidence > 50, got {face['Confidence']}"
Issue-Key: rekognition.delete-faces-partial-failure-and-state|run#13|AssertionError|assert bogus_face_id not in unsuccessful_ids, (
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# Rekognition collection/face fidelity: wrong not-found message, fabricated IndexFaces records, Face without Confidence, and DeleteFaces mis-reporting unknown face ids

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IndexFaces

## Description
Four net-new root causes in the Rekognition collection and face path, all reached over the live emulator by the behavioral suite (service rekognition is bootstrapped and answering). Related open issues name two of these faults from the WIRE corpus and are referenced per cause below: 20261029T191740Z-rekognition-error-bodies-omit-the-code-member-and.md (missing-collection error body) and 20261029T191740Z-rekognition-reports-stale-facemodelversion-6-0-and-fabricates.md (fabricated face records / DeleteFaces shape). Neither was marked as OWNING these behavioral checks, so they are filed here with their check ids; a fixer closing them should re-replay the check ids listed under each cause.

Cause 1 of 4: the ResourceNotFoundException message for a missing collection uses wording Rekognition never produces (DescribeCollection and DeleteCollection).
AWS: Smithy: DescribeCollection and DeleteCollection both declare ResourceNotFoundException (client error, httpError 400). Recorded behavior for a collection id that does not exist: HTTP 400, error code ResourceNotFoundException, Message "Collection:<CollectionId> not found!" — the fixed text is "Collection:" (no space after the colon) and " not found!" (trailing exclamation mark); the only variable part is the CollectionId copied from the request.
Emulator: the error code is correct (ResourceNotFoundException) but the Message is "The collection id: <CollectionId> does not exist" — different fixed text in a different shape, with the request's CollectionId interpolated after a colon-space.
Why: a client (or test harness, or operator runbook) that matches on the documented missing-collection message to distinguish "the collection is gone" from other 400s never matches, and log/alert rules keyed on AWS's wording silently stop firing. The same wording is returned for both DescribeCollection and DeleteCollection, so neither path is usable for that discrimination.
Fix: the not-found message construction in the DescribeCollection and DeleteCollection handlers (or the shared collection-lookup helper they call) in the Rekognition service crate's ops module. This is the same text fault that open issue 20261029T191740Z-rekognition-error-bodies-omit-the-code-member-and.md describes as its causes 1 and 5 from the wire corpus.
Traces: rekognition.collection-lifecycle-create-describe-delete (DescribeCollection after delete, and DeleteCollection of an already-deleted collection), rekognition.create-collection-duplicate-id-rejected (DescribeCollection after the restore delete), rekognition.list-collections-reflects-created-and-deleted (re-DeleteCollection).

Cause 2 of 4: IndexFaces synthesises a face record for an image that contains no detectable face.
AWS: Smithy: IndexFacesResponse = FaceRecords (FaceRecordList), OrientationCorrection, FaceModelVersion, UnindexedFaces; nothing in the model forces a record. Recorded behavior only: for an image with no detectable face IndexFaces succeeds with HTTP 200 and an EMPTY FaceRecords list and an EMPTY UnindexedFaces list, and the collection's FaceCount stays 0.
Emulator: HTTP 200 with a non-empty FaceRecords list for a synthetic solid-colour PNG that contains no face at all, i.e. the handler emits one record per submitted image regardless of image content.
Why: a caller is told a face was indexed when none was. Code that counts indexed faces, stores the returned FaceId, or branches on "no face detected" (a very common upload-validation branch) takes the wrong path, and the collection's FaceCount diverges from AWS for the rest of the session.
Fix: the IndexFaces handler and the collection face state — only record faces actually detected in the submitted image, and return an empty FaceRecords/UnindexedFaces list otherwise. Open issue 20261029T191740Z-rekognition-reports-stale-facemodelversion-6-0-and-fabricates.md describes the same fabrication from the wire corpus (its cause 2) but does not carry this check id.
Traces: rekognition.index-faces-no-face-image-returns-empty.

Cause 3 of 4: the Face member of an IndexFaces FaceRecord omits Confidence.
AWS: Smithy: Face.Confidence targets Percent (float, @range min 0 max 100), documented as "Confidence level that the bounding box contains a face"; Face also carries BoundingBox. Recorded behavior: a successful IndexFaces of an image containing a real face returns FaceRecords[0].Face with FaceId, BoundingBox and Confidence populated (a face accepted for indexing scores well above 50).
Emulator: FaceRecords[0].Face is returned with no Confidence member at all — the check fails with KeyError: 'Confidence' when reading it, so any SDK consumer dereferencing the modelled member gets nothing.
Why: every client that gates indexing on the face's confidence (threshold filtering is the documented use of this member) reads a missing value and either crashes or treats the face as zero-confidence; the response is not a valid Face as modelled.
Fix: the IndexFaces response serializer / the Face record built by the IndexFaces handler — populate Confidence (and BoundingBox) on each emitted Face.
Traces: rekognition.index-faces-success-face-record.

Cause 4 of 4: DeleteFaces reports a never-indexed FaceId as an unsuccessful deletion instead of ignoring it.
AWS: Smithy: DeleteFacesResponse = DeletedFaces (FaceIdList) and UnsuccessfulFaceDeletions (list of UnsuccessfulFaceDeletion{FaceId, UserId, Reasons}). Recorded behavior: UnsuccessfulFaceDeletions reports only faces the collection holds but cannot delete (e.g. a face associated with a User); a FaceId the collection never held is simply absent from BOTH lists, and the call is a 200 with the genuinely deleted ids in DeletedFaces.
Emulator: for DeleteFaces(FaceIds=[<indexed id>, <never-indexed uuid>]) the never-indexed uuid comes back inside UnsuccessfulFaceDeletions (with Reasons FACE_NOT_FOUND), so the response reports a partial failure AWS does not report.
Why: a client that treats a non-empty UnsuccessfulFaceDeletions as a partial failure raises/retries on a call AWS reports as clean, and reconciliation logic that re-queues "unsuccessful" ids loops forever on ids that never existed.
Fix: the DeleteFaces handler's face-id resolution — drop unknown face ids from both result lists and reserve UnsuccessfulFaceDeletions for faces present in the collection that could not be deleted. Open issue 20261029T191740Z-rekognition-reports-stale-facemodelversion-6-0-and-fabricates.md covers the same inversion from the wire corpus (its cause 4).
Traces: rekognition.delete-faces-partial-failure-and-state.

## Evidence
Replay of simparity-behavioral-aws-v1 against the booted emulator; 6 surviving failures (all other rekognition failures were ALREADY FILED scaffold stubs, and the dynamodb__lambda__rekognition__s3 e2e check is OWNED by an existing issue).

Cause 1 — [mismatch] rekognition.collection-lifecycle-create-describe-delete: 'AssertionError: describe after delete message: \'The collection id: sim2real-rek-coll-3ffb265b6acf does not exist\'' where the expected message is 'Collection:sim2real-rek-coll-3ffb265b6acf not found!'. Fixed text expected: 'Collection:' + ' not found!'; variable part: the CollectionId from the request (sim2real-rek-coll-<suffix> is one run's generated id, not part of the requirement). Error code matched (ResourceNotFoundException); only the message diverges.
Cause 1 — [mismatch] rekognition.create-collection-duplicate-id-rejected: 'AssertionError: after deleting \'sim2real-rek-dupe-d82cfd9524bd\', DescribeCollection returned message \'The collection id: sim2real-rek-dupe-d82cfd9524bd does not exist\''. Note the duplicate-create path itself matched AWS (ResourceAlreadyExistsException, 'The collection id: <id> already exists'), so only the not-found wording is wrong.
Cause 1 — [mismatch] rekognition.list-collections-reflects-created-and-deleted: 're-deleting sim2real-rek-list-a-6db3d0fd6036 returned message \'The collection id: sim2real-rek-list-a-6db3d0fd6036 does not exist\'' (DeleteCollection of an already-deleted collection); expected 'Collection:<id> not found!'.

Cause 2 — [mismatch] rekognition.index-faces-no-face-image-returns-empty: assertion `result["FaceRecords"] == []` failed after IndexFaces on a generated 240x240 solid-colour PNG; AWS returns FaceRecords == [] and UnindexedFaces == [] with FaceCount 0.

Cause 3 — [error] rekognition.index-faces-success-face-record: `KeyError: 'Confidence'` at `assert face["Confidence"] > 50` where face = result['FaceRecords'][0]['Face'] after IndexFaces of a real face photograph; the record exists but has no Confidence member.

Cause 4 — [mismatch] rekognition.delete-faces-partial-failure-and-state: "AssertionError: expected bogus FaceId '93b6be97-2589-48d0-a59b-e792905a19a5' absent from UnsuccessfulFaceDeletions, found in {'93b6be97-2589-48d0-a59b-e792905a19a5'}" (that uuid is a per-run generated value; the requirement is that any never-indexed FaceId appears in neither list).

## Steps to Reproduce
1. Cause 1: CreateCollection(CollectionId=X); DeleteCollection(CollectionId=X); then DescribeCollection(CollectionId=X) and DeleteCollection(CollectionId=X) again. Assert both are refused with HTTP 400, error code ResourceNotFoundException and Message exactly "Collection:X not found!" (fixed text 'Collection:' and ' not found!', X taken from the request). Replay checks rekognition.collection-lifecycle-create-describe-delete, rekognition.create-collection-duplicate-id-rejected and rekognition.list-collections-reflects-created-and-deleted.
2. Cause 2: CreateCollection(CollectionId=Y); IndexFaces(CollectionId=Y, Image={'Bytes': <solid-colour PNG with no face>}). Assert HTTP 200 with FaceRecords == [] and UnindexedFaces == [], then DescribeCollection(CollectionId=Y).FaceCount == 0. Replay check rekognition.index-faces-no-face-image-returns-empty.
3. Cause 3: CreateCollection(CollectionId=Z); IndexFaces(CollectionId=Z, Image={'Bytes': <photograph of one face>}). Assert FaceRecords has exactly one entry and FaceRecords[0].Face contains Confidence (a float in 0..100, >50 for an indexed face) and BoundingBox. Replay check rekognition.index-faces-success-face-record.
4. Cause 4: with that indexed face id F in collection Z, call DeleteFaces(CollectionId=Z, FaceIds=[F, <random uuid never indexed>]). Assert DeletedFaces == [F] and that the random uuid appears in NEITHER DeletedFaces nor UnsuccessfulFaceDeletions; then ListFaces/DescribeCollection show FaceCount 0. Replay check rekognition.delete-faces-partial-failure-and-state.
