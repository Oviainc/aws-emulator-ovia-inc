// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mailmanager-mailmanagersvc service.
//!
//! Timestamps are stored as epoch MILLISECONDS (`i64`) and rendered into the modelled
//! `Timestamp` members with [`::aws_smithy_types::DateTime::from_millis`] on the way
//! out. Keeping an integer in the store (rather than a `DateTime`) is what lets
//! [`bump_millis`] guarantee that an update is *observably* later than the create that
//! preceded it even when both land inside one clock tick.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{ArchiveRetention, Tag};

/// The two `ArchiveState` values this service's model declares; see
/// `crate::ops::types::ARCHIVE_STATE_VALUES`, which is generated from the model's
/// `ArchiveState` enum.
pub const ARCHIVE_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ARCHIVE_STATE_ACTIVE`]. The model documents it as "The archive has been marked
/// for deletion and will be permanently deleted in 30 days. No further modifications can
/// be made in this state."
pub const ARCHIVE_STATE_PENDING_DELETION: &str = "PENDING_DELETION";

/// How long an archive stays in `PENDING_DELETION` before it is permanently removed.
///
/// This duration is DOCUMENTED, not chosen: the model's `DeleteArchive` documentation
/// states "The archive and all of its contents will be permanently deleted 30 days after
/// entering the pending deletion state, regardless of the configured retention period",
/// and `Archive$ArchiveState` repeats "will be permanently deleted in 30 days".
pub const PENDING_DELETION_WINDOW_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;

/// The `RetentionPeriod` recorded for an archive created without a `Retention`.
///
/// The model declares no `@default` for `CreateArchiveRequest$Retention` yet makes
/// `GetArchiveResponse$Retention` **required**, so a record must carry one. `PERMANENT`
/// is the one `RetentionPeriod` value (see `crate::ops::types::RETENTION_PERIOD_VALUES`)
/// under which no mail is ever automatically deleted, so it is the only default that
/// cannot discard data the caller never asked to have discarded.
pub const DEFAULT_RETENTION_PERIOD: &str = "PERMANENT";

/// The region an ARN carries when the request names none; `::engine::emulator_arn`
/// documents `us-east-1` as the emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most address lists this emulator keeps, i.e. its per-account address-list quota.
///
/// `CreateAddressList` models `ServiceQuotaExceededException` ("Occurs when an operation
/// exceeds a predefined service quota or limit") precisely because the real service caps
/// the resource. The model does not state the number, so this value is CHOSEN on the same
/// reasoning as [`MAX_ARCHIVES`]: large enough that ordinary use never reaches it, small
/// enough that a caller cannot grow the process's memory without limit. It bounds the
/// whole address-list side of the store: `address_list_names` holds one entry per live
/// address list, and `address_list_idempotency` is only written on a create that
/// succeeded.
pub const MAX_ADDRESS_LISTS: usize = 100;

/// The most archives this emulator keeps, i.e. its per-account archive quota.
///
/// `CreateArchive` is the only operation that grows this service's store, and it models
/// `ServiceQuotaExceededException` ("Occurs when an operation exceeds a predefined service
/// quota or limit") precisely because the real service caps the resource. The model does
/// not state the number, so this value is CHOSEN: it is large enough that ordinary use
/// never reaches it and small enough that a caller cannot grow the process's memory
/// without limit. It bounds the whole store, not just `archives`: `archive_names` holds
/// one entry per live archive, and `idempotency` is only written on a create that
/// succeeded, so neither can exceed this count, and `purge_permanently_deleted`'s walk is
/// bounded by it too.
pub const MAX_ARCHIVES: usize = 100;

/// One stored email archive. The single source of truth for every archive operation:
/// `CreateArchive` writes it, `GetArchive`/`ListArchives` read it back, and
/// `UpdateArchive`/`DeleteArchive` mutate it in place.
#[derive(Debug, Clone)]
pub struct ArchiveRecord {
    pub archive_id: String,
    pub archive_name: String,
    pub archive_arn: String,
    /// `ARCHIVE_STATE_ACTIVE` or `ARCHIVE_STATE_PENDING_DELETION`.
    pub archive_state: String,
    pub retention: ArchiveRetention,
    pub kms_key_arn: Option<String>,
    pub created_at_millis: i64,
    pub last_updated_at_millis: i64,
    pub tags: Vec<Tag>,
    /// When the archive becomes permanently deleted, set by `DeleteArchive` to
    /// [`PENDING_DELETION_WINDOW_MILLIS`] after the call. `None` while the archive is
    /// `ACTIVE`.
    pub pending_deletion_at_millis: Option<i64>,
}

/// One stored address list: "a list of emails and domains that are used in MailManager
/// Ingress endpoints and Rules for email management" (the model's `AddressList` doc).
///
/// The single source of truth for every address-list operation: `CreateAddressList` writes
/// it and `GetAddressList` reads it back, and the member operations
/// (`RegisterMemberToAddressList`, `GetMemberOfAddressList`, `ListMembersOfAddressList`)
/// read and write [`Self::members`]. The model declares no status member for an address
/// list, so a created one is immediately usable — there is no transition for the clock to
/// owe.
#[derive(Debug, Clone)]
pub struct AddressListRecord {
    pub address_list_id: String,
    pub address_list_name: String,
    pub address_list_arn: String,
    pub created_at_millis: i64,
    pub last_updated_at_millis: i64,
    pub tags: Vec<Tag>,
    /// The addresses that are members of this list: each stored address
    /// ([`member_address_key`]) against the instant it was added, which is the
    /// `CreatedTimestamp` a member read reports.
    ///
    /// A `BTreeMap`, so one address is one member however often it is registered,
    /// `ListMembersOfAddressList` walks the members in one stable order, and a continuation
    /// token can name the address to resume at. Bounded by [`MAX_ADDRESS_LIST_MEMBERS`].
    /// The membership lives INSIDE the list record, so deleting the list takes it along.
    pub members: BTreeMap<String, i64>,
}

/// The most members this emulator keeps in one address list, i.e. its per-list membership
/// quota.
///
/// `RegisterMemberToAddressList` models `ServiceQuotaExceededException` ("Occurs when an
/// operation exceeds a predefined service quota or limit") precisely because the real
/// service caps a list's membership. The model does not state the number, so this value is
/// CHOSEN on the same reasoning as [`MAX_ADDRESS_LISTS`]: an `Address` is at most 320
/// bytes, so [`MAX_ADDRESS_LISTS`] lists of this many members bound the addresses this
/// process holds to tens of megabytes, while leaving ordinary use far from the ceiling.
pub const MAX_ADDRESS_LIST_MEMBERS: usize = 1_000;

/// The key an address-list member is stored and looked up under: the address with its
/// DOMAIN folded to lower case and its local part left exactly as the caller spelled it.
///
/// A domain is a DNS name, which is compared without regard to case, while the local part
/// of an address is not — so `user@Example.COM` and `user@example.com` name ONE member,
/// while `User@example.com` and `user@example.com` name two. A value that is a bare domain
/// has no local part at all, so it folds whole.
///
/// The reading is the workspace's one (`::service_common::ses_identities::identity_key`),
/// which the SES surfaces key their email identities by: these are the same mail addresses
/// under the same rule, so there is one answer here to what folds and what does not.
pub fn member_address_key(address: &str) -> String {
    ::service_common::ses_identities::identity_key(address)
}

/// Whether `address` is a value an address list can hold.
///
/// `SavedAddress$Address` is "The email or domain that constitutes the address", so either
/// an email address or a bare domain name; the form of each is the workspace's one reading
/// (`::service_common::email_address`). `Address` is modelled only as a 3..320 string, so
/// the boundary validator admits any text of that length and reading the form is the
/// handler's job: a value that is neither names no mailbox and no domain.
pub fn is_registrable_address(address: &str) -> bool {
    ::service_common::email_address::is_email_address(address)
        || ::service_common::email_address::is_domain_name(address)
}

/// What a member operation reports for an `Address` that is neither an email address nor a
/// domain name ([`is_registrable_address`]).
///
/// `Address` is `@sensitive`, so the refusal names the member and the requirement and
/// states no value — the same half a boundary refusal drops for such a member.
pub const ADDRESS_NOT_AN_ADDRESS: &str = "Address must be an email address or a domain name.";

/// The `ImportJobStatus` a created address-list import job carries.
///
/// `CreateAddressListImportJob` creates the job and hands back the URL its input file is
/// uploaded to, so nothing has been imported when the call returns; `CREATED` is the one
/// `ImportJobStatus` (see `crate::ops::types::IMPORT_JOB_STATUS_VALUES`, generated from
/// the model's enum) that says exactly that, and the model's `ImportJob$Status` leaves
/// `StartTimestamp`, `CompletedTimestamp` and both item counts optional precisely because
/// a job in it has none of them yet.
pub const IMPORT_JOB_STATUS_CREATED: &str = "CREATED";

/// The most address-list import jobs this emulator keeps.
///
/// A resource guard this emulator OWNS, not an AWS-published quota:
/// `CreateAddressListImportJob` models no quota error at all, so a create past this
/// ceiling is refused with the modelled `ValidationException`. It bounds both the job
/// records and the input files uploaded against them, so the store holds at most this
/// many times [`MAX_IMPORT_FILE_BYTES`] of caller-supplied bytes.
pub const MAX_IMPORT_JOBS: usize = 100;

/// The most bytes one import job's uploaded input file is retained as.
///
/// The bytes live in this process's heap for as long as the job record does, so the upload
/// plane bounds them: a larger body is refused there rather than stored. A resource guard
/// this emulator owns, not an AWS-published limit; an import file is a list of addresses,
/// so this holds tens of thousands of them.
pub const MAX_IMPORT_FILE_BYTES: usize = 1024 * 1024;

/// The input file that was uploaded to one import job's pre-signed URL: the bytes, and
/// when they arrived.
#[derive(Debug, Clone)]
pub struct UploadedImportFile {
    pub content: Vec<u8>,
    pub uploaded_at_millis: i64,
}

/// One stored address-list import job: "an import job for an address list" (the model's
/// `CreateAddressListImportJob` doc).
///
/// The single source of truth for every import-job operation:
/// `CreateAddressListImportJob` writes it and `GetAddressListImportJob` reads it back. The
/// members the model leaves optional are the ones only a RUNNING import fills
/// ([`Self::started_at_millis`] and below), so a created job carries none of them.
#[derive(Debug, Clone)]
pub struct ImportJobRecord {
    pub job_id: String,
    pub address_list_id: String,
    pub name: String,
    /// One of `crate::ops::types::IMPORT_JOB_STATUS_VALUES`;
    /// [`IMPORT_JOB_STATUS_CREATED`] until an import runs.
    pub status: String,
    /// The `ImportDataFormat$ImportDataType` the job was created for, one of
    /// `crate::ops::types::IMPORT_DATA_TYPE_VALUES`.
    pub import_data_type: String,
    /// The URL the input file is uploaded to, minted once when the job was created: a
    /// caller that reads the job back gets the same target it was first handed.
    pub pre_signed_url: String,
    pub created_at_millis: i64,
    /// What the upload plane received at [`Self::pre_signed_url`], if anything yet.
    pub uploaded_file: Option<UploadedImportFile>,
    pub started_at_millis: Option<i64>,
    pub completed_at_millis: Option<i64>,
    pub imported_items_count: Option<i32>,
    pub failed_items_count: Option<i32>,
    pub error: Option<String>,
}

/// Per-service state for com-amazonaws-mailmanager-mailmanagersvc. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap`, so `ListArchives` walks the archives in one stable,
/// deterministic order and a pagination token can name the next key to resume from.
///
/// BOUNDED: `CreateArchive` refuses a create that would take `archives` past
/// [`MAX_ARCHIVES`], `CreateAddressList` one that would take `address_lists` past
/// [`MAX_ADDRESS_LISTS`] and `CreateAddressListImportJob` one that would take
/// `import_jobs` past [`MAX_IMPORT_JOBS`]; every other map carries at most one entry per
/// live archive, address list or import job, so no caller can grow this store without
/// limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsMailmanagerMailmanagersvcState {
    /// Archives by `ArchiveId`.
    pub archives: BTreeMap<String, ArchiveRecord>,
    /// `ArchiveName` -> `ArchiveId`; the index behind the name-uniqueness
    /// `ConflictException`.
    pub archive_names: BTreeMap<String, String>,
    /// `ClientToken` -> the create it completed; what makes a retried `CreateArchive`
    /// return the id the first call minted instead of creating a second archive.
    pub idempotency: BTreeMap<String, IdempotentCreate>,
    /// Monotonic counter feeding [`ComAmazonawsMailmanagerMailmanagersvcState::mint_archive_id`];
    /// it is what makes a minted id unique even within one clock tick.
    pub archive_sequence: u64,
    /// Address lists by `AddressListId`.
    pub address_lists: BTreeMap<String, AddressListRecord>,
    /// `AddressListName` -> `AddressListId`; the index behind the name-uniqueness
    /// `ConflictException`.
    pub address_list_names: BTreeMap<String, String>,
    /// `ClientToken` -> the `CreateAddressList` it completed. Separate from
    /// [`Self::idempotency`] so a token recorded for one resource is never read as a retry
    /// of a create of the other.
    pub address_list_idempotency: BTreeMap<String, IdempotentCreate>,
    /// Monotonic counter feeding
    /// [`ComAmazonawsMailmanagerMailmanagersvcState::mint_address_list_id`]; see
    /// [`Self::archive_sequence`].
    pub address_list_sequence: u64,
    /// Address-list import jobs by `JobId`.
    pub import_jobs: BTreeMap<String, ImportJobRecord>,
    /// `ClientToken` -> the `CreateAddressListImportJob` it completed. Separate from the
    /// other two token maps so a token recorded for one resource is never read as a retry
    /// of a create of another.
    pub import_job_idempotency: BTreeMap<String, IdempotentCreate>,
    /// Monotonic counter feeding
    /// [`ComAmazonawsMailmanagerMailmanagersvcState::mint_import_job_id`]; see
    /// [`Self::archive_sequence`].
    pub import_job_sequence: u64,
    /// The secret THIS store's pagination tokens are authenticated under; see
    /// [`encode_page_token`].
    pub page_token_key: PageTokenKey,
}

