// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lakeformation-awslakeformation service.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// One registered data-lake location: the resource ARN Lake Formation manages, the IAM
/// role it uses to reach it, and the registration flags the caller set.
///
/// `role_arn` is stored as a plain `String` because a registration always HAS a data
/// access role — `RegisterResource` refuses a request that names neither `RoleArn` nor
/// `UseServiceLinkedRole` ("If UseServiceLinkedRole is not set to true, you must provide
/// or set the RoleArn" — `RegisterResource`'s own documentation), and
/// `UseServiceLinkedRole` resolves to [`service_linked_role_arn`].
///
/// The three nullable flags keep the `Option` the model gives them (`NullableBoolean`),
/// so a flag the caller never set is reported absent rather than invented as `false`.
#[derive(Debug, Clone)]
pub struct RegisteredResource {
    /// The ARN this registration is keyed by; echoed as `ResourceInfo$ResourceArn`.
    pub resource_arn: String,
    /// The IAM role that registered the resource (`ResourceInfo$RoleArn`).
    pub role_arn: String,
    /// `ResourceInfo$WithFederation`, as set by the last register/update.
    pub with_federation: Option<bool>,
    /// `ResourceInfo$HybridAccessEnabled`, as set by the last register/update.
    pub hybrid_access_enabled: Option<bool>,
    /// `ResourceInfo$WithPrivilegedAccess`. `RegisterResource`'s input member is
    /// modelled `Boolean` with `default false`, so a registration always has a value.
    pub with_privileged_access: Option<bool>,
    /// `ResourceInfo$ExpectedResourceOwnerAccount`, when the caller named one.
    pub expected_resource_owner_account: Option<String>,
    /// When this registration last changed — reported as `ResourceInfo$LastModified`.
    pub last_modified: SystemTime,
}

impl RegisteredResource {
    /// This registration as the modelled `ResourceInfo` structure, which is what
    /// `DescribeResource` (and, once implemented, `ListResources`) answers with.
    pub fn to_resource_info(&self) -> crate::ops::types::ResourceInfo {
        crate::ops::types::ResourceInfo {
            resource_arn: Some(self.resource_arn.clone()),
            role_arn: Some(self.role_arn.clone()),
            last_modified: Some(::aws_smithy_types::DateTime::from(self.last_modified)),
            with_federation: self.with_federation,
            hybrid_access_enabled: self.hybrid_access_enabled,
            with_privileged_access: self.with_privileged_access,
            // `VerificationStatus` reports whether the registered ROLE can actually
            // reach the location ("VERIFIED - Registered role has sufficient
            // permissions …" — `ResourceInfo$VerificationStatus`). This emulator runs
            // no IAM evaluation and touches no S3 location, so nothing performed that
            // check and the optional member is left absent rather than carrying a
            // status for work that never ran.
            verification_status: None,
            expected_resource_owner_account: self.expected_resource_owner_account.clone(),
        }
    }
}

/// One stored LF-tag definition: the Data Catalog it lives in, its key, and the ordered
/// set of values the key may take.
///
/// `tag_values` is kept ordered and de-duplicated and is never empty — `TagValueList` is
/// modelled `len 1..50` (`smithy_query(query="shape",shape="TagValueList")`), so a tag
/// whose values were all removed could not be rendered back on the wire.
#[derive(Debug, Clone, Default)]
pub struct LfTag {
    /// The catalog this tag belongs to; echoed by `GetLFTag` / `ListLFTags`.
    pub catalog_id: String,
    /// The stored key-name (`LFTagKey`, len 1..128).
    pub tag_key: String,
    /// The values the key may take: ordered, de-duplicated, 1..50 entries.
    pub tag_values: Vec<String>,
}

