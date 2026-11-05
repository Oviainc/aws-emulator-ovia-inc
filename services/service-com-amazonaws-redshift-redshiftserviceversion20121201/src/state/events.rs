// SPDX-License-Identifier: CC0-1.0

//! The event surface: the published source types, categories and severities, the
//! catalog of events a control-plane write raises, and the subscriptions that
//! are scoped to them.

use super::*;

/// How many entries one request's `SourceIds` list may carry.
///
/// The emulator's own bound, and for the same reason as the collection quotas
/// above: every entry costs a lookup in the account's own sources and a string
/// on the stored subscription, so an unbounded list would let one request size
/// both. Set at the number of sources of one type an account can hold
/// ([`CLUSTER_QUOTA`], [`PARAMETER_GROUP_QUOTA`]) — and a subscription that
/// wants every object of a type names NO ids at all
/// (`smithy_query(query="doc", shape="CreateEventSubscriptionMessage",
/// member="SourceIds")` — "If not specified, then events are returned for all
/// objects within the source type specified"), so no workflow needs a longer
/// one.
pub const MAX_SUBSCRIPTION_SOURCE_IDS: usize = CLUSTER_QUOTA;

/// How many entries one request's `EventCategories` list may carry: the
/// published catalog's own size, since a longer list has to name one category
/// twice.
pub const MAX_SUBSCRIPTION_EVENT_CATEGORIES: usize = EVENT_CATEGORIES.len();

/// The source types an event notification subscription can be scoped to, in the
/// order the model lists them.
///
/// Source: `smithy_query(query="shape", shape="SourceType")` — the enum's values
/// — which is also the list on `smithy_query(query="doc",
/// shape="CreateEventSubscriptionMessage", member="SourceType")`: "Valid values:
/// cluster, cluster-parameter-group, cluster-security-group, cluster-snapshot,
/// and scheduled-action."
pub const EVENT_SOURCE_TYPES: [&str; 5] = [
    EVENT_SOURCE_TYPE_CLUSTER,
    EVENT_SOURCE_TYPE_PARAMETER_GROUP,
    EVENT_SOURCE_TYPE_SECURITY_GROUP,
    EVENT_SOURCE_TYPE_SNAPSHOT,
    EVENT_SOURCE_TYPE_SCHEDULED_ACTION,
];

/// The source type of events a provisioned cluster publishes.
pub const EVENT_SOURCE_TYPE_CLUSTER: &str = "cluster";
/// The source type of events a cluster parameter group publishes.
pub const EVENT_SOURCE_TYPE_PARAMETER_GROUP: &str = "cluster-parameter-group";
/// The source type of events a cluster security group publishes.
pub const EVENT_SOURCE_TYPE_SECURITY_GROUP: &str = "cluster-security-group";
/// The source type of events a cluster snapshot publishes.
pub const EVENT_SOURCE_TYPE_SNAPSHOT: &str = "cluster-snapshot";
/// The source type of events a scheduled action publishes.
pub const EVENT_SOURCE_TYPE_SCHEDULED_ACTION: &str = "scheduled-action";
/// The source type of events a reserved compute node publishes.
///
/// Amazon Redshift raises events for reserved nodes, and the refusal the event
/// CATALOG read answers an unrecognised `SourceType` with
/// ([`invalid_event_source_type_message`]) enumerates this type beside the five
/// above. The model's `SourceType` enum (`smithy_query(query="shape",
/// shape="SourceType")`) does NOT list it, so it is not a type a SUBSCRIPTION
/// can be scoped to: it belongs to the catalog-read vocabulary alone.
pub const EVENT_SOURCE_TYPE_RESERVED_NODE: &str = "reserved-node";

