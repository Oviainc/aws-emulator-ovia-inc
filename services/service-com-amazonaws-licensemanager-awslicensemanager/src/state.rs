// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-licensemanager-awslicensemanager service.
//!
//! The self-managed **license configuration** is this service's primary resource.
//! Every configuration is stored exactly once, keyed by its ARN, so each operation
//! of the lifecycle — create, get, list, update, delete — reads and writes the one
//! record, and a later read reports what an earlier call persisted.
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

use crate::ops::types::{
    ConsumedLicenseSummary, LicenseConfiguration, ManagedResourceSummary, ProductInformation, Tag,
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

/// The most tags one resource may carry. Emulator resource guard, matching the
/// 50-tags-per-resource ceiling AWS applies to tagged resources generally; enforced
/// in [`ComAmazonawsLicensemanagerAwslicensemanagerState::merge_tags`].
pub const MAX_TAGS: usize = 50;

/// The most entries this emulator stores for any other caller-sized list on a
/// configuration — `LicenseRules`, `ProductInformationList`, a product
/// information's filter list and that filter's own value list.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// The longest caller-supplied text this emulator stores in a configuration or a
/// tag.
pub const MAX_TEXT_LEN: usize = 2048;

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

/// Per-service state for com-amazonaws-licensemanager-awslicensemanager. A handler
/// reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLicensemanagerAwslicensemanagerState {
    /// Every license configuration, keyed by its ARN. A `BTreeMap` so
    /// `ListLicenseConfigurations` walks one stable, total order on every call and a
    /// page token can name the ARN the next page starts at — no token table, and no
    /// page that skips or repeats a configuration created between two calls.
    pub license_configurations: BTreeMap<String, StoredLicenseConfiguration>,
    /// Tags, keyed by resource ARN then tag key. Shared by `TagResource`,
    /// `UntagResource` and `ListTagsForResource`, and seeded from the `Tags` passed
    /// to `CreateLicenseConfiguration`.
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
    /// * `arn` must name a configuration the store holds, so the number of KEYS in
    ///   the outer map can never exceed the number of stored configurations, and is
    ///   therefore bounded by [`MAX_LICENSE_CONFIGURATIONS`]. A tag map entry is
    ///   also dropped with its configuration by [`Self::remove_configuration`], so
    ///   the map cannot outlive what it describes.
    /// * that ARN is length-bounded like every other stored string, so no single
    ///   key can be arbitrarily large.
    /// * the merge may not leave `arn` carrying more than [`MAX_TAGS`] distinct
    ///   keys.
    pub fn merge_tags(&mut self, arn: &str, tags: &[Tag]) -> Result<(), String> {
        if let Some(why) = text_rejection("ResourceArn", Some(arn)) {
            return Err(why);
        }
        if !self.license_configurations.contains_key(arn) {
            return Err(format!(
                "no license configuration exists with the ARN '{arn}'"
            ));
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
        let taken = |candidate: &str| {
            self.license_configurations
                .keys()
                .any(|arn| arn.ends_with(candidate))
        };
        for _ in 0..1024 {
            self.id_counter = self.id_counter.wrapping_add(1);
            let candidate = configuration_id_from_seed(
                now_nanos() ^ self.id_counter.rotate_left(17),
                self.id_counter,
            );
            if !taken(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded hex counters are also `lic-[0-9a-f]{32}`, and there are always
        // more of them than there are stored configurations.
        (0u64..=self.license_configurations.len() as u64)
            .map(|n| format!("{CONFIGURATION_ID_PREFIX}{n:032x}"))
            .find(|candidate| !taken(candidate))
            .unwrap_or_else(|| {
                format!(
                    "{CONFIGURATION_ID_PREFIX}{:032x}",
                    self.license_configurations.len()
                )
            })
    }
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

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN, so
/// anything that is not an AWS region token is refused here and the default is used
/// instead: a generated ARN can never carry arbitrary caller text.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// One `lic-<32 hex>` id from `seed`.
fn configuration_id_from_seed(seed: u64, salt: u64) -> String {
    let mut state = seed;
    let high = splitmix64(&mut state) ^ salt.rotate_right(29);
    let low = splitmix64(&mut state);
    format!("{CONFIGURATION_ID_PREFIX}{high:016x}{low:016x}")
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
