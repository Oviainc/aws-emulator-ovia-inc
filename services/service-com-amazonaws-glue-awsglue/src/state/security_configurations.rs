// SPDX-License-Identifier: CC0-1.0

//! The stored security-configuration records, and the usage profiles whose configuration is
//! checked by the same encryption-and-configuration slice of this service.

use super::*;

/// A stored security configuration: the set of encryption properties
/// `CreateSecurityConfiguration` registers for Glue to encrypt data at rest with.
///
/// Every member of the modelled `SecurityConfiguration` shape is held here and nothing
/// else: the model gives a security configuration no id, status or owner. The name and the
/// encryption configuration come from the request; the creation instant is the one
/// server-set member, recorded when the create stored the record and never changed
/// afterwards, because the model exposes no update for this resource.
#[derive(Debug, Clone)]
pub struct SecurityConfigurationEntry {
    /// The configuration's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::security_configurations`].
    pub name: String,
    /// "The time at which this security configuration was created", the instant the create
    /// was served, so every later read of this record reports the same timestamp.
    pub created_at: SystemTime,
    /// "The encryption configuration associated with this security configuration", stored
    /// exactly as the create sent it: a read reports the properties the client wrote and
    /// nothing the server invented.
    pub encryption_configuration: EncryptionConfiguration,
}

impl SecurityConfigurationEntry {
    /// Build a stored security configuration from a `CreateSecurityConfiguration` request
    /// served at `now`. The definition comes from the request; the creation instant is the
    /// clock's.
    pub fn from_create_input(input: &CreateSecurityConfigurationInput, now: SystemTime) -> Self {
        Self {
            name: input.name.clone(),
            created_at: now,
            encryption_configuration: input.encryption_configuration.clone(),
        }
    }

    /// Rebuild the modelled `SecurityConfiguration` shape from what is stored, so a
    /// listing reports the same configuration a single read does.
    pub fn to_security_configuration(&self) -> SecurityConfiguration {
        SecurityConfiguration {
            name: Some(self.name.clone()),
            created_time_stamp: Some(to_date_time(self.created_at)),
            encryption_configuration: Some(self.encryption_configuration.clone()),
        }
    }
}

/// One usage profile: the job and session parameter set `CreateUsageProfile` registered
/// under a name, plus the two instants the SERVER stamps on it.
///
/// Every member of the modelled `GetUsageProfileResponse` is held here, plus the create's
/// `Tags`, which that shape does not carry — so a later read reports the profile the write
/// stored and nothing invented on the way out.
#[derive(Debug, Clone)]
pub struct UsageProfileEntry {
    /// The profile's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::usage_profiles`].
    pub name: String,
    /// "A description of the usage profile", or `None` when the write named none.
    pub description: Option<String>,
    /// "A `ProfileConfiguration` object specifying the job and session values for the
    /// profile", stored exactly as the write sent it.
    ///
    /// Each of its two members is a map that is stored as sent, so an EMPTY map and an
    /// ABSENT one stay different: an empty `JobConfiguration` is a profile that configures
    /// no job parameter, while an absent one is a profile that does not speak about job
    /// parameters at all, and a read reports back whichever the write carried.
    pub configuration: ProfileConfiguration,
    /// Server-set, written once at create so every later read reports the same instant.
    pub created_on: SystemTime,
    /// Server-set; moves with every write to this profile.
    pub last_modified_on: SystemTime,
    /// The create's `Tags`, which the `GetUsageProfile` response has no member for.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations, for the reason [`CustomEntityTypeEntry::tags`]
    /// gives: [`TaggedResourceKind`] carries no usage profile variant.
    pub tags: HashMap<String, String>,
}

impl UsageProfileEntry {
    /// Build a stored usage profile from a `CreateUsageProfile` request plus the instant
    /// the create was served at, which is both its create and its last-modified instant.
    pub fn from_create_input(input: &CreateUsageProfileInput, now: SystemTime) -> Self {
        Self {
            name: input.name.clone(),
            description: input.description.clone(),
            configuration: input.configuration.clone(),
            created_on: now,
            last_modified_on: now,
            tags: input.tags.clone().unwrap_or_default(),
        }
    }
}

