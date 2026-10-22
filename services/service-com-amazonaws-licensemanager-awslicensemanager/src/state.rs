// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-licensemanager-awslicensemanager service.
//!
//! The self-managed **license configuration** is this service's primary resource.
//! Every configuration is stored exactly once, keyed by its ARN, so each operation
//! of the lifecycle — create, get, list, update, delete — reads and writes the one
//! record, and a later read reports what an earlier call persisted.
//!
//! A **license** — the granted, entitlement-carrying resource `CreateLicense` mints
//! and `GetLicense` reports — is stored the same way, once, keyed by its ARN, beside
//! the `ClientToken` the request that minted it carried, so a repeat of that request
//! reports the same license instead of minting a second one.
//!
//! Tags live in their own ARN-keyed map rather than on the record: `TagResource`,
//! `UntagResource` and `ListTagsForResource` are modelled against a generic
//! `ResourceArn`, so the map is the single place the tags of any taggable resource
//! this service grows later will live.
//!
//! Authorization and throttling are deliberately OUT OF SCOPE for this slice: the
//! emulator verifies every request signature centrally and models no per-principal
//! ownership of a configuration, so the `AccessDeniedException`,
//! `AuthorizationException` and `RateLimitExceededException` variants these
//! operations declare are never constructed here. They are modelled, not enforced.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    ConsumedLicenseSummary, ConsumptionConfiguration, DatetimeRange, Entitlement, Issuer,
    IssuerDetails, License, LicenseConfiguration, ManagedResourceSummary, Metadata,
    ProductInformation, Tag, ENTITLEMENT_UNIT_VALUES, RENEW_TYPE_VALUES,
};

/// The ARN service namespace for this service: the model's `arnNamespace` and sigv4
/// name, `license-manager`
/// (`smithy_query(query="service_info")`).
pub const ARN_SERVICE: &str = "license-manager";

/// The resource portion a license-configuration ARN carries before its id, so a
/// generated ARN reads
/// `arn:aws:license-manager:<region>:<account>:license-configuration:lic-<32 hex>`.
pub const CONFIGURATION_ARN_RESOURCE_PREFIX: &str = "license-configuration:";

/// The prefix of a license-configuration id.
pub const CONFIGURATION_ID_PREFIX: &str = "lic-";

/// Number of hex characters after [`CONFIGURATION_ID_PREFIX`] in a configuration id.
pub const CONFIGURATION_ID_HEX_LEN: usize = 32;

/// The resource portion a license ARN carries before its id, so a generated ARN
/// reads `arn:aws:license-manager:<home region>:<account>:license:l-<32 hex>`.
pub const LICENSE_ARN_RESOURCE_PREFIX: &str = "license:";

/// The prefix of a license id.
pub const LICENSE_ID_PREFIX: &str = "l-";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The status a license configuration carries until an `UpdateLicenseConfiguration`
/// moves it. `CreateLicenseConfiguration` returns a configuration that is usable at
/// once, so it is created in this terminal status and owes the clock no transition.
///
/// One of the modelled `LicenseConfigurationStatus` values
/// (`smithy_query(query="shape",shape="LicenseConfigurationStatus")`).
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// The status a license carries from the moment `CreateLicense` mints it: AWS hands
/// back a license that can be checked out at once, so it is created in this terminal
/// status and owes the clock no transition. One of the modelled `LicenseStatus`
/// values (`smithy_query(query="shape",shape="LicenseStatus")`); the non-terminal
/// `PENDING_AVAILABLE` belongs to a license a grant workflow is still activating,
/// which nothing in this slice produces.
pub const LICENSE_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The version `CreateLicense` gives a license it mints. License versions are
/// numbered from 1 and only `CreateLicenseVersion` moves one on
/// (`smithy_query(query="operation",operation="CreateLicenseVersion")`).
pub const FIRST_LICENSE_VERSION: &str = "1";

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// delete or untag, and the model publishes NO length for any of the lists or
// strings involved — so the centralized constraint walk cannot refuse an oversized
// one and each growth site carries its bound here. Each cap is this emulator's
// resource guard, set well above any realistic use; this service publishes no
// quota the model carries.
// ---------------------------------------------------------------------------

/// The most license configurations this emulator will hold. A create past it is
/// refused with the modelled `ResourceLimitExceededException` (HTTP 400), enforced
/// in [`ComAmazonawsLicensemanagerAwslicensemanagerState::insert_configuration`].
pub const MAX_LICENSE_CONFIGURATIONS: usize = 500;

/// The most licenses this emulator will hold, enforced in
/// [`ComAmazonawsLicensemanagerAwslicensemanagerState::insert_license`]. A create
/// past it stores nothing; `CreateLicense` models no resource-limit error, so its
/// handler renders the refusal as the modelled `ServerInternalException`.
pub const MAX_LICENSES: usize = 500;