/// The `SourceType` vocabulary the event CATALOG read admits, in the order its
/// own refusal enumerates them.
///
/// Wider than [`EVENT_SOURCE_TYPES`], the types a subscription can be scoped to,
/// by [`EVENT_SOURCE_TYPE_RESERVED_NODE`]; the order differs from the model
/// enum's because the refusal's enumeration fixes it.
pub const EVENT_CATEGORY_SOURCE_TYPES: [&str; 6] = [
    EVENT_SOURCE_TYPE_CLUSTER,
    EVENT_SOURCE_TYPE_SECURITY_GROUP,
    EVENT_SOURCE_TYPE_SNAPSHOT,
    EVENT_SOURCE_TYPE_PARAMETER_GROUP,
    EVENT_SOURCE_TYPE_SCHEDULED_ACTION,
    EVENT_SOURCE_TYPE_RESERVED_NODE,
];

/// The admitted spelling of the catalog-read source type `requested` names, or
/// `None` when it is outside [`EVENT_CATEGORY_SOURCE_TYPES`]. Matched as
/// [`published_source_type`] matches a source type.
pub fn admitted_event_source_type(requested: &str) -> Option<&'static str> {
    published_catalog_value(&EVENT_CATEGORY_SOURCE_TYPES, requested)
}

/// What the event catalog read answers a `SourceType` outside the vocabulary it
/// admits: the fixed sentence, then that vocabulary enumerated in single quotes
/// and in [`EVENT_CATEGORY_SOURCE_TYPES`] order. The offending value is NOT
/// echoed, and the sentence carries no trailing period.
pub fn invalid_event_source_type_message() -> String {
    let valid = EVENT_CATEGORY_SOURCE_TYPES
        .iter()
        .map(|source_type| format!("'{source_type}'"))
        .collect::<Vec<String>>()
        .join(", ");
    format!("Invalid event source type. Valid types are {valid}")
}

/// The event categories a subscription can publish, in the order the model lists
/// them.
///
/// Source: `smithy_query(query="doc", shape="CreateEventSubscriptionMessage",
/// member="EventCategories")` — "Values: configuration, management, monitoring,
/// security, pending".
pub const EVENT_CATEGORIES: [&str; 5] = [
    EVENT_CATEGORY_CONFIGURATION,
    EVENT_CATEGORY_MANAGEMENT,
    EVENT_CATEGORY_MONITORING,
    EVENT_CATEGORY_SECURITY,
    EVENT_CATEGORY_PENDING,
];

/// The category of an event reporting a change to a resource's configuration.
pub const EVENT_CATEGORY_CONFIGURATION: &str = "configuration";
/// The category of an event reporting a resource being managed — created,
/// renamed or deleted.
pub const EVENT_CATEGORY_MANAGEMENT: &str = "management";
/// The category of an event reporting what a resource is doing.
pub const EVENT_CATEGORY_MONITORING: &str = "monitoring";
/// The category of an event reporting access to a resource.
pub const EVENT_CATEGORY_SECURITY: &str = "security";
/// The category of an event reporting work Amazon Redshift still has to do.
pub const EVENT_CATEGORY_PENDING: &str = "pending";

/// The event severities a subscription can publish, in the order the model lists
/// them.
///
/// Source: `smithy_query(query="doc", shape="CreateEventSubscriptionMessage",
/// member="Severity")` — "Values: ERROR, INFO".
pub const EVENT_SEVERITIES: [&str; 2] = [EVENT_SEVERITY_ERROR, EVENT_SEVERITY_INFO];

/// The severity of an event reporting something that went wrong.
pub const EVENT_SEVERITY_ERROR: &str = "ERROR";
/// The severity of an event reporting something that happened normally.
pub const EVENT_SEVERITY_INFO: &str = "INFO";