/// Per-service state for com-amazonaws-lakeformation-awslakeformation. A handler reaches it through [`service_state`].
///
/// # What stops this growing without end
///
/// An LF-tag is a durable control-plane record: AWS gives it no expiry, so this store
/// has no TTL and no eviction, and a tag is released only by `DeleteLFTag` (which also
/// drops a catalog left empty). What bounds it instead are three ceilings checked by
/// `CreateLFTag`, the single site that inserts: [`MAX_LF_TAGS_PER_CATALOG`] per catalog,
/// [`MAX_LF_TAG_CATALOGS`] catalogs, and [`MAX_LF_TAGS_TOTAL`] tags across every catalog
/// — the last two are what bound the store when the caller varies `CatalogId`, since the
/// catalog id is request-supplied. Passing any of them answers the modelled
/// `ResourceNumberLimitExceededException`, never a silent insert.
///
/// `registered_resources` is bounded the same way, by [`MAX_REGISTERED_RESOURCES`] checked
/// in `RegisterResource`, the single site that inserts into it.
#[derive(Debug, Default)]
pub struct ComAmazonawsLakeformationAwslakeformationState {
    /// Catalog id -> (tag key -> tag). Both levels are `BTreeMap`s so enumeration order
    /// is stable and sorted, which is what makes `ListLFTags`' exclusive-start
    /// continuation token resume exactly where the previous page stopped.
    pub lf_tags: BTreeMap<String, BTreeMap<String, LfTag>>,
    /// Resource ARN -> its registration. A `BTreeMap` for the same reason: `ListResources`
    /// pages over a stable, sorted order, and `DescribeResource` is an exact-ARN lookup.
    pub registered_resources: BTreeMap<String, RegisteredResource>,
}

impl ComAmazonawsLakeformationAwslakeformationState {
    /// How many LF-tags the store holds across every catalog — what
    /// [`MAX_LF_TAGS_TOTAL`] bounds.
    pub fn total_lf_tags(&self) -> usize {
        self.lf_tags.values().map(BTreeMap::len).sum()
    }
}

/// The largest page `ListLFTags` can answer, and the page size it uses when the caller
/// names none.
///
/// Grounded in the model, not chosen: the output member `LFTags` targets `LFTagsList`,
/// which is `len 1..50`, so no single response can carry more than 50 LF-tag pairs even
/// though `MaxResults` is modelled `range 1..1000`.
pub const MAX_LF_TAGS_PAGE: i32 = 50;

/// The most values one LF-tag can hold.
///
/// Grounded in the model: `TagValues` targets `TagValueList`, which is `len 1..50`
/// (`smithy_query(query="shape",shape="TagValueList")`), so a tag can neither lose every
/// value nor grow past 50.
pub const MAX_TAG_VALUES: usize = 50;

/// The most LF-tags one Data Catalog can hold here.
///
/// That the number of LF-tags a catalog may hold IS bounded comes from the model:
/// `CreateLFTag` declares `ResourceNumberLimitExceededException` — "A resource numerical
/// limit was exceeded" (HTTP 400). The model does not carry the number, and AWS publishes
/// it as an adjustable service quota, so this ceiling is the emulator's own choice; what
/// the model fixes is that passing it answers that error instead of growing the store
/// without limit.
pub const MAX_LF_TAGS_PER_CATALOG: usize = 1000;

/// The most Data Catalogs this store will hold LF-tags for.
///
/// `CatalogId` is request-supplied, so the per-catalog ceiling alone would let a caller
/// that varies it grow the store without end. This bounds the partition count itself;
/// a create that would open one catalog too many answers the modelled
/// `ResourceNumberLimitExceededException`. Emulator-chosen, like the ceiling above.
pub const MAX_LF_TAG_CATALOGS: usize = 100;

/// The most LF-tags this store will hold across every catalog.
///
/// The store-wide ceiling, so no partitioning of the same number of create calls can
/// grow resident state past it. A create that would pass it answers the modelled
/// `ResourceNumberLimitExceededException`. Emulator-chosen, like the ceilings above.
pub const MAX_LF_TAGS_TOTAL: usize = 10_000;

/// The most data-lake locations this store will hold registrations for.
///
/// That the number of registered locations IS bounded comes from the model:
/// `RegisterResource` declares `ResourceNumberLimitExceededException` — "A resource
/// numerical limit was exceeded" (HTTP 400). The model does not carry the number, and AWS
/// publishes it as an adjustable service quota, so this ceiling is the emulator's own
/// choice; what the model fixes is that passing it answers that error instead of growing
/// the store without limit.
pub const MAX_REGISTERED_RESOURCES: usize = 1_000;

