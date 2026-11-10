// SPDX-License-Identifier: CC0-1.0

//! The storage and encryption configuration of
//! [`super::ComAmazonawsIotsitewiseAwsiotsitewiseState`].

use super::*;

/// Every `${VariableName}` reference `configuration` makes, as
/// `(member name, reference)` pairs — the members a refusal names.
pub(super) fn configuration_references(
    configuration: &ComputationModelConfiguration,
) -> Vec<(&'static str, &str)> {
    match configuration.anomaly_detection.as_ref() {
        Some(anomaly_detection) => vec![
            ("inputProperties", anomaly_detection.input_properties.as_str()),
            ("resultProperty", anomaly_detection.result_property.as_str()),
        ],
        None => Vec::new(),
    }
}

/// How deeply a binding value's `list` members may nest.
///
/// A binding value's `list` holds binding values, so the shape a caller sends is
/// as deep as the caller made it; this is the depth past which a value is
/// refused instead of walked, so the walk below cannot be driven by a request
/// into unbounded recursion. The emulator's own bound, not a published AWS one.
pub const MAX_BINDING_VALUE_DEPTH: usize = 8;

/// Refuse a binding value that does not set exactly one member, or that nests
/// deeper than [`MAX_BINDING_VALUE_DEPTH`]; see [`check_computation_model`].
/// `depth` is this value's own nesting level, the outermost being 1.
pub(super) fn check_binding_value(
    variable: &str,
    value: &ComputationModelDataBindingValue,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_BINDING_VALUE_DEPTH {
        return Err(format!(
            "The computationModelDataBinding value of {variable} nests lists more than \
             {MAX_BINDING_VALUE_DEPTH} deep."
        ));
    }
    let set = [
        value.asset_model_property.is_some(),
        value.asset_property.is_some(),
        value.list.is_some(),
    ]
    .into_iter()
    .filter(|member| *member)
    .count();
    if set != 1 {
        return Err(format!(
            "The computationModelDataBinding value of {variable} must set exactly one of \
             assetModelProperty, assetProperty and list, but sets {set}."
        ));
    }
    for element in value.list.iter().flatten() {
        check_binding_value(variable, element, depth + 1)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The account's storage configuration
//
// `PutStorageConfiguration` "configures storage settings for IoT SiteWise" and
// `DescribeStorageConfiguration` "retrieves information about the storage
// configuration": ONE account-wide setting rather than a resource, so the
// record it is kept in, the rules a selection is read against, and what the two
// operations report all live here once, the way the default encryption
// configuration above does.
// ---------------------------------------------------------------------------

/// The `StorageType` that keeps data in the service-managed hot tier.
///
/// Source: `smithy_query(query="shape", shape="StorageType")` — the enum holds
/// exactly this value and [`STORAGE_TYPE_MULTI_LAYER`]. It is also the tier an
/// account nothing has configured stores into, which is what the `storageType`
/// documentation states of it ("IoT SiteWise saves your data into the hot tier").
pub const STORAGE_TYPE_SITEWISE_DEFAULT: &str = "SITEWISE_DEFAULT_STORAGE";

/// The `StorageType` that keeps data in the hot tier AND in a customer managed
/// Amazon S3 bucket (the cold tier); see [`STORAGE_TYPE_SITEWISE_DEFAULT`].
pub const STORAGE_TYPE_MULTI_LAYER: &str = "MULTI_LAYER_STORAGE";

/// The `DisassociatedDataStorageState` / `WarmTierState` value that turns one of
/// those features on. Both enums hold exactly this value and
/// [`STORAGE_STATE_DISABLED`]
/// (`smithy_query(query="shape", shape="DisassociatedDataStorageState,WarmTierState")`).
pub const STORAGE_STATE_ENABLED: &str = "ENABLED";

/// The value that turns one of them off; see [`STORAGE_STATE_ENABLED`].
pub const STORAGE_STATE_DISABLED: &str = "DISABLED";

/// Whether NULL and NaN ingestion is disallowed for an account whose
/// configuration does not name it, as `disallowIngestNullNaN`'s own
/// documentation states it: "By default the feature is allowed. The feature is
/// disallowed if the value is true."
pub const DEFAULT_DISALLOW_INGEST_NULL_NAN: bool = false;

/// The `DisassociatedDataStorageState` an account carries while no put has named
/// one: time series that aren't associated with asset properties are accepted.
///
/// This is the state the service ENFORCES for such an account — ingest
/// addressed by the alias of a stream no asset property carries is accepted
/// (see `crate::ops::batch_put_asset_property_value`) — so it is also the state
/// a read of the configuration reports for the member.
pub const DEFAULT_DISASSOCIATED_DATA_STORAGE: &str = STORAGE_STATE_ENABLED;

/// The `WarmTierState` an account carries while no put has named one: the warm
/// tier is a tier the account opts into, so nothing has provisioned it and the
/// preconditions that require it are not met (see
/// `crate::ops::create_bulk_import_job`).
pub const DEFAULT_WARM_TIER: &str = STORAGE_STATE_DISABLED;

/// What a caller selects a storage configuration with: the members
/// `PutStorageConfiguration` carries, as ONE value, so the rules over them and
/// the store that keeps them name a single thing rather than seven.
#[derive(Debug, Clone, Default)]
pub struct StorageConfigurationSelection {
    /// The modelled `storageType`, required on the wire.
    pub storage_type: String,
    /// The modelled `multiLayerStorage` — the cold-tier destination, which only
    /// [`STORAGE_TYPE_MULTI_LAYER`] has one.
    pub multi_layer_storage: Option<MultiLayerStorage>,
    /// The modelled `disassociatedDataStorage`.
    pub disassociated_data_storage: Option<String>,
    /// The modelled `retentionPeriod` — how long the hot tier keeps data.
    pub retention_period: Option<RetentionPeriod>,
    /// The modelled `warmTier`.
    pub warm_tier: Option<String>,
    /// The modelled `warmTierRetentionPeriod`.
    pub warm_tier_retention_period: Option<WarmTierRetentionPeriod>,
    /// The modelled `disallowIngestNullNaN`.
    pub disallow_ingest_null_na_n: Option<bool>,
}

/// Refuse a storage configuration this service cannot accept, in the wording the
/// refusal carries; `Ok` for one it can.
///
/// The rules the shapes state, over the selection as a whole:
///
/// * `storageType` is a `StorageType`, `disassociatedDataStorage` a
///   `DisassociatedDataStorageState` and `warmTier` a `WarmTierState`, each read
///   against the generated, model-synced value table the way
///   `PutLoggingOptions` reads its own enum's;
/// * "If you specified MULTI_LAYER_STORAGE for the storage type, you must
///   specify a MultiLayerStorage object", whose two members carry an ARN each;
/// * a retention period names either a number of days or that it is unlimited —
///   "If you specified a value for this parameter, the `unlimited` parameter
///   must be `false`" and "If configured to `true`, you must not specify a value
///   for the `numberOfDays` parameter";
/// * a `warmTierRetentionPeriod` is set "only if cold tier is enabled", as the
///   member's own documentation states — the cold tier being the customer
///   managed bucket [`STORAGE_TYPE_MULTI_LAYER`] keeps a copy in.
///
/// Refusing a `multiLayerStorage` beside the hot-tier-only type is this
/// emulator's own choice rather than a documented rule, and the same one the
/// default encryption configuration makes over `kmsKeyId`: storing the
/// destination would leave the service either reporting a cold-tier destination
/// for a configuration that keeps no cold copy, or silently dropping a member
/// the caller set.
pub fn check_storage_selection(selection: &StorageConfigurationSelection) -> Result<(), String> {
    let storage_type = selection.storage_type.as_str();
    if !STORAGE_TYPE_VALUES.contains(&storage_type) {
        return Err(format!(
            "The storageType {storage_type} is not supported; it must be one of {}.",
            STORAGE_TYPE_VALUES.join(" | ")
        ));
    }
    match (
        storage_type == STORAGE_TYPE_MULTI_LAYER,
        selection.multi_layer_storage.as_ref(),
    ) {
        (true, None) => {
            return Err(format!(
                "A multiLayerStorage is required when storageType is {STORAGE_TYPE_MULTI_LAYER}."
            ))
        }
        (false, Some(_)) => {
            return Err(format!(
                "A multiLayerStorage must be omitted when storageType is \
                 {STORAGE_TYPE_SITEWISE_DEFAULT}."
            ))
        }
        (true, Some(storage)) => check_customer_managed_s3(&storage.customer_managed_s3_storage)?,
        (false, None) => {}
    }
    check_storage_state(
        "disassociatedDataStorage",
        selection.disassociated_data_storage.as_deref(),
        DISASSOCIATED_DATA_STORAGE_STATE_VALUES,
    )?;
    check_storage_state(
        "warmTier",
        selection.warm_tier.as_deref(),
        WARM_TIER_STATE_VALUES,
    )?;
    if let Some(period) = selection.retention_period.as_ref() {
        check_retention_period("retentionPeriod", period.number_of_days, period.unlimited)?;
    }
    if let Some(period) = selection.warm_tier_retention_period.as_ref() {
        if storage_type != STORAGE_TYPE_MULTI_LAYER {
            return Err(format!(
                "A warmTierRetentionPeriod can be set only when storageType is \
                 {STORAGE_TYPE_MULTI_LAYER}."
            ));
        }
        check_retention_period(
            "warmTierRetentionPeriod",
            period.number_of_days,
            period.unlimited,
        )?;
    }
    Ok(())
}

/// Refuse a value outside the enum `member` is modelled as, by that enum's
/// generated value table. An absent member selects nothing and is left to the
/// documented default.
pub(super) fn check_storage_state(member: &str, value: Option<&str>, values: &[&str]) -> Result<(), String> {
    match value {
        Some(value) if !values.contains(&value) => Err(format!(
            "The {member} {value} is not supported; it must be one of {}.",
            values.join(" | ")
        )),
        _ => Ok(()),
    }
}

/// Refuse a retention period that names no period, or names two.
///
/// Both retention-period shapes carry the same pair of members with the same
/// rules, so they are read here rather than once per member: a number of days
/// goes with `unlimited` false, `unlimited` true goes with no number of days,
/// and a period that names neither selects no retention at all.
///
/// A count of days below one is no period either, whatever the member it is
/// given in: the modelled `NumberOfDays` is a plain integer with no range
/// (`smithy_query(query="shape", shape="NumberOfDays")`), so nothing in the
/// model or the member documentation states a minimum, and the only bound read
/// here is that a span of days is a positive number of them. This emulator's
/// own floor, not a published AWS one — a larger minimum is left to the
/// modelled constraint walk, which is where a documented one would land.
pub(super) fn check_retention_period(
    member: &str,
    number_of_days: Option<i32>,
    unlimited: Option<bool>,
) -> Result<(), String> {
    match (number_of_days, unlimited.unwrap_or(false)) {
        (Some(_), true) => Err(format!(
            "A {member} must not specify numberOfDays when unlimited is true."
        )),
        (None, false) => Err(format!(
            "A {member} must specify either numberOfDays or unlimited true."
        )),
        (Some(days), false) if days < 1 => Err(format!(
            "The {member} numberOfDays {days} is not a number of days; it must be at least 1."
        )),
        _ => Ok(()),
    }
}

/// Refuse a cold-tier destination whose members do not carry what they are
/// modelled as: `s3ResourceArn` and `roleArn` are both a modelled `ARN`, so a
/// value outside that type is one this service could not report back.
pub(super) fn check_customer_managed_s3(storage: &CustomerManagedS3Storage) -> Result<(), String> {
    for (member, value) in [
        ("s3ResourceArn", &storage.s3_resource_arn),
        ("roleArn", &storage.role_arn),
    ] {
        if !is_modelled_arn(value) || value.chars().count() > ARN_MAX_LENGTH {
            return Err(format!("The {member} {value} is not an ARN this service can store."));
        }
    }
    Ok(())
}

/// The storage configuration a put stored: the selection it named, when it
/// named it, and when the configuration it accepted falls due to be in force.
#[derive(Debug, Clone)]
pub struct StorageConfigurationRecord {
    /// What the put selected. Replaced whole by the next put.
    pub selection: StorageConfigurationSelection,
    /// The instant of the put, which `DescribeStorageConfiguration` reports as
    /// `lastUpdateDate` ("The date the storage configuration was last updated").
    pub updated_at: SystemTime,
    /// When [`CONFIGURATION_STATE_UPDATE_IN_PROGRESS`] ->
    /// [`CONFIGURATION_STATE_ACTIVE`] falls due: the instant of the put plus
    /// [`ACTIVATION_DELAY`]. Stored rather than slept for, so nothing blocks and
    /// a caller reading straight after the put still observes the state the
    /// service reports while it applies the change.
    pub active_at: SystemTime,
}

impl StorageConfigurationRecord {
    /// The `ConfigurationState` this configuration reports at `now`.
    pub fn state(&self, now: SystemTime) -> &'static str {
        configuration_state(self.active_at, now)
    }
}

/// What the two storage-configuration operations report: the same members, so
/// they read them from one place.
#[derive(Debug, Clone)]
pub struct StorageConfiguration {
    /// The configuration in force, with the state the account carries filled in
    /// for each member it does not name — see [`documented_defaults`].
    pub selection: StorageConfigurationSelection,
    /// The modelled `configurationStatus`.
    pub status: ConfigurationStatus,
    /// The modelled `lastUpdateDate`, absent while no put has configured the
    /// account: nothing has updated the configuration then.
    pub last_update_date: Option<SystemTime>,
}

/// `selection` with the state the account carries filled in for each member it
/// leaves unnamed: hot-tier data is "kept indefinitely" when no
/// `retentionPeriod` names a span, NULL/NaN ingestion is allowed when no
/// `disallowIngestNullNaN` disallows it, and each tier-state member carries the
/// state the service enforces for an account that never named it
/// ([`DEFAULT_DISASSOCIATED_DATA_STORAGE`] / [`DEFAULT_WARM_TIER`]). The members
/// that name a RESOURCE rather than a state (`multiLayerStorage` and the warm
/// tier's own retention period) have no such value and are reported only when
/// the configuration names them.
pub(super) fn documented_defaults(selection: &StorageConfigurationSelection) -> StorageConfigurationSelection {
    StorageConfigurationSelection {
        retention_period: selection.retention_period.clone().or(Some(RetentionPeriod {
            number_of_days: None,
            unlimited: Some(true),
        })),
        disassociated_data_storage: selection
            .disassociated_data_storage
            .clone()
            .or_else(|| Some(DEFAULT_DISASSOCIATED_DATA_STORAGE.to_string())),
        warm_tier: selection
            .warm_tier
            .clone()
            .or_else(|| Some(DEFAULT_WARM_TIER.to_string())),
        disallow_ingest_null_na_n: selection
            .disallow_ingest_null_na_n
            .or(Some(DEFAULT_DISALLOW_INGEST_NULL_NAN)),
        ..selection.clone()
    }
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The storage configuration the account reports at `now`: the one the last
    /// put stored, or the hot-tier default while nothing has stored any.
    ///
    /// A pure READ: an unconfigured account is not materialised, and a
    /// configuration still settling is reported from the instant its put already
    /// recorded, so reading cannot change what a later caller sees.
    pub fn storage_configuration(&self, now: SystemTime) -> StorageConfiguration {
        let status = |state: &str| ConfigurationStatus {
            state: state.to_string(),
            // No error: nothing in this emulator fails to apply a
            // configuration, so the modelled `error` details have nothing to
            // report.
            error: None,
        };
        match &self.storage {
            Some(record) => StorageConfiguration {
                selection: documented_defaults(&record.selection),
                status: status(record.state(now)),
                last_update_date: Some(record.updated_at),
            },
            None => StorageConfiguration {
                selection: documented_defaults(&StorageConfigurationSelection {
                    storage_type: STORAGE_TYPE_SITEWISE_DEFAULT.to_string(),
                    ..Default::default()
                }),
                status: status(CONFIGURATION_STATE_ACTIVE),
                last_update_date: None,
            },
        }
    }

    /// Make `selection` the account's storage configuration, and report what
    /// that call answers with; `Err` with the refusal's wording for a selection
    /// the configuration in force does not admit.
    ///
    /// The whole setting is replaced: a put is the last word on the account's
    /// storage, so one naming the hot tier alone drops the cold-tier destination
    /// an earlier put selected. The one member that does not go back is
    /// `disassociatedDataStorage` — "After the disassociatedDataStorage is
    /// enabled, you can't disable it" — so a selection that would leave a state
    /// a PUT enabled anything but enabled is refused here, under the same lock
    /// the write takes, rather than in the handler where two puts could race
    /// past it. That rule is read against what a put NAMED rather than against
    /// the state [`documented_defaults`] reports for an account no put has
    /// configured: nothing has enabled a feature such an account merely carries.
    /// The configuration the put accepts falls due at [`ACTIVATION_DELAY`], the
    /// span this service settles every transition the clock owes at.
    ///
    /// Read `selection` against [`check_storage_selection`] first: this stores
    /// what it is given.
    pub fn set_storage_configuration(
        &mut self,
        selection: StorageConfigurationSelection,
        now: SystemTime,
    ) -> Result<StorageConfiguration, String> {
        let enabled = |state: Option<&str>| state == Some(STORAGE_STATE_ENABLED);
        let named_before = self
            .storage
            .as_ref()
            .and_then(|record| record.selection.disassociated_data_storage.as_deref());
        if enabled(named_before) && !enabled(selection.disassociated_data_storage.as_deref()) {
            return Err(format!(
                "The disassociatedDataStorage is {STORAGE_STATE_ENABLED} and cannot be disabled; \
                 a put must keep naming {STORAGE_STATE_ENABLED}."
            ));
        }
        self.storage = Some(StorageConfigurationRecord {
            selection,
            updated_at: now,
            active_at: now + ACTIVATION_DELAY,
        });
        Ok(self.storage_configuration(now))
    }
}