/// One event this service publishes: the identifier Amazon Redshift gives it,
/// the source type that raises it, the categories and the severity it is
/// published under, and the description of what it reports.
#[derive(Debug, Clone, Copy)]
pub struct CatalogEvent {
    /// The `EventId` the event is published under, e.g. `REDSHIFT-EVENT-1003`.
    pub event_id: &'static str,
    /// The source type that raises it, one of [`EVENT_SOURCE_TYPES`].
    pub source_type: &'static str,
    /// The categories it is published under, each one of [`EVENT_CATEGORIES`].
    pub event_categories: &'static [&'static str],
    /// Its severity, one of [`EVENT_SEVERITIES`].
    pub severity: &'static str,
    /// What it reports, with every part taken from the resource and the clock
    /// written as a `[placeholder]` — the catalog describes the event, it does
    /// not report one occurrence of it.
    pub event_description: &'static str,
}

impl CatalogEvent {
    /// This entry as the modelled `EventInfoMap` a catalog read reports.
    pub fn to_api(&self) -> EventInfoMap {
        EventInfoMap {
            event_id: Some(self.event_id.to_string()),
            event_categories: Some(
                self.event_categories
                    .iter()
                    .map(|category| (*category).to_string())
                    .collect(),
            ),
            event_description: Some(self.event_description.to_string()),
            severity: Some(self.severity.to_string()),
        }
    }
}

