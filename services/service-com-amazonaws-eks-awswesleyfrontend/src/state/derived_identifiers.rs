// SPDX-License-Identifier: CC0-1.0

//! The values this service DERIVES rather than stores: the Region a request is
//! signed for, the digests every minted identifier is built from, and the ids and
//! ARNs each resource of this service carries.

use super::*;

/// The region a request is signed for, read from its SigV4 credential scope.
///
/// Every ARN and endpoint this service mints is regional, so the region comes
/// from the caller rather than from a constant. A request with no parseable
/// scope (authentication off, no `Authorization` header) falls back to
/// [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The first 16 bytes of `seed`'s SHA-256 digest, as lowercase hex. The shared
/// step of the identifier this service derives from stable content
/// ([`derive_cluster_resource_id`]); 16 bytes is the width it publishes.
pub(super) fn digest_hex(seed: &str) -> String {
    hex::encode(&Sha256::digest(seed.as_bytes())[..16])
}

/// 16 bytes rendered in the hyphenated `8-4-4-4-12` lowercase-hex layout a UUID
/// takes. The one renderer for every one of this service's UUID-shaped
/// identifiers ([`new_update_id`], [`derive_activation_id`] and
/// [`derive_access_entry_id`]).
pub(super) fn hyphenate_uuid(bytes: &[u8; 16]) -> String {
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A fresh identifier in the shape EKS gives an `Update$id`: "a UUID that is
/// used to track the update" — a version-4 UUID, `8-4-4-4-12` lowercase hex with
/// the version and variant bits RFC 4122 §4.4 fixes.
///
/// `sequence` is a counter the store hands out
/// ([`ComAmazonawsEksAwswesleyfrontendState::next_update_sequence`]), so two ids
/// minted in the same nanosecond cannot collide. An update id names a resource for
/// correlation, never a credential.
pub fn new_update_id(sequence: u64) -> String {
    new_uuid_v4(sequence)
}

/// The id of the call this service made to ANOTHER service on the caller's
/// behalf, as it is reported back to that caller.
///
/// A request identifier is generated per call — it is what correlates one answer
/// with the call behind it — so it is drawn afresh here and derived from nothing
/// in the request.
pub fn new_peer_request_id() -> String {
    new_uuid_v4(0)
}

/// A fresh version-4 UUID, `8-4-4-4-12` lowercase hex with the version and
/// variant bits RFC 4122 §4.4 fixes — the one minter behind every identifier this
/// service gives that shape to.
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2`
/// crate) and hex-encoded (the `hex` crate): the OS-seeded randomness a
/// `RandomState` draws, the system clock in nanoseconds, and `sequence`.
pub(super) fn new_uuid_v4(sequence: u64) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    hyphenate_uuid(&bytes)
}

/// The 32-hex-character resource identifier EKS gives a cluster, which appears in
/// the cluster's API server endpoint host and in its OIDC issuer URL.
///
/// Derived from the cluster ARN — which already carries the account, region and
/// name — so it is distinct per cluster and stable for the record's whole life.
/// It is computed once, when the cluster is created, and stored on the record;
/// no read recomputes it.
pub fn derive_cluster_resource_id(arn: &str) -> String {
    digest_hex(arn).to_ascii_uppercase()
}

/// The cluster's Kubernetes API server endpoint, in the host form EKS publishes:
/// `https://<resource id>.<shard>.<region>.eks.amazonaws.com`.
pub fn derive_cluster_endpoint(resource_id: &str, region: &str) -> String {
    let shard = resource_id
        .get(..3)
        .unwrap_or("eks")
        .to_ascii_lowercase();
    format!("https://{resource_id}.{shard}.{region}.eks.amazonaws.com")
}

/// The cluster's OIDC identity provider issuer URL, in the form EKS publishes:
/// `https://oidc.eks.<region>.amazonaws.com/id/<resource id>`.
pub fn derive_oidc_issuer(resource_id: &str, region: &str) -> String {
    format!("https://oidc.eks.{region}.amazonaws.com/id/{resource_id}")
}

/// A fresh 32-hex-character resource identifier, in the shape
/// [`derive_cluster_resource_id`] gives a cluster.
///
/// A cluster's id is derived from its ARN, because the ARN already identifies it
/// uniquely. An identity provider configuration's id is the other way round — it
/// is a part of the ARN — so there is nothing to derive it from and it is minted
/// per call. The bits are [`new_update_id`]'s, which this crate already mints for
/// an `Update$id` from OS-seeded randomness, the clock and the store's own
/// counter; this only drops the hyphens and upper-cases the hex, which is the
/// shape a cluster's resource id has.
pub fn new_resource_id(sequence: u64) -> String {
    new_update_id(sequence).replace('-', "").to_uppercase()
}

/// The ARN of an identity provider configuration, in the form EKS publishes:
/// `arn:aws:eks:<region>:<account>:identityproviderconfig/<cluster>/<type>/<name>/<resource id>`.
///
/// The configuration is a sub-resource of its cluster, so the cluster name, the
/// configuration type and its name are all path components of the resource
/// portion, with the server-minted id last. It is computed once, when the
/// configuration is associated, and stored on the record; no read recomputes it.
pub fn identity_provider_config_arn(
    region: &str,
    cluster_name: &str,
    config_type: &str,
    config_name: &str,
    resource_id: &str,
) -> String {
    ::engine::emulator_arn(
        "eks",
        region,
        &format!("identityproviderconfig/{cluster_name}/{config_type}/{config_name}/{resource_id}"),
    )
}

/// The IPv6 service CIDR a cluster gets when `ipFamily` is `ipv6`.
///
/// The model states the rule: "Kubernetes assigns services `IPv6` addresses from
/// the unique local address range `(fc00::/7)`. You can't specify a custom
/// `IPv6` CIDR block" (`KubernetesNetworkConfigRequest$ipFamily`) — so the block
/// is always server-assigned. It is derived from the cluster's resource id, which
/// puts it inside `fd00::/8`, and stored with the record.
pub fn derive_service_ipv6_cidr(resource_id: &str) -> String {
    let hextet = |range: std::ops::Range<usize>| {
        resource_id
            .get(range)
            .unwrap_or("0000")
            .to_ascii_lowercase()
    };
    format!("fd{}:{}:{}::/108", hextet(0..2), hextet(2..6), hextet(6..10))
}

/// The node group's own identifier, the last segment of its `nodegroupArn`.
///
/// An EKS node group ARN is
/// `arn:aws:eks:<region>:<account>:nodegroup/<cluster>/<nodegroup>/<id>`, where
/// the trailing id distinguishes two node groups that carried the same name at
/// different times. It is derived from the region, the two names and the
/// creation instant — so it is distinct per creation and stable for the
/// renderer for this service's UUID-shaped identifiers, through
/// [`derive_uuid_from_seed`]. It is computed once, when the node group is
/// created, and stored on the record; no read recomputes it.
pub fn derive_nodegroup_id(
    region: &str,
    cluster_name: &str,
    nodegroup_name: &str,
    created_at: SystemTime,
) -> String {
    let created_nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    derive_uuid_from_seed(&format!(
        "{region}/{cluster_name}/{nodegroup_name}/{created_nanos}"
    ))
}

/// The identifier EKS puts in the last segment of an add-on's ARN
/// (`arn:aws:eks:<region>:<account>:addon/<cluster>/<add-on>/<id>`), in the
/// 8-4-4-4-12 hex form AWS prints it in ([`hyphenate_uuid`], the one renderer
/// for this service's UUID-shaped identifiers).
///
/// Derived from the add-on's own key — region, cluster and add-on name — so it
/// is distinct per add-on and stable for the record's whole life. It is computed
/// once, when the add-on is created, and stored on the record; no read
/// recomputes it.
pub fn derive_addon_resource_id(region: &str, cluster_name: &str, addon_name: &str) -> String {
    derive_uuid_from_seed(&format!("{region}/{cluster_name}/{addon_name}"))
}

/// The Fargate profile's own identifier, the last segment of its
/// `fargateProfileArn`.
///
/// An EKS Fargate profile ARN is
/// `arn:aws:eks:<region>:<account>:fargateprofile/<cluster>/<profile>/<id>`,
/// where the trailing id distinguishes two profiles that carried the same name
/// at different times. It is derived from the region, the two names and the
/// creation instant — so it is distinct per creation and stable — through
/// [`derive_uuid_from_seed`], the one renderer for this service's UUID-shaped
/// identifiers. It is computed once, when the profile is created, and stored on
/// the record; no read recomputes it.
pub fn derive_fargate_profile_id(
    region: &str,
    cluster_name: &str,
    fargate_profile_name: &str,
    created_at: SystemTime,
) -> String {
    let created_nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos());
    derive_uuid_from_seed(&format!(
        "{region}/{cluster_name}/{fargate_profile_name}/{created_nanos}"
    ))
}

