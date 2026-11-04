// SPDX-License-Identifier: CC0-1.0

//! The configuration-set plane of this service's state: one configuration set, the
//! event destinations it publishes to, and the wording every operation that addresses
//! one refuses with.
//!
//! Split out of the state module root so the records a configuration-set operation
//! reads and writes live beside the rules that govern them; every item is re-exported
//! from [`crate::state`], so a handler still reaches it as `crate::state::…`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{modelled_enum_value, MAX_CONFIGURATION_SET_NAME_LEN};

/// Whether `name` is a `ConfigurationSetName` SES accepts.
///
/// From the member's documentation: "The name can contain up to 64 alphanumeric
/// characters, including letters, numbers, hyphens (-) and underscores (_) only" —
/// so a name is a non-empty run of at most [`MAX_CONFIGURATION_SET_NAME_LEN`] ASCII
/// letters, digits, hyphens and underscores.
pub fn configuration_set_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_CONFIGURATION_SET_NAME_LEN
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// How this service words the refusal of a `ConfigurationSetName` outside the
/// documented character and length rule, stated once for every operation that takes
/// one.
pub fn configuration_set_name_invalid(configuration_set_name: &str) -> String {
    format!(
        "Configuration set name {configuration_set_name} is invalid. The name can contain up to \
         {MAX_CONFIGURATION_SET_NAME_LEN} alphanumeric characters, including letters, numbers, \
         hyphens (-) and underscores (_) only."
    )
}

/// How this service words `NotFoundException` for a configuration set it does not
/// hold.
///
/// One wording for every operation that takes a `ConfigurationSetName`, so a client
/// sees the same sentence whichever call first tells it the set is not there. SES
/// delimits the name it interpolated with angle brackets, so the sentence stays
/// readable for a name that is itself a sentence fragment.
pub fn configuration_set_not_found(configuration_set_name: &str) -> String {
    format!("Configuration set <{configuration_set_name}> does not exist.")
}

/// How this service words `AlreadyExistsException` for a configuration-set name the
/// account already holds.
///
/// One wording for every operation that mints a configuration set. The sentence is
/// fixed text around the one variable part, the name from the request, which SES
/// delimits with angle brackets exactly as it does in the sentence for a set it does
/// not hold ([`configuration_set_not_found`]) — the form every refusal of this service
/// that names the resource the request addressed takes.
pub fn configuration_set_already_exists(configuration_set_name: &str) -> String {
    format!("Configuration set <{configuration_set_name}> already exists.")
}

/// How this service words the refusal of a send whose configuration set has sending
/// disabled (`SendingOptions$SendingEnabled` false, which
/// `PutConfigurationSetSendingOptions` is what turns off).
///
/// One wording for every send path that resolves a `ConfigurationSetName`, so whichever
/// send a client tries while the set is paused reports the pause in the same sentence.
/// The set's name is the only variable part, taken from the request.
pub fn configuration_set_sending_paused(configuration_set_name: &str) -> String {
    format!("Sending is paused for configuration set {configuration_set_name}")
}