/// The events this service publishes, grouped by the source type that raises
/// them and ordered by identifier within each group.
///
/// Source: the published Amazon Redshift event catalog ("Amazon Redshift event
/// categories and event messages", Amazon Redshift Management Guide), which
/// gives each event its identifier, the source type that raises it, its
/// category, its severity, and the message it carries with every part taken from
/// the resource and the clock written as a `[placeholder]`. The identifiers are
/// numbered by CATEGORY — the 1000 range for `configuration`, 2000 for
/// `management`, 3000 for `monitoring`, 4000 for `security` and 5000 for
/// `pending` — which is why [`EVENT_ID_PARAMETER_GROUP_CREATED`], the event a
/// cluster parameter group's creation raises and the one this service's own
/// event path names, sits in the configuration range.
///
/// Every source type the catalog read admits carries at least one event: an
/// entry reporting an empty event list would say that source type raises no
/// Amazon Redshift event at all, which is true of none of them.
pub const EVENT_CATALOG: &[CatalogEvent] = &[
    // The cluster itself: its own management lifecycle, the configuration
    // changes applied to it, what it is observed doing, the access to it, and
    // the work Amazon Redshift still owes it.
    CatalogEvent {
        event_id: EVENT_ID_CLUSTER_MODIFIED,
        source_type: EVENT_SOURCE_TYPE_CLUSTER,
        event_categories: &[EVENT_CATEGORY_CONFIGURATION],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Your Amazon Redshift cluster [cluster name] was modified at [time] UTC. \
             The changes you requested are being applied.",
    },
    CatalogEvent {
        event_id: EVENT_ID_CLUSTER_CREATED,
        source_type: EVENT_SOURCE_TYPE_CLUSTER,
        event_categories: &[EVENT_CATEGORY_MANAGEMENT],
        severity: EVENT_SEVERITY_INFO,
        event_description: "Your Amazon Redshift cluster [cluster name] was created at [time] UTC.",
    },
    CatalogEvent {
        event_id: EVENT_ID_CLUSTER_DELETED,
        source_type: EVENT_SOURCE_TYPE_CLUSTER,
        event_categories: &[EVENT_CATEGORY_MANAGEMENT],
        severity: EVENT_SEVERITY_INFO,
        event_description: "Your Amazon Redshift cluster [cluster name] was deleted at [time] UTC.",
    },
    CatalogEvent {
        event_id: EVENT_ID_CLUSTER_RENAMED,
        source_type: EVENT_SOURCE_TYPE_CLUSTER,
        event_categories: &[EVENT_CATEGORY_MANAGEMENT],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Your Amazon Redshift cluster [cluster name] was renamed to [new cluster name] \
             at [time] UTC.",
    },
    CatalogEvent {
        event_id: EVENT_ID_CLUSTER_REBOOTED,
        source_type: EVENT_SOURCE_TYPE_CLUSTER,
        event_categories: &[EVENT_CATEGORY_MONITORING],
        severity: EVENT_SEVERITY_INFO,
        event_description: "Your Amazon Redshift cluster [cluster name] was rebooted at [time] UTC.",
    },
    CatalogEvent {
        event_id: EVENT_ID_CLUSTER_RESTORED,
        source_type: EVENT_SOURCE_TYPE_CLUSTER,
        event_categories: &[EVENT_CATEGORY_MONITORING],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Your Amazon Redshift cluster [cluster name] was restored from snapshot \
             [snapshot name] at [time] UTC.",
    },
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-3002",
        source_type: EVENT_SOURCE_TYPE_CLUSTER,
        event_categories: &[EVENT_CATEGORY_MONITORING],
        severity: EVENT_SEVERITY_ERROR,
        event_description:
            "Your Amazon Redshift cluster [cluster name] is unavailable at [time] UTC. \
             Amazon Redshift is working to make the cluster available again.",
    },
    CatalogEvent {
        event_id: EVENT_ID_MASTER_PASSWORD_UPDATED,
        source_type: EVENT_SOURCE_TYPE_CLUSTER,
        event_categories: &[EVENT_CATEGORY_SECURITY],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "The master user password for your Amazon Redshift cluster [cluster name] \
             was updated at [time] UTC.",
    },
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-5000",
        source_type: EVENT_SOURCE_TYPE_CLUSTER,
        event_categories: &[EVENT_CATEGORY_PENDING],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Your Amazon Redshift cluster [cluster name] has a pending maintenance update \
             scheduled between [start time] and [end time] UTC. The cluster is not available \
             while the update is applied.",
    },
    // The cluster parameter group: the configuration a cluster is created from,
    // whose changes are what this source type publishes.
    CatalogEvent {
        event_id: EVENT_ID_PARAMETER_GROUP_UPDATED,
        source_type: EVENT_SOURCE_TYPE_PARAMETER_GROUP,
        event_categories: &[EVENT_CATEGORY_CONFIGURATION],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Cluster parameter group [parameter group name] was updated at [time] UTC. \
             If you changed only dynamic parameters, associated clusters are being modified now. \
             If you changed static parameters, all updates, including dynamic parameters, are \
             applied when you reboot the associated clusters.",
    },
    CatalogEvent {
        event_id: EVENT_ID_PARAMETER_GROUP_CREATED,
        source_type: EVENT_SOURCE_TYPE_PARAMETER_GROUP,
        event_categories: &[EVENT_CATEGORY_CONFIGURATION],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Cluster parameter group [parameter group name] was created at [time] UTC.",
    },
    CatalogEvent {
        event_id: EVENT_ID_PARAMETER_GROUP_DELETED,
        source_type: EVENT_SOURCE_TYPE_PARAMETER_GROUP,
        event_categories: &[EVENT_CATEGORY_CONFIGURATION],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Cluster parameter group [parameter group name] was deleted at [time] UTC.",
    },
    // The cluster security group: which hosts may connect to a cluster.
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-1100",
        source_type: EVENT_SOURCE_TYPE_SECURITY_GROUP,
        event_categories: &[EVENT_CATEGORY_CONFIGURATION],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Cluster security group [cluster security group name] was created at [time] UTC.",
    },
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-1101",
        source_type: EVENT_SOURCE_TYPE_SECURITY_GROUP,
        event_categories: &[EVENT_CATEGORY_CONFIGURATION],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Cluster security group [cluster security group name] was updated at [time] UTC. \
             The clusters it is associated with are being modified now.",
    },
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-1102",
        source_type: EVENT_SOURCE_TYPE_SECURITY_GROUP,
        event_categories: &[EVENT_CATEGORY_CONFIGURATION],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Cluster security group [cluster security group name] was deleted at [time] UTC.",
    },
    // The cluster snapshot: the backups taken of a cluster.
    CatalogEvent {
        event_id: EVENT_ID_SNAPSHOT_CREATED,
        source_type: EVENT_SOURCE_TYPE_SNAPSHOT,
        event_categories: &[EVENT_CATEGORY_MONITORING],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "A snapshot [snapshot name] of your Amazon Redshift cluster [cluster name] \
             was created at [time] UTC.",
    },
    CatalogEvent {
        event_id: EVENT_ID_SNAPSHOT_DELETED,
        source_type: EVENT_SOURCE_TYPE_SNAPSHOT,
        event_categories: &[EVENT_CATEGORY_MONITORING],
        severity: EVENT_SEVERITY_INFO,
        event_description: "The snapshot [snapshot name] was deleted at [time] UTC.",
    },
    // The scheduled action: the operation Amazon Redshift performs on a cluster
    // on a schedule.
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-2200",
        source_type: EVENT_SOURCE_TYPE_SCHEDULED_ACTION,
        event_categories: &[EVENT_CATEGORY_MANAGEMENT],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "The scheduled action [scheduled action name] was created at [time] UTC.",
    },
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-2201",
        source_type: EVENT_SOURCE_TYPE_SCHEDULED_ACTION,
        event_categories: &[EVENT_CATEGORY_MANAGEMENT],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "The scheduled action [scheduled action name] was deleted at [time] UTC.",
    },
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-5200",
        source_type: EVENT_SOURCE_TYPE_SCHEDULED_ACTION,
        event_categories: &[EVENT_CATEGORY_PENDING],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "The scheduled action [scheduled action name] is scheduled to run \
             [target action] at [time] UTC.",
    },
    // The reserved compute node: the reservations an account buys and exchanges.
    // These three entries are this emulator's OWN published catalog rather than
    // a quotation of the service's text: the source type has to carry events —
    // a map reporting none would say reserved nodes raise nothing — and what
    // they report is read off the model instead of invented. The two states a
    // reservation passes through are the ones
    // `smithy_query(query="doc", shape="ReservedNode", member="State")`
    // publishes — "pending-payment…the sale has been approved, but payment has
    // not yet been confirmed" and "active…owned by the caller and is available
    // for use" — and the third is the move
    // `smithy_query(query="doc", operation="AcceptReservedNodeExchange")`
    // performs ("Exchanges a DC1 Reserved Node for a DC2 Reserved Node"). The
    // identifiers continue the numbering convention above: the management range,
    // with a block of its own per source type.
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-2300",
        source_type: EVENT_SOURCE_TYPE_RESERVED_NODE,
        event_categories: &[EVENT_CATEGORY_MANAGEMENT],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Your reserved node [reserved node id] was purchased at [time] UTC. \
             The sale has been approved and the payment has not yet been confirmed.",
    },
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-2301",
        source_type: EVENT_SOURCE_TYPE_RESERVED_NODE,
        event_categories: &[EVENT_CATEGORY_MANAGEMENT],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "The payment for your reserved node [reserved node id] was confirmed at \
             [time] UTC. The reserved node is available for use.",
    },
    CatalogEvent {
        event_id: "REDSHIFT-EVENT-2302",
        source_type: EVENT_SOURCE_TYPE_RESERVED_NODE,
        event_categories: &[EVENT_CATEGORY_MANAGEMENT],
        severity: EVENT_SEVERITY_INFO,
        event_description:
            "Your reserved node [reserved node id] was exchanged for reserved node \
             [target reserved node id] at [time] UTC.",
    },
];

