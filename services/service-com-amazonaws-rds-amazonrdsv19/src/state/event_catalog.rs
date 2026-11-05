// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The event source types and their categories, as `DescribeEventCategories` reports
/// them.
///
/// The source types are the valid values documented on
/// `DescribeEventCategoriesMessage$SourceType` ("`db-instance` | `db-cluster` |
/// `db-parameter-group` | `db-security-group` | `db-snapshot` | `db-cluster-snapshot` |
/// `db-proxy`"), in that order; the categories of each are the ones published in "Amazon
/// RDS event categories and event messages" (Amazon RDS User Guide), in the order that
/// table lists them (alphabetical): the wire order of `EventCategories` is not documented
/// anywhere in the model or the API reference, so the ONE order this catalog can state a
/// source for is the published table's.
///
/// `db-instance` carries the two categories that table lists beyond the ones a DB
/// instance shares with the other source types — `backtrack` (an Aurora MySQL backtrack
/// event) and `security` — because a client builds a `CreateEventSubscription`
/// `EventCategories` argument out of this map and AWS accepts both for that source type;
/// `db-cluster-snapshot` carries `notification` beside `backup` for the same reason.
///
/// A source type the model's `SourceType` enum
/// (`smithy_query(query="shape",shape="SourceType")`) carries but this member's prose does
/// not list is published too, at the position the enum declares it in, when this emulator
/// can cite the categories that source type publishes: `custom-engine-version` is one —
/// the published custom-engine-version event table carries a single `failure` event — and
/// a client that subscribes to a source type has to be able to read its categories from
/// this one map. The three remaining enum values — `blue-green-deployment`,
/// `db-shard-group` and `zero-etl` — have no published category table this emulator can
/// cite, so it publishes no map for them rather than an invented one, and a request naming
/// one is refused with the code a value this parameter does not accept gets, naming the
/// set it is not in.
pub const EVENT_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "db-instance",
        &[
            "availability",
            "backtrack",
            "backup",
            "configuration change",
            "creation",
            "deletion",
            "failover",
            "failure",
            "low storage",
            "maintenance",
            "notification",
            "read replica",
            "recovery",
            "restoration",
            "security",
            "security patching",
        ],
    ),
    (
        "db-cluster",
        &[
            "configuration change",
            "creation",
            "deletion",
            "failover",
            "failure",
            "global-failover",
            "maintenance",
            // A cluster also publishes the events of the two things only a CLUSTER does:
            // it is migrated onto the service (`migration`), and it runs serverless
            // capacity (`serverless`). Both are rows of the published DB cluster event
            // table and both are categories a cluster subscription may name.
            "migration",
            "notification",
            "serverless",
        ],
    ),
    ("db-parameter-group", &["configuration change"]),
    ("db-security-group", &["configuration change", "failure"]),
    (
        "db-snapshot",
        &["creation", "deletion", "notification", "restoration"],
    ),
    ("db-cluster-snapshot", &["backup", "notification"]),
    // The published custom-engine-version event table has one row, a failed CEV creation.
    ("custom-engine-version", &["failure"]),
    (
        "db-proxy",
        &["configuration change", "creation", "deletion", "failure"],
    ),
];

/// The event categories of `source_type`, or `None` when it names no published source
/// type. Matching is exact: the documented values are all lowercase.
pub fn event_categories_of(source_type: &str) -> Option<&'static [&'static str]> {
    EVENT_CATEGORIES
        .iter()
        .find(|(published, _)| *published == source_type)
        .map(|(_, categories)| *categories)
}

/// The message the refusal of a `SourceType` that names no published event source type
/// carries.
///
/// Amazon RDS words this one as a statement about the PARAMETER and not about the value:
/// the fixed sentence says the source type is not valid and that the feature may be
/// disabled, then names the types that ARE valid — it does not echo the offending value
/// back, so neither does this. Each type is named in single quotes, the way the service
/// writes a list of literal values inside a sentence, and in the order
/// [`EVENT_CATEGORIES`] publishes them.
pub fn invalid_event_source_type_message() -> String {
    let quoted: Vec<String> = EVENT_CATEGORIES
        .iter()
        .map(|(source_type, _)| format!("'{source_type}'"))
        .collect();
    format!(
        "Invalid event source type. Feature may be disabled. Valid types are {}",
        quoted.join(", ")
    )
}

/// One source type's categories as the modelled `EventCategoriesMap`.
pub fn event_categories_to_wire(source_type: &str, categories: &[&str]) -> EventCategoriesMap {
    EventCategoriesMap {
        source_type: Some(source_type.to_string()),
        event_categories: Some(
            categories
                .iter()
                .map(|category| (*category).to_string())
                .collect(),
        ),
    }
}