/// The Lake Formation service-linked role a registration uses when the request sets
/// `UseServiceLinkedRole` and names no `RoleArn`.
///
/// The role name is the one `RegisterResource`'s documentation names
/// ("AWSServiceRoleForLakeFormationDataAccess"); the ARN is built for this emulator's
/// account, and matches `IAMRoleArn`'s `^arn:aws:iam::[0-9]*:role/` pattern.
pub fn service_linked_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/aws-service-role/lakeformation.amazonaws.com/AWSServiceRoleForLakeFormationDataAccess",
        ::engine::emulator_account_id()
    )
}

/// `Ok(())` when `arn` has the shape of an Amazon Resource Name, else why it does not.
///
/// `ResourceArnString` carries no pattern in the model, so the boundary validator admits
/// any string and the handler is what must refuse a value that is not an ARN at all. The
/// six-field `arn:partition:service:region:account-id:resource` layout is the general ARN
/// format; `region` and `account-id` are empty for an Amazon S3 location
/// (`arn:aws:s3:::my-bucket/`), so only `partition`, `service` and `resource` are required
/// to be non-empty.
pub fn validate_resource_arn(arn: &str) -> Result<(), String> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let malformed = fields.len() < 6
        || fields[0] != "arn"
        || fields[1].is_empty()
        || fields[2].is_empty()
        || fields[5].is_empty();
    if malformed {
        return Err(format!(
            "ResourceArn {arn:?} is not a valid Amazon Resource Name: expected \
             arn:<partition>:<service>:<region>:<account-id>:<resource>"
        ));
    }
    Ok(())
}

/// The Data Catalog a request addresses: its `CatalogId` when it carries one, else the
/// caller's account id ("The identifier for the Data Catalog. By default, the account
/// ID." — `CatalogIdString`'s own documentation).
pub fn resolve_catalog_id(requested: Option<&str>) -> String {
    match requested {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => ::engine::emulator_account_id().to_string(),
    }
}

/// The values of `values`, in first-seen order, with duplicates dropped.
///
/// An LF-tag's values are a SET of the values the key may take, so a repeated value in
/// one request, or a value already stored, must not appear twice in the stored tag.
pub fn dedup_values<'a, I>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a String>,
{
    // A linear scan of the output, not a hash set: `TagValueList` is `len 1..50`, so the
    // input is at most 50 values and first-seen order is what must be preserved.
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if !out.iter().any(|existing| existing == value) {
            out.push(value.clone());
        }
    }
    out
}

/// The field separator inside a continuation token's plaintext.
///
/// `U+001F` (unit separator) cannot appear in either field: `CatalogIdString` is
/// `^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$` and `LFTagKey` is
/// `^([\p{L}\p{Z}\p{N}_.:\/=+\-@%]*)$`, neither of which admits a C0 control other than
/// tab — so splitting on it can never cut a value in half.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// The tag identifying a token this service minted, and the token layout's version.
const TOKEN_PREFIX: &str = "lftags";
const TOKEN_VERSION: &str = "1";

/// An opaque continuation token standing for "resume strictly after this (catalog, key)".
///
/// Hex-wrapped so a caller treats it as opaque and round-trips it unchanged, and
/// self-describing so a token this service did not mint is detectable (see
/// [`decode_page_token`]).
pub fn encode_page_token(catalog_id: &str, tag_key: &str) -> String {
    let plain =
        format!("{TOKEN_PREFIX}{TOKEN_SEPARATOR}{TOKEN_VERSION}{TOKEN_SEPARATOR}{catalog_id}{TOKEN_SEPARATOR}{tag_key}");
    hex::encode(plain)
}

/// The `(catalog id, tag key)` exclusive start a token names, or `None` when the token
/// was not minted by [`encode_page_token`] (so the caller must be refused rather than
/// silently served page one).
pub fn decode_page_token(token: &str) -> Option<(String, String)> {
    let bytes = hex::decode(token).ok()?;
    let plain = String::from_utf8(bytes).ok()?;
    let mut fields = plain.split(TOKEN_SEPARATOR);
    let prefix = fields.next()?;
    let version = fields.next()?;
    let catalog_id = fields.next()?;
    let tag_key = fields.next()?;
    if prefix != TOKEN_PREFIX || version != TOKEN_VERSION || fields.next().is_some() {
        return None;
    }
    Some((catalog_id.to_string(), tag_key.to_string()))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLakeformationAwslakeformationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLakeformationAwslakeformationState>)
}
