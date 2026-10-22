// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workdocs-awsgorillaboyservice (WorkDocs) service.
//!
//! The document lifecycle inside a folder is served entirely from here: `CreateFolder`,
//! `InitiateDocumentVersionUpload`, `UpdateDocumentVersion`, `GetDocument`,
//! `DescribeFolderContents`, `DeleteDocument` and `DescribeRootFolders` all read and
//! write this one store, so a resource a create minted is the resource a later read
//! resolves.
//!
//! # The folder tree
//!
//! A [`Folder`] records its own `parent_folder_id`, and a [`Document`] records the
//! folder it is parented to, so a folder's contents are derived from those two maps
//! ([`ComAmazonawsWorkdocsAwsgorillaboyserviceState::child_index`], one pass over the
//! store) rather than from a second index that could fall out of sync with them.
//!
//! # The seeded user
//!
//! WorkDocs resources hang off a user's special folders, so [`Default`] seeds ONE
//! synthetic user ([`synthetic_user_id`]) with the two folders `DescribeRootFolders`
//! describes: a root folder and a recycle bin. Their ids are minted and stored here, so
//! a client discovers them with `DescribeRootFolders` instead of being told a literal.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{DocumentMetadata, DocumentVersionMetadata, FolderMetadata};

/// `ResourceStateType`'s `ACTIVE` — see `types::RESOURCE_STATE_TYPE_VALUES`, the
/// model's own value list for that enum.
pub const RESOURCE_STATE_ACTIVE: &str = "ACTIVE";

/// `DocumentStatusType`'s `INITIALIZED`: an upload has been initiated and the version's
/// content has not been committed yet. See `types::DOCUMENT_STATUS_TYPE_VALUES`.
pub const DOCUMENT_STATUS_INITIALIZED: &str = "INITIALIZED";

/// `DocumentStatusType`'s `ACTIVE`: the version is committed. `UpdateDocumentVersion` is
/// the only transition into it, and `DocumentVersionStatus` (its `VersionStatus` member)
/// models exactly this one value.
pub const DOCUMENT_STATUS_ACTIVE: &str = "ACTIVE";

/// `FolderContentType` — the `Type` filter `DescribeFolderContents` accepts.
/// See `types::FOLDER_CONTENT_TYPE_VALUES`.
pub const FOLDER_CONTENT_ALL: &str = "ALL";
/// See [`FOLDER_CONTENT_ALL`].
pub const FOLDER_CONTENT_DOCUMENT: &str = "DOCUMENT";
/// See [`FOLDER_CONTENT_ALL`].
pub const FOLDER_CONTENT_FOLDER: &str = "FOLDER";

/// `ResourceSortType` — the `Sort` criteria. See `types::RESOURCE_SORT_TYPE_VALUES`.
pub const SORT_DATE: &str = "DATE";
/// See [`SORT_DATE`].
pub const SORT_NAME: &str = "NAME";

/// `OrderType`. See `types::ORDER_TYPE_VALUES`.
pub const ORDER_ASCENDING: &str = "ASCENDING";
/// See [`ORDER_ASCENDING`].
pub const ORDER_DESCENDING: &str = "DESCENDING";

/// The documented ceiling `LimitExceededException` reports: "The maximum of 100,000
/// files and folders under the parent folder has been exceeded." (that error's own
/// documentation in this service's model).
pub const MAX_CHILDREN_PER_FOLDER: usize = 100_000;

/// What a create that would pass [`MAX_CHILDREN_PER_FOLDER`] reports: the sentence the
/// model documents for `LimitExceededException`.
pub const FOLDER_FULL_MESSAGE: &str =
    "The maximum of 100,000 files and folders under the parent folder has been exceeded.";

/// What a create that would pass [`MAX_STORED_RESOURCES`] reports. Worded distinctly
/// from [`FOLDER_FULL_MESSAGE`] because the ceiling it names is the emulator's own
/// store-wide one, not the per-parent rule the service documents.
pub const STORE_FULL_MESSAGE: &str =
    "The maximum number of files and folders this emulator stores has been exceeded.";

/// The default page size `DescribeFolderContents` documents: "By default, Amazon
/// WorkDocs returns the first 100 active document and folder metadata items."
pub const DEFAULT_PAGE_LIMIT: usize = 100;

/// `LimitType`'s modelled maximum (`range 1..999`). The boundary validator already
/// refuses a larger `Limit`, and every listing clamps to it again before doing any
/// per-item work, so the work one request can ask for is bounded by this even if a
/// future caller path reaches a handler without that validation.
pub const MAX_PAGE_LIMIT: usize = 999;

