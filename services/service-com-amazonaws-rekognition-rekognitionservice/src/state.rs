// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rekognition-rekognitionservice service.
//!
//! Besides the state struct this module holds everything the face-collection
//! handlers share — ARN construction, the request's region, the record-to-modelled
//! shape conversions, the list cursor codec, the store's capacity bounds and the
//! image-identity digest — so the nine operation files agree on one implementation of
//! each instead of carrying copies that can drift apart.
//!
//! ## What this emulator does and does not compute
//!
//! Rekognition's face operations are backed by a face detector and a face
//! recogniser. The RECOGNISER does not exist in this process: nothing here maps a face
//! to an identity, so no image is matched against another image's face. What IS real is
//! the collection store — a collection, the faces indexed into it, their minted ids, and
//! the counts, filters, pages and lookups computed from them — and, for the operations
//! that analyse one submitted image, the geometric face DETECTOR in
//! [`face_detection`]: it decodes the image and measures the faces it finds in the
//! pixels, reporting a box, a confidence, landmarks, a roll and a quality it measured
//! and leaving every classifier's judgement (an age, a gender, an emotion) absent.
//!
//! Nothing here reports a value nothing measured. A STORED face therefore carries its
//! identity and nothing else: no bounding box, no confidence, no landmarks, pose or
//! quality, no demographic attribute and no orientation correction — `IndexFaces` records
//! what the collection holds, and the recogniser that would tie a stored face to an image
//! is what is missing, so the recogniser-backed operations (`CompareFaces`,
//! `SearchFacesByImage`, `RecognizeCelebrities`, …) stay unimplemented 501s.
//!
//! The one comparison this emulator can make about stored faces is IDENTITY:
//! [`image_signature`] digests what an image is, so two indexed faces can be told to have
//! come from the same image content or not. `SearchFaces` reports exactly that and no
//! graded score — see its own module docs.

pub mod face_detection;
pub mod image_source;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{Face, Image};

/// The region an emulated request is treated as having been made in when its
/// SigV4 credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` this service's ARNs carry, from the
/// model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: rekognition`).
pub const ARN_NAMESPACE: &str = "rekognition";

/// The face model version a collection created by this emulator is associated
/// with. `CreateCollection` documents that "when you create a collection, it is
/// associated with the latest version of the face model version"; the version is
/// an opaque string on the wire, so one value is pinned here and every operation
/// echoes the collection's own stored copy rather than this constant.
pub const FACE_MODEL_VERSION: &str = "6.0";

/// The page size `ListFaces` / `ListCollections` use when the caller sends no
/// `MaxResults`: the largest page the model allows (`PageSize` range 0..4096).
pub const DEFAULT_PAGE_SIZE: usize = 4096;

/// The `SearchFaces` cap used when the caller sends no `MaxFaces`: the largest the
/// model allows (`MaxFaces` range 1..4096). The model documents no default for it.
pub const DEFAULT_MAX_FACES: usize = 4096;

/// The `SearchFaces` match floor used when the caller sends no
/// `FaceMatchThreshold`. Documented on the member itself
/// (`smithy_query(query="doc",shape="SearchFacesRequest",member="FaceMatchThreshold")`:
/// "The default value is 80%").
pub const DEFAULT_FACE_MATCH_THRESHOLD: f32 = 80.0;

/// The `Similarity` `SearchFaces` reports for the only match this emulator can
/// establish: a face indexed from the same image content as the searched one. It is
/// the top of the modelled `Percent` range, and the ONLY value reported, because this
/// process computes no graded similarity.
pub const IDENTICAL_CONTENT_SIMILARITY: f32 = 100.0;

/// How many collections this store will hold, and how many faces one collection will
/// hold, before a create or an index is refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// This store lives in the emulator's memory, so it is bounded far below Rekognition's
/// own published quotas: these are the EMULATOR's limits, not AWS's, and they exist so
/// that a caller repeating `CreateCollection` or `IndexFaces` is refused rather than
/// allowed to grow the process without limit. The modelled error is the same one AWS
/// answers past its quota ("The size of the collection exceeds the allowed limit"), so
/// a client sees a quota refusal either way.
pub const MAX_COLLECTIONS: usize = 100;
pub const MAX_FACES_PER_COLLECTION: usize = 1_000;