/// How long a raised event stays readable: an event leaves the log once this
/// much time has passed since it happened.
///
/// Source: `smithy_query(query="doc", operation="DescribeEvents")` — "Returns
/// events related to clusters, security groups, snapshots, and parameter groups
/// for the past 14 days." The window is the service's own published retention,
/// not a span this emulator chose.
pub const EVENT_RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// How many minutes before the request an event read reaches back when the
/// request names neither a `StartTime` nor a `Duration`.
///
/// Source: `smithy_query(query="doc", shape="DescribeEventsMessage",
/// member="Duration")` — "The number of minutes prior to the time of the request
/// for which to retrieve events […] Default: 60", which the operation's own
/// documentation states as "By default, the past hour of events are returned."
pub const EVENT_DURATION_DEFAULT_MINUTES: i64 = 60;

/// How many seconds one minute of a `Duration` is, so the member's documented
/// UNIT is applied in one place.
pub(super) const SECONDS_PER_MINUTE: i64 = 60;

/// The closed interval, in whole seconds since the epoch, that one event read
/// reports over: `(start, end)`.
///
/// The three members decide it exactly as they are documented: `EndTime` is "the
/// end of the time interval for which to retrieve events" and defaults to the
/// time of the request; `StartTime` is "the beginning of the time interval to
/// retrieve events for"; and `Duration` is "the number of minutes prior to the
/// time of the REQUEST" — so when no `StartTime` is named the interval begins
/// that many minutes before `now`, never before `EndTime`, which is what the
/// member's own example states ("if the request is sent at 18:00 and you specify
/// a duration of 60, then only events which have occurred after 17:00 will be
/// returned").
pub fn event_window(
    start_time: Option<::aws_smithy_types::DateTime>,
    end_time: Option<::aws_smithy_types::DateTime>,
    duration_minutes: Option<i32>,
    now: SystemTime,
) -> (i64, i64) {
    let now_secs = ::aws_smithy_types::DateTime::from(now).secs();
    let end = end_time.map_or(now_secs, |at| at.secs());
    let start = match start_time {
        Some(at) => at.secs(),
        None => {
            let minutes = duration_minutes.map_or(EVENT_DURATION_DEFAULT_MINUTES, i64::from);
            now_secs.saturating_sub(minutes.saturating_mul(SECONDS_PER_MINUTE))
        }
    };
    (start, end)
}

