// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3control-awss3controlservicev20180820 service.
//!
//! Besides the state struct itself this module holds the derivations every
//! handler shares — the access point ARN, alias, network origin and
//! `ListAccessPoints` page token, the Storage Lens configuration ARN, and the
//! refusals both resource lifecycles answer — so the handlers under `src/ops/`
//! compute each one way instead of once per handler.

use crate::ops::types::{
    AccessPoint, PublicAccessBlockConfiguration, StorageLensConfiguration, StorageLensTag, Tag,
    VpcConfiguration, NETWORK_ORIGIN_VALUES,
};
use std::collections::BTreeMap;

/// One stored access point: everything `GetAccessPoint`, `ListAccessPoints` and
/// `GetAccessPointPolicy` report, recorded by `CreateAccessPoint`.
#[derive(Debug, Clone)]
pub struct AccessPointRecord {
    pub name: String,
    /// The account that owns the access point — the first half of the key, kept
    /// on the record so a record passed on its own still names its owner.
    pub account_id: String,
    pub bucket: String,
    pub bucket_account_id: Option<String>,
    pub arn: String,
    pub alias: String,
    /// One of [`NETWORK_ORIGIN_VALUES`] — `VPC` when the access point is
    /// VPC-restricted, otherwise `Internet`.
    pub network_origin: String,
    pub vpc_configuration: Option<VpcConfiguration>,
    pub public_access_block_configuration: Option<PublicAccessBlockConfiguration>,
    pub creation_date: ::aws_smithy_types::DateTime,
    /// The tags `CreateAccessPoint` was given. Recorded here because they are
    /// part of the resource; the operations that report them
    /// (`ListTagsForResource`, `TagResource`, `UntagResource`) are still stubs
    /// with their own gap issues, and will read this field rather than a second
    /// copy.
    pub tags: Vec<Tag>,
    /// The document the last `PutAccessPointPolicy` stored, byte for byte;
    /// `None` until one is put, which is what `GetAccessPointPolicy` reports as
    /// `NoSuchAccessPointPolicy`.
    pub policy: Option<String>,
}

impl AccessPointRecord {
    /// The record as the `AccessPoint` summary shape `ListAccessPoints` returns.
    pub fn summary(&self) -> AccessPoint {
        AccessPoint {
            name: self.name.clone(),
            network_origin: self.network_origin.clone(),
            vpc_configuration: self.vpc_configuration.clone(),
            bucket: self.bucket.clone(),
            access_point_arn: Some(self.arn.clone()),
            alias: Some(self.alias.clone()),
            bucket_account_id: self.bucket_account_id.clone(),
            // S3 on Outposts / FSx data sources are not emulated, so a record
            // created here is attached to no data source.
            data_source_id: None,
            data_source_type: None,
        }
    }
}

/// How many access points the store holds before `CreateAccessPoint` refuses
/// with `TooManyAccessPoints`.
///
/// S3 Control publishes an adjustable quota on access points per account per
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an emulator
/// serves, and it is what stops the state growing without end: every record it
/// holds is released by `DeleteAccessPoint`, which drops that access point's
/// policy with it, so together with [`MAX_POLICY_BYTES`] the bytes this store
/// can hold are bounded.
pub const ACCESS_POINT_QUOTA: usize = 1_000;

/// The largest policy document `PutAccessPointPolicy` accepts and retains, in
/// bytes.
///
/// Amazon S3 documents a 20 KB maximum for a bucket or access point policy, so a
/// document past it is one the real service would not store either. It is also
/// what bounds the bytes a single record can hold.
pub const MAX_POLICY_BYTES: usize = 20 * 1024;

/// How a stored access point is keyed: the owning account id and the access
/// point name, which is the pair a name is unique within.
pub type AccessPointKey = (String, String);