impl ComAmazonawsMailmanagerMailmanagersvcState {
    /// Materialise the removals the clock already owes: drop every archive whose
    /// pending-deletion window has elapsed, along with its name and idempotency index
    /// entries.
    ///
    /// Called by the reporting operations too. That is the one write a read is allowed:
    /// the archive was due to be gone at this instant whether or not anybody called, so
    /// applying it here cannot change what a later caller would otherwise have seen.
    pub fn purge_permanently_deleted(&mut self, now_millis: i64) {
        let expired: Vec<String> = self
            .archives
            .values()
            .filter(|record| {
                record
                    .pending_deletion_at_millis
                    .is_some_and(|due| due <= now_millis)
            })
            .map(|record| record.archive_id.clone())
            .collect();
        if expired.is_empty() {
            return;
        }
        for id in &expired {
            if let Some(record) = self.archives.remove(id) {
                // Only drop the name index entry if it still points at THIS archive: a
                // later archive may legitimately have taken the freed name.
                if self.archive_names.get(&record.archive_name) == Some(&record.archive_id) {
                    self.archive_names.remove(&record.archive_name);
                }
            }
        }
        let live: std::collections::BTreeSet<String> = self.archives.keys().cloned().collect();
        self.idempotency
            .retain(|_, create| live.contains(&create.resource_id));
    }