/// The add-on's ARN, in the form EKS publishes:
/// `arn:aws:eks:<region>:<account>:addon/<cluster>/<add-on>/<resource id>`.
pub fn derive_addon_arn(
    region: &str,
    cluster_name: &str,
    addon_name: &str,
    resource_id: &str,
) -> String {
    ::engine::emulator_arn(
        "eks",
        region,
        &format!("addon/{cluster_name}/{addon_name}/{resource_id}"),
    )
}

/// The version `CreateAddon` assigns when the caller names none and the add-on
/// catalog publishes no build of that add-on for the cluster's Kubernetes
/// version.
///
/// "The version of the add-on. The version must match one of the versions
/// returned by `DescribeAddonVersions`" (`CreateAddon$addonVersion`) — and when
/// the member is absent the real service installs the default version for the
/// cluster's Kubernetes version. That default comes from the catalog
/// ([`ComAmazonawsEksAwswesleyfrontendState::default_addon_version`]), which is
/// the ONE source of a published add-on's versions, so the catalog and an
/// installed add-on cannot disagree.
///
/// This is the fallback for a request the catalog cannot answer: the published
/// catalog is the add-ons [`ADDON_CATALOG`] carries for the Kubernetes versions
/// the calendar carries ([`VERSION_CALENDAR`]), so a cluster whose record
/// somehow reports a Kubernetes version outside that calendar still reports a
/// version for its add-ons. It builds one in the `vX.Y.Z-eksbuild.N`
/// form every EKS add-on version takes out of the cluster's own Kubernetes
/// version, so a cluster on one Kubernetes version always reports the same
/// default and the value a caller reads back is the one the record was created
/// with.
pub fn derive_default_addon_version(kubernetes_version: &str) -> String {
    let mut parts = kubernetes_version
        .split('.')
        .filter(|part| !part.is_empty());
    let major = parts.next().unwrap_or("1");
    let minor = parts.next().unwrap_or("0");
    let patch = parts.next().unwrap_or("0");
    format!("v{major}.{minor}.{patch}-eksbuild.1")
}