/// The identifier of the event a cluster parameter group's creation raises.
pub const EVENT_ID_PARAMETER_GROUP_CREATED: &str = "REDSHIFT-EVENT-1003";

/// The identifier of the event a change to a cluster parameter group's values
/// raises — the write that `ModifyClusterParameterGroup` makes and the one
/// `ResetClusterParameterGroup` makes, both of which leave the group holding
/// different values than before.
pub const EVENT_ID_PARAMETER_GROUP_UPDATED: &str = "REDSHIFT-EVENT-1000";

/// The identifier of the event a cluster parameter group's deletion raises.
pub const EVENT_ID_PARAMETER_GROUP_DELETED: &str = "REDSHIFT-EVENT-1004";

/// The identifier of the event a change to a cluster's configuration raises.
pub const EVENT_ID_CLUSTER_MODIFIED: &str = "REDSHIFT-EVENT-1200";

/// The identifier of the event a cluster's creation raises.
pub const EVENT_ID_CLUSTER_CREATED: &str = "REDSHIFT-EVENT-2000";

/// The identifier of the event a cluster's deletion raises.
pub const EVENT_ID_CLUSTER_DELETED: &str = "REDSHIFT-EVENT-2001";

/// The identifier of the event a cluster's rename raises.
pub const EVENT_ID_CLUSTER_RENAMED: &str = "REDSHIFT-EVENT-2002";

/// The identifier of the event a cluster's restart raises.
pub const EVENT_ID_CLUSTER_REBOOTED: &str = "REDSHIFT-EVENT-3000";

/// The identifier of the event the creation of a cluster FROM a snapshot raises.
pub const EVENT_ID_CLUSTER_RESTORED: &str = "REDSHIFT-EVENT-3001";

/// The identifier of the event a change to a cluster's admin password raises.
pub const EVENT_ID_MASTER_PASSWORD_UPDATED: &str = "REDSHIFT-EVENT-4000";