/// The most tags one resource may carry. Emulator resource guard, matching the
/// 50-tags-per-resource ceiling AWS applies to tagged resources generally; enforced
/// in [`ComAmazonawsLicensemanagerAwslicensemanagerState::merge_tags`].
pub const MAX_TAGS: usize = 50;

/// The most entries this emulator stores for any other caller-sized list on a
/// configuration or a license — `LicenseRules`, `ProductInformationList`, a product
/// information's filter list, that filter's own value list, a license's
/// `Entitlements` and its `LicenseMetadata`.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// The longest caller-supplied text this emulator stores in a configuration, a
/// license or a tag.
pub const MAX_TEXT_LEN: usize = 2048;

/// The domain tag mixed into the preimage of an issuer key fingerprint, so a digest
/// this service derives for an issuer identity cannot be confused with a digest it
/// derives for anything else.
const ISSUER_FINGERPRINT_DOMAIN: &str = "license-manager:issuer:v1";

/// The issuer key fingerprint a license of this issuer carries.
///
/// `KeyFingerprint` ("Issuer key fingerprint") is on the READ shape only:
/// `IssuerDetails`, which `License.Issuer` targets, carries `Name`, `SignKey` and
/// `KeyFingerprint`, while `Issuer` — the shape a create carries — has only `Name`
/// and `SignKey`
/// (`smithy_query(query="shape",shape="Issuer,IssuerDetails")`). No caller can
/// supply a fingerprint, so the service derives it, and it derives it from the
/// ISSUER IDENTITY alone — the name and the signing key the create named — so one
/// issuer always fingerprints the same way however many licenses it issues, two
/// different issuers never share a fingerprint, and every read of a license reports
/// one stable value that `CheckoutLicense` can resolve against as the second half of
/// its (`ProductSKU`, `KeyFingerprint`) key.
///
/// The form mirrors the colon-delimited fingerprint License Manager reports for a
/// license issuer, `aws:<account>:<issuer name>:<fingerprint>`, with the last
/// segment the SHA-256 digest of the length-prefixed issuer identity — length
/// prefixes so no name/key boundary can be shifted to spell another issuer's
/// preimage. An absent `SignKey` names no key and so hashes as an empty one. No key
/// material is held or signed here: the digest IDENTIFIES the issuer, it does not
/// certify it.
pub fn issuer_key_fingerprint(name: &str, sign_key: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest.update(ISSUER_FINGERPRINT_DOMAIN.as_bytes());
    for part in [name, sign_key.unwrap_or_default()] {
        digest.update(format!(":{}:", part.len()).as_bytes());
        digest.update(part.as_bytes());
    }
    format!(
        "aws:{}:{name}:{}",
        ::engine::emulator_account_id(),
        ::hex::encode(digest.finalize())
    )
}

/// The `IssuerDetails` a read of a license reports for the `Issuer` its create
/// carried: the name and signing key exactly as given, plus the fingerprint this
/// service derives for that issuer.
///
/// Every path that stores a license issuer goes through here, so one issuer
/// fingerprints the same way whichever operation minted the license.
pub fn issuer_details(issuer: Issuer) -> IssuerDetails {
    IssuerDetails {
        key_fingerprint: Some(issuer_key_fingerprint(
            &issuer.name,
            issuer.sign_key.as_deref(),
        )),
        name: Some(issuer.name),
        sign_key: issuer.sign_key,
    }
}

/// One stored license configuration: every member `GetLicenseConfiguration`
/// reports, except the tags, which are keyed by ARN in
/// [`ComAmazonawsLicensemanagerAwslicensemanagerState::tags`].
#[derive(Debug, Clone, Default)]
pub struct StoredLicenseConfiguration {
    /// `lic-<32 hex>`: the ARN's last segment and this configuration's id.
    pub id: String,
    /// This record's store key.
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub license_counting_type: String,
    pub license_count: Option<i64>,
    pub license_count_hard_limit: Option<bool>,
    pub license_rules: Vec<String>,
    pub product_information_list: Vec<ProductInformation>,
    pub disassociate_when_not_found: Option<bool>,
    pub license_expiry: Option<i64>,
    /// One of the modelled `LicenseConfigurationStatus` values.
    pub status: String,
    pub owner_account_id: String,
    /// Licenses assigned to resources. Nothing in this slice associates a resource
    /// with a configuration (`UpdateLicenseSpecificationsForResource` and the
    /// checkout operations are still scaffolded), so it stays at the value a
    /// configuration with no association has.
    pub consumed_licenses: i64,
    pub consumed_license_summary_list: Vec<ConsumedLicenseSummary>,
    pub managed_resource_summary_list: Vec<ManagedResourceSummary>,
}