/// One stored S3 Storage Lens configuration: the document the last
/// `PutStorageLensConfiguration` stored, and the tag set that configuration
/// carries.
#[derive(Debug, Clone)]
pub struct StorageLensConfigurationRecord {
    /// The configuration document as the last put left it — stored WHOLE, never
    /// merged with the document before it, with `StorageLensArn` set to the ARN
    /// this service assigns (that member is documented read-only).
    pub configuration: StorageLensConfiguration,
    /// The configuration's tag set.
    ///
    /// Held BESIDE the document rather than inside it, because the two are
    /// separate resources on the wire: `GetStorageLensConfiguration` reports no
    /// tags at all, and `GetStorageLensConfigurationTagging` /
    /// `PutStorageLensConfigurationTagging` are what report and replace them.
    pub tags: Vec<StorageLensTag>,
}

/// How a stored S3 Storage Lens configuration is keyed: the owning account id
/// and the configuration id, which is the pair an id is unique within.
pub type StorageLensConfigurationKey = (String, String);

/// How many S3 Storage Lens configurations the store holds before
/// `PutStorageLensConfiguration` refuses to add another.
///
/// S3 Storage Lens limits how many configurations an account may hold in a
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an
/// emulator serves, and together with [`MAX_STORAGE_LENS_TAGS`] it is what
/// stops this part of the state growing without end. (The record a put adds is
/// released by `DeleteStorageLensConfiguration`, which is still a scaffold with
/// its own gap issue; until it is written the bound is the only limit.)
pub const STORAGE_LENS_CONFIGURATION_QUOTA: usize = 1_000;

/// The largest tag set an S3 Storage Lens configuration carries.
///
/// The model documents it on both members that carry a tag set
/// (`PutStorageLensConfigurationRequest$Tags` and
/// `PutStorageLensConfigurationTaggingRequest$Tags`): "You can set up to a
/// maximum of 50 tags."
pub const MAX_STORAGE_LENS_TAGS: usize = 50;

/// Per-service state for com-amazonaws-s3control-awss3controlservicev20180820. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsS3controlAwss3controlservicev20180820State {
    /// Access points by (account id, name).
    ///
    /// A `BTreeMap` rather than a `HashMap` so the one scan order every reader
    /// sees is sorted by that key: `ListAccessPoints` is `@paginated`, and a
    /// page token can only resume a scan whose order does not change between
    /// calls.
    ///
    /// BOUNDED: `CreateAccessPoint` refuses past [`ACCESS_POINT_QUOTA`] records
    /// and `PutAccessPointPolicy` refuses a document past
    /// [`MAX_POLICY_BYTES`], so neither the number of records nor the size of
    /// one is caller-chosen without limit. `DeleteAccessPoint` releases a
    /// record and its policy.
    pub access_points: BTreeMap<AccessPointKey, AccessPointRecord>,

    /// S3 Storage Lens configurations by (account id, configuration id).
    ///
    /// A `BTreeMap` for the same reason as `access_points`:
    /// `ListStorageLensConfigurations` is `@paginated`, and a page token can
    /// only resume a scan whose order does not change between calls.
    ///
    /// BOUNDED: `PutStorageLensConfiguration` refuses a NEW configuration past
    /// [`STORAGE_LENS_CONFIGURATION_QUOTA`] records, and both puts that carry a
    /// tag set refuse one past [`MAX_STORAGE_LENS_TAGS`] tags; every member of
    /// the document itself is length-bounded by the model.
    pub storage_lens_configurations:
        BTreeMap<StorageLensConfigurationKey, StorageLensConfigurationRecord>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3controlAwss3controlservicev20180820State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3controlAwss3controlservicev20180820State>)
}

/// The region this request was signed for, which is the region the resources it
/// touches live in. Falls back to the AWS SDK default region when the request
/// carries no SigV4 credential scope (authentication may be off).
pub fn signing_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request is answered in — the region the emulator's own
/// generated credentials file names (`vendor/server/src/lib.rs`).
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN of an access point: the `accesspoint/<name>` resource in this
/// service's ARN namespace (`s3`, per the model's `awsService.arnNamespace`).
///
/// The account segment comes from [`::emulator_server::emulator_arn`], the single
/// source of truth for the canonical emulator account — a generated emulator
/// never puts another account id in an ARN it emits.
pub fn access_point_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("accesspoint/{name}"))
}

