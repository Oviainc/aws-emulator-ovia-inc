// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssoadmin-swbexternalservice service.
//!
//! The records here are the service's own, wire-independent view of an IAM Identity
//! Center instance, its permission sets and its account assignments. Handlers read and
//! write them through [`service_state`] and project them onto the modeled shapes in
//! `crate::ops::types`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AccountAssignment, AccountAssignmentOperationStatus, InstanceMetadata, PermissionSet,
    RegionMetadata, Tag, TrustedTokenIssuerConfiguration,
};

/// The identity-store instance every `InstanceArn`-taking operation resolves against.
///
/// IAM Identity Center allows ONE instance per account, so the emulator seeds exactly
/// one at first touch of this state (see the `Default` impl) rather than minting
/// instances on demand. The id is fixed so repeated `ListInstances` calls — and the
/// permission-set ARNs derived from it — are stable for the life of the process.
///
/// `ssoins-` + 16 characters, the form the model's `InstanceArn` pattern accepts:
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
pub const SEED_INSTANCE_ID: &str = "ssoins-7223b1d1ac5e1f0e";

/// The identity store connected to the seeded instance: `d-` + 10 hex digits, which
/// `InstanceMetadata$IdentityStoreId` (`^[a-zA-Z0-9-]*$`, 1..64) accepts and which
/// keeps `IdentityStoreArn` inside its modeled 62..93 length.
pub const SEED_IDENTITY_STORE_ID: &str = "d-9067f3a1c2";

/// The seeded instance's primary Region. Matches `RegionName`'s
/// `^([a-z]+-){2,3}\d$` pattern and the region every emulated ARN is built in.
pub const SEED_REGION: &str = "us-east-1";

/// The seeded instance's display name (`NameType`, `^[\w+=,.@-]+$`).
pub const SEED_INSTANCE_NAME: &str = "IdentityCenter";

/// `InstanceStatus`/`RegionStatus` value for a usable instance: the model's
/// `InstanceStatus` enum is `CREATE_IN_PROGRESS | CREATE_FAILED | DELETE_IN_PROGRESS |
/// ACTIVE`, and a seeded instance is ready, so it is `ACTIVE` and stays `ACTIVE`.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// Whether permission sets are enabled on the seeded instance.
///
/// They are: this service implements `CreatePermissionSet`, `DescribePermissionSet`,
/// `ListPermissionSets` and the account-assignment operations against that instance, so
/// reporting anything else from `DescribeInstance` would contradict what the instance
/// actually serves.
pub const SEED_PERMISSION_SETS_ENABLED: bool = true;

/// `StatusValues::SUCCEEDED` — the terminal status of an assignment operation this
/// emulator has already applied in full.
pub const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The page size a paginated read uses when the caller sends no `MaxResults`, and the
/// ceiling it clamps a caller's value to: the model bounds every `MaxResults` member of
/// this service at `range(min: 1, max: 100)`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The maximum number of records this emulator keeps in ANY ONE of its caller-driven
/// stores ([`ComAmazonawsSsoadminSwbexternalserviceState::permission_sets`],
/// [`ComAmazonawsSsoadminSwbexternalserviceState::account_assignments`],
/// [`ComAmazonawsSsoadminSwbexternalserviceState::assignment_operations`]).
///
/// This is NOT AWS's service quota — IAM Identity Center's published permission-set and
/// assignment limits are per-instance and adjustable, and nothing here claims to
/// reproduce them. It is the emulator's own MEMORY CEILING: every successful create
/// inserts a record that lives until a caller deletes it, so without a ceiling a caller
/// could grow the process until the host runs out of memory. A request that would cross
/// it is refused with the modelled `ServiceQuotaExceededException`, which AWS documents
/// as "the principal has crossed the permitted number of resources that can be
/// created", so the refusal is one a real client already handles.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The message a store at [`MAX_RECORDS_PER_STORE`] refuses a new record with; `store`
/// names the records it holds, e.g. `"permission sets"`.
pub fn quota_exceeded_message(store: &str) -> String {
    format!(
        "The principal has crossed the permitted number of resources that can be \
         created: this emulator holds at most {MAX_RECORDS_PER_STORE} {store}"
    )
}

/// One enabled Region of an IAM Identity Center instance.
#[derive(Debug, Clone)]
pub struct Region {
    pub region_name: String,
    pub status: String,
    pub added_date: DateTime,
    pub is_primary_region: bool,
}