/// The identifier of the event a cluster snapshot's creation raises.
pub const EVENT_ID_SNAPSHOT_CREATED: &str = "REDSHIFT-EVENT-3100";

/// The identifier of the event a cluster snapshot's deletion raises.
pub const EVENT_ID_SNAPSHOT_DELETED: &str = "REDSHIFT-EVENT-3101";

/// The published catalog as `DescribeEventCategories` reports it: one entry per
/// source type this read admits, in the order its own vocabulary lists them,
/// each carrying the events [`EVENT_CATALOG`] publishes for that type.
///
/// EVERY admitted source type is reported, because the read "displays a list of
/// event categories for all event source types" (`smithy_query(query="doc",
/// operation="DescribeEventCategories")`) — which is the whole vocabulary this
/// read admits ([`EVENT_CATEGORY_SOURCE_TYPES`]), wider than the types a
/// SUBSCRIPTION can be scoped to ([`EVENT_SOURCE_TYPES`]): a caller enumerating
/// the event ids of a source type the read admits finds the events that type
/// raises rather than an entry the response never carried.
pub fn published_event_catalog() -> Vec<EventCategoriesMap> {
    EVENT_CATEGORY_SOURCE_TYPES
        .iter()
        .map(|source_type| EventCategoriesMap {
            source_type: Some((*source_type).to_string()),
            events: Some(
                EVENT_CATALOG
                    .iter()
                    .filter(|event| event.source_type == *source_type)
                    .map(CatalogEvent::to_api)
                    .collect(),
            ),
        })
        .collect()
}

/// The status of an event notification subscription whose topic is still the one
/// it was created against.
///
/// Source: `smithy_query(query="doc", shape="EventSubscription",
/// member="Status")` — "Can be one of the following: active | no-permission |
/// topic-not-exist", where the other two are the states a topic that revoked
/// Amazon Redshift's permission, or that was deleted after the subscription was
/// created, leaves a subscription in. Neither has happened to a subscription
/// this store holds, so every one of them reports this value.
pub const SUBSCRIPTION_STATUS_ACTIVE: &str = "active";

/// Whether a create that does not name `Enabled` yields an ACTIVE subscription.
///
/// Source: `smithy_query(query="doc", shape="CreateEventSubscriptionMessage",
/// member="Enabled")` — "set to true to activate the subscription, and set to
/// false to create the subscription but not activate it". The member's whole
/// choice is between activating and deliberately NOT activating, so a request
/// that does not ask for the inactive form gets the active one.
pub const EVENT_SUBSCRIPTION_ENABLED_BY_DEFAULT: bool = true;

/// The published spelling of the source type `requested` names, or `None` when
/// this service publishes no such type.
///
/// Compared case-insensitively and answered in the published spelling, so one
/// catalog value is stored and reported in exactly one form however a caller
/// spelled it.
pub fn published_source_type(requested: &str) -> Option<&'static str> {
    published_catalog_value(&EVENT_SOURCE_TYPES, requested)
}

/// The published spelling of the event category `requested` names, or `None`
/// when this service publishes no such category. Matched as
/// [`published_source_type`] matches a source type.
pub fn published_event_category(requested: &str) -> Option<&'static str> {
    published_catalog_value(&EVENT_CATEGORIES, requested)
}

/// The published spelling of the event severity `requested` names, or `None`
/// when this service publishes no such severity. Matched as
/// [`published_source_type`] matches a source type.
pub fn published_event_severity(requested: &str) -> Option<&'static str> {
    published_catalog_value(&EVENT_SEVERITIES, requested)
}