/// The source types an event notification SUBSCRIPTION may name.
///
/// The set is the one the model's `SourceType` enum carries
/// (`crate::ops::types::SOURCE_TYPE_VALUES`), which is `db-shard-group` wider than the
/// list `CreateEventSubscriptionMessage$SourceType`'s prose spells out; the ORDER is the
/// one Amazon RDS lists the valid types in when it refuses a value outside the set
/// ([`invalid_event_subscription_source_type_message`]), so the refusal names them exactly
/// as the service does.
///
/// Wider than the source types [`EVENT_CATEGORIES`] carries a category map for, which is
/// the narrower list `DescribeEventCategories`'s own member documents: a subscription may
/// be created for a source type whose published categories this emulator does not have,
/// and [`check_event_categories`] says what that means for the categories such a
/// subscription may name.
pub const EVENT_SUBSCRIPTION_SOURCE_TYPES: [&str; 11] = [
    "db-instance",
    "db-security-group",
    "db-parameter-group",
    "db-snapshot",
    "db-cluster",
    "db-cluster-snapshot",
    "custom-engine-version",
    "db-proxy",
    "blue-green-deployment",
    "db-shard-group",
    "zero-etl",
];

/// The message Amazon RDS refuses a `SourceType` no subscription can be created for with.
///
/// A statement about the PARAMETER and not about the value: the fixed sentence says the
/// source type is not valid and then names the types that ARE valid — it echoes nothing
/// from the request, so neither does this. Each type is named in single quotes, the way the
/// service writes a list of literal values inside a sentence, in the order
/// [`EVENT_SUBSCRIPTION_SOURCE_TYPES`] carries them, and the sentence ends with that list
/// (no trailing period). The sibling refusal a `DescribeEvents` source type gets is worded
/// with one more clause ([`invalid_event_source_type_message`]) and over the narrower set
/// that report covers.
pub fn invalid_event_subscription_source_type_message() -> String {
    let quoted: Vec<String> = EVENT_SUBSCRIPTION_SOURCE_TYPES
        .iter()
        .map(|source_type| format!("'{source_type}'"))
        .collect();
    format!("Invalid event source type. Valid types are {}", quoted.join(", "))
}

/// How Amazon RDS refuses a request that names event categories without the source type
/// they belong to: ONE fixed sentence, which is why it is a constant — it echoes neither
/// the categories nor the subscription, so the same text answers every such request.
pub const CATEGORIES_NEED_A_SOURCE_TYPE: &str =
    "If SourceType is null, Categories must also be null.";

/// The constraint `CreateEventSubscriptionMessage$SourceIds` states in those words: "If
/// `SourceIds` are supplied, `SourceType` must also be provided." The ids name resources of
/// one source type, so there is nothing to resolve them against without it.
pub const SOURCE_IDS_NEED_A_SOURCE_TYPE: &str =
    "If SourceIds are supplied, SourceType must also be provided.";

/// Refuse a `SourceType` a subscription cannot be created for; the caller answers it with
/// the Query API's `InvalidParameterValue`.
pub fn check_event_subscription_source_type(source_type: &str) -> Result<(), String> {
    if EVENT_SUBSCRIPTION_SOURCE_TYPES.contains(&source_type) {
        return Ok(());
    }
    Err(invalid_event_subscription_source_type_message())
}

/// The `SourceNotFoundFault` this service answers a request that names an event source
/// which does not exist, built HERE so every handler that resolves a source identifier
/// words the one fault the same way.
///
/// Its text is fixed — `Could not find source :` — and its only variable part is the
/// identifier the request supplied, carried verbatim: RDS interpolates neither the source
/// type the identifier was resolved against nor the region the call was signed for, and
/// appends no trailing period.
pub fn source_not_found(source_identifier: &str) -> SourceNotFoundFault {
    SourceNotFoundFault {
        message: Some(format!("Could not find source :{source_identifier}")),
    }
}

/// The categories of `source_type` a subscription may subscribe to, with the one that
/// is not among them, if any: `EventCategories` is "a list of event categories for a
/// particular source type (SourceType) that you want to subscribe to", so a category
/// that source type does not publish names no events at all.
///
/// A source type [`EVENT_CATEGORIES`] carries no published map for is not checked —
/// there is nothing to check it against, and refusing every category for it would refuse
/// the ones AWS accepts. Matching is exact: the published categories are all lowercase.
pub fn check_event_categories(source_type: &str, categories: &[String]) -> Result<(), String> {
    let Some(published) = event_categories_of(source_type) else {
        return Ok(());
    };
    for category in categories {
        if !published.contains(&category.as_str()) {
            return Err(format!(
                "{category} is not a published event category of {source_type} \
                 (its categories are {})",
                published.join(", ")
            ));
        }
    }
    Ok(())
}