/// The total number of records (folders, documents and document versions) this
/// emulator holds, across every parent.
///
/// The service documents a ceiling of 100,000 files and folders under ONE parent
/// folder; because every folder created becomes a parent of its own, that per-parent
/// rule alone bounds nothing in aggregate. The emulator keeps the whole tree in memory
/// and reclaims it only on an explicit delete, so the same number caps the store as a
/// whole: a create past it answers the modelled `LimitExceededException` rather than
/// letting one caller grow the process without limit.
pub const MAX_STORED_RESOURCES: usize = 100_000;

/// The `Include` field name that `DescribeFolderContents` documents: 'Specify
/// "INITIALIZED" to include initialized documents.'
pub const INCLUDE_INITIALIZED: &str = "INITIALIZED";

/// The smallest step this service's clock advances between two calls, so a resource
/// modified after it was created reports a strictly later `ModifiedTimestamp` even when
/// the host clock has coarser resolution than the two calls are apart.
const CLOCK_TICK: Duration = Duration::from_millis(1);

/// The one synthetic WorkDocs user this emulator serves, as `IdType`
/// (`^[&\w+-.@]+$`, 1..256). Every resource is created by, and every root folder
/// belongs to, this user: the emulator models no directory, so there is exactly one
/// caller identity and `AuthenticationToken` selects no other.
pub fn synthetic_user_id() -> String {
    format!("user-{}", ::engine::emulator_account_id())
}

/// A lowercase-hex SHA-256 of `input`: 64 characters, which satisfies `ResourceIdType`
/// and `HashType` (`^[\w+-.@]+$` / `^[&\w+-.@]+$`, both at most 128 long).
pub fn digest(input: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// `SystemTime` as the `TimestampType` the wire carries.
pub fn timestamp(time: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(time)
}

/// A client-supplied `TimestampType` as a `SystemTime`, or `None` when it names an
/// instant outside what the host clock type can represent.
pub fn system_time(value: ::aws_smithy_types::DateTime) -> Option<SystemTime> {
    let nanos = Duration::from_nanos(u64::from(value.subsec_nanos()));
    let secs = value.secs();
    if secs >= 0 {
        UNIX_EPOCH
            .checked_add(Duration::from_secs(secs.unsigned_abs()))
            .and_then(|t| t.checked_add(nanos))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(secs.unsigned_abs()))
            .and_then(|t| t.checked_add(nanos))
    }
}

/// One version of a document.
#[derive(Debug, Clone)]
pub struct DocumentVersion {
    /// `DocumentVersionIdType`. Milliseconds since the epoch, the form WorkDocs version
    /// ids take, so the `BTreeMap` holding them orders versions by upload order.
    pub id: String,
    pub name: Option<String>,
    pub content_type: Option<String>,
    pub size_in_bytes: Option<i64>,
    /// `DocumentStatusType`: [`DOCUMENT_STATUS_INITIALIZED`] until
    /// `UpdateDocumentVersion` commits it to [`DOCUMENT_STATUS_ACTIVE`].
    pub status: String,
    pub created: SystemTime,
    pub modified: SystemTime,
    pub content_created: Option<SystemTime>,
    pub content_modified: Option<SystemTime>,
    pub creator_id: String,
    /// Thumbnail URLs by `DocumentThumbnailType`. Empty: the emulator renders none.
    pub thumbnail: HashMap<String, String>,
    /// Content URLs by `DocumentSourceType`. Empty: see [`DocumentVersion::metadata`].
    pub source: HashMap<String, String>,
}

impl DocumentVersion {
    /// This version as the modelled `DocumentVersionMetadata`.
    ///
    /// `Signature` is left absent on purpose: the model documents it as "The signature of
    /// the document", i.e. a hash of the version's CONTENT, and no content ever reaches
    /// this emulator (the upload itself goes to the URL
    /// `InitiateDocumentVersionUpload` hands back, which no operation in this slice
    /// serves). Reporting a hash of the request's metadata instead would be a value that
    /// looks computed from content that was never received. `Thumbnail`/`Source` are
    /// absent for the same reason.
    pub fn metadata(&self) -> DocumentVersionMetadata {
        DocumentVersionMetadata {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            content_type: self.content_type.clone(),
            size: self.size_in_bytes,
            signature: None,
            status: Some(self.status.clone()),
            created_timestamp: Some(timestamp(self.created)),
            modified_timestamp: Some(timestamp(self.modified)),
            content_created_timestamp: self.content_created.map(timestamp),
            content_modified_timestamp: self.content_modified.map(timestamp),
            creator_id: Some(self.creator_id.clone()),
            thumbnail: non_empty_map(&self.thumbnail),
            source: non_empty_map(&self.source),
        }
    }
}