/// The entry of a published catalog `requested` names, case-insensitively.
pub(super) fn published_catalog_value(catalog: &[&'static str], requested: &str) -> Option<&'static str> {
    catalog
        .iter()
        .find(|published| published.eq_ignore_ascii_case(requested))
        .copied()
}

/// The service an Amazon SNS topic ARN names.
pub const SNS_ARN_SERVICE: &str = "sns";

/// Whether a value is the ARN of an Amazon SNS TOPIC.
///
/// Source: the Amazon SNS documentation's ARN form for a topic,
/// `arn:<partition>:sns:<region>:<account-id>:<topic-name>` — six
/// colon-separated segments, the service `sns`, and a resource portion that is
/// the topic name alone (a subscription ARN carries a further
/// `<topic-name>:<subscription-id>` segment, and a topic ARN names no
/// subresource). Checked on the FORM alone: whether that topic exists is Amazon
/// SNS's own state, which this service does not hold.
pub fn is_sns_topic_arn(value: &str) -> bool {
    if !is_arn(value) {
        return false;
    }
    let segments: Vec<&str> = value.split(':').collect();
    segments.len() == 6
        && segments[2] == SNS_ARN_SERVICE
        && !segments[3].is_empty()
        && !segments[4].is_empty()
        && !segments[5].is_empty()
}

/// Why a request naming a value no published catalog holds is refused, naming
/// the member, the value and every value the catalog publishes.
pub fn unpublished_value_message(member: &str, value: &str, published: &[&str]) -> String {
    format!(
        "Invalid value for {member}: '{value}'. Valid Values: {}",
        published.join(" | ")
    )
}

/// What an event notification subscription publishes, resolved from one create
/// or modify request: the sources it is scoped to, the event categories and the
/// severity.
///
/// Each member is `None` when the request names none, which is what an
/// unscoped subscription has: "If you do not specify either the SourceType nor
/// the SourceIdentifier, you will be notified of events generated from all
/// Amazon Redshift sources belonging to your Amazon Web Services account."
#[derive(Debug, Default)]
pub struct SubscriptionScope {
    /// The published source type the subscription is scoped to.
    pub source_type: Option<String>,
    /// The identifiers of that type the subscription is scoped to.
    pub source_ids: Option<Vec<String>>,
    /// The published event categories the subscription publishes.
    pub event_categories: Option<Vec<String>>,
    /// The published severity the subscription publishes.
    pub severity: Option<String>,
}

/// Why a request naming no event notification subscription is refused, naming
/// the subscription it could not find.
///
/// The wording Amazon Redshift answers a missing subscription with, shared by
/// every operation that looks one up so a client sees one message for one
/// condition.
pub fn subscription_not_found_message(subscription_name: &str) -> String {
    format!("Event Subscription {subscription_name} not found")
}

/// Why a subscription configuration cannot be served, classified so each
/// operation answers it with the fault its own model declares for that class.
///
/// One classification for the create and the modify path, so the two can never
/// refuse the same configuration differently.
#[derive(Debug)]
pub enum SubscriptionRefusal {
    /// A parameter value this service cannot honour at all — a source type it
    /// does not publish, or source ids named without a type to read them
    /// against. The model declares no client-input fault for either, so it is
    /// answered with the `InvalidParameterValue` code Amazon Redshift itself
    /// uses.
    InvalidParameterValue(String),
    /// A source identifier naming no event source this account holds —
    /// `SourceNotFoundFault`.
    SourceNotFound(String),
    /// An event category this service does not publish —
    /// `SubscriptionCategoryNotFoundFault`.
    CategoryNotFound(String),
    /// An event severity this service does not publish —
    /// `SubscriptionSeverityNotFoundFault`.
    SeverityNotFound(String),
}

/// Why source ids cannot be read without a source type to read them against.
///
/// Source: `smithy_query(query="doc", shape="CreateEventSubscriptionMessage",
/// member="SourceType")` — "You must specify a source type in order to specify
/// source IDs."
pub const SOURCE_IDS_NEED_A_SOURCE_TYPE: &str =
    "You must specify a source type in order to specify source IDs.";