/// Why the service refuses a submitted `ProfileConfiguration`, as the `Message` of the
/// refusal, or `None` when every parameter it configures is coherent.
///
/// A `ConfigurationObject` states one parameter's `DefaultValue` together with the
/// `MinValue`/`MaxValue` range it may take ("Specifies the values that an admin sets for
/// each job or session parameter configured in a Glue usage profile"), so those three have
/// to agree: a default outside the range, or a minimum above the maximum, describes a
/// parameter NO value satisfies, and such a write is refused rather than stored.
///
/// Only values that are NUMBERS are ordered ([`numeric_configuration_value`]): a parameter
/// whose values are names — a worker type, a boolean — carries no range to compare, and
/// what it admits is its `AllowedValues` list rather than a bound.
///
/// The walk is in a fixed order — job parameters before session ones, and by parameter
/// name within each map, whose own iteration order is not the request's — so a
/// configuration with several incoherent parameters is always refused for the same one.
/// Within one parameter the default is checked against the maximum first, then against
/// the minimum, then the two bounds against each other, which is the order the service
/// reports a parameter breaking more than one of them in.
pub fn usage_profile_configuration_defect(configuration: &ProfileConfiguration) -> Option<String> {
    for (section, parameters) in [
        ("JobConfiguration", configuration.job_configuration.as_ref()),
        (
            "SessionConfiguration",
            configuration.session_configuration.as_ref(),
        ),
    ] {
        let Some(parameters) = parameters else {
            continue;
        };
        let mut names: Vec<&String> = parameters.keys().collect();
        names.sort();
        for name in names {
            let Some(object) = parameters.get(name) else {
                continue;
            };
            if let Some(message) = configuration_object_defect(section, name, object) {
                return Some(message);
            }
        }
    }
    None
}

/// The incoherence in ONE parameter's `ConfigurationObject`, as the message it is refused
/// with. See [`usage_profile_configuration_defect`] for the rule and the order.
///
/// The bound each message carries is the TEXT the request sent, not the number it parsed
/// to, so the refusal quotes the value the client wrote.
pub(super) fn configuration_object_defect(
    section: &str,
    parameter: &str,
    object: &ConfigurationObject,
) -> Option<String> {
    let default = object
        .default_value
        .as_deref()
        .and_then(numeric_configuration_value);
    let minimum = object
        .min_value
        .as_deref()
        .and_then(numeric_configuration_value)
        .zip(object.min_value.as_deref());
    let maximum = object
        .max_value
        .as_deref()
        .and_then(numeric_configuration_value)
        .zip(object.max_value.as_deref());

    if let Some((default, (maximum, text))) = default.zip(maximum) {
        if default > maximum {
            return Some(messages::configuration_default_above_max(
                parameter, section, text,
            ));
        }
    }
    if let Some((default, (minimum, text))) = default.zip(minimum) {
        if default < minimum {
            return Some(messages::configuration_default_below_min(
                parameter, section, text,
            ));
        }
    }
    if let Some(((minimum, _), (maximum, text))) = minimum.zip(maximum) {
        if minimum > maximum {
            return Some(messages::configuration_min_above_max(
                parameter, section, text,
            ));
        }
    }
    None
}

/// The number a `ConfigurationObject` bound states, or `None` when the value is not one.
///
/// `ConfigValueString` is free text under `^[a-zA-Z0-9_.-]+$`, so a bound may equally be a
/// count (`10`), a fraction (`0.0625`) or a name (`G.1X`). Only a decimal integer or
/// fraction, optionally signed, is read as a number: the float parser would otherwise also
/// accept `inf`, `NaN` and `1e9`, which are parameter NAMES under that pattern and not
/// quantities this service orders.
pub(super) fn numeric_configuration_value(value: &str) -> Option<f64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let well_formed = digits.chars().any(|character| character.is_ascii_digit())
        && digits
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
        && digits.matches('.').count() <= 1;
    if !well_formed {
        return None;
    }
    value.parse::<f64>().ok()
}
