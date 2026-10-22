// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3files-s3files service, plus the rules more
//! than one operation in the file-system lifecycle applies: id and ARN minting,
//! idempotency-token replay, the `creating` -> `available` transition the clock owes a
//! freshly created resource, and the opaque `nextToken` the listings issue.
//!
//! Every implemented operation computes its answer from the records below — nothing is
//! canned, and nothing is echoed straight back out of a request without being stored
//! first.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ConflictException, InternalServerException, PosixUser, ResourceNotFoundException,
    RootDirectory, ServiceQuotaExceededException, Tag, ValidationException,
};

/// The `LifeCycleState` value a resource carries while the service is still creating
/// it. The model documents it as "CREATING (the file system is being created)".
pub const STATUS_CREATING: &str = "creating";

/// The `LifeCycleState` value a created resource carries once it is usable. The model
/// documents it as "AVAILABLE (the file system is available for use)".
pub const STATUS_AVAILABLE: &str = "available";

/// How long a file system or access point stays [`STATUS_CREATING`] before the clock
/// owes it [`STATUS_AVAILABLE`].
///
/// A resource the service reports in a non-terminal lifecycle state has a transition
/// the clock owes. The span is deliberately non-zero, so a caller that reads straight
/// after a create still observes `creating` the way it does against the real service.
/// The instant it comes due is stored on the record and materialised by
/// [`ComAmazonawsS3filesS3filesState::settle`]; nothing ever sleeps for it. The length
/// is the emulator's own choice — AWS publishes no creation time for this service.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// This service's ARN namespace and sigv4 signing name, from the model's
/// `aws.api#service` / `aws.auth#sigv4` traits (`arnNamespace`/`name` = `s3files`).
pub const ARN_SERVICE: &str = "s3files";

/// The region an ARN names when the request carries no usable SigV4 credential scope
/// to read one from (an unsigned call still routes by endpoint host).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `maxResults` value every listing in this model documents as its default, from
/// the `@default(100)` on each `maxResults` member.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// `maxResults` ceiling for the listings whose member is `@range(1, 100)`:
/// `ListFileSystemsRequest$maxResults` and `ListMountTargetsRequest$maxResults`.
pub const MAX_RESULTS_LIMIT: i32 = 100;

/// `maxResults` ceiling for `ListAccessPoints`, whose
/// `ListAccessPointsRequest$maxResults` is `@range(1, 1000)` — a different bound from
/// [`MAX_RESULTS_LIMIT`], so each listing passes its own to [`page_of`].
pub const MAX_RESULTS_LIMIT_ACCESS_POINTS: i32 = 1_000;

/// How many file systems this emulator will hold.
///
/// `CreateFileSystem` models `ServiceQuotaExceededException` (HTTP 402) for exceeding a
/// quota, and a generated emulator keeps every resource in memory for the life of the
/// process, so a create loop needs a ceiling. The number is NOT a fidelity claim about
/// any published AWS quota (those are adjustable): it is the emulator's own bound, set
/// high enough that no ordinary workflow meets it.
pub const MAX_FILE_SYSTEMS: usize = 1_000;

/// How many access points this emulator will hold under one file system. Same
/// reasoning as [`MAX_FILE_SYSTEMS`]; `CreateAccessPoint` models the same error.
pub const MAX_ACCESS_POINTS_PER_FILE_SYSTEM: usize = 1_000;

/// How many `clientToken` -> resource-id records are kept for idempotent replay.
///
/// Tokens are caller-supplied, so the map is caller-driven growth holding no resource
/// of its own. A record naming a deleted resource is dropped with it, and beyond this
/// many live records the oldest is evicted. An evicted token is indistinguishable from
/// one whose idempotency window has passed: the next create carrying it mints a new
/// resource.
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 10_000;

/// The number of hex characters in a minted `fs-`/`fsap-` suffix.
///
/// `FileSystemId` is `^(…|fs-[0-9a-f]{17,40})$` and `AccessPointId` is
/// `^(…|fsap-[0-9a-f]{17,40})$`, so 17 lowercase hex characters is the shortest
/// suffix both patterns accept.
const MINTED_ID_HEX: usize = 17;

/// The number of hex characters in a client token this service mints for a caller that
/// sent none. `ClientToken` is `@length(1, 64)` with pattern `^(.+)$`.
const MINTED_TOKEN_HEX: usize = 32;