/// The ARN of an S3 Storage Lens configuration.
///
/// `StorageLensConfiguration$StorageLensArn` documents the member as read-only
/// and states the format it follows:
/// `arn:aws:s3:<region>:<account-id>:storage-lens/<config-id>` — so the service
/// assigns it from the configuration's own identity rather than taking whatever
/// a request submitted. The account segment comes from
/// [`::emulator_server::emulator_arn`], the single source of truth for the
/// canonical emulator account.
pub fn storage_lens_configuration_arn(region: &str, config_id: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("storage-lens/{config_id}"))
}

/// The suffix length AWS access point aliases carry between the name and the
/// `-s3alias` tail: a 26-character, service-assigned string.
const ALIAS_SUFFIX_LEN: usize = 26;

/// The tail every S3 access point alias ends in.
const ALIAS_TAIL: &str = "-s3alias";

/// `Alias`'s modelled maximum length (`com.amazonaws.s3control#Alias`, len 0..63).
const ALIAS_MAX_LEN: usize = 63;

/// The alias of an access point: `<name>-<26 service-assigned characters>-s3alias`,
/// the form S3 reports for an access point alias.
///
/// The middle segment is derived from the access point's identity (its owning
/// account and name) rather than drawn at random, so it is one value per access
/// point: stable across every later read, and different for a different access
/// point. The name segment is truncated when the whole alias would pass the
/// modelled 63-character maximum.
pub fn access_point_alias(account_id: &str, name: &str) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(format!("{account_id}/{name}").as_bytes());
    let suffix: String = hex::encode(digest).chars().take(ALIAS_SUFFIX_LEN).collect();

    let room_for_name = ALIAS_MAX_LEN - ALIAS_TAIL.len() - ALIAS_SUFFIX_LEN - 1;
    let lowered = name.to_lowercase();
    let head: String = lowered.chars().take(room_for_name).collect();
    format!("{head}-{suffix}{ALIAS_TAIL}")
}

/// The network origin of an access point: `VPC` when the create call restricted
/// it to a VPC, otherwise `Internet` — the rule the `AccessPoint$NetworkOrigin`
/// documentation states.
pub fn network_origin(vpc_configuration: Option<&VpcConfiguration>) -> String {
    let vpc_restricted = vpc_configuration.is_some_and(|vpc| !vpc.vpc_id.is_empty());
    let wanted = if vpc_restricted { "VPC" } else { "Internet" };
    // The spelling that goes on the wire is the one the generated
    // NETWORK_ORIGIN_VALUES carries (from the model's `NetworkOrigin` enum),
    // found by VALUE rather than by position: neither a reordering of the
    // generated list nor a typo in the name above can change what is returned.
    NETWORK_ORIGIN_VALUES
        .iter()
        .copied()
        .find(|value| *value == wanted)
        .unwrap_or(wanted)
        .to_string()
}

/// Encode a `ListAccessPoints` page token: the opaque form of the last access
/// point name the previous page returned.
///
/// Encoded rather than passed through so a caller does not come to depend on a
/// token's contents, the way an AWS page token's contents are not part of its
/// contract. The encoding is reversible and is NOT a secret or an access check:
/// a token only names a position in the account's own scan.
pub fn encode_page_token(last_name: &str) -> String {
    hex::encode(last_name.as_bytes())
}

/// Decode a page token produced by [`encode_page_token`], or `None` when it was
/// not produced by it.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

// ---------------------------------------------------------------------------
// The refusals the access point lifecycle shares.
//
// S3 Control's model declares no errors for these operations, so each is built
// through `Scaffolded::service_failure`, which carries the wire `<Code>` and the
// HTTP status a client branches on. They live here, once, so four handlers
// cannot drift on a code.
// ---------------------------------------------------------------------------

/// `NoSuchAccessPoint` (HTTP 404): no access point of that name is owned by the
/// account the request names.
pub fn no_such_access_point<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NoSuchAccessPoint",
        404,
        format!("The specified access point does not exist: {name}"),
    )
}