/// One face stored in a collection by `IndexFaces`.
///
/// Its members are the face's IDENTITY — what the service minted and what the caller
/// supplied. Nothing a detector would have had to measure is stored, because nothing
/// measured it.
#[derive(Debug, Clone)]
pub struct StoredFace {
    /// The id this service minted for the face; the collection's key for it.
    pub face_id: String,
    /// The id this service minted for the image the face was indexed from.
    pub image_id: String,
    /// `IndexFaces.ExternalImageId`, carried through unchanged.
    pub external_image_id: Option<String>,
    /// The face model version of the collection at the time of indexing.
    pub index_faces_model_version: String,
    /// The user this face is associated with, set by `AssociateFaces` (not yet
    /// implemented), so it is `None` for every face this slice stores. It is carried
    /// here, and honoured by `ListFaces`' `UserId` filter and `DeleteFaces`'
    /// `ASSOCIATED_TO_AN_EXISTING_USER` refusal, as pre-wiring for the user slice:
    /// those two paths cannot be reached until `CreateUser`/`AssociateFaces` exist.
    pub user_id: Option<String>,
    /// The digest of the image content this face was indexed from: what lets
    /// `SearchFaces` tell whether two stored faces came from the same image.
    pub signature: [u8; 32],
    /// This face's position in the collection's insertion order. Monotonic per
    /// collection and never reused, so a `ListFaces` cursor built from it neither
    /// repeats nor skips a face when other faces are added or deleted between
    /// pages.
    pub sequence: u64,
}

impl StoredFace {
    /// This face as the modelled `Face`.
    ///
    /// `BoundingBox` and `Confidence` are absent for the same reason the rest of the
    /// detector's output is: no detector ran, so there is no box and no confidence to
    /// report.
    pub fn modeled(&self) -> Face {
        Face {
            face_id: Some(self.face_id.clone()),
            bounding_box: None,
            image_id: Some(self.image_id.clone()),
            external_image_id: self.external_image_id.clone(),
            confidence: None,
            index_faces_model_version: Some(self.index_faces_model_version.clone()),
            user_id: self.user_id.clone(),
        }
    }
}

/// One user of a collection, as created by `CreateUser` and populated by
/// `AssociateFaces` — neither implemented yet, so `DescribeCollection` computes a
/// `UserCount` of zero from this map rather than reporting a constant.
#[derive(Debug, Clone)]
pub struct StoredUser {
    pub user_id: String,
    pub user_status: String,
    pub face_ids: Vec<String>,
}

/// One stored face collection: what `CreateCollection` was given, the ARN and
/// creation time the service recorded for it, and the faces indexed into it.
#[derive(Debug)]
pub struct Collection {
    pub collection_id: String,
    pub arn: String,
    pub face_model_version: String,
    pub creation_timestamp: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// Insertion-ordered: `FaceCount` is its length and `ListFaces` reports it in
    /// this order.
    pub faces: Vec<StoredFace>,
    pub users: BTreeMap<String, StoredUser>,
    /// The sequence number the next indexed face takes.
    pub next_sequence: u64,
}

impl Collection {
    /// A collection with no faces and no users, as `CreateCollection` records it.
    pub fn new(
        collection_id: String,
        arn: String,
        tags: BTreeMap<String, String>,
        creation_timestamp: SystemTime,
    ) -> Self {
        Self {
            collection_id,
            arn,
            face_model_version: FACE_MODEL_VERSION.to_string(),
            creation_timestamp,
            tags,
            faces: Vec::new(),
            users: BTreeMap::new(),
            next_sequence: 0,
        }
    }

    /// The stored face with `face_id`, if this collection holds it.
    pub fn face(&self, face_id: &str) -> Option<&StoredFace> {
        self.faces.iter().find(|face| face.face_id == face_id)
    }
}

/// Per-service state for com-amazonaws-rekognition-rekognitionservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRekognitionRekognitionserviceState {
    /// Every live collection, keyed by its `CollectionId` — the resource's identity
    /// in this API, and case-sensitive per `CreateCollection`'s documentation. A
    /// `BTreeMap` so `ListCollections` order, and therefore its pagination, is
    /// deterministic. Bounded by [`MAX_COLLECTIONS`].
    pub collections: BTreeMap<String, Collection>,
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is
/// not signed.
///
/// The scope is caller-supplied and this value reaches both the ARNs this service reports
/// and the address an in-process read of Amazon S3 is sent to, so a value that is not
/// shaped like a Region identifier is not reflected: the real service would not have
/// served the request under a Region it does not have.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_like(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is shaped like an AWS Region identifier: two or more
/// hyphen-separated lowercase-letter parts followed by a digit group (`us-east-1`,
/// `ap-southeast-3`, `us-gov-west-1`, `cn-north-1`).
fn is_region_like(region: &str) -> bool {
    let Some((parts, index)) = region.rsplit_once('-') else {
        return false;
    };
    region.len() <= 32
        && !index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && parts.split('-').count() >= 2
        && parts
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase()))
}

/// The ARN of a collection in `region`:
/// `arn:aws:rekognition:<region>:<account>:collection/<CollectionId>`.
pub fn collection_arn(region: &str, collection_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("collection/{collection_id}"),
    )
}

/// The cursor scope of each paginated list, so one list's token cannot be replayed
/// against another and a hex string the service never minted names no position.
pub const FACES_CURSOR_SCOPE: &str = "faces";
pub const COLLECTIONS_CURSOR_SCOPE: &str = "collections";