/// A document and every version of it.
#[derive(Debug, Clone)]
pub struct Document {
    /// `ResourceIdType`.
    pub id: String,
    pub creator_id: String,
    pub parent_folder_id: String,
    pub created: SystemTime,
    pub modified: SystemTime,
    /// `ResourceStateType`.
    pub resource_state: String,
    /// The version `LatestVersionMetadata` reports: the most recently initiated one.
    pub latest_version_id: String,
    pub versions: BTreeMap<String, DocumentVersion>,
    pub labels: Vec<String>,
    pub custom_metadata: HashMap<String, String>,
}

impl Document {
    /// The version `LatestVersionMetadata` reports, or `None` when every version has
    /// been deleted.
    pub fn latest_version(&self) -> Option<&DocumentVersion> {
        self.versions.get(&self.latest_version_id)
    }

    /// The name of the latest version — a document's name IS its latest version's name
    /// in WorkDocs: `DocumentMetadata` carries no `Name` member of its own.
    pub fn name(&self) -> Option<&str> {
        self.latest_version().and_then(|v| v.name.as_deref())
    }

    /// This document as the modelled `DocumentMetadata`.
    pub fn metadata(&self) -> DocumentMetadata {
        DocumentMetadata {
            id: Some(self.id.clone()),
            creator_id: Some(self.creator_id.clone()),
            parent_folder_id: Some(self.parent_folder_id.clone()),
            created_timestamp: Some(timestamp(self.created)),
            modified_timestamp: Some(timestamp(self.modified)),
            latest_version_metadata: self.latest_version().map(DocumentVersion::metadata),
            resource_state: Some(self.resource_state.clone()),
            labels: non_empty_list(&self.labels),
        }
    }
}

/// A folder in the tree.
#[derive(Debug, Clone)]
pub struct Folder {
    /// `ResourceIdType`.
    pub id: String,
    /// Absent on a seeded special folder: WorkDocs names neither the root folder nor
    /// the recycle bin.
    pub name: Option<String>,
    pub creator_id: String,
    /// `None` only for a special folder, which is the top of the user's tree.
    pub parent_folder_id: Option<String>,
    pub created: SystemTime,
    pub modified: SystemTime,
    /// `ResourceStateType`.
    pub resource_state: String,
    pub labels: Vec<String>,
    pub custom_metadata: HashMap<String, String>,
}

/// One child of a folder, as [`ChildIndex::children`] reports it.
#[derive(Debug, Clone, Copy)]
pub enum ChildRef<'a> {
    Folder(&'a Folder),
    Document(&'a Document),
}

impl<'a> ChildRef<'a> {
    /// The child's id — the stable tie-break for both sort orders.
    pub fn id(self) -> &'a str {
        match self {
            Self::Folder(folder) => &folder.id,
            Self::Document(document) => &document.id,
        }
    }

    /// The child's name: a folder's own, a document's latest version's.
    pub fn name(self) -> Option<&'a str> {
        match self {
            Self::Folder(folder) => folder.name.as_deref(),
            Self::Document(document) => document.name(),
        }
    }

    /// The instant `Sort=DATE` orders by.
    pub fn modified(self) -> SystemTime {
        match self {
            Self::Folder(folder) => folder.modified,
            Self::Document(document) => document.modified,
        }
    }

    /// The `FolderContentType` this child is filtered by.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Folder(_) => FOLDER_CONTENT_FOLDER,
            Self::Document(_) => FOLDER_CONTENT_DOCUMENT,
        }
    }
}

/// Per-service state for com-amazonaws-workdocs-awsgorillaboyservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsWorkdocsAwsgorillaboyserviceState {
    /// Every folder, by `ResourceIdType`.
    pub folders: HashMap<String, Folder>,
    /// Every document, by `ResourceIdType`.
    pub documents: HashMap<String, Document>,
    /// A user's root folder id, by `IdType`. Seeded; see the module docs.
    pub root_folders: HashMap<String, String>,
    /// A user's recycle bin folder id, by `IdType`. Seeded beside the root folder,
    /// because `DescribeRootFolders` describes both special folders.
    pub recycle_bin_folders: HashMap<String, String>,
    /// Source of minted ids. Only ever consumed through [`Self::mint_id`], so an id is
    /// never handed out twice.
    next_id: u64,
    /// The last instant [`Self::now`] stamped, so the clock is strictly monotone.
    clock_floor: SystemTime,
}

