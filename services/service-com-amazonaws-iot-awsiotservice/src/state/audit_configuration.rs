// SPDX-License-Identifier: CC0-1.0

//! The account's DEVICE DEFENDER AUDIT CONFIGURATION: which audit checks are enabled,
//! where audit notifications are sent, and the role the audit runs under.
//!
//! One record for the account and not a map, for the reason
//! [`super::IndexingConfiguration`] is one: `DescribeAccountAuditConfiguration` and
//! `UpdateAccountAuditConfiguration` name no resource
//! (`smithy_query(query="operation",operation="DescribeAccountAuditConfiguration")` binds
//! `GET /audit/configuration` with an empty request), so there is one audit
//! configuration per account. An account that has never called the update still HAS one —
//! every check disabled, no notification target enabled and no role — which is what makes
//! the read a pure read: it never has to create the record it reports.
//!
//! The record holds only what a caller CONFIGURED. What a reader reports is the whole
//! catalog of audit checks and of notification target types, each carrying the account's
//! setting for it or the disabled default
//! ([`AccountAuditConfiguration::check_configurations`],
//! [`AccountAuditConfiguration::notification_target_configurations`]): the model documents
//! the read as the way to "see the list of all checks, including those that are enabled"
//! (`smithy_query(query="doc",shape="UpdateAccountAuditConfigurationRequest",member="auditCheckConfigurations")`),
//! so a check nobody has configured is reported as disabled rather than left out.

use std::collections::{BTreeMap, HashMap};

use crate::ops::types::{AuditCheckConfiguration, AuditNotificationTarget};

/// Every Device Defender audit check this account can enable, in the order a reader
/// reports them.
///
/// `AuditCheckName` is a plain string in the model
/// (`smithy_query(query="shape",shape="AuditCheckName")`) whose documentation points at
/// `DescribeAccountAuditConfiguration` for "the list of all checks", so the catalog is
/// not in the model: these are the check names AWS IoT Device Defender documents for
/// `auditCheckConfigurations` (the audit checks reference of the Device Defender
/// developer guide), which are the names an audit task and a scheduled audit's
/// `targetCheckNames` select from.
pub const AUDIT_CHECK_NAMES: &[&str] = &[
    "AUTHENTICATED_COGNITO_ROLE_OVERLY_PERMISSIVE_CHECK",
    "CA_CERTIFICATE_EXPIRING_CHECK",
    "CA_CERTIFICATE_KEY_QUALITY_CHECK",
    "CONFLICTING_CLIENT_IDS_CHECK",
    "DEVICE_CERTIFICATE_AGE_CHECK",
    "DEVICE_CERTIFICATE_EXPIRING_CHECK",
    "DEVICE_CERTIFICATE_KEY_QUALITY_CHECK",
    "DEVICE_CERTIFICATE_SHARED_CHECK",
    "INTERMEDIATE_CA_REVOKED_FOR_ACTIVE_DEVICE_CERTIFICATES_CHECK",
    "IOT_POLICY_OVERLY_PERMISSIVE_CHECK",
    "IOT_POLICY_POTENTIAL_MISCONFIGURATION_CHECK",
    "IOT_ROLE_ALIAS_ALLOWS_ACCESS_TO_UNUSED_SERVICES_CHECK",
    "IOT_ROLE_ALIAS_OVERLY_PERMISSIVE_CHECK",
    "LOGGING_DISABLED_CHECK",
    "REVOKED_CA_CERTIFICATE_STILL_ACTIVE_CHECK",
    "REVOKED_DEVICE_CERTIFICATE_STILL_ACTIVE_CHECK",
    "UNAUTHENTICATED_COGNITO_ROLE_OVERLY_PERMISSIVE_CHECK",
];

/// Is `check_name` one of the account's audit checks ([`AUDIT_CHECK_NAMES`])?
pub fn is_audit_check(check_name: &str) -> bool {
    AUDIT_CHECK_NAMES.contains(&check_name)
}