/// A stored file system: everything `CreateFileSystem` was given, plus what the
/// service itself minted.
#[derive(Debug, Clone)]
pub struct FileSystem {
    pub file_system_id: String,
    pub file_system_arn: String,
    pub bucket: String,
    /// The prefix the file system is scoped to. `prefix` carries `@default("")`, so a
    /// caller that named none has an empty prefix rather than an absent one.
    pub prefix: String,
    pub kms_key_id: Option<String>,
    pub role_arn: String,
    pub owner_id: String,
    /// A `LifeCycleState` value: [`STATUS_CREATING`] until the clock owes
    /// [`STATUS_AVAILABLE`].
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time: SystemTime,
    /// When `creating` becomes `available`. See [`PROVISIONING_SPAN`].
    pub available_due_at: SystemTime,
    pub client_token: Option<String>,
    /// The file system's name. The model documents it as "derived from the Name tag if
    /// present", so it is never stored independently of [`Self::tags`].
    pub name: Option<String>,
    pub tags: Vec<Tag>,
    /// The IAM resource policy attached to this file system, if one is attached.
    ///
    /// The policy is a sub-resource of the file system (`PutFileSystemPolicy` is bound
    /// to `/file-systems/{fileSystemId}/policy`), so it lives on the record and is
    /// released with it rather than in a map of its own. The document is stored exactly
    /// as it was put — the service is given an opaque JSON document, so re-rendering it
    /// would change what the caller gets back.
    pub policy: Option<String>,
    /// Creation order, so a listing paginates deterministically.
    pub sequence: u64,
}

/// A stored access point, a child of exactly one file system.
#[derive(Debug, Clone)]
pub struct AccessPoint {
    pub access_point_id: String,
    pub access_point_arn: String,
    pub file_system_id: String,
    pub client_token: String,
    pub owner_id: String,
    /// A `LifeCycleState` value: [`STATUS_CREATING`] until the clock owes
    /// [`STATUS_AVAILABLE`].
    pub status: String,
    /// When `creating` becomes `available`. See [`PROVISIONING_SPAN`].
    pub available_due_at: SystemTime,
    pub posix_user: Option<PosixUser>,
    pub root_directory: Option<RootDirectory>,
    pub name: Option<String>,
    pub tags: Vec<Tag>,
    /// Creation order within the owning file system.
    pub sequence: u64,
}

/// Apply the `creating` -> `available` transition in place once `now` has reached
/// `due`.
fn settle_status(status: &mut String, due: SystemTime, now: SystemTime) {
    if status == STATUS_CREATING && now >= due {
        *status = STATUS_AVAILABLE.to_string();
    }
}

/// Per-service state for com-amazonaws-s3files-s3files. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsS3filesS3filesState {
    /// Every file system, keyed by its minted `fs-` id.
    pub file_systems: HashMap<String, FileSystem>,
    /// Every access point, keyed by its minted `fsap-` id.
    /// [`AccessPoint::file_system_id`] binds one to its parent.
    pub access_points: HashMap<String, AccessPoint>,
    /// Monotonic creation counter. It orders list pages and backs the opaque
    /// `nextToken`, so a cursor keeps resuming correctly across deletes.
    next_sequence: u64,
    /// How many ids this store has minted. Part of every id's seed, so two calls in
    /// the same nanosecond still get different ids.
    minted: u64,
    /// `<scope>/<clientToken>` -> the resource id the FIRST request carrying that
    /// token minted, so an `@idempotencyToken` replay returns that resource instead of
    /// creating a second one. The scope is the operation plus, for a child resource,
    /// the parent it was created under, so one token cannot be shared across parents.
    client_tokens: HashMap<String, String>,
    /// The keys of [`Self::client_tokens`] in the order they were first recorded, so
    /// the oldest is the one evicted at [`MAX_REMEMBERED_CLIENT_TOKENS`].
    client_token_order: VecDeque<String>,
}

impl ComAmazonawsS3filesS3filesState {
    /// The next creation sequence number.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// A fresh lowercase-hex string of `chars` characters.
    ///
    /// Hex is what every id pattern in this model accepts, so a minted id always
    /// satisfies the model's own constraint. `minted` advances on every call and is
    /// hashed into the seed with the wall clock and the process id, so no two minted
    /// values collide.
    fn mint_hex(&mut self, label: &str, chars: usize) -> String {
        self.minted = self.minted.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let seed = format!("{label}|{}|{}|{nanos}", self.minted, std::process::id());
        let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
        let mut hex = ::hex::encode(digest);
        hex.truncate(chars);
        hex
    }

