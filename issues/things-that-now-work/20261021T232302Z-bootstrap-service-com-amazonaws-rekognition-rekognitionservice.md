Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 96234e472a4dbed875261b6f3ae0a9ed15fa6e7c

# Bootstrap the rekognition emulator

_Filed: 2026-10-21T23:23:02Z_
_Operation: service-com-amazonaws-rekognition-rekognitionservice | Class: missing_

> _Note: this issue was identified against the code base at commit `96234e472a4dbed875261b6f3ae0a9ed15fa6e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 75 operations in `services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. The service holds no state, so no client workflow works.

Implement a FIRST, SMALL, stateful slice: the face-collection lifecycle, which is Rekognition's primary stored resource (projects/datasets/stream-processors/video jobs stay stubbed and will be filed as individual gap issues later).

Happy path to implement, in order (wire protocol `awsJson1_1`, `POST /` with `X-Amz-Target: RekognitionService.<Op>`):

1. **CreateCollection** — in: `CollectionId` (required, 1..255, `^[a-zA-Z0-9_.\-]+$`), `Tags`. out: `StatusCode` (200), `CollectionArn` (`arn:aws:rekognition:<region>:<account>:collection/<CollectionId>`), `FaceModelVersion` (e.g. `"6.0"`). Must persist the collection and record `CreationTimestamp`. Re-creating the same `CollectionId` => `ResourceAlreadyExistsException` (HTTP 400).
2. **IndexFaces** — in: `CollectionId` (required), `Image` (required; `Bytes` blob or `S3Object`), `ExternalImageId`, `DetectionAttributes`, `MaxFaces`, `QualityFilter`. out: `FaceRecords` (list of `FaceRecord{Face{FaceId, BoundingBox, ImageId, ExternalImageId, Confidence, IndexFacesModelVersion}, FaceDetail{BoundingBox, Confidence, Landmarks, Pose, Quality}}`), `FaceModelVersion`, `UnindexedFaces`. Must MINT a fresh uuid `FaceId` and `ImageId` per indexed face, STORE the face in the named collection, honour `MaxFaces` (at most N faces indexed), and carry `ExternalImageId` through onto the stored face. Unknown `CollectionId` => `ResourceNotFoundException`.
3. **ListFaces** — in: `CollectionId` (required), `MaxResults` (0..4096), `NextToken`, `UserId`, `FaceIds`. out: `Faces` (the faces actually stored by step 2, with the same `FaceId`/`ExternalImageId`), `NextToken` when truncated, `FaceModelVersion`. Pagination must be real: `MaxResults` caps the page and a returned `NextToken` resumes where the previous page ended; `FaceIds` filters.
4. **DescribeCollection** — in: `CollectionId` (required). out: `FaceCount` (COUNTED from stored faces, not hardcoded), `FaceModelVersion`, `CollectionARN` (identical to the ARN returned by CreateCollection), `CreationTimestamp` (the stored creation time), `UserCount`.
5. **SearchFaces** — in: `CollectionId` (required), `FaceId` (required, uuid pattern), `MaxFaces` (1..4096), `FaceMatchThreshold`. out: `SearchedFaceId` (echo of the looked-up face, which must EXIST in the collection — otherwise `ResourceNotFoundException`), `FaceMatches` (list of `FaceMatch{Similarity, Face}` drawn from the OTHER faces stored in that collection, excluding the searched face, capped by `MaxFaces` and filtered by `FaceMatchThreshold`), `FaceModelVersion`.
6. **DeleteFaces** — in: `CollectionId` (required), `FaceIds` (required, 1..4096). out: `DeletedFaces` (only ids that were actually present and are now removed), `UnsuccessfulFaceDeletions` for the rest. A subsequent `ListFaces`/`DescribeCollection` must reflect the removal.
7. **DeleteCollection** — in: `CollectionId` (required). out: `StatusCode` (200). Removes the collection and its faces; a following `DescribeCollection`/`ListFaces` on it => `ResourceNotFoundException`. `ListCollections` (`CollectionIds`, `FaceModelVersions`, `NextToken`) should list exactly the live collections so the lifecycle is observable.

Every one of these MUST compute its response from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`), reached through the guard. No canned responses, no echoing the input back, no minting a `FaceId` that is never stored. A create followed by a describe must return what was created.

## State Model

```
RekognitionState {
    // CollectionId -> Collection
    collections: BTreeMap<String, Collection>,
}