/// The account's audit configuration, as this service persists it.
///
/// Every map holds ONLY what an update configured; a reader fills the rest of the
/// catalog in with the disabled default, so the two cannot drift about what a check
/// nobody configured reports.
///
/// The record is BOUNDED by its own catalogs rather than by a separate ceiling, because
/// every key a caller can write is one of a fixed set: a key that names no audit check,
/// no modelled `configName` or no modelled `AuditNotificationType` is refused before
/// anything is written ([`AccountAuditConfiguration::apply_update`]), so the record holds
/// at most one entry per [`AUDIT_CHECK_NAMES`] name — each with at most one entry per
/// modelled `configName`, whose value carries the model's own `length 1..64` — and one
/// per modelled `AuditNotificationType`, however many updates arrive.
#[derive(Debug, Default, Clone)]
pub struct AccountAuditConfiguration {
    /// "The ARN of the role that grants permission to IoT to access information about
    /// your devices, policies, certificates, and other items as required when
    /// performing an audit", as the last update supplied it.
    ///
    /// `None` until the first `UpdateAccountAuditConfiguration`, which is also how this
    /// record answers whether the account HAS configured auditing: the model documents
    /// "On the first call to UpdateAccountAuditConfiguration, this parameter is
    /// required".
    pub role_arn: Option<String>,
    /// The per-check settings an update configured, keyed by audit check name.
    ///
    /// The modelled `AuditCheckConfiguration` is stored as it arrived — its `enabled`
    /// flag and its `configuration` map of `configName` to `configValue` — so the reader
    /// hands back exactly what the writer recorded. `BTreeMap` (not `HashMap`) so the
    /// stored order is stable, like every other record in this store. At most one entry
    /// per [`AUDIT_CHECK_NAMES`] name: a key outside the catalog is refused.
    pub checks: BTreeMap<String, AuditCheckConfiguration>,
    /// The notification targets an update configured, keyed by the modelled
    /// `AuditNotificationType` — at most one entry per value that enum declares, since a
    /// key outside it is refused.
    pub notification_targets: BTreeMap<String, AuditNotificationTarget>,
}

/// Why a writer refused an audit configuration update, mapped to the operation's
/// modelled `InvalidRequestException` by the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditConfigurationRefusal {
    /// A key of `auditCheckConfigurations` that names no audit check
    /// ([`AUDIT_CHECK_NAMES`]).
    UnknownCheck(String),
    /// A key of one check's `configuration` map that names no `configName` the model
    /// declares.
    UnknownConfigName {
        /// The check whose configuration carried it.
        check_name: String,
        /// The key that names no `configName`.
        config_name: String,
    },
    /// A key of `auditNotificationTargetConfigurations` that names no
    /// `AuditNotificationType` the model declares.
    UnknownNotificationType(String),
    /// The account has no audit configuration yet and the request supplied no
    /// `roleArn`.
    RoleArnRequired,
    /// The account has no audit configuration yet and the request enabled no check.
    EnabledCheckRequired,
}

impl AuditConfigurationRefusal {
    /// The message a handler refuses the request with — the member and the rule it
    /// broke, so the caller can fix the request from the text alone.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownCheck(check_name) => {
                format!("Invalid audit check name: {check_name}")
            }
            Self::UnknownConfigName {
                check_name,
                config_name,
            } => {
                format!("Invalid configName for audit check {check_name}: {config_name}")
            }
            Self::UnknownNotificationType(notification_type) => {
                format!("Invalid audit notification type: {notification_type}")
            }
            Self::RoleArnRequired => "On the first call to UpdateAccountAuditConfiguration, \
                 roleArn is required"
                .to_string(),
            Self::EnabledCheckRequired => {
                "On the first call to UpdateAccountAuditConfiguration, auditCheckConfigurations \
                 must specify at least one enabled check"
                    .to_string()
            }
        }
    }
}

impl AccountAuditConfiguration {
    /// Has the account configured auditing at all?
    ///
    /// The role is what the first update must supply, so a record holding one is a
    /// record some update wrote.
    pub fn is_configured(&self) -> bool {
        self.role_arn.is_some()
    }