impl StoredLicenseConfiguration {
    /// Whether hard license enforcement is used — what the documented
    /// `enforceLicenseCount` list filter scopes on.
    pub fn enforces_license_count(&self) -> bool {
        self.license_count_hard_limit.unwrap_or(false)
    }

    /// Whether the available licenses have been exceeded — what the documented
    /// `usagelimitExceeded` list filter scopes on. A configuration that sets no
    /// `LicenseCount` has no limit to exceed.
    pub fn usage_limit_exceeded(&self) -> bool {
        self.license_count
            .is_some_and(|count| self.consumed_licenses > count)
    }

    /// This record as the modelled `LicenseConfiguration` structure
    /// `ListLicenseConfigurations` reports.
    ///
    /// The shape carries no `Tags` member — only `GetLicenseConfiguration` and
    /// `ListTagsForResource` report tags — so none are rendered here.
    /// `AutomatedDiscoveryInformation` is left absent: automated discovery is a
    /// service setting nothing in this slice turns on, so there is no last-run time
    /// to report.
    pub fn wire(&self) -> LicenseConfiguration {
        LicenseConfiguration {
            license_configuration_id: Some(self.id.clone()),
            license_configuration_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            license_counting_type: Some(self.license_counting_type.clone()),
            license_rules: Some(self.license_rules.clone()),
            license_count: self.license_count,
            license_count_hard_limit: self.license_count_hard_limit,
            disassociate_when_not_found: self.disassociate_when_not_found,
            consumed_licenses: Some(self.consumed_licenses),
            status: Some(self.status.clone()),
            owner_account_id: Some(self.owner_account_id.clone()),
            consumed_license_summary_list: Some(self.consumed_license_summary_list.clone()),
            managed_resource_summary_list: Some(self.managed_resource_summary_list.clone()),
            product_information_list: Some(self.product_information_list.clone()),
            automated_discovery_information: None,
            license_expiry: self.license_expiry,
        }
    }
}

/// One stored license: every member `GetLicense` reports, plus the identity and the
/// `ClientToken` of the request that minted it.
///
/// `Tags` are NOT held here — a license is tagged through the same ARN-keyed map
/// every other taggable resource of this service uses.
#[derive(Debug, Clone, Default)]
pub struct StoredLicense {
    /// `l-<32 hex>`: the ARN's last segment and this license's id.
    pub id: String,
    /// This record's store key.
    pub arn: String,
    pub license_name: String,
    pub product_name: String,
    pub product_sku: String,
    /// The issuer, as the `IssuerDetails` a read reports: the `Issuer` the create
    /// carried plus the `KeyFingerprint` [`issuer_details`] derived for it, which is
    /// on the read shape only and so can come from nowhere else.
    pub issuer: IssuerDetails,
    pub home_region: String,
    /// One of the modelled `LicenseStatus` values.
    pub status: String,
    pub validity: DatetimeRange,
    pub beneficiary: String,
    pub entitlements: Vec<Entitlement>,
    pub consumption_configuration: ConsumptionConfiguration,
    /// Absent when the create carried none, so a read reports what the caller set
    /// and nothing more.
    pub license_metadata: Option<Vec<Metadata>>,
    /// The ISO8601-UTC instant this record was minted at.
    pub create_time: String,
    /// The license version this record is, as a decimal counted from
    /// [`FIRST_LICENSE_VERSION`].
    pub version: String,
    /// The `ClientToken` of the request that minted this license, so a repeat of
    /// that request is answered from this record instead of minting another.
    pub client_token: String,
}