    /// Mint a `fs-` file system id.
    pub fn mint_file_system_id(&mut self) -> String {
        format!("fs-{}", self.mint_hex("file-system", MINTED_ID_HEX))
    }

    /// Mint a `fsap-` access point id.
    pub fn mint_access_point_id(&mut self) -> String {
        format!("fsap-{}", self.mint_hex("access-point", MINTED_ID_HEX))
    }

    /// Mint the `clientToken` of a create whose caller sent none.
    ///
    /// `clientToken` is an `@idempotencyToken`, which an AWS SDK fills in for the
    /// caller; a raw HTTP caller may omit it, and `CreateAccessPointResponse$clientToken`
    /// is `@required`, so the service mints the token it recorded the creation under.
    pub fn mint_client_token(&mut self) -> String {
        self.mint_hex("client-token", MINTED_TOKEN_HEX)
    }

    /// Materialise every transition the clock already owes.
    ///
    /// Called at the top of each handler — including the reporting ones, which may
    /// write nothing else: the change was due at this instant whether or not anybody
    /// called.
    pub fn settle(&mut self, now: SystemTime) {
        for file_system in self.file_systems.values_mut() {
            settle_status(
                &mut file_system.status,
                file_system.available_due_at,
                now,
            );
        }
        for access_point in self.access_points.values_mut() {
            settle_status(
                &mut access_point.status,
                access_point.available_due_at,
                now,
            );
        }
    }

    /// The id a previous request carrying this `clientToken` in this `scope` minted, if
    /// any.
    pub fn replayed_id(&self, scope: &str, client_token: Option<&str>) -> Option<String> {
        let token = client_token?;
        self.client_tokens.get(&token_key(scope, token)).cloned()
    }

    /// Record which resource a `clientToken` minted in this `scope`, evicting the
    /// oldest record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held.
    pub fn remember_client_token(&mut self, scope: &str, client_token: &str, id: &str) {
        let key = token_key(scope, client_token);
        if self
            .client_tokens
            .insert(key.clone(), id.to_string())
            .is_none()
        {
            self.client_token_order.push_back(key);
        }
        while self.client_token_order.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            if let Some(oldest) = self.client_token_order.pop_front() {
                self.client_tokens.remove(&oldest);
            }
        }
    }

    /// Drop the idempotency records naming any of `ids`.
    ///
    /// Called when a resource is deleted: the record can no longer replay anything, and
    /// nothing else would ever reclaim it.
    pub fn forget_client_tokens_for(&mut self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        let Self {
            client_tokens,
            client_token_order,
            ..
        } = self;
        client_tokens.retain(|_, id| !ids.contains(id));
        client_token_order.retain(|key| client_tokens.contains_key(key));
    }

    /// File systems in creation order, optionally only those on one `bucket`.
    pub fn file_systems_in_order(&self, bucket: Option<&str>) -> Vec<&FileSystem> {
        let mut ordered: Vec<&FileSystem> = self
            .file_systems
            .values()
            .filter(|file_system| bucket.is_none_or(|wanted| file_system.bucket == wanted))
            .collect();
        ordered.sort_by_key(|file_system| file_system.sequence);
        ordered
    }

    /// The access points bound to `file_system_id`, in creation order.
    pub fn access_points_in_order(&self, file_system_id: &str) -> Vec<&AccessPoint> {
        let mut ordered: Vec<&AccessPoint> = self
            .access_points
            .values()
            .filter(|access_point| access_point.file_system_id == file_system_id)
            .collect();
        ordered.sort_by_key(|access_point| access_point.sequence);
        ordered
    }

    /// The ids of the access points bound to `file_system_id`, in creation order.
    pub fn access_point_ids_of(&self, file_system_id: &str) -> Vec<String> {
        self.access_points_in_order(file_system_id)
            .into_iter()
            .map(|access_point| access_point.access_point_id.clone())
            .collect()
    }
}

fn token_key(scope: &str, client_token: &str) -> String {
    format!("{scope}/{client_token}")
}

