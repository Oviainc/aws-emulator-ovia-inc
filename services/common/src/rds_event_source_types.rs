// SPDX-License-Identifier: CC0-1.0

//! The event source types the ONE `rds` front door names in that refusal, and how it words
//! it.
//!
//! Amazon RDS, Amazon DocumentDB and Amazon Neptune are served by that one front door, so
//! the set of source types it names as valid — and the sentence it refuses a `SourceType`
//! outside what it can answer with — is one fact, held here rather than once per service
//! view.

/// Every event source type this front door NAMES as valid when it refuses a `SourceType`
/// it cannot answer, in the order it names them.
///
/// It is the set the REFUSAL publishes, not a gate: nothing accepts a `SourceType` by
/// asking this list. What a request naming one of them is served with is decided where that
/// service view holds the answer — `DescribeEventCategories` reports a map only for the
/// source types whose published categories that view can cite, and an event subscription is
/// created only for the source types its own create accepts — which is what the sentence's
/// "Feature may be disabled." clause says about a type named here and still not served.
///
/// The list is wider than any one model's `SourceType` enum
/// (`smithy_query(query="shape",shape="SourceType")` carries eleven values for the Amazon
/// RDS view and six for the Amazon DocumentDB one): the front door names the source types
/// of the resources it has grown since, and this refusal is what publishes them. The order
/// is the front door's own, which is why it is written out rather than sorted.
pub const EVENT_SOURCE_TYPES: [&str; 16] = [
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
    "data-group",
    "db-cluster-group",
    "integration-s3tables",
    "branch",
    "db-cluster-group-snapshot",
];

/// The message this front door refuses a `SourceType` naming no event source type with.
///
/// The sentence is a statement about the PARAMETER rather than about the value: it says the
/// source type is not valid and that the feature may be disabled, then names the types that
/// ARE valid — it echoes no part of the offending value, so neither does this. Each type is
/// named in single quotes, the way the service writes a list of literal values inside a
/// sentence, in the order [`EVENT_SOURCE_TYPES`] carries them, and the sentence ends with
/// that list: there is no terminal period.
pub fn invalid_event_source_type_message() -> String {
    let quoted: Vec<String> = EVENT_SOURCE_TYPES
        .iter()
        .map(|source_type| format!("'{source_type}'"))
        .collect();
    format!(
        "Invalid event source type. Feature may be disabled. Valid types are {}",
        quoted.join(", ")
    )
}