impl Region {
    /// This Region as the modeled `RegionMetadata` shape.
    pub fn to_shape(&self) -> RegionMetadata {
        RegionMetadata {
            region_name: Some(self.region_name.clone()),
            status: Some(self.status.clone()),
            added_date: Some(self.added_date),
            is_primary_region: self.is_primary_region,
        }
    }
}

/// An IAM Identity Center instance.
#[derive(Debug, Clone)]
pub struct Instance {
    pub instance_arn: String,
    pub identity_store_id: String,
    pub identity_store_arn: String,
    pub owner_account_id: String,
    pub name: String,
    pub created_date: DateTime,
    pub status: String,
    pub primary_region: String,
    pub regions: Vec<Region>,
    /// `DescribeInstance$PermissionSetsEnabled` — "Indicates whether permission sets
    /// are enabled for this Identity Center instance". It is a property of the
    /// instance, not of the request, so it is held here rather than decided per read;
    /// `InstanceMetadata` (what `ListInstances` projects) carries no such member, so
    /// only `DescribeInstance` reports it.
    pub permission_sets_enabled: bool,
}

impl Instance {
    /// This instance as the modeled `InstanceMetadata` shape.
    ///
    /// `StatusReason` is left absent: the model documents it as additional context for
    /// an instance in a non-ACTIVE state, and there is none to give for an ACTIVE one.
    pub fn to_metadata(&self) -> InstanceMetadata {
        InstanceMetadata {
            instance_arn: Some(self.instance_arn.clone()),
            identity_store_id: Some(self.identity_store_id.clone()),
            identity_store_arn: Some(self.identity_store_arn.clone()),
            owner_account_id: Some(self.owner_account_id.clone()),
            name: Some(self.name.clone()),
            created_date: Some(self.created_date),
            status: Some(self.status.clone()),
            status_reason: None,
            primary_region: Some(self.primary_region.clone()),
            regions: Some(self.regions.iter().map(Region::to_shape).collect()),
        }
    }
}

/// A permission set, keyed in [`ComAmazonawsSsoadminSwbexternalserviceState::permission_sets`]
/// by the ARN minted for it at creation.
///
/// `managed_policies` and `inline_policy` are held here so the policy-attachment
/// operations (still scaffolded) have one record to extend rather than a second map.
#[derive(Debug, Clone)]
pub struct PermissionSetRecord {
    pub permission_set_arn: String,
    pub instance_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub created_date: DateTime,
    pub session_duration: Option<String>,
    pub relay_state: Option<String>,
    pub tags: Vec<Tag>,
    pub managed_policies: Vec<String>,
    pub inline_policy: Option<String>,
}

impl PermissionSetRecord {
    /// This record as the modeled `PermissionSet` shape — what both
    /// `CreatePermissionSet` and `DescribePermissionSet` return, so a describe answers
    /// byte-identically to the create that stored it.
    pub fn to_shape(&self) -> PermissionSet {
        PermissionSet {
            name: Some(self.name.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            description: self.description.clone(),
            created_date: Some(self.created_date),
            session_duration: self.session_duration.clone(),
            relay_state: self.relay_state.clone(),
        }
    }
}

/// A trusted token issuer connected to one IAM Identity Center instance, keyed in
/// [`ComAmazonawsSsoadminSwbexternalserviceState::trusted_token_issuers`] by the ARN
/// minted for it at creation.
///
/// The `Tags` a create carries are held here (rather than dropped) so the tagging
/// operations have one record to extend rather than a second map, and so the
/// idempotency fingerprint below covers every parameter of the request that made it.
#[derive(Debug, Clone)]
pub struct TrustedTokenIssuerRecord {
    pub trusted_token_issuer_arn: String,
    pub instance_arn: String,
    pub name: String,
    pub trusted_token_issuer_type: String,
    pub configuration: TrustedTokenIssuerConfiguration,
    pub tags: Vec<Tag>,
    /// The `ClientToken` of the create that minted this issuer, when it carried one.
    ///
    /// Held ON the record, so the idempotency promise lives exactly as long as the
    /// resource it names: a deleted issuer frees its token instead of leaving a record
    /// that would answer a replay with an ARN nothing resolves.
    pub client_token: Option<String>,
    /// [`trusted_token_issuer_fingerprint`] of the request that minted this issuer, so
    /// a retry carrying the SAME token can be told from the same token carrying a
    /// DIFFERENT request.
    pub request_fingerprint: String,
}

/// An assignment of a permission set to a principal on one account.
///
/// Its identity is the tuple (instance, account, permission set, principal type,
/// principal id); [`ComAmazonawsSsoadminSwbexternalserviceState::assignment_position`]
/// is the single place that tuple is compared.
#[derive(Debug, Clone)]
pub struct AccountAssignmentRecord {
    pub instance_arn: String,
    pub account_id: String,
    pub target_type: String,
    pub permission_set_arn: String,
    pub principal_type: String,
    pub principal_id: String,
    pub created_date: DateTime,
    /// The `RequestId` of the operation that granted this assignment, so a repeat of
    /// that create answers from the SAME stored status record instead of writing a
    /// second one for work that was already done.
    pub request_id: String,
}

impl AccountAssignmentRecord {
    /// The record's sort/pagination key within one (account, permission set) filter:
    /// the part of its identity the filter does not already fix.
    pub fn page_key(&self) -> String {
        format!("{}#{}", self.principal_type, self.principal_id)
    }