/// Whether `name` has the FORM an Amazon EKS add-on name takes: a non-empty
/// string of ASCII alphanumerics, `-`, `.` and `_`.
///
/// `CreateAddon$addonName` targets a plain `String` with no `length` or
/// `pattern` trait (`smithy_query query="operation" operation="CreateAddon"`),
/// so the boundary validator accepts anything — including the empty string —
/// and this is the check that refuses a value no add-on name can be. It is the
/// FORM check that runs BEFORE the catalog one
/// ([`ComAmazonawsEksAwswesleyfrontendState::publishes_addon_for`]), which is
/// what "the name must match one of the names returned by
/// `DescribeAddonVersions`" states: a value no add-on name can be is reported as
/// the form it broke rather than as a catalog miss. The character set is the one
/// every published add-on name uses, EKS-managed (`vpc-cni`) and Marketplace
/// (`<publisher>_<product>`) alike.
///
/// The LENGTH ceiling is [`MAX_KUBERNETES_NAME_LEN`], the DNS-1123 subdomain
/// limit this module already bounds Kubernetes names by: an add-on name names
/// the Kubernetes workloads the add-on installs, it becomes part of this store's
/// key and of the add-on's ARN, and no published add-on name comes near it — so
/// a name past it names no add-on the real service would accept either.
pub fn is_addon_name_shaped(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_KUBERNETES_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// The sentence Amazon EKS refuses an `addonName` the catalog does not publish
/// for the cluster's Kubernetes version with.
///
/// "The name must match one of the names returned by `DescribeAddonVersions`"
/// (`CreateAddon$addonName`), and that listing is per Kubernetes version, so the
/// refusal names both the add-on asked for and the Kubernetes version of the
/// cluster it was asked for — the version held on the cluster record, not
/// anything the request carried.
#[must_use]
pub fn addon_not_supported_message(addon_name: &str, kubernetes_version: &str) -> String {
    format!(
        "Addon {addon_name} specified is not supported in {kubernetes_version} kubernetes \
         version"
    )
}

/// The sentence Amazon EKS refuses an `UpdateAddon` of an add-on that has not
/// SETTLED with.
///
/// An add-on is in use by whatever has not finished with it — its own install, a
/// delete, or an update already running — and the refusal names the add-on the
/// request asked to update and the `AddonStatus` it is currently in, both read
/// from the request and from the record rather than from anything the body
/// carried. There is no trailing stop.
#[must_use]
pub fn addon_not_updatable_message(addon_name: &str, status: &str) -> String {
    format!("Addon {addon_name} cannot be updated as it is currently in {status} state")
}

/// The sentence Amazon EKS refuses an `addonVersion` the catalog does not offer
/// for the cluster's Kubernetes version with.
///
/// It names no value: the build asked for and the cluster's Kubernetes version
/// are both in the request and the record the caller already has, so the
/// refusal is one fixed sentence for every build the catalog does not offer —
/// one it does not publish at all, and one whose `compatibilities` do not carry
/// that Kubernetes version.
pub const ADDON_VERSION_NOT_SUPPORTED_MESSAGE: &str = "Addon version specified is not supported";

/// The digest one registration's server-generated values are derived from.
///
/// `domain` separates the values derived for the same registration, so the
/// activation id does not reveal the activation code; the cluster ARN and the
/// creation instant together make it distinct per registration, so registering
/// the same name again after deregistering it mints fresh values rather than
/// repeating the previous ones.
pub(super) fn registration_digest(arn: &str, created_at: SystemTime, domain: &str) -> [u8; 32] {
    let nanos = created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0u8]);
    hasher.update(arn.as_bytes());
    hasher.update([0u8]);
    hasher.update(nanos.to_be_bytes());
    hasher.finalize().into()
}