/// The value of the tag named `key`, if the resource carries one.
pub fn tag_value(tags: &[Tag], key: &str) -> Option<String> {
    tags.iter()
        .find(|tag| tag.key == key)
        .map(|tag| tag.value.clone())
}

/// The name a resource takes from its tags.
///
/// From the model doc on `CreateFileSystemResponse$name`: "The name of the file
/// system, derived from the Name tag if present." Tags are the only name a caller can
/// supply to either create in this slice, so both resources read it the same way.
pub fn name_from_tags(tags: &[Tag]) -> Option<String> {
    tag_value(tags, "Name")
}

/// The region this request was signed for, read from its SigV4 credential scope.
///
/// A scope region is honoured only when it can appear in an s3files ARN at all — the
/// modelled `FileSystemArn` pins that segment to `[0-9a-z-:]+` — so a hostile or
/// nonsense scope can never make this service mint an ARN its own model rejects.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `FileSystemArn`: `arn:aws:s3files:<region>:<account>:file-system/<fileSystemId>`,
/// the form the model documents on `CreateFileSystemResponse$fileSystemArn`.
pub fn file_system_arn(region: &str, file_system_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("file-system/{file_system_id}"),
    )
}

/// An `AccessPointArn`, which the model nests under its file system:
/// `<fileSystemArn>/access-point/<accessPointId>` (pattern
/// `^arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}/access-point/fsap-[0-9a-f]{17,40}$`).
pub fn access_point_arn(file_system_arn: &str, access_point_id: &str) -> String {
    format!("{file_system_arn}/access-point/{access_point_id}")
}

/// `true` when `value` is `<prefix>` followed by 17..=40 lowercase hex characters —
/// the shape every id in this model takes.
fn is_minted_id(prefix: &str, value: &str) -> bool {
    let Some(hex) = value.strip_prefix(prefix) else {
        return false;
    };
    (17..=40).contains(&hex.len()) && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The `fs-` id a `FileSystemId` names.
///
/// `FileSystemId` accepts EITHER the bare id or the file system's ARN
/// (`^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40}|fs-[0-9a-f]{17,40})$`),
/// so every operation that takes one resolves it through here and both forms reach the
/// same record. `None` means the value is neither form, which a handler answers as a
/// `ValidationException`.
pub fn file_system_id_of(value: &str) -> Option<&str> {
    let candidate = if value.starts_with("arn:") {
        value.rsplit_once("file-system/").map(|(_, id)| id)?
    } else {
        value
    };
    is_minted_id("fs-", candidate).then_some(candidate)
}

/// The `fsap-` id an `AccessPointId` names, from either the bare id or the access
/// point's ARN. See [`file_system_id_of`].
pub fn access_point_id_of(value: &str) -> Option<&str> {
    let candidate = if value.starts_with("arn:") {
        value.rsplit_once("/access-point/").map(|(_, id)| id)?
    } else {
        value
    };
    is_minted_id("fsap-", candidate).then_some(candidate)
}

/// The modelled server error for a request that could not reach this service's store.
///
/// Every exception below reports its own shape name as `errorCode`: `ErrorCode` is a
/// `@length(min: 1)` string with no modelled value list and no documented catalogue, so
/// the shape name is the one stable value per modelled error that a client branching on
/// `errorCode` can rely on.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        error_code: "InternalServerException".to_string(),
        message: Some("the S3 Files store is unavailable".to_string()),
    }
}

/// The modelled 404 for a resource this service does not hold.
pub fn not_found(message: impl Into<String>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        error_code: "ResourceNotFoundException".to_string(),
        message: Some(message.into()),
    }
}

/// The `message` the modelled `ResourceNotFoundException` reports for a file system
/// that holds no IAM resource policy.
///
/// ONE constant sentence naming the kind of resource that is missing, carrying nothing
/// from the request: the policy sub-resource has no id of its own to report, and the
/// file system is already named by the request's own path label. This is the sentence
/// S3 Files answers with, so a client matching on it keeps working.
pub const POLICY_NOT_FOUND_MESSAGE: &str = "File system policy does not exist.";