    /// Mint a fresh `ArchiveId`: 32 lowercase hex characters (see [`mint_id`]), well
    /// inside `ArchiveIdString`'s modelled 1..66 length.
    pub fn mint_archive_id(&mut self, now_millis: i64) -> String {
        self.archive_sequence = self.archive_sequence.wrapping_add(1);
        mint_id(self.archive_sequence, now_millis)
    }

    /// Mint a fresh `AddressListId`, the same way [`Self::mint_archive_id`] mints an
    /// archive id: 32 lowercase hex characters, which satisfy `AddressListId`'s modelled
    /// 1..255 length and `^[a-zA-Z0-9-]+$` pattern.
    pub fn mint_address_list_id(&mut self, now_millis: i64) -> String {
        self.address_list_sequence = self.address_list_sequence.wrapping_add(1);
        mint_id(self.address_list_sequence, now_millis)
    }

    /// Mint a fresh import-job `JobId`, the same way [`Self::mint_archive_id`] mints an
    /// archive id: 32 lowercase hex characters, which satisfy `JobId`'s modelled 1..255
    /// length and `^[a-zA-Z0-9-]+$` pattern.
    pub fn mint_import_job_id(&mut self, now_millis: i64) -> String {
        self.import_job_sequence = self.import_job_sequence.wrapping_add(1);
        mint_id(self.import_job_sequence, now_millis)
    }
}