    /// Record `role_arn`, `checks` and `notification_targets` on this configuration:
    /// the ONE write every writer of the account's audit configuration makes.
    ///
    /// A member the request OMITS changes nothing — all three are optional — and a
    /// supplied map is merged key by key, because a check or a target is configured
    /// individually: the model documents that a caller enables and disables single
    /// checks and that a check in use by a scheduled audit cannot be disabled, which is
    /// a statement about one key, not about the whole map.
    ///
    /// Every rule is checked before ANYTHING is written, so a request the service
    /// refuses leaves the stored configuration exactly as it was.
    pub fn apply_update(
        &mut self,
        role_arn: Option<String>,
        checks: Option<HashMap<String, AuditCheckConfiguration>>,
        notification_targets: Option<HashMap<String, AuditNotificationTarget>>,
    ) -> Result<(), AuditConfigurationRefusal> {
        if let Some(checks) = checks.as_ref() {
            // A map KEY is not refused by the boundary validator — neither
            // `auditCheckConfigurations`'s key, which the model leaves a plain string,
            // nor a key the model does model as an enum — so the store is what knows
            // every catalog here. Refusing a key outside its catalog is also what keeps
            // the record bounded: at most one entry per audit check, each carrying at
            // most one entry per modelled `configName`.
            for (check_name, configuration) in checks {
                if !is_audit_check(check_name) {
                    return Err(AuditConfigurationRefusal::UnknownCheck(check_name.clone()));
                }
                for config_name in configuration.configuration.iter().flat_map(HashMap::keys) {
                    if !crate::ops::types::CONFIG_NAME_VALUES.contains(&config_name.as_str()) {
                        return Err(AuditConfigurationRefusal::UnknownConfigName {
                            check_name: check_name.clone(),
                            config_name: config_name.clone(),
                        });
                    }
                }
            }
        }
        if let Some(notification_targets) = notification_targets.as_ref() {
            for notification_type in notification_targets.keys() {
                if !crate::ops::types::AUDIT_NOTIFICATION_TYPE_VALUES
                    .contains(&notification_type.as_str())
                {
                    return Err(AuditConfigurationRefusal::UnknownNotificationType(
                        notification_type.clone(),
                    ));
                }
            }
        }
        if !self.is_configured() {
            if role_arn.is_none() {
                return Err(AuditConfigurationRefusal::RoleArnRequired);
            }
            let enables_a_check = checks
                .as_ref()
                .is_some_and(|checks| checks.values().any(|check| check.enabled));
            if !enables_a_check {
                return Err(AuditConfigurationRefusal::EnabledCheckRequired);
            }
        }
        if let Some(role_arn) = role_arn {
            self.role_arn = Some(role_arn);
        }
        for (check_name, configuration) in checks.unwrap_or_default() {
            self.checks.insert(check_name, configuration);
        }
        for (notification_type, target) in notification_targets.unwrap_or_default() {
            self.notification_targets.insert(notification_type, target);
        }
        Ok(())
    }

    /// Every audit check, each carrying the account's setting for it or the disabled
    /// default — what `DescribeAccountAuditConfiguration` reports as
    /// `auditCheckConfigurations`.
    pub fn check_configurations(&self) -> HashMap<String, AuditCheckConfiguration> {
        AUDIT_CHECK_NAMES
            .iter()
            .map(|check_name| {
                // A check nobody configured is reported disabled, which is the
                // modelled default of both members (`enabled` is `@default(false)` and
                // `configuration` is optional).
                let configuration = self.checks.get(*check_name).cloned().unwrap_or_default();
                ((*check_name).to_string(), configuration)
            })
            .collect()
    }

    /// Every notification target type the model declares
    /// (`AUDIT_NOTIFICATION_TYPE_VALUES`), each carrying the account's target for it or
    /// the disabled default — what `DescribeAccountAuditConfiguration` reports as
    /// `auditNotificationTargetConfigurations`.
    pub fn notification_target_configurations(&self) -> HashMap<String, AuditNotificationTarget> {
        crate::ops::types::AUDIT_NOTIFICATION_TYPE_VALUES
            .iter()
            .map(|notification_type| {
                // A target nobody configured is reported disabled, under the same
                // modelled defaults as a check nobody configured.
                let target = self
                    .notification_targets
                    .get(*notification_type)
                    .cloned()
                    .unwrap_or_default();
                ((*notification_type).to_string(), target)
            })
            .collect()
    }
}