Collection {
    collection_id:   String,          // key, case-sensitive
    arn:             String,          // arn:aws:rekognition:<region>:<account>:collection/<id>
    face_model_version: String,       // e.g. "6.0"; echoed by Create/Index/List/Describe
    creation_timestamp: SystemTime,   // returned by DescribeCollection
    tags:            BTreeMap<String, String>,   // from CreateCollection.Tags; served by ListTagsForResource
    faces:           Vec<StoredFace>, // insertion-ordered; FaceCount = faces.len()
    users:           BTreeMap<String, StoredUser>, // UserId -> user; UserCount = users.len()
}

StoredFace {
    face_id:            String,   // uuid v4, minted by IndexFaces
    image_id:           String,   // uuid v4, per indexed image
    external_image_id:  Option<String>,  // from IndexFaces.ExternalImageId
    bounding_box:       BoundingBox { Width, Height, Left, Top },
    confidence:         f32,
    index_faces_model_version: String,
    user_id:            Option<String>,  // set by AssociateFaces (out of this slice)
}

StoredUser { user_id: String, user_status: UserStatus, face_ids: Vec<String> }
```

Pagination tokens for `ListFaces`/`ListCollections` should encode an offset (or last-seen key) into the ordered collection so a resumed page neither repeats nor skips entries; a malformed token => `InvalidPaginationTokenException`.

Done = an integration test that runs the workflow in "Steps to reproduce" end-to-end with correct, stateful bodies (not merely 2xx). Operations outside this slice keep returning 501 and are expected to be filed separately.

## Evidence

Scaffold census: `services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/create_collection.rs` lines 4 and 184 show the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateCollection"))`; all 75 op files in that directory are generated from the same template, and README.md documents the 501 behaviour of unwritten stubs.

Contract grounding — `OPERATIONS.md` (generated from `smithy-model/com-amazonaws-rekognition-rekognitionservice.json`): CreateCollection (line 45-49: `CollectionId` required, out `StatusCode`/`CollectionArn`/`FaceModelVersion`, errors incl. `ResourceAlreadyExistsException`), IndexFaces (line 290-294: `CollectionId`+`Image` required, out `FaceRecords`/`UnindexedFaces`/`FaceModelVersion`, `ResourceNotFoundException`), ListFaces (line 317-322: @paginated inputToken=NextToken outputToken=NextToken items=Faces, `MaxResults` 0..4096), DescribeCollection (line 136-140: out `FaceCount`/`CollectionARN`/`CreationTimestamp`/`UserCount`), SearchFaces (line 370-374: `FaceId` required, out `SearchedFaceId`/`FaceMatches`), DeleteFaces (line 100-104: out `DeletedFaces`/`UnsuccessfulFaceDeletions`), DeleteCollection (line 88-92: out `StatusCode`), ListCollections (line 296-301). Shape members for `Face`, `FaceRecord`, `FaceDetail`, `Image` confirmed via smithy_query. Wire protocol `awsJson1_1` with `X-Amz-Target: RekognitionService.<Op>` per OPERATIONS.md line 8-15. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI rekognition client at it.
2. CreateCollection CollectionId=my-faces -> expect 200 with StatusCode=200, CollectionArn ending in `collection/my-faces`, a FaceModelVersion. Currently: HTTP 501 NotImplemented.
3. CreateCollection CollectionId=my-faces again -> expect ResourceAlreadyExistsException (HTTP 400).
4. ListCollections -> expect CollectionIds to contain `my-faces`.
5. IndexFaces CollectionId=my-faces, Image.Bytes=<jpeg>, ExternalImageId=alice -> expect 200 with one FaceRecords entry whose Face.FaceId is a fresh uuid and Face.ExternalImageId=alice; capture the FaceId.
6. IndexFaces again with ExternalImageId=bob -> capture the second FaceId.
7. ListFaces CollectionId=my-faces -> expect both stored faces with the exact FaceIds and ExternalImageIds from the two IndexFaces calls.
8. ListFaces CollectionId=my-faces MaxResults=1 -> expect exactly 1 face plus a NextToken; pass that NextToken back and expect the OTHER face and no repeats.
9. DescribeCollection CollectionId=my-faces -> expect FaceCount=2, CollectionARN identical to the CreateCollection ARN, and a CreationTimestamp.
10. SearchFaces CollectionId=my-faces, FaceId=<alice FaceId>, MaxFaces=5 -> expect SearchedFaceId=<alice FaceId> and FaceMatches containing bob's face, never alice herself.
11. SearchFaces with a uuid that was never indexed -> expect ResourceNotFoundException (HTTP 400).
12. DeleteFaces CollectionId=my-faces FaceIds=[<alice FaceId>] -> expect DeletedFaces=[alice FaceId]; then DescribeCollection -> FaceCount=1 and ListFaces returns only bob.
13. DeleteCollection CollectionId=my-faces -> expect StatusCode=200; then DescribeCollection my-faces -> ResourceNotFoundException, and ListCollections no longer lists it.