/// `NoSuchAccessPointPolicy` (HTTP 404): the access point exists but carries no
/// policy, because none was ever put on it (or it was deleted).
pub fn no_such_access_point_policy<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NoSuchAccessPointPolicy",
        404,
        format!("The specified access point does not have a policy: {name}"),
    )
}

/// `AccessPointAlreadyOwnedByYou` (HTTP 409): the account already owns an access
/// point of that name, so creating it again would be a second resource under one
/// name.
pub fn access_point_already_owned_by_you<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessPointAlreadyOwnedByYou",
        409,
        format!("An access point with the same name already exists: {name}"),
    )
}

/// `TooManyAccessPoints` (HTTP 400): the store already holds
/// [`ACCESS_POINT_QUOTA`] access points, so another record would grow it without
/// end. Released by `DeleteAccessPoint`.
pub fn too_many_access_points<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyAccessPoints",
        400,
        format!("This emulator holds the maximum of {ACCESS_POINT_QUOTA} access points."),
    )
}

/// `MalformedPolicy` (HTTP 400): the policy document is not one this service
/// will store — it is not JSON, or it is larger than [`MAX_POLICY_BYTES`].
pub fn malformed_policy<E>(why: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "MalformedPolicy",
        400,
        format!("The specified policy is not a policy this service can store: {why}"),
    )
}

/// `NoSuchConfiguration` (HTTP 404): no S3 Storage Lens configuration of that
/// id is owned by the account the request names.
///
/// S3 Control declares no error for the Storage Lens operations, so — exactly
/// as for the access point refusals above — the refusal is carried in the
/// S3-style XML `<Code>`/`<Message>` envelope with the code and status the real
/// service answers a missing configuration with.
pub fn no_such_storage_lens_configuration<E>(config_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NoSuchConfiguration",
        404,
        format!("The specified configuration does not exist: {config_id}"),
    )
}

/// `InvalidRequestException` (HTTP 400): a `PutStorageLensConfiguration` whose
/// URI names one configuration id and whose document names another.
///
/// Both members are documented as the id OF the S3 Storage Lens configuration
/// (`PutStorageLensConfigurationRequest$ConfigId`, `StorageLensConfiguration$Id`),
/// so a request carrying two different ids names no one configuration. The code
/// and status are this service's own modelled `InvalidRequestException`
/// (`com.amazonaws.s3control#InvalidRequestException`, a 400 client error),
/// which this operation's model does not list.
pub fn storage_lens_configuration_id_mismatch<E>(
    config_id: &str,
    document_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InvalidRequestException",
        400,
        format!(
            "ConfigId is {config_id} but the submitted StorageLensConfiguration's Id is \
             {document_id}; they must name the same configuration."
        ),
    )
}

/// `TooManyTagsException` (HTTP 400): the request carries more than
/// [`MAX_STORAGE_LENS_TAGS`] tags, the maximum the model documents for a
/// Storage Lens configuration's tag set.
///
/// The code and status are this service's own modelled `TooManyTagsException`
/// ("Amazon S3 throws this exception if you have too many tags in your tag
/// set", a 400 client error), which these operations' models do not list.
pub fn too_many_storage_lens_tags<E>(submitted: usize) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyTagsException",
        400,
        format!(
            "A Storage Lens configuration carries at most {MAX_STORAGE_LENS_TAGS} tags; \
             the request carries {submitted}."
        ),
    )
}

/// `TooManyConfigurations` (HTTP 400): the store already holds
/// [`STORAGE_LENS_CONFIGURATION_QUOTA`] Storage Lens configurations, so another
/// record would grow it without end. Replacing an existing configuration is
/// never refused by this bound: it adds no record.
pub fn too_many_storage_lens_configurations<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyConfigurations",
        400,
        format!(
            "This emulator holds the maximum of {STORAGE_LENS_CONFIGURATION_QUOTA} \
             Storage Lens configurations."
        ),
    )
}

/// `InternalError` (HTTP 500): [`service_state`] answered `None`, so this
/// process has no shared store to read. Never answered with a fresh engine — a
/// private store would accept writes nobody else can see.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InternalError",
        500,
        "The emulator's shared state is not reachable from this request.",
    )
}