impl Default for ComAmazonawsWorkdocsAwsgorillaboyserviceState {
    fn default() -> Self {
        let mut state = Self {
            folders: HashMap::new(),
            documents: HashMap::new(),
            root_folders: HashMap::new(),
            recycle_bin_folders: HashMap::new(),
            next_id: 0,
            clock_floor: SystemTime::now(),
        };
        state.seed_user(&synthetic_user_id());
        state
    }
}

impl ComAmazonawsWorkdocsAwsgorillaboyserviceState {
    /// Create `user`'s two special folders and record their minted ids.
    fn seed_user(&mut self, user: &str) {
        let root = self.create_special_folder(user);
        let recycle_bin = self.create_special_folder(user);
        self.root_folders.insert(user.to_string(), root);
        self.recycle_bin_folders.insert(user.to_string(), recycle_bin);
    }

    /// A parentless, unnamed folder owned by `user`, stored under a minted id.
    fn create_special_folder(&mut self, user: &str) -> String {
        let id = self.mint_id("folder");
        let created = self.now();
        self.folders.insert(
            id.clone(),
            Folder {
                id: id.clone(),
                name: None,
                creator_id: user.to_string(),
                parent_folder_id: None,
                created,
                modified: created,
                resource_state: RESOURCE_STATE_ACTIVE.to_string(),
                labels: Vec::new(),
                custom_metadata: HashMap::new(),
            },
        );
        id
    }

    /// The current instant, never earlier than [`CLOCK_TICK`] after the previous one.
    ///
    /// Mutating: a caller that only REPORTS state must not call it.
    pub fn now(&mut self) -> SystemTime {
        let wall = SystemTime::now();
        let floor = self.clock_floor.checked_add(CLOCK_TICK).unwrap_or(wall);
        let stamped = if wall > floor { wall } else { floor };
        self.clock_floor = stamped;
        stamped
    }

    /// A fresh `ResourceIdType` for a resource of kind `kind`.
    pub fn mint_id(&mut self, kind: &str) -> String {
        self.next_id = self.next_id.saturating_add(1);
        digest(&format!("workdocs:{kind}:{}", self.next_id))
    }

    /// A fresh `DocumentVersionIdType` for a version created at `created`: milliseconds
    /// since the epoch, stepped past any id `taken` already holds.
    pub fn mint_version_id(created: SystemTime, taken: &BTreeMap<String, DocumentVersion>) -> String {
        let mut millis = created
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        while taken.contains_key(&millis.to_string()) {
            millis = millis.saturating_add(1);
        }
        millis.to_string()
    }

    /// The number of children `folder_id` holds, counted in ONE pass over the two
    /// resource maps and without allocating: what a create checks against
    /// [`MAX_CHILDREN_PER_FOLDER`].
    pub fn count_children(&self, folder_id: &str) -> usize {
        let folders = self
            .folders
            .values()
            .filter(|folder| folder.parent_folder_id.as_deref() == Some(folder_id))
            .count();
        let documents = self
            .documents
            .values()
            .filter(|document| document.parent_folder_id == folder_id)
            .count();
        folders.saturating_add(documents)
    }

    /// Every record the store holds: folders, documents and document versions. What a
    /// create checks against [`MAX_STORED_RESOURCES`].
    pub fn total_resources(&self) -> usize {
        let versions: usize = self
            .documents
            .values()
            .map(|document| document.versions.len())
            .sum();
        self.folders
            .len()
            .saturating_add(self.documents.len())
            .saturating_add(versions)
    }

    /// The whole parent -> children relation, built in ONE pass over the two resource
    /// maps.
    ///
    /// Every listing goes through this rather than re-scanning the store per folder it
    /// reports: a page of F folders would otherwise cost F full scans (and F hashes
    /// over them) for one request, so the work a caller can ask for would be the
    /// PRODUCT of its page size and the whole store. Built once, a request costs one
    /// pass plus the children it actually reports, which is bounded by the store size
    /// however large the page is.
    pub fn child_index(&self) -> ChildIndex<'_> {
        let mut children: HashMap<&str, Vec<ChildRef<'_>>> = HashMap::new();
        for folder in self.folders.values() {
            if let Some(parent) = folder.parent_folder_id.as_deref() {
                children.entry(parent).or_default().push(ChildRef::Folder(folder));
            }
        }
        for document in self.documents.values() {
            children
                .entry(document.parent_folder_id.as_str())
                .or_default()
                .push(ChildRef::Document(document));
        }
        ChildIndex { children }
    }
}