/// One configuration set: the rules a send that names it is subject to.
///
/// A member a caller configured is held as supplied, and one no caller has configured
/// is held ABSENT rather than as the documented default: a read reports the
/// configuration the set carries and nothing it never took. A send through a set that
/// holds no `DeliveryOptions` is delivered under SES's own policy, which `TlsPolicy`
/// documents as the opportunistic one ("if the value is Optional, messages can be
/// delivered in plain text if a TLS connection can't be established"); that fall-back is
/// the policy itself, never a value this record stores.
#[derive(Debug, Clone)]
pub struct ConfigurationSetRecord {
    /// The `ConfigurationSetName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `TrackingOptions`: the custom open/click redirect domain, when the caller
    /// configured one. SES tracks through its own domain otherwise, so an
    /// unconfigured set holds nothing here.
    pub tracking_options: Option<crate::ops::types::TrackingOptions>,
    /// `DeliveryOptions`, as supplied: a set whose delivery nobody configured holds
    /// nothing here and sends under SES's own opportunistic policy.
    pub delivery_options: Option<crate::ops::types::DeliveryOptions>,
    /// `ReputationOptions$ReputationMetricsEnabled`: whether SES publishes this set's
    /// reputation metrics, which a caller turns off rather than on (see
    /// [`ConfigurationSetRecord::new`]).
    pub reputation_metrics_enabled: bool,
    /// `SendingOptions$SendingEnabled`.
    pub sending_enabled: bool,
    /// `SuppressionOptions`, as supplied: a set that configures none falls back to
    /// the tenant or account suppression preferences, so nothing is stored.
    pub suppression_options: Option<crate::ops::types::SuppressionOptions>,
    /// `VdmOptions`, as supplied.
    pub vdm_options: Option<crate::ops::types::VdmOptions>,
    /// `MessageSecurityOptions`, as supplied.
    pub message_security_options: Option<crate::ops::types::MessageSecurityOptions>,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// The event destinations this set publishes its events to, keyed by the
    /// `EventDestinationName` they were created with — a name that "identifies the
    /// event destination within the configuration set", so it is keyed exactly as
    /// supplied and unique only within this set. Ordered, so a read enumerates them
    /// deterministically.
    pub event_destinations: BTreeMap<String, EventDestinationRecord>,
    /// When the set was created.
    pub created_at: SystemTime,
    /// When the set's reputation metrics were last given a fresh start.
    ///
    /// `ReputationOptions$LastFreshStart` is "the date and time when the reputation
    /// metrics were last given a fresh start", and SES documents that where sending
    /// has never been disabled and re-enabled this is the time the configuration set
    /// was created \u2014 so it starts at the create instant and moves only when a fresh
    /// start happens.
    pub last_fresh_start: SystemTime,
}