    /// This record as the modeled `AccountAssignment` shape.
    pub fn to_shape(&self) -> AccountAssignment {
        AccountAssignment {
            account_id: Some(self.account_id.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            principal_type: Some(self.principal_type.clone()),
            principal_id: Some(self.principal_id.clone()),
        }
    }
}

/// The status record of one create/delete assignment operation, keyed by its
/// `RequestId` so `DescribeAccountAssignment*Status` and
/// `ListAccountAssignment*Status` read real history rather than a synthesised answer.
#[derive(Debug, Clone)]
pub struct AssignmentOperationRecord {
    pub request_id: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub target_id: String,
    pub target_type: String,
    pub permission_set_arn: String,
    pub principal_type: String,
    pub principal_id: String,
    pub created_date: DateTime,
}

impl AssignmentOperationRecord {
    /// This record as the modeled `AccountAssignmentOperationStatus` shape.
    pub fn to_shape(&self) -> AccountAssignmentOperationStatus {
        AccountAssignmentOperationStatus {
            status: Some(self.status.clone()),
            request_id: Some(self.request_id.clone()),
            failure_reason: self.failure_reason.clone(),
            target_id: Some(self.target_id.clone()),
            target_type: Some(self.target_type.clone()),
            permission_set_arn: Some(self.permission_set_arn.clone()),
            principal_type: Some(self.principal_type.clone()),
            principal_id: Some(self.principal_id.clone()),
            created_date: Some(self.created_date),
        }
    }
}

/// Per-service state for com-amazonaws-ssoadmin-swbexternalservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSsoadminSwbexternalserviceState {
    /// Instances by `InstanceArn`. Seeded with the one ACTIVE instance; an ARN absent
    /// from this map is the `ResourceNotFoundException` every instance-taking
    /// operation answers with.
    pub instances: BTreeMap<String, Instance>,
    /// Permission sets by `PermissionSetArn`. A `BTreeMap` so `ListPermissionSets`
    /// enumerates in one stable order and can page by key. Holds at most
    /// [`MAX_RECORDS_PER_STORE`] records; a create that would cross that ceiling is
    /// refused rather than grown into.
    pub permission_sets: BTreeMap<String, PermissionSetRecord>,
    /// Account assignments. At most one row per identity tuple, and at most
    /// [`MAX_RECORDS_PER_STORE`] rows in all.
    pub account_assignments: Vec<AccountAssignmentRecord>,
    /// Trusted token issuers by `TrustedTokenIssuerArn`. A `BTreeMap` so an
    /// enumeration walks one stable order and can page by key. Holds at most
    /// [`MAX_RECORDS_PER_STORE`] records.
    pub trusted_token_issuers: BTreeMap<String, TrustedTokenIssuerRecord>,
    /// Assignment operation statuses by `RequestId`: one per assignment currently
    /// granted, so a repeated create reads the record it already wrote instead of
    /// adding another, and a delete that removes an assignment removes its status
    /// record with it. Holds at most [`MAX_RECORDS_PER_STORE`] records.
    pub assignment_operations: BTreeMap<String, AssignmentOperationRecord>,
}

impl Default for ComAmazonawsSsoadminSwbexternalserviceState {
    /// Seeds the one ACTIVE instance, so the very first `ListInstances` of a process
    /// reports a usable target and no handler has to write on a read to create one.
    fn default() -> Self {
        let created_date = DateTime::from(SystemTime::now());
        let instance = Instance {
            instance_arn: instance_arn(SEED_INSTANCE_ID),
            identity_store_id: SEED_IDENTITY_STORE_ID.to_string(),
            identity_store_arn: format!(
                "arn:aws:identitystore::{}:identitystore/{SEED_IDENTITY_STORE_ID}",
                ::engine::emulator_account_id()
            ),
            owner_account_id: ::engine::emulator_account_id().to_string(),
            name: SEED_INSTANCE_NAME.to_string(),
            created_date,
            status: STATUS_ACTIVE.to_string(),
            primary_region: SEED_REGION.to_string(),
            regions: vec![Region {
                region_name: SEED_REGION.to_string(),
                status: STATUS_ACTIVE.to_string(),
                added_date: created_date,
                is_primary_region: true,
            }],
            // The seeded instance is the one `CreatePermissionSet` /
            // `ListPermissionSets` / `CreateAccountAssignment` operate against, so
            // permission sets ARE enabled on it.
            permission_sets_enabled: SEED_PERMISSION_SETS_ENABLED,
        };
        let mut instances = BTreeMap::new();
        instances.insert(instance.instance_arn.clone(), instance);
        Self {
            instances,
            permission_sets: BTreeMap::new(),
            account_assignments: Vec::new(),
            trusted_token_issuers: BTreeMap::new(),
            assignment_operations: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsSsoadminSwbexternalserviceState {
    /// The instance `instance_arn` names, or `None` when no such instance exists.
    pub fn instance(&self, instance_arn: &str) -> Option<&Instance> {
        self.instances.get(instance_arn)
    }

    /// The permission set `permission_set_arn` names WITHIN `instance_arn`.
    ///
    /// A permission set belongs to exactly one instance, so an ARN that exists under a
    /// different instance is as absent here as one that was never created.
    pub fn permission_set(
        &self,
        instance_arn: &str,
        permission_set_arn: &str,
    ) -> Option<&PermissionSetRecord> {
        self.permission_sets
            .get(permission_set_arn)
            .filter(|record| record.instance_arn == instance_arn)
    }

    /// Whether `name` is already taken by a permission set of `instance_arn`.
    pub fn permission_set_name_taken(&self, instance_arn: &str, name: &str) -> bool {
        self.permission_sets
            .values()
            .any(|record| record.instance_arn == instance_arn && record.name == name)
    }

    /// The trusted token issuer `trusted_token_issuer_arn` names, or `None` when no
    /// such issuer exists.
    pub fn trusted_token_issuer(
        &self,
        trusted_token_issuer_arn: &str,
    ) -> Option<&TrustedTokenIssuerRecord> {
        self.trusted_token_issuers.get(trusted_token_issuer_arn)
    }

    /// The trusted token issuer a previous `CreateTrustedTokenIssuer` minted under
    /// `client_token`, if one still exists.
    pub fn trusted_token_issuer_for_client_token(
        &self,
        client_token: &str,
    ) -> Option<&TrustedTokenIssuerRecord> {
        self.trusted_token_issuers
            .values()
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }

    /// The index of the assignment with this identity tuple, if it exists.
    pub fn assignment_position(
        &self,
        instance_arn: &str,
        account_id: &str,
        permission_set_arn: &str,
        principal_type: &str,
        principal_id: &str,
    ) -> Option<usize> {
        self.account_assignments.iter().position(|row| {
            row.instance_arn == instance_arn
                && row.account_id == account_id
                && row.permission_set_arn == permission_set_arn
                && row.principal_type == principal_type
                && row.principal_id == principal_id
        })
    }
}

/// The ARN of the instance whose id is `instance_id`.
///
/// `arn:aws:sso:::instance/<id>` — an IAM Identity Center instance ARN carries NO
/// region and NO account segment (the model's pattern ends the prefix at `sso:::`),
/// which is why it is not built through `engine::emulator_arn`; the owning account is
/// reported separately as `InstanceMetadata$OwnerAccountId`.
pub fn instance_arn(instance_id: &str) -> String {
    format!("arn:aws:sso:::instance/{instance_id}")
}

/// Mint a fresh `PermissionSetArn` under `instance_arn`.
///
/// `arn:<partition>:sso:::permissionSet/<instance id>/ps-<16 chars>`, per the model's
/// `PermissionSetArn` pattern — the partition and the instance id are carried over
/// from the owning instance's ARN, and only the `ps-` suffix is minted.
pub fn mint_permission_set_arn(instance_arn: &str) -> String {
    let partition = instance_arn.split(':').nth(1).unwrap_or("aws");
    let instance_id = instance_arn.rsplit('/').next().unwrap_or(SEED_INSTANCE_ID);
    // 16 hex digits, inside `ps-[a-zA-Z0-9-./]{16}` and unique per call so two creates
    // never collide.
    let suffix = &mint_digest()[..16];
    format!("arn:{partition}:sso:::permissionSet/{instance_id}/ps-{suffix}")
}

/// Mint a fresh `TrustedTokenIssuerArn` under `instance_arn`.
///
/// `arn:<partition>:sso::<account>:trustedTokenIssuer/<instance id>/tti-<uuid>`, per the
/// model's `TrustedTokenIssuerArn` pattern — unlike an instance or permission-set ARN,
/// this one carries the owning ACCOUNT (`:sso::\d{12}:`) and no region, so it is built
/// here rather than through `engine::emulator_arn`. The partition and instance id come
/// from the owning instance's ARN; only the `tti-` UUID is minted.
pub fn mint_trusted_token_issuer_arn(instance_arn: &str) -> String {
    let partition = instance_arn.split(':').nth(1).unwrap_or("aws");
    let instance_id = instance_arn.rsplit('/').next().unwrap_or(SEED_INSTANCE_ID);
    format!(
        "arn:{partition}:sso::{}:trustedTokenIssuer/{instance_id}/tti-{}",
        ::engine::emulator_account_id(),
        mint_uuid_v4(),
    )
}

/// A stable digest of every parameter a `@idempotencyToken`-carrying
/// `CreateTrustedTokenIssuer` is scoped to.
///
/// It is what tells a genuine retry (the same `ClientToken` AND the same request) from
/// the same token carrying a different request, which is a conflict rather than a
/// replay. `Tags` go in SORTED, because a tag set carries no order the caller chose.
pub fn trusted_token_issuer_fingerprint(
    instance_arn: &str,
    name: &str,
    trusted_token_issuer_type: &str,
    configuration: &TrustedTokenIssuerConfiguration,
    tags: &[Tag],
) -> String {
    let mut parts = vec![
        instance_arn.to_string(),
        name.to_string(),
        trusted_token_issuer_type.to_string(),
    ];
    match configuration {
        TrustedTokenIssuerConfiguration::OidcJwtConfiguration(oidc) => {
            parts.push("OidcJwtConfiguration".to_string());
            parts.push(oidc.issuer_url.clone());
            parts.push(oidc.claim_attribute_path.clone());
            parts.push(oidc.identity_store_attribute_path.clone());
            parts.push(oidc.jwks_retrieval_option.clone());
        }
    }
    let mut sorted: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    sorted.sort_unstable();
    for (key, value) in sorted {
        parts.push(key.to_string());
        parts.push(value.to_string());
    }
    request_fingerprint(&parts)
}

/// A stable digest of a request's parts, each LENGTH-PREFIXED so `["a", "bc"]` and
/// `["ab", "c"]` cannot collide.
pub fn request_fingerprint<S: AsRef<str>>(parts: &[S]) -> String {
    use ::sha2::Digest as _;
    let mut hasher = ::sha2::Sha256::new();
    for part in parts {
        let part = part.as_ref();
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    ::hex::encode(hasher.finalize())
}

/// A fresh `RequestId`: the hyphenated lowercase UUID the model's `UUId` shape
/// requires (`length 36..36`, `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`),
/// laid out as a version-4 UUID (the `4` in the third group, and the RFC 4122 variant
/// nibble `8`..`b` opening the fourth).
pub fn mint_request_id() -> String {
    mint_uuid_v4()
}

/// A fresh hyphenated lowercase version-4 UUID: `[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-
/// [89ab][0-9a-f]{3}-[0-9a-f]{12}` (the `4` names the version and the RFC 4122 variant
/// nibble `8`..`b` opens the fourth group). It is the form the model's `UUId` shape and
/// the `tti-` segment of `TrustedTokenIssuerArn` both require.
fn mint_uuid_v4() -> String {
    let digest = mint_digest();
    let variant = match u8::from_str_radix(&digest[30..31], 16).unwrap_or_default() % 4 {
        0 => '8',
        1 => '9',
        2 => 'a',
        _ => 'b',
    };
    format!(
        "{}-{}-4{}-{variant}{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..15],
        &digest[15..18],
        &digest[18..30],
    )
}

/// Per-process counter behind every minted identifier, so two mints in one process
/// differ whatever the clock reports.
static MINT_SEQUENCE: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

/// 64 lowercase hex digits unique to this mint.
///
/// SHA-256 of this process's id, the wall clock in nanoseconds and [`MINT_SEQUENCE`]:
/// the counter separates two mints within one process, the pid and clock separate two
/// processes, so no identifier built from it repeats. Nothing here comes from the
/// request, so a caller can neither predict an id nor steer it into another's.
fn mint_digest() -> String {
    use ::sha2::Digest as _;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(::std::process::id().to_le_bytes());
    hasher.update(
        SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    hasher.update(
        MINT_SEQUENCE
            .fetch_add(1, ::std::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
    ::hex::encode(hasher.finalize())
}

/// Cursor scope of `ListInstances`' pagination token. The enumeration takes no
/// filter, so its scope is the operation alone.
pub const CURSOR_INSTANCES: &str = "instances";

/// Cursor scope of a `ListPermissionSets` enumeration: the operation and the instance
/// whose permission sets it walks.
pub fn cursor_permission_sets(instance_arn: &str) -> String {
    format!("permissionSets|{instance_arn}")
}

/// Cursor scope of a `ListAccountAssignments` enumeration: the operation and the whole
/// filter that selected the rows, since the key only distinguishes rows WITHIN it.
pub fn cursor_account_assignments(
    instance_arn: &str,
    account_id: &str,
    permission_set_arn: &str,
) -> String {
    format!("accountAssignments|{instance_arn}|{account_id}|{permission_set_arn}")
}

/// The refusal a `NextToken` this service did not issue earns, as the message of the
/// caller's own `ValidationException`.
pub const BAD_CURSOR_MESSAGE: &str =
    "NextToken is not a pagination token issued by this service for this enumeration";

/// Encode a pagination cursor: the SCOPE of the enumeration it belongs to (the
/// operation and the filter that selected the rows) and the key of the last item
/// returned on this page.
///
/// The cursor is SELF-IDENTIFYING — it carries a version marker and that scope, so a
/// token this service never issued (including one that happens to be valid hex) and a
/// token issued for a DIFFERENT scope (another operation, another instance, another
/// account or permission-set filter) are both rejected rather than silently resuming
/// inside an enumeration whose keys mean something else. Hex keeps the token inside
/// `Token`'s `^[-a-zA-Z0-9+=/_]*$` pattern even though the scope and the keys (ARNs)
/// contain characters it rejects, and it is reversible, so the next page resumes from
/// the key rather than from a positional offset that a concurrent create would shift.
pub fn encode_page_token(scope: &str, key: &str) -> String {
    ::hex::encode(format!("ssoadmin:v1:{scope}:{key}"))
}

/// Decode a cursor [`encode_page_token`] issued for `scope`; `None` for anything else.
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("ssoadmin:v1:{scope}:"))
        .map(str::to_string)
}

/// The key a paginated read resumes after, from the caller's `NextToken`.
///
/// `Ok(None)` starts at the beginning (no token). `Err` is the refusal the caller's own
/// `ValidationException` carries: answering an empty page for a token this service did
/// not issue would silently end a caller's enumeration early.
pub fn cursor_after(
    scope: &str,
    next_token: Option<&str>,
) -> Result<Option<String>, &'static str> {
    match next_token {
        None => Ok(None),
        Some(token) => decode_page_token(scope, token)
            .map(Some)
            .ok_or(BAD_CURSOR_MESSAGE),
    }
}

/// One page of a key-ordered sequence.
///
/// `ordered` is ascending by key; `after` is the key the previous page ended on, so
/// this page is the items whose key sorts strictly after it. The returned token is
/// `Some` only when at least one item was left over — the last page carries none.
pub fn page<T>(
    scope: &str,
    ordered: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<T>, Option<String>) {
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let mut items = Vec::new();
    let mut last_key: Option<String> = None;
    let mut truncated = false;
    for (key, item) in ordered {
        if after.is_some_and(|cursor| key.as_str() <= cursor) {
            continue;
        }
        if items.len() == limit {
            truncated = true;
            break;
        }
        last_key = Some(key);
        items.push(item);
    }
    let next_token = if truncated {
        last_key.as_deref().map(|key| encode_page_token(scope, key))
    } else {
        None
    };
    (items, next_token)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsoadminSwbexternalserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsoadminSwbexternalserviceState>)
}