/// The identifier minted for the `sequence`-th resource of its kind: 32 lowercase hex
/// characters.
///
/// The low half is a bijection of that per-kind sequence number, so two resources can
/// never share an id; the high half mixes in the creation instant so ids are not
/// consecutive and a caller cannot derive the next one from the last.
fn mint_id(sequence: u64, now_millis: i64) -> String {
    let high = splitmix64((now_millis as u64) ^ sequence.rotate_left(32));
    let low = splitmix64(sequence);
    format!("{high:016x}{low:016x}")
}

/// The secret this service's page tokens are authenticated under — the shared one every
/// paginated read of this emulator uses, so no two services disagree about what makes a
/// continuation token one the service ISSUED.
///
/// A continuation token carries the position itself (there is no page kept on the side to
/// look up), so only the authenticator over that position tells a token this store minted
/// from one a caller wrote: a paginated read can then refuse a value it did not issue
/// instead of serving a page from a position the caller chose. A token minted under another
/// store's key — another emulator process, a restarted one — names no position here either.
pub use ::service_common::page_tokens::PageTokenKey;

/// Encode `cursor` as an opaque `NextToken` this store can later recognise as its own: the
/// authenticated bytes of the cursor, hex-encoded.
///
/// Hex is this service's spelling for an opaque token — it is what makes the value one a
/// caller round-trips rather than constructs. The widest cursor is an `Address` (320 bytes
/// at most), so the token is at most 704 characters, inside `PaginationToken`'s modelled
/// 1..2048 length.
pub fn encode_page_token(key: &PageTokenKey, cursor: &str) -> String {
    ::hex::encode(key.mint(cursor))
}