impl StoredLicense {
    /// This record as the modelled `License` structure `GetLicense` reports.
    pub fn wire(&self) -> License {
        License {
            license_arn: Some(self.arn.clone()),
            license_name: Some(self.license_name.clone()),
            product_name: Some(self.product_name.clone()),
            product_sku: Some(self.product_sku.clone()),
            issuer: Some(self.issuer.clone()),
            home_region: Some(self.home_region.clone()),
            status: Some(self.status.clone()),
            validity: Some(self.validity.clone()),
            beneficiary: Some(self.beneficiary.clone()),
            entitlements: Some(self.entitlements.clone()),
            consumption_configuration: Some(self.consumption_configuration.clone()),
            license_metadata: self.license_metadata.clone(),
            create_time: Some(self.create_time.clone()),
            version: Some(self.version.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-licensemanager-awslicensemanager. A handler
/// reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLicensemanagerAwslicensemanagerState {
    /// Every license configuration, keyed by its ARN. A `BTreeMap` so
    /// `ListLicenseConfigurations` walks one stable, total order on every call and a
    /// page token can name the ARN the next page starts at — no token table, and no
    /// page that skips or repeats a configuration created between two calls.
    pub license_configurations: BTreeMap<String, StoredLicenseConfiguration>,
    /// Every license, keyed by its ARN, for the same reason: one record per license,
    /// walked in one stable total order.
    pub licenses: BTreeMap<String, StoredLicense>,
    /// The ARN each accepted `CreateLicense` `ClientToken` minted, so the repeat of
    /// a request is idempotent. Written only by
    /// [`Self::insert_license`] and dropped with its license by
    /// [`Self::remove_license`], so it holds at most one entry per stored license
    /// and is bounded by [`MAX_LICENSES`].
    pub license_client_tokens: BTreeMap<String, String>,
    /// Tags, keyed by resource ARN then tag key. Shared by `TagResource`,
    /// `UntagResource` and `ListTagsForResource`, and seeded from the `Tags` passed
    /// to `CreateLicenseConfiguration` or `CreateLicense`.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// Mixed into each generated id so two creates in the same nanosecond differ.
    id_counter: u64,
}

impl ComAmazonawsLicensemanagerAwslicensemanagerState {
    /// The configuration `arn` names, for a reporting operation.
    pub fn configuration(&self, arn: &str) -> Option<&StoredLicenseConfiguration> {
        self.license_configurations.get(arn)
    }

    /// The configuration `arn` names, for an operation the spec says mutates it.
    ///
    /// `id` and `arn` are this record's identity — the store is keyed on the ARN —
    /// so no handler reassigns them through this handle.
    pub fn configuration_mut(&mut self, arn: &str) -> Option<&mut StoredLicenseConfiguration> {
        self.license_configurations.get_mut(arn)
    }

    /// Store a newly created configuration, refusing it — and changing nothing —
    /// when the store already holds [`MAX_LICENSE_CONFIGURATIONS`] or already holds
    /// a configuration under this ARN.
    ///
    /// The bound is enforced HERE, at the growth site, so no handler can grow the
    /// store past it by forgetting a pre-check, and an ARN collision can never
    /// silently replace a stored configuration. `false` means nothing was stored;
    /// the caller renders that as its own modelled resource-limit error.
    #[must_use]
    pub fn insert_configuration(&mut self, configuration: StoredLicenseConfiguration) -> bool {
        if self.license_configurations.len() >= MAX_LICENSE_CONFIGURATIONS
            || self
                .license_configurations
                .contains_key(&configuration.arn)
        {
            return false;
        }
        self.license_configurations
            .insert(configuration.arn.clone(), configuration);
        true
    }

    /// Remove the configuration `arn` names, with its tags. `None` when nothing
    /// matched.
    pub fn remove_configuration(&mut self, arn: &str) -> Option<StoredLicenseConfiguration> {
        let configuration = self.license_configurations.remove(arn)?;
        self.tags.remove(arn);
        Some(configuration)
    }

    /// The license `arn` names, for a reporting operation.
    pub fn license(&self, arn: &str) -> Option<&StoredLicense> {
        self.licenses.get(arn)
    }

    /// The license the request carrying `client_token` minted, or `None` when no
    /// accepted `CreateLicense` has carried it. This is what makes the repeat of a
    /// create idempotent: the second call reports this record and mints nothing.
    pub fn license_for_client_token(&self, client_token: &str) -> Option<&StoredLicense> {
        let arn = self.license_client_tokens.get(client_token)?;
        self.licenses.get(arn)
    }

    /// Store a newly minted license, with the `ClientToken` it was minted under,
    /// refusing it — and changing nothing — when the store already holds
    /// [`MAX_LICENSES`] or already holds a license under this ARN.
    ///
    /// The bound is enforced HERE, at the growth site, so no handler can grow the
    /// store past it by forgetting a pre-check, and an ARN collision can never
    /// silently replace a stored license. `false` means nothing was stored.
    #[must_use]
    pub fn insert_license(&mut self, license: StoredLicense) -> bool {
        if self.licenses.len() >= MAX_LICENSES || self.licenses.contains_key(&license.arn) {
            return false;
        }
        // The token map is written only here, so it can never name an ARN the store
        // does not hold. `ClientToken` is `^\S+$`, so the empty string is not one.
        if !license.client_token.is_empty() {
            self.license_client_tokens
                .insert(license.client_token.clone(), license.arn.clone());
        }
        self.licenses.insert(license.arn.clone(), license);
        true
    }

    /// Remove the license `arn` names, with its tags and the client token it was
    /// minted under. `None` when nothing matched.
    pub fn remove_license(&mut self, arn: &str) -> Option<StoredLicense> {
        let license = self.licenses.remove(arn)?;
        self.tags.remove(arn);
        self.license_client_tokens.retain(|_, minted| minted != arn);
        Some(license)
    }

    /// Whether `arn` names a resource this service stores, and can therefore tag: a
    /// license configuration or a license. Both are ARN-keyed and capped, so the
    /// number of KEYS in the tag map is bounded by
    /// [`MAX_LICENSE_CONFIGURATIONS`] + [`MAX_LICENSES`].
    ///
    /// `TagResource`, `UntagResource` and `ListTagsForResource` are modelled against
    /// a generic `ResourceArn`, so each of them asks THIS question rather than
    /// naming one resource kind.
    pub fn is_taggable(&self, arn: &str) -> bool {
        self.license_configurations.contains_key(arn) || self.licenses.contains_key(arn)
    }

    /// The tags stored for `arn`, as the modelled `TagList`. Empty when none were
    /// ever set.
    pub fn tags_of(&self, arn: &str) -> Vec<Tag> {
        self.tags
            .get(arn)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| Tag {
                        key: Some(key.clone()),
                        value: Some(value.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The number of distinct tag keys `arn` would carry after merging `tags`.
    fn tag_count_after_merge(&self, arn: &str, tags: &[Tag]) -> usize {
        let mut keys: std::collections::BTreeSet<&str> = self
            .tags
            .get(arn)
            .map(|stored| stored.keys().map(String::as_str).collect())
            .unwrap_or_default();
        for tag in tags {
            if let Some(key) = tag.key.as_deref() {
                keys.insert(key);
            }
        }
        keys.len()
    }

    /// Merge `tags` into the tags of `arn`, replacing the value of a key already
    /// present. A tag whose `Key` is absent carries nothing to key on and is
    /// skipped; an absent `Value` stores the empty string, which is what the
    /// `Value` member admits.
    ///
    /// Every bound on this growth site is enforced HERE, so no handler can grow the
    /// map past one by forgetting a pre-check, and `Err` always means nothing was
    /// written (the caller renders it as its own modelled refusal):
    ///
    /// * `arn` must name a resource the store holds, so the number of KEYS in the
    ///   outer map can never exceed the number of stored configurations and
    ///   licenses, and is therefore bounded by [`MAX_LICENSE_CONFIGURATIONS`] +
    ///   [`MAX_LICENSES`]. A tag map entry is also dropped with its resource by
    ///   [`Self::remove_configuration`] or [`Self::remove_license`], so the map
    ///   cannot outlive what it describes.
    /// * that ARN is length-bounded like every other stored string, so no single
    ///   key can be arbitrarily large.
    /// * the merge may not leave `arn` carrying more than [`MAX_TAGS`] distinct
    ///   keys.
    pub fn merge_tags(&mut self, arn: &str, tags: &[Tag]) -> Result<(), String> {
        if let Some(why) = text_rejection("ResourceArn", Some(arn)) {
            return Err(why);
        }
        if !self.is_taggable(arn) {
            return Err(format!("no resource exists with the ARN '{arn}'"));
        }
        let after = self.tag_count_after_merge(arn, tags);
        if after > MAX_TAGS {
            return Err(format!(
                "this resource would carry {after} tags, more than the {MAX_TAGS} this service accepts"
            ));
        }
        let stored = self.tags.entry(arn.to_string()).or_default();
        for tag in tags {
            if let Some(key) = tag.key.as_deref() {
                stored.insert(key.to_string(), tag.value.clone().unwrap_or_default());
            }
        }
        Ok(())
    }

    /// Drop `keys` from the tags of `arn`. A key the resource does not carry is not
    /// an error: untagging is idempotent.
    pub fn remove_tag_keys(&mut self, arn: &str, keys: &[String]) {
        if let Some(stored) = self.tags.get_mut(arn) {
            for key in keys {
                stored.remove(key);
            }
            if stored.is_empty() {
                self.tags.remove(arn);
            }
        }
    }

    /// A `lic-<32 hex>` id no stored configuration is using.
    pub fn fresh_configuration_id(&mut self) -> String {
        let configurations = &self.license_configurations;
        fresh_id(
            &mut self.id_counter,
            CONFIGURATION_ID_PREFIX,
            configurations.len(),
            |candidate| configurations.keys().any(|arn| arn.ends_with(candidate)),
        )
    }

    /// An `l-<32 hex>` id no stored license is using.
    pub fn fresh_license_id(&mut self) -> String {
        let licenses = &self.licenses;
        fresh_id(
            &mut self.id_counter,
            LICENSE_ID_PREFIX,
            licenses.len(),
            |candidate| licenses.keys().any(|arn| arn.ends_with(candidate)),
        )
    }
}

/// One `<prefix><32 hex>` id that `taken` does not already claim.
///
/// Shared by every id this service mints, so each kind differs only in its prefix
/// and in the records it must not collide with. `stored` is how many records that
/// kind already holds, which bounds the fallback walk below.
fn fresh_id(
    counter: &mut u64,
    prefix: &str,
    stored: usize,
    taken: impl Fn(&str) -> bool,
) -> String {
    for _ in 0..1024 {
        *counter = counter.wrapping_add(1);
        let candidate = id_from_seed(prefix, now_nanos() ^ counter.rotate_left(17), *counter);
        if !taken(&candidate) {
            return candidate;
        }
    }
    // Bounded fallback, so a pathological clock can never spin here: the
    // zero-padded hex counters are also `<prefix>[0-9a-f]{32}`, and there are always
    // more of them than there are stored records.
    (0u64..=stored as u64)
        .map(|n| format!("{prefix}{n:032x}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| format!("{prefix}{stored:032x}"))
}

/// The members a license-minting request persists, borrowed for validation.
///
/// `CreateLicense` and `CreateLicenseVersion` carry the same license body, so the
/// rule that decides whether that body is storable lives once, here. Pass `None` for
/// a member the request does not carry.
#[derive(Debug)]
pub struct LicenseInput<'a> {
    pub license_name: &'a str,
    pub product_name: &'a str,
    pub product_sku: Option<&'a str>,
    pub issuer: &'a Issuer,
    pub home_region: &'a str,
    pub validity: &'a DatetimeRange,
    pub beneficiary: Option<&'a str>,
    pub entitlements: &'a [Entitlement],
    pub consumption_configuration: &'a ConsumptionConfiguration,
    pub license_metadata: Option<&'a [Metadata]>,
    pub tags: Option<&'a [Tag]>,
}

/// Why the license body `input` carries is unacceptable, or `None` when every member
/// of it fits the store.
pub fn license_input_rejection(input: &LicenseInput<'_>) -> Option<String> {
    text_rejection("LicenseName", Some(input.license_name))
        .or_else(|| text_rejection("ProductName", Some(input.product_name)))
        .or_else(|| text_rejection("ProductSKU", input.product_sku))
        .or_else(|| text_rejection("Issuer.Name", Some(&input.issuer.name)))
        .or_else(|| text_rejection("Issuer.SignKey", input.issuer.sign_key.as_deref()))
        .or_else(|| text_rejection("Beneficiary", input.beneficiary))
        .or_else(|| region_rejection("HomeRegion", input.home_region))
        .or_else(|| validity_rejection(input.validity))
        .or_else(|| {
            list_rejection(
                "Entitlements",
                input.entitlements.len(),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| input.entitlements.iter().find_map(entitlement_rejection))
        .or_else(|| consumption_configuration_rejection(input.consumption_configuration))
        .or_else(|| {
            list_rejection(
                "LicenseMetadata",
                input.license_metadata.map_or(0, <[Metadata]>::len),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| {
            input
                .license_metadata
                .unwrap_or_default()
                .iter()
                .find_map(|entry| {
                    text_rejection("a LicenseMetadata entry's Name", entry.name.as_deref()).or_else(
                        || text_rejection("a LicenseMetadata entry's Value", entry.value.as_deref()),
                    )
                })
        })
        .or_else(|| input.tags.and_then(tag_list_rejection))
}

/// Why a caller-supplied Region is unacceptable, or `None` when it is a Region
/// token.
///
/// The value lands inside a generated ARN, so anything that is not an AWS Region
/// token is refused rather than stored: an ARN can never carry arbitrary caller
/// text. The value itself is not echoed back, because the model bounds it at no
/// length.
fn region_rejection(member: &str, region: &str) -> Option<String> {
    (!is_region_token(region)).then(|| {
        format!(
            "{member} must be an AWS Region name: up to 64 alphanumeric-or-hyphen characters"
        )
    })
}

/// Why one `Entitlement` entry is unacceptable, or `None` when it fits the store.
fn entitlement_rejection(entitlement: &Entitlement) -> Option<String> {
    text_rejection("an Entitlements entry's Name", Some(&entitlement.name))
        .or_else(|| {
            text_rejection(
                "an Entitlements entry's Value",
                entitlement.value.as_deref(),
            )
        })
        .or_else(|| {
            // The catalog is the modelled `EntitlementUnit` enum; this is the
            // handler-side guard, so nothing outside it can reach the store whatever
            // the centralized constraint walk does.
            (!ENTITLEMENT_UNIT_VALUES.contains(&entitlement.unit.as_str())).then(|| {
                format!(
                    "an Entitlements entry's Unit must be one of [{}], got '{}'",
                    ENTITLEMENT_UNIT_VALUES.join(", "),
                    entitlement.unit
                )
            })
        })
        // `MaxCount` is the "maximum entitlement count", so it has no meaning below
        // zero; the model gives it the unconstrained `Long`.
        .or_else(|| negative_rejection("an Entitlements entry's MaxCount", entitlement.max_count))
}

/// Why a `ConsumptionConfiguration` is unacceptable, or `None` when it fits.
fn consumption_configuration_rejection(
    configuration: &ConsumptionConfiguration,
) -> Option<String> {
    configuration
        .renew_type
        .as_deref()
        .filter(|renew_type| !RENEW_TYPE_VALUES.contains(renew_type))
        .map(|renew_type| {
            format!(
                "ConsumptionConfiguration.RenewType must be one of [{}], got '{renew_type}'",
                RENEW_TYPE_VALUES.join(", ")
            )
        })
        .or_else(|| {
            configuration
                .provisional_configuration
                .as_ref()
                .and_then(|provisional| {
                    minutes_rejection(
                        "ConsumptionConfiguration.ProvisionalConfiguration.MaxTimeToLiveInMinutes",
                        provisional.max_time_to_live_in_minutes,
                    )
                })
        })
        .or_else(|| {
            configuration
                .borrow_configuration
                .as_ref()
                .and_then(|borrow| {
                    minutes_rejection(
                        "ConsumptionConfiguration.BorrowConfiguration.MaxTimeToLiveInMinutes",
                        borrow.max_time_to_live_in_minutes,
                    )
                })
        })
}

/// Why a time-to-live in minutes is unacceptable, or `None` when it is not
/// negative: a maximum time to live is a duration, so it has no meaning below zero.
fn minutes_rejection(member: &str, minutes: i32) -> Option<String> {
    (minutes < 0).then(|| format!("{member} must not be negative, got {minutes}"))
}

/// Why a `Validity` range is unacceptable, or `None` when it fits the store.
///
/// A range runs forwards: `End`, when the request carries one, may not precede
/// `Begin`. The ordering is decided on the PARSED instants, so a zone offset is
/// honoured rather than compared as text; a bound the ISO8601 parser does not accept
/// is left alone, because the member's modelled pattern is what admits it.
fn validity_rejection(validity: &DatetimeRange) -> Option<String> {
    text_rejection("Validity.Begin", Some(&validity.begin))
        .or_else(|| text_rejection("Validity.End", validity.end.as_deref()))
        .or_else(|| {
            let begin = parse_instant(&validity.begin)?;
            let end = parse_instant(validity.end.as_deref()?)?;
            (end < begin).then(|| {
                "Validity.End must not be earlier than Validity.Begin".to_string()
            })
        })
}

/// `text` as seconds since the Unix epoch, or `None` when it is not an ISO8601
/// instant this emulator can order.
fn parse_instant(text: &str) -> Option<f64> {
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|instant| instant.as_secs_f64())
}

/// Why a caller-supplied string is unacceptable, or `None` when it fits the store.
///
/// The model publishes no length for any string on these requests, so this is where
/// the bound lives, measured in UTF-8 bytes — the units the store actually holds.
/// Every operation that stores one applies this one rule.
pub fn text_rejection(member: &str, value: Option<&str>) -> Option<String> {
    let value = value?;
    (value.len() > MAX_TEXT_LEN).then(|| {
        format!(
            "{member} is {} bytes long, more than the {MAX_TEXT_LEN} this service accepts",
            value.len()
        )
    })
}

/// Why a caller-sized list is unacceptable, or `None` when it fits the store.
fn list_rejection(member: &str, len: usize, cap: usize) -> Option<String> {
    (len > cap).then(|| format!("{member} carries {len} entries, more than the {cap} this service accepts"))
}

/// Why a count or an instant is unacceptable, or `None` when it is not negative.
///
/// `LicenseCount` is "the number of licenses managed by the license configuration"
/// and `LicenseExpiry` is an instant, so neither has a meaning below zero. The model
/// gives both the unconstrained `BoxLong`, so this is where the rule lives.
fn negative_rejection(member: &str, value: Option<i64>) -> Option<String> {
    value.filter(|value| *value < 0).map(|value| {
        format!("{member} must not be negative, got {value}")
    })
}

/// Why the members `CreateLicenseConfiguration` and `UpdateLicenseConfiguration`
/// store are unacceptable, or `None` when every one of them fits the store.
///
/// Both operations accept the same members and persist them into the same record, so
/// the rule lives once here. Pass `None` for a member the request omitted.
pub fn configuration_input_rejection(
    name: Option<&str>,
    description: Option<&str>,
    license_count: Option<i64>,
    license_expiry: Option<i64>,
    license_rules: Option<&[String]>,
    product_information_list: Option<&[ProductInformation]>,
    tags: Option<&[Tag]>,
) -> Option<String> {
    text_rejection("Name", name)
        .or_else(|| text_rejection("Description", description))
        .or_else(|| negative_rejection("LicenseCount", license_count))
        .or_else(|| negative_rejection("LicenseExpiry", license_expiry))
        .or_else(|| {
            list_rejection(
                "LicenseRules",
                license_rules.map_or(0, <[String]>::len),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| {
            license_rules
                .unwrap_or_default()
                .iter()
                .find_map(|rule| text_rejection("a LicenseRules entry", Some(rule)))
        })
        .or_else(|| {
            list_rejection(
                "ProductInformationList",
                product_information_list.map_or(0, <[ProductInformation]>::len),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| {
            product_information_list
                .unwrap_or_default()
                .iter()
                .find_map(product_information_rejection)
        })
        .or_else(|| tags.and_then(tag_list_rejection))
}

/// Why one `ProductInformation` entry is unacceptable, or `None` when it fits.
fn product_information_rejection(product_information: &ProductInformation) -> Option<String> {
    text_rejection(
        "a ProductInformationList entry's ResourceType",
        Some(&product_information.resource_type),
    )
    .or_else(|| {
        list_rejection(
            "a ProductInformationList entry's ProductInformationFilterList",
            product_information.product_information_filter_list.len(),
            MAX_COLLECTION_ENTRIES,
        )
    })
    .or_else(|| {
        product_information
            .product_information_filter_list
            .iter()
            .find_map(|filter| {
                text_rejection(
                    "a ProductInformationFilter's ProductInformationFilterName",
                    Some(&filter.product_information_filter_name),
                )
                .or_else(|| {
                    text_rejection(
                        "a ProductInformationFilter's ProductInformationFilterComparator",
                        Some(&filter.product_information_filter_comparator),
                    )
                })
                .or_else(|| {
                    list_rejection(
                        "a ProductInformationFilter's ProductInformationFilterValue",
                        filter
                            .product_information_filter_value
                            .as_ref()
                            .map_or(0, Vec::len),
                        MAX_COLLECTION_ENTRIES,
                    )
                })
                .or_else(|| {
                    filter
                        .product_information_filter_value
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .find_map(|value| {
                            text_rejection(
                                "a ProductInformationFilterValue entry",
                                Some(value),
                            )
                        })
                })
            })
    })
}

/// Why a `TagList` is unacceptable, or `None` when it fits the store.
///
/// Shared by `CreateLicenseConfiguration` and `TagResource`: both store the same
/// list against the same ARN-keyed map.
pub fn tag_list_rejection(tags: &[Tag]) -> Option<String> {
    list_rejection("Tags", tags.len(), MAX_TAGS).or_else(|| {
        tags.iter().find_map(|tag| {
            text_rejection("a tag Key", tag.key.as_deref())
                .or_else(|| text_rejection("a tag Value", tag.value.as_deref()))
        })
    })
}

/// Record that a request changed stored state, as this service's attribution trail.
///
/// One line per accepted mutation, naming the operation, the resource and the
/// caller's own SigV4 access key id. The access key id is an identifier, not a
/// secret, and nothing else off the request is logged — never a credential, a tag
/// value or any other request body member.
pub fn record_mutation(operation: &str, arn: &str, parts: &::http::request::Parts) {
    let akid = ::engine::extract_akid(&parts.headers);
    ::tracing::info!(
        target: "license_manager.audit",
        operation,
        resource = arn,
        access_key_id = %akid,
        "license-manager state mutated"
    );
}

/// The ARN of the license configuration `id` in `region`.
pub fn configuration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONFIGURATION_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The ARN of the license `id` in `region`, which for a license is its home Region:
/// a license is a resource OF the Region the create named as its home.
pub fn license_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{LICENSE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// Now, as the ISO8601-UTC instant `License.CreateTime` carries.
///
/// `Format::DateTime` renders RFC 3339 in UTC, `YYYY-MM-DDThh:mm:ss[.fff]Z`, which
/// is what the member's modelled pattern admits.
pub fn now_iso8601() -> String {
    ::aws_smithy_types::DateTime::from(SystemTime::now())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        // That format renders every instant this type can hold; the epoch is the
        // bounded fallback, so no handler has to carry a formatting failure.
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Whether `region` is an AWS Region token: the only text this service lets into a
/// generated ARN's Region segment.
pub fn is_region_token(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 64
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN, so
/// anything that is not an AWS region token is refused here and the default is used
/// instead: a generated ARN can never carry arbitrary caller text.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_token(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// One `<prefix><32 hex>` id from `seed`.
fn id_from_seed(prefix: &str, seed: u64, salt: u64) -> String {
    let mut state = seed;
    let high = splitmix64(&mut state) ^ salt.rotate_right(29);
    let low = splitmix64(&mut state);
    format!("{prefix}{high:016x}{low:016x}")
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLicensemanagerAwslicensemanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLicensemanagerAwslicensemanagerState>)
}