/// The parent -> children relation of one store snapshot, as
/// [`ComAmazonawsWorkdocsAwsgorillaboyserviceState::child_index`] built it.
#[derive(Debug)]
pub struct ChildIndex<'a> {
    children: HashMap<&'a str, Vec<ChildRef<'a>>>,
}

impl<'a> ChildIndex<'a> {
    /// Every child of `folder_id`, in no particular order (the caller orders them).
    pub fn children(&self, folder_id: &str) -> &[ChildRef<'a>] {
        self.children.get(folder_id).map_or(&[], Vec::as_slice)
    }

    /// The ids of `folder_id`'s children, sorted: the input
    /// [`folder_signature`] hashes.
    pub fn child_ids(&self, folder_id: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .children(folder_id)
            .iter()
            .map(|child| child.id().to_string())
            .collect();
        ids.sort_unstable();
        ids
    }

    /// `folder` as the modelled `FolderMetadata`, with the signature its current
    /// contents give it.
    pub fn folder_metadata(&self, folder: &Folder) -> FolderMetadata {
        folder_metadata(folder, Some(folder_signature(&self.child_ids(&folder.id))))
    }
}

/// The `Signature` of a folder whose sorted child ids are `child_ids`: the model
/// documents it as "The unique identifier created from the subfolders and documents of
/// the folder", so it is a hash over exactly that child set and changes when the set
/// does.
pub fn folder_signature(child_ids: &[String]) -> String {
    digest(&child_ids.join(","))
}

/// `folder` as the modelled `FolderMetadata`, carrying `signature`.
///
/// Split from the hashing so a listing can copy the metadata out while the store is
/// locked and hash afterwards, with the guard already released.
pub fn folder_metadata(folder: &Folder, signature: Option<String>) -> FolderMetadata {
    FolderMetadata {
        id: Some(folder.id.clone()),
        name: folder.name.clone(),
        creator_id: Some(folder.creator_id.clone()),
        parent_folder_id: folder.parent_folder_id.clone(),
        created_timestamp: Some(timestamp(folder.created)),
        modified_timestamp: Some(timestamp(folder.modified)),
        resource_state: Some(folder.resource_state.clone()),
        signature,
        labels: non_empty_list(&folder.labels),
        // `Size`/`LatestVersionSize` are the stored size of the folder's own
        // metadata, which this emulator does not account for.
        size: None,
        latest_version_size: None,
    }
}

/// `CustomMetadataMap` and the URL maps are modelled with a minimum length, and an
/// absent map is how WorkDocs reports "none", so an empty one is omitted rather than
/// serialized as `{}`.
pub fn non_empty_map(map: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    (!map.is_empty()).then(|| map.clone())
}

/// `Labels` likewise: omitted when the resource carries none.
pub fn non_empty_list(list: &[String]) -> Option<Vec<String>> {
    (!list.is_empty()).then(|| list.to_vec())
}

/// An opaque `PageMarkerType` naming the next item offset within one query.
///
/// The token is bound to `scope` — the query parameters whose page it is — so a marker
/// replayed against a different listing is detected instead of silently resolving to an
/// unrelated offset. Hex keeps it within `PageMarkerType`'s 1..2048 length.
pub fn encode_marker(offset: usize, scope: &str) -> String {
    hex::encode(format!("{offset}:{}", scope_tag(scope)))
}

/// The offset `marker` names within `scope`, or `None` when it is not a marker this
/// service issued for this query.
pub fn decode_marker(marker: &str, scope: &str) -> Option<usize> {
    let decoded = hex::decode(marker).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (offset, tag) = text.split_once(':')?;
    (tag == scope_tag(scope)).then_some(())?;
    offset.parse().ok()
}

/// A short, fixed-width fingerprint of a query, so the marker cannot be reused across
/// queries and stays well inside `PageMarkerType`'s length.
fn scope_tag(scope: &str) -> String {
    digest(scope)[..16].to_string()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWorkdocsAwsgorillaboyserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWorkdocsAwsgorillaboyserviceState>)
}