/// The `connectorConfig.activationId` of a registration: "a unique ID associated
/// with the cluster for registration purposes"
/// (`ConnectorConfigResponse$activationId`), in the hyphenated 8-4-4-4-12 hex
/// form an activation id takes.
///
/// Computed once, by `RegisterCluster`, and stored on the record — no read
/// recomputes it, so it is stable for the registration's whole life.
pub fn derive_activation_id(arn: &str, created_at: SystemTime) -> String {
    let digest = registration_digest(arn, created_at, "activation-id");
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    hyphenate_uuid(&bytes)
}

/// The `connectorConfig.activationCode` of a registration: "a unique code
/// associated with the cluster for registration purposes"
/// (`ConnectorConfigResponse$activationCode`).
///
/// It is NOT a credential this emulator authenticates anything with: the code is
/// consumed by the connector agent against AWS Systems Manager, which this
/// emulator does not serve, so nothing here accepts it and no request is
/// authorized by it (an operation that returns a real credential mints it
/// through the credential store instead). It is derived like the activation id,
/// from a separate domain, so a read-back returns the same value the
/// registration reported.
pub fn derive_activation_code(arn: &str, created_at: SystemTime) -> String {
    let digest = registration_digest(arn, created_at, "activation-code");
    // 15 bytes encode to 20 base64 characters with no padding.
    ::aws_smithy_types::base64::encode(&digest[..15])
}