/// The pagination cursor of the `scope` list for a page that starts at `key`.
///
/// Hex of a scope-tagged key rather than the key itself, so a caller cannot read
/// structure into the token and has nothing to build one from: a `NextToken` is opaque
/// and only ever travels back unchanged, to the list that minted it.
pub fn encode_cursor(scope: &str, key: &str) -> String {
    ::hex::encode(format!("{scope}:{key}").as_bytes())
}

/// The start key a cursor minted by [`encode_cursor`] for `scope` carries, or `None`
/// when the token was not minted by that list of this service.
pub fn decode_cursor(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("{scope}:"))
        .map(ToString::to_string)
}

/// The `ListFaces` cursor for a page that starts at the face with `sequence`.
pub fn encode_sequence_cursor(sequence: u64) -> String {
    encode_cursor(FACES_CURSOR_SCOPE, &sequence.to_string())
}

/// The sequence number a cursor minted by [`encode_sequence_cursor`] carries, or
/// `None` when the token was not minted by this service's `ListFaces`.
pub fn decode_sequence_cursor(token: &str) -> Option<u64> {
    decode_cursor(FACES_CURSOR_SCOPE, token)?.parse().ok()
}

/// The page size a `MaxResults` of `requested` asks for, capped to what the model
/// allows and defaulting to [`DEFAULT_PAGE_SIZE`] when the caller sent none.
///
/// `PageSize`'s range starts at 0, so a caller may ask for an empty page; see
/// [`page_ends_here`] for what a list then answers. A value that is not a page size at
/// all (a negative one, which the modelled range already refuses at the boundary) asks
/// for no entries rather than for the largest page.
pub fn page_size(requested: Option<i32>) -> usize {
    requested.map_or(DEFAULT_PAGE_SIZE, |max| {
        usize::try_from(max).unwrap_or(0).min(DEFAULT_PAGE_SIZE)
    })
}

/// Whether a list must end the iteration rather than mint a `NextToken`, given the
/// page size it served.
///
/// A `MaxResults` of 0 is inside the modelled range and asks for no entries, so the
/// answer carries none. No cursor is minted for it either: the cursor would name the
/// position the caller is already at, and a client that followed it would ask for
/// nothing again forever. Asking for zero entries is answered with zero entries and a
/// finished iteration.
pub fn page_ends_here(page_size: usize) -> bool {
    page_size == 0
}

/// What an `Image` identifies, as bytes to digest: the image bytes themselves, or
/// the S3 object's identity when the image is stored in S3 (its content is not
/// reachable from this process).
///
/// `None` when the member carries neither form, which is what makes a request
/// refusable: the model marks both members optional but the service needs one.
fn image_content_key(image: &Image) -> Option<Vec<u8>> {
    if let Some(bytes) = image.bytes.as_ref().filter(|bytes| !bytes.is_empty()) {
        let mut key = b"bytes:".to_vec();
        key.extend_from_slice(bytes);
        return Some(key);
    }
    let object = image.s3_object.as_ref()?;
    let bucket = object.bucket.as_deref().filter(|b| !b.is_empty())?;
    let name = object.name.as_deref().filter(|n| !n.is_empty())?;
    let version = object.version.as_deref().unwrap_or_default();
    Some(format!("s3:{bucket}/{name}#{version}").into_bytes())
}

/// The digest of an image's content: the identity `IndexFaces` records for the image
/// a face came from, so that two stored faces can be told apart from two stored faces
/// that came from the same image.
///
/// Domain-separated so an image passed as bytes and an S3 object whose key
/// happens to spell the same text never collide. It identifies an image; it says
/// nothing about what is in it.
pub fn image_signature(image: &Image) -> Option<[u8; 32]> {
    let key = image_content_key(image)?;
    let digest = Sha256::digest(&key);
    Some(digest.into())
}

/// How many ids this process has minted. Part of every minted id, so two ids are
/// distinct even when the clock below reports the same instant twice.
static MINTED_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh identifier in the canonical uuid form the model requires of a `FaceId`
/// (`pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), minted
/// from `domain`, this process's mint counter and the clock, and therefore unique:
/// the counter alone makes every input distinct, and the digest of distinct inputs
/// does not repeat. It names a resource for correlation and is not a secret.
fn mint_id(domain: &str) -> String {
    let serial = MINTED_IDS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let digest = Sha256::digest(format!("{domain}:{serial}:{nanos}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // The version and variant nibbles of a random-form uuid, so the id is a canonical
    // v4 uuid string and not merely 32 hex characters with hyphens.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A fresh `FaceId` for a face being indexed.
pub fn new_face_id() -> String {
    mint_id("face")
}

/// A fresh `ImageId` for the image an `IndexFaces` call submitted.
pub fn new_image_id() -> String {
    mint_id("image")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRekognitionRekognitionserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRekognitionRekognitionserviceState>)
}