/// Why `policy` is not a document `PutFileSystemPolicy` can accept, or `None` when it
/// is one.
///
/// `PutFileSystemPolicyRequest$policy` documents: "The policy must be a valid JSON
/// document that follows IAM policy syntax." The IAM JSON policy grammar makes the
/// top-level element a single JSON object whose `Statement` element is required and
/// holds either one statement block or an array of statement blocks. Nothing beyond
/// that is checked here: the remaining elements are free-form to this service, which
/// stores the document rather than evaluating it.
pub fn policy_document_rejection(policy: &str) -> Option<String> {
    let document = match ::serde_json::from_str::<::serde_json::Value>(policy) {
        Ok(document) => document,
        Err(error) => return Some(format!("it is not a valid JSON document ({error})")),
    };
    let Some(elements) = document.as_object() else {
        return Some("the top-level element of an IAM policy document is a JSON object".to_string());
    };
    let Some(statement) = elements.get("Statement") else {
        return Some("an IAM policy document requires a Statement element".to_string());
    };
    let blocks: Vec<&::serde_json::Value> = match statement {
        ::serde_json::Value::Object(_) => vec![statement],
        ::serde_json::Value::Array(blocks) => blocks.iter().collect(),
        _ => {
            return Some(
                "Statement holds one statement block or an array of statement blocks"
                    .to_string(),
            )
        }
    };
    if blocks.is_empty() {
        return Some("Statement holds at least one statement block".to_string());
    }
    if blocks.iter().any(|block| !block.is_object()) {
        return Some("every statement block in Statement is a JSON object".to_string());
    }
    None
}

/// The modelled 400 for a request value this service cannot accept.
pub fn invalid(message: impl Into<String>) -> ValidationException {
    ValidationException {
        error_code: "ValidationException".to_string(),
        message: Some(message.into()),
    }
}

/// The modelled 409 for a request that conflicts with the current state of
/// `resource_id`.
pub fn conflict(
    resource_id: &str,
    resource_type: &str,
    message: impl Into<String>,
) -> ConflictException {
    ConflictException {
        error_code: "ConflictException".to_string(),
        message: Some(message.into()),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The modelled 402 for a create that would take this store past one of its ceilings.
pub fn quota_exceeded(message: impl Into<String>) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        error_code: "ServiceQuotaExceededException".to_string(),
        message: Some(message.into()),
    }
}

/// One page of a `@paginated` listing: the items to return, plus the cursor that
/// resumes exactly where the page ended (`None` when this page is the last).
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `nextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Encode the resume cursor that follows the item with sequence number `sequence`.
///
/// Opaque to the caller (hex of the internal cursor text), so a caller cannot build
/// one and the service can tell a forged token from one it issued.
pub fn encode_cursor(sequence: u64) -> String {
    ::hex::encode(format!("s3files:1:after-sequence:{sequence}"))
}

/// Decode a cursor minted by [`encode_cursor`]; `None` starts at the beginning.
pub fn decode_cursor(next_token: Option<&str>) -> Result<u64, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(0);
    };
    let bytes = ::hex::decode(token).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    text.strip_prefix("s3files:1:after-sequence:")
        .and_then(|sequence| sequence.parse::<u64>().ok())
        .ok_or(MalformedCursor)
}

/// Take the page of `ordered` (sorted by sequence number) that follows `after`, at most
/// `max_results` items long, where `max_results_limit` is the `@range` maximum the
/// calling operation's own `maxResults` member declares.
///
/// Each `maxResults` is `@range(1, <limit>)` with `@default(100)`, so the boundary
/// validator has already refused anything outside the range; a request that named none
/// arrives as the out-of-range 0, which takes the documented default.
pub fn page_of<T>(
    ordered: impl IntoIterator<Item = (u64, T)>,
    after: u64,
    max_results: i32,
    max_results_limit: i32,
) -> Page<T> {
    let limit = if (1..=max_results_limit).contains(&max_results) {
        max_results as usize
    } else {
        DEFAULT_MAX_RESULTS.min(max_results_limit) as usize
    };
    let mut items = Vec::new();
    let mut last_sequence = after;
    let mut more_follow = false;
    for (sequence, item) in ordered.into_iter().filter(|(sequence, _)| *sequence > after) {
        if items.len() == limit {
            // One item beyond the page proves a next page exists; the cursor names
            // where this page ended, so resuming never repeats or skips an item.
            more_follow = true;
            break;
        }
        last_sequence = sequence;
        items.push(item);
    }
    Page {
        items,
        next_token: more_follow.then(|| encode_cursor(last_sequence)),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3filesS3filesState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3filesS3filesState>)
}