/// The cursor a `NextToken` carries, or `None` for any token THIS store did not mint — one
/// that is not hex, one too short to carry an authenticator, one whose authenticator does
/// not match the cursor it travels with, and one whose cursor is not UTF-8.
pub fn decode_page_token(key: &PageTokenKey, token: &str) -> Option<String> {
    key.cursor_of(&::hex::decode(token).ok()?)
}

/// The message a handler reports when the shared store cannot be reached.
///
/// Each handler still builds its own modelled error (the generated error enums are
/// per-operation), so this is the one part of that answer they can share.
pub const STORE_UNAVAILABLE: &str = "service state is unavailable; retry the request";

/// What one `ClientToken` recorded: which resource the first call created, and a
/// description of the request it created it from.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    pub resource_id: String,
    pub fingerprint: String,
}

/// A stable description of the members an archive is created from.
///
/// An `@idempotencyToken` identifies a RETRY of one request, so a token presented with a
/// different request is not a retry: comparing this description is what lets
/// `CreateArchive` tell the two apart instead of answering the second with the first
/// one's id. `Tags` are part of it too, because the record stores them.
pub fn create_archive_fingerprint(
    archive_name: &str,
    retention: Option<&ArchiveRetention>,
    kms_key_arn: Option<&str>,
    tags: Option<&[Tag]>,
) -> String {
    let retention = match retention {
        Some(ArchiveRetention::RetentionPeriod(period)) => period.as_str(),
        None => "",
    };
    // `\u{1e}` (record separator) cannot appear in any of these members, so the joined
    // description is unambiguous.
    format!(
        "{archive_name}\u{1e}{retention}\u{1e}{}\u{1e}{}",
        kms_key_arn.unwrap_or_default(),
        render_tags(tags)
    )
}