impl ConfigurationSetRecord {
    /// A set as `CreateConfigurationSet` stores it when the request configures
    /// nothing.
    pub fn new(name: &str, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            tracking_options: None,
            delivery_options: None,
            // Publishing a configuration set's reputation metrics is something a caller
            // turns OFF (`PutConfigurationSetReputationOptions` "enable or disable
            // collection of reputation metrics for emails that you send using this
            // configuration set"): a recorded read of a set created without reputation
            // options reports collection as enabled, so that is the state a set starts
            // in and a request is what moves it.
            reputation_metrics_enabled: true,
            // Sending through a configuration set is something a caller PAUSES
            // (`PutConfigurationSetSendingOptions` "enable or disable email sending
            // for messages that use this configuration set"), so a newly created set
            // does not itself hold sending back.
            sending_enabled: true,
            suppression_options: None,
            vdm_options: None,
            message_security_options: None,
            tags: Vec::new(),
            // A set publishes events only where a caller attaches a destination to
            // it, so a newly created set has none.
            event_destinations: BTreeMap::new(),
            created_at: now,
            last_fresh_start: now,
        }
    }

    /// The modelled `ReputationOptions` view of this record.
    pub fn reputation_options(&self) -> crate::ops::types::ReputationOptions {
        crate::ops::types::ReputationOptions {
            reputation_metrics_enabled: self.reputation_metrics_enabled,
            last_fresh_start: Some(::aws_smithy_types::DateTime::from(self.last_fresh_start)),
        }
    }

    /// The modelled `SendingOptions` view of this record.
    pub fn sending_options(&self) -> crate::ops::types::SendingOptions {
        crate::ops::types::SendingOptions {
            sending_enabled: self.sending_enabled,
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// One event destination of a configuration set: where the events a send through
/// that set produces are published to, and which of them are.
#[derive(Debug, Clone)]
pub struct EventDestinationRecord {
    /// The `EventDestinationName` this record is keyed by, as the request spelled it.
    pub name: String,
    /// `EventDestinationDefinition$Enabled`: whether SES publishes to this
    /// destination at all.
    pub enabled: bool,
    /// `EventDestinationDefinition$MatchingEventTypes`: the event types this
    /// destination receives, in the order SES reports them
    /// ([`sorted_event_types`]) rather than the order one request happened to list
    /// them in. A definition that named none receives nothing, which is an empty list
    /// rather than an absent member — `EventDestination` models the member as required.
    pub matching_event_types: Vec<String>,
    /// The Kinesis Data Firehose stream this destination publishes to, when it is one.
    pub kinesis_firehose_destination: Option<crate::ops::types::KinesisFirehoseDestination>,
    /// The CloudWatch dimensions this destination publishes, when it is one.
    pub cloud_watch_destination: Option<crate::ops::types::CloudWatchDestination>,
    /// The SNS topic this destination publishes to, when it is one.
    pub sns_destination: Option<crate::ops::types::SnsDestination>,
    /// The EventBridge event bus this destination publishes to, when it is one.
    pub event_bridge_destination: Option<crate::ops::types::EventBridgeDestination>,
    /// The Pinpoint application this destination publishes to, when it is one.
    pub pinpoint_destination: Option<crate::ops::types::PinpointDestination>,
}

impl EventDestinationRecord {
    /// The destination `name` as the definition in a create or update request
    /// describes it.
    pub fn new(name: &str, definition: crate::ops::types::EventDestinationDefinition) -> Self {
        Self {
            name: name.to_string(),
            enabled: definition.enabled,
            matching_event_types: sorted_event_types(
                definition.matching_event_types.unwrap_or_default(),
            ),
            kinesis_firehose_destination: definition.kinesis_firehose_destination,
            cloud_watch_destination: definition.cloud_watch_destination,
            sns_destination: definition.sns_destination,
            event_bridge_destination: definition.event_bridge_destination,
            pinpoint_destination: definition.pinpoint_destination,
        }
    }

    /// Make `definition` this destination's configuration.
    ///
    /// `UpdateConfigurationSetEventDestination` "update[s] the configuration of an
    /// event destination", and the `EventDestinationDefinition` it requires is a
    /// whole definition of that destination — which services receive events from the
    /// set and which event types they receive — so afterwards the destination
    /// publishes what the definition describes and nothing else. The name is not part
    /// of the definition: it identifies the destination within the set and an update
    /// addresses the destination by it, so it is kept.
    pub fn replace_definition(&mut self, definition: crate::ops::types::EventDestinationDefinition) {
        let name = self.name.clone();
        *self = Self::new(&name, definition);
    }

    /// The modelled `EventDestination` view of this record.
    ///
    /// One projection shared by every operation that reports a set's event
    /// destinations, so a create and the read after it cannot disagree.
    pub fn view(&self) -> crate::ops::types::EventDestination {
        crate::ops::types::EventDestination {
            name: self.name.clone(),
            enabled: self.enabled,
            matching_event_types: self.matching_event_types.clone(),
            kinesis_firehose_destination: self.kinesis_firehose_destination.clone(),
            cloud_watch_destination: self.cloud_watch_destination.clone(),
            sns_destination: self.sns_destination.clone(),
            event_bridge_destination: self.event_bridge_destination.clone(),
            pinpoint_destination: self.pinpoint_destination.clone(),
        }
    }
}

/// `event_types` in the order SES reports a destination's matching event types:
/// ascending by the event-type name.
///
/// The order is SES's own and not the order a request listed the types in — a recorded
/// read of a destination created with its types in another order answers them
/// ascending — so it is applied where a destination's types enter the store, and every
/// read of that destination reports the one order.
pub fn sorted_event_types(mut event_types: Vec<String>) -> Vec<String> {
    event_types.sort();
    event_types
}

/// How this service words `AlreadyExistsException` for an event-destination name a
/// configuration set already holds, stated once for every operation that attaches
/// one.
pub fn event_destination_already_exists(
    configuration_set_name: &str,
    event_destination_name: &str,
) -> String {
    format!(
        "Event destination {event_destination_name} already exists in configuration set \
         {configuration_set_name}."
    )
}

/// How this service words `NotFoundException` for an event destination a configuration
/// set does not hold.
///
/// One wording for every operation that addresses a destination within a set, so a
/// client sees the same sentence whichever call first tells it the destination is not
/// there. The sentence is fixed text naming the kind of resource that was not found,
/// and its one variable part is the CONFIGURATION SET the request addressed the
/// destination within, taken from the request path; the destination's own name is not
/// part of it.
pub fn event_destination_not_found(configuration_set_name: &str) -> String {
    format!("No EventDestination found for {configuration_set_name}")
}

/// The delivery configuration a write of ONE configuration set stores, from the members
/// the request carried, or the refusal wording for a value outside a modelled value set.
///
/// Shared by every operation that writes a set's delivery configuration, so the
/// configuration a set ends up with does not depend on which one wrote it. A member the
/// request does not state is stored ABSENT rather than defaulted, so a read reports what
/// the caller configured and a send through a set that configured nothing is delivered
/// under SES's own opportunistic policy; a request that states NO member at all leaves the
/// set carrying no delivery configuration — `Ok(None)` — so a read reports no member
/// rather than an object a caller configured nothing in. `TlsPolicy` carries its value as
/// a plain string, so its value set is enforced here with the service's shared wording
/// ([`modelled_enum_value`]).
pub fn configuration_set_delivery_options(
    options: crate::ops::types::DeliveryOptions,
) -> Result<Option<crate::ops::types::DeliveryOptions>, String> {
    let tls_policy = match options.tls_policy {
        Some(policy) => Some(modelled_enum_value(
            "TlsPolicy",
            policy,
            crate::ops::types::TLS_POLICY_VALUES,
        )?),
        None => None,
    };
    let configured = crate::ops::types::DeliveryOptions {
        tls_policy,
        sending_pool_name: options.sending_pool_name,
        // The modelled range (300..=50400 seconds) is refused on the request boundary,
        // so a value that reaches here is one SES would attempt delivery for.
        max_delivery_seconds: options.max_delivery_seconds,
    };
    let states_a_member = configured.tls_policy.is_some()
        || configured.sending_pool_name.is_some()
        || configured.max_delivery_seconds.is_some();
    Ok(states_a_member.then_some(configured))
}

/// The `VdmOptions` a write of ONE configuration set's VDM preferences stores, from the
/// members the request carried, or the refusal wording for a value outside a modelled
/// value set.
///
/// Shared by every operation that writes a set's VDM preferences, so the preferences a
/// set ends up with do not depend on which one wrote them. Each nested member carries
/// its `FeatureStatus` as a plain string, so the value set is enforced here with the
/// service's shared wording ([`modelled_enum_value`]), and a member the request does
/// not state is stored ABSENT rather than defaulted.
pub fn configuration_set_vdm_options(
    options: crate::ops::types::VdmOptions,
) -> Result<crate::ops::types::VdmOptions, String> {
    let dashboard_options = match options.dashboard_options {
        Some(dashboard) => Some(crate::ops::types::DashboardOptions {
            engagement_metrics: match dashboard.engagement_metrics {
                Some(status) => Some(modelled_enum_value(
                    "EngagementMetrics",
                    status,
                    crate::ops::types::FEATURE_STATUS_VALUES,
                )?),
                None => None,
            },
        }),
        None => None,
    };
    let guardian_options = match options.guardian_options {
        Some(guardian) => Some(crate::ops::types::GuardianOptions {
            optimized_shared_delivery: match guardian.optimized_shared_delivery {
                Some(status) => Some(modelled_enum_value(
                    "OptimizedSharedDelivery",
                    status,
                    crate::ops::types::FEATURE_STATUS_VALUES,
                )?),
                None => None,
            },
        }),
        None => None,
    };
    Ok(crate::ops::types::VdmOptions {
        dashboard_options,
        guardian_options,
    })
}

/// `definition` once every enum-valued member in it carries one of the model's own
/// values for that member, and the message naming the offending member otherwise.
///
/// One check for every operation that takes an `EventDestinationDefinition`, so the
/// create of a destination and an update of the same destination cannot accept
/// different values. The value sets are the generated consts, so they follow the
/// model.
pub fn checked_event_destination_definition(
    definition: crate::ops::types::EventDestinationDefinition,
) -> Result<crate::ops::types::EventDestinationDefinition, String> {
    let matching_event_types = match definition.matching_event_types {
        Some(event_types) => Some(
            event_types
                .into_iter()
                .map(|event_type| {
                    modelled_enum_value(
                        "MatchingEventTypes",
                        event_type,
                        crate::ops::types::EVENT_TYPE_VALUES,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };
    let cloud_watch_destination = match definition.cloud_watch_destination {
        Some(destination) => Some(crate::ops::types::CloudWatchDestination {
            dimension_configurations: destination
                .dimension_configurations
                .into_iter()
                .map(|dimension| {
                    Ok(crate::ops::types::CloudWatchDimensionConfiguration {
                        dimension_value_source: modelled_enum_value(
                            "CloudWatchDestination.DimensionConfigurations.DimensionValueSource",
                            dimension.dimension_value_source,
                            crate::ops::types::DIMENSION_VALUE_SOURCE_VALUES,
                        )?,
                        ..dimension
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        }),
        None => None,
    };
    Ok(crate::ops::types::EventDestinationDefinition {
        matching_event_types,
        cloud_watch_destination,
        ..definition
    })
}