/// A stable description of the members an address list is created from; see
/// [`create_archive_fingerprint`] for what it is compared against and why.
pub fn create_address_list_fingerprint(address_list_name: &str, tags: Option<&[Tag]>) -> String {
    format!("{address_list_name}\u{1e}{}", render_tags(tags))
}

/// A stable description of the members an address-list import job is created from; see
/// [`create_archive_fingerprint`] for what it is compared against and why.
pub fn create_import_job_fingerprint(
    address_list_id: &str,
    name: &str,
    import_data_type: &str,
) -> String {
    format!("{address_list_id}\u{1e}{name}\u{1e}{import_data_type}")
}

/// The tags of a create request, rendered into one unambiguous string for a fingerprint.
///
/// Sorted: the same set in a different order is the same request. `\u{1f}` (unit
/// separator) and `\u{1e}` (record separator) cannot appear in a `TagKey` or `TagValue`
/// (both are patterned), so the rendering is injective.
fn render_tags(tags: Option<&[Tag]>) -> String {
    let mut rendered: Vec<String> = tags
        .unwrap_or_default()
        .iter()
        .map(|tag| format!("{}\u{1f}{}", tag.key, tag.value))
        .collect();
    rendered.sort();
    rendered.join("\u{1e}")
}

/// SplitMix64, a standard 64-bit mixing function: a bijection on `u64`, so distinct
/// inputs always give distinct outputs. Used only to scatter minted identifiers.
///
/// Per-service, like [`now_millis`] and [`request_region`], because the framework has no
/// shared home for any of them: `vendor/` is read-only from here, and a shared crate under
/// `services/` would exist for three helpers of a few lines each. Other bootstrapped
/// services carry their own copies for the same reason.
fn splitmix64(input: u64) -> u64 {
    let mut z = input.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The current wall-clock instant as epoch milliseconds. Per-service for the reason given
/// on [`splitmix64`].
pub fn now_millis() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// The new `LastUpdatedTimestamp` for a record last written at `previous`.
///
/// `now` unless the clock has not advanced since `previous`, in which case one
/// millisecond past `previous` — so an update is always observably later than the write
/// before it, which is what a caller comparing two reads relies on.
pub fn bump_millis(previous: i64, now: i64) -> i64 {
    now.max(previous.saturating_add(1))
}

/// Render a stored epoch-millisecond instant as the modelled `Timestamp`.
pub fn timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The region this request was signed for, for the ARNs the response carries.
///
/// A signed AWS SDK request names it in the SigV4 credential scope; an unsigned one
/// names no region, so the emulator's default stands in.
///
/// Per-service by necessity: the natural shared home for this is beside
/// `emulator_server::sigv4_credential_scope`, and that crate is vendored and read-only
/// from here, so each bootstrapped service wraps it in its own `state.rs` until the
/// framework grows the accessor. See also the note on [`splitmix64`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an archive, in this service's ARN namespace.
///
/// `smithy_query(query="service_info")` reports `arnNamespace: ses` for MailManager, and
/// the archive resource type is `mail-manager-archive`.
pub fn archive_arn(region: &str, archive_id: &str) -> String {
    ::engine::emulator_arn("ses", region, &format!("mail-manager-archive/{archive_id}"))
}

/// The ARN of an address list, in this service's ARN namespace.
///
/// `AddressListArn` carries `@arnReference(service: "ses")`
/// (`smithy_query(query="shape",shape="AddressListArn")`), and SES names the Mail Manager
/// address-list resource type `mailmanager-address-list` in an ARN.
pub fn address_list_arn(region: &str, address_list_id: &str) -> String {
    ::engine::emulator_arn(
        "ses",
        region,
        &format!("mailmanager-address-list/{address_list_id}"),
    )
}

/// The segment that names THIS service's upload plane behind the reserved data-plane path
/// namespace (`::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`), so the
/// locator [`import_job_upload_url`] hands out and the route that accepts the upload
/// (`crate::service`) cannot drift, and so the path collides with no modelled route of any
/// service.
pub const IMPORT_JOB_UPLOAD_PLANE: &str = "address-list-import-jobs";

/// The path the input file of `job_id` is uploaded at:
/// `/_aws/{plane}/{resource}`, the reserved shape for a data plane this emulator serves on
/// its one endpoint.
pub fn import_job_upload_path(job_id: &str) -> String {
    format!(
        "/{}/{IMPORT_JOB_UPLOAD_PLANE}/{job_id}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// The job whose upload target `path` addresses, or `None` for every other path.
///
/// Reads the same namespace and plane segments the locator is built from. The id has to
/// have the FORM this service mints one in, so a path of the same shape addressed to
/// another mounted service is not mistaken for a locator this service published.
pub fn parse_import_job_upload_path(path: &str) -> Option<&str> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};

    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != IMPORT_JOB_UPLOAD_PLANE {
        return None;
    }
    let job_id = rest.strip_prefix('/')?;
    is_job_id_shaped(job_id).then_some(job_id)
}

/// Whether `candidate` could be a `JobId` at all: the modelled length 1..255 and pattern
/// `^[a-zA-Z0-9-]+$` (`smithy_query(query="shape",shape="JobId")`).
fn is_job_id_shaped(candidate: &str) -> bool {
    (1..=255).contains(&candidate.len())
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Where a client uploads the input file of the import job `job_id`:
/// `CreateAddressListImportJobResponse$PreSignedUrl`, "The pre-signed URL target for
/// uploading the input file".
///
/// AWS answers an Amazon S3 pre-signed URL here. This emulator provisions no object store,
/// so the URL names THIS job's input file on the endpoint THIS REQUEST ARRIVED AT, and
/// this service accepts the upload on that path itself (see `crate::service`) — a URL a
/// client can really write the file to, rather than one pointing at the real service. NO
/// signature is attached: this emulator signs no URLs, and a fabricated signature would
/// claim a computation nothing performed; the minted, unguessable `JobId` is what names
/// the target.
pub fn import_job_upload_url(host: &str, job_id: &str) -> String {
    format!("https://{host}{}", import_job_upload_path(job_id))
}

/// The endpoint host this request reached the emulator on, so a URL this service hands out
/// resolves back to where the caller is already talking rather than naming a host nothing
/// serves.
///
/// The reading is the workspace's one
/// (`::service_common::data_plane_addressing::reached_endpoint_host`): the host the request
/// addressed — its `Host` header, or the authority of an absolute-form URI for a request
/// that carries its endpoint there — when it is shaped like a host, and this service's
/// published endpoint for the region it was signed for otherwise.
pub fn request_endpoint_host(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> String {
    ::service_common::data_plane_addressing::reached_endpoint_host(
        &ctx.parts().uri.to_string(),
        ctx.parts()
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        crate::service::ENDPOINT_PREFIX,
        region,
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMailmanagerMailmanagersvcState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMailmanagerMailmanagersvcState>)
}
