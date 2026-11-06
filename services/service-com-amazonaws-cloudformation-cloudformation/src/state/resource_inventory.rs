// SPDX-License-Identifier: CC0-1.0

//! THE ACCOUNT'S INVENTORY: the resources this account holds in one Region, as the services
//! that OWN them report them.
//!
//! A resource scan is an inventory of an account and Region, and what it exists FOR is
//! discovering the resources CloudFormation does not manage: `ScannedResource` reports
//! `ManagedByStack` of every row ("if `true`, the resource is managed by a CloudFormation
//! stack"), and `ListResourceScanResources` "indicates whether each returned resource is
//! already managed by CloudFormation" — a statement that only has content where a row CAN be
//! unmanaged. A resource created straight through its owning service's API belongs to no
//! stack, so this service's own stack records are not where it can be found: the service that
//! owns the resource is, which is what this module asks.
//!
//! One listing per resource type, made in-process to the owning service through the same
//! transport a deployment's create rides on ([`super::provisioning`]), and therefore bounded
//! by the same Region the request was signed for: each of these listings reports what its
//! service holds in the Region the call addressed, so an inventory read for one Region never
//! reports what another holds.
//!
//! ## What the catalogue covers
//!
//! One entry per resource type this emulator can enumerate: a type whose owning service
//! publishes a listing of the resources of that type, and whose resources this emulator's
//! services really hold ([`Inventory`]). A type with no entry contributes nothing here, which
//! leaves a scan reporting it exactly as before — the resources of that type a stack of this
//! service recorded — so an entry ADDS what the account holds outside CloudFormation and
//! takes nothing away ([`merged`]).
//!
//! A service that refuses a listing, or that this emulator is not serving at all, reports
//! nothing: the refusal leaves that type out of the inventory rather than inventing rows for
//! it or failing the scan that asked.
//!
//! ## What a row says, and what it does NOT
//!
//! A row carries the resource's own identity and nothing about CloudFormation: its type, the
//! physical id its owning service knows it by, the primary identifier a client names it by
//! ([`super::primary_identifier`]) and the tags it carries in that service. It records NO
//! stack, because the owning service's listing says nothing about one — whether a stack
//! manages the resource is this service's own record to answer, which [`merged`] is what
//! applies: a row the account holds that one of this service's live stacks also records is
//! that stack's row, managed by it; every other row is unmanaged.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::provisioning::{
    call_json_rpc, call_query, xml_element, xml_elements, Refusal, AWS_JSON_1_0,
};
use super::resource_scans::FoundResource;

/// How many pages of ONE listing a read of the inventory walks before it stops.
///
/// Every listing here is paged, and the cursor comes from the answering service, so the walk
/// ends when that service issues no further cursor. This is what BOUNDS the work one call can
/// ask for: the calls a read makes are at most one per page plus one per resource a page
/// reported, so this bound times the answering service's own page size is the whole of it —
/// which at the page sizes those services publish sits at the 100,000 resources a resource
/// scan is documented to cover, far above what this emulator's stores hold at once
/// ([`crate::state::MAX_CONCURRENT_RESOURCES`]).
const MAX_PAGES: usize = 1_000;

/// The resource types a read of the inventory enumerates, one entry per type.
const CATALOGUE: &[Inventory] = &[
    Inventory::SnsTopics,
    Inventory::SqsQueues,
    Inventory::DynamoDbTables,
];

/// One resource type this emulator can enumerate in the service that owns it: which listing
/// reports the resources of that type, and which read reports one resource's tags.
///
/// The variants are the types whose owning service is emulated here AND publishes a listing
/// of every resource of the type in a Region — Amazon SNS's `ListTopics`, Amazon SQS's
/// `ListQueues` and Amazon DynamoDB's `ListTables`, each `@paginated` over the resources of
/// one account and Region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Inventory {
    /// `AWS::SNS::Topic`, enumerated by Amazon SNS's `ListTopics`.
    SnsTopics,
    /// `AWS::SQS::Queue`, enumerated by Amazon SQS's `ListQueues`.
    SqsQueues,
    /// `AWS::DynamoDB::Table`, enumerated by Amazon DynamoDB's `ListTables`.
    DynamoDbTables,
}

/// Amazon SNS's signing name, and the API version an awsQuery request names beside its
/// action, from that service's own model (`sigv4Name: sns`, `xmlNamespace` version
/// `2010-03-31`).
const SNS_SIGNING_NAME: &str = "sns";
const SNS_API_VERSION: &str = "2010-03-31";

/// Amazon SQS's signing name and the service shape its awsJson `X-Amz-Target` names an
/// operation under (`sigv4Name: sqs`, service shape `AmazonSQS`).
const SQS_SIGNING_NAME: &str = "sqs";
const SQS_TARGET_PREFIX: &str = "AmazonSQS";

/// Amazon DynamoDB's signing name and `X-Amz-Target` prefix (`sigv4Name: dynamodb`, service
/// shape `DynamoDB_20120810`).
const DYNAMODB_SIGNING_NAME: &str = "dynamodb";
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

impl Inventory {
    /// The CloudFormation resource type whose resources this entry enumerates, as a row's
    /// `ResourceType` reports it ("the type of the resource, such as `AWS::DynamoDB::Table`").
    fn resource_type(self) -> &'static str {
        match self {
            Self::SnsTopics => super::template::SNS_TOPIC,
            Self::SqsQueues => super::template::SQS_QUEUE,
            // The type's name lives beside the arm that provisions it, which is the one
            // definition of it this crate holds.
            Self::DynamoDbTables => super::provisioning::dynamodb_table::DYNAMODB_TABLE,
        }
    }

    /// The physical ids of every resource of this type the account holds in `region`, as the
    /// owning service reports them — the value that names the resource OUTSIDE
    /// CloudFormation, which for these three types is the one a stack would record as the
    /// resource's physical id (a topic's ARN, a queue's url, a table's name).
    async fn physical_ids(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
    ) -> Result<Vec<String>, Refusal> {
        match self {
            Self::SnsTopics => sns_topic_arns(ctx, region).await,
            Self::SqsQueues => sqs_queue_urls(ctx, region).await,
            Self::DynamoDbTables => dynamodb_table_names(ctx, region).await,
        }
    }

    /// The tags one resource of this type carries in the service that owns it — the set a
    /// `ListResourceScanResources` request's `TagKey` and `TagValue` select on, read from the
    /// service that holds them rather than from any record of this service's own.
    async fn tags(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        physical_id: &str,
    ) -> Result<BTreeMap<String, String>, Refusal> {
        match self {
            Self::SnsTopics => sns_topic_tags(ctx, region, physical_id).await,
            Self::SqsQueues => sqs_queue_tags(ctx, region, physical_id).await,
            Self::DynamoDbTables => dynamodb_table_tags(ctx, region, physical_id).await,
        }
    }
}

/// Every resource of a catalogued type this account holds in `region`, as the services that
/// own them report them right now.
///
/// A POINT-IN-TIME read, made by the operation that needs it and never cached: the caller is
/// what records the rows it took ([`crate::state::resource_scans`]), so nothing here keeps
/// state and a second read reports the account as it is then.
///
/// A listing a service refuses — including one this emulator is not serving — leaves that
/// type out rather than failing the read: the account's inventory is what the services that
/// answered hold. A resource whose tags cannot be read is reported with none, since the row
/// itself is what the listing found.
pub async fn read(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Vec<FoundResource> {
    let mut found = Vec::new();
    for entry in CATALOGUE {
        let Ok(physical_ids) = entry.physical_ids(ctx, region).await else {
            continue;
        };
        for physical_id in physical_ids {
            let tags = entry
                .tags(ctx, region, &physical_id)
                .await
                .unwrap_or_default();
            found.push(FoundResource {
                resource_type: entry.resource_type().to_string(),
                resource_identifier: identifier_of(entry.resource_type(), &physical_id),
                physical_resource_id: physical_id,
                // The owning service's listing says nothing about a stack: a row is
                // UNMANAGED until one of this service's own stack records claims it
                // ([`merged`]).
                logical_resource_id: String::new(),
                stack_id: String::new(),
                tags,
            });
        }
    }
    found
}

/// The whole set of resources a scan of one Region READ: the rows this service's own live
/// stacks record, and every resource of the account's inventory no such row already accounts
/// for.
///
/// The stack records come FIRST and win, which is what decides `ManagedByStack` per row: a
/// resource a live stack of this Region manages is reported with the stack that manages it
/// (and the logical id and the tags that stack gave it), and the same resource the owning
/// service also reports is the one row, not two. Everything else the account holds is
/// reported as what it is — a resource CloudFormation does not manage.
///
/// A row is identified for that purpose by its type and the physical id its owning service
/// knows it by, which is the one value both sides name the same resource with.
pub fn merged(
    stack_records: Vec<FoundResource>,
    inventory: Vec<FoundResource>,
) -> Vec<FoundResource> {
    // The resources already accounted for, indexed by what identifies one, so the merge
    // costs one lookup per row of the inventory rather than a walk of the rows so far.
    let mut taken: BTreeSet<(String, String)> = stack_records
        .iter()
        .map(|resource| {
            (
                resource.resource_type.clone(),
                resource.physical_resource_id.clone(),
            )
        })
        .collect();
    let mut rows = Vec::with_capacity(stack_records.len() + inventory.len());
    rows.extend(stack_records);
    for resource in inventory {
        let key = (
            resource.resource_type.clone(),
            resource.physical_resource_id.clone(),
        );
        if taken.insert(key) {
            rows.push(resource);
        }
    }
    rows
}

/// The `ResourceIdentifier` a row of `resource_type` carries for the resource `physical_id`
/// names: "the key is the name of one of the primary identifiers for the resource … the value
/// is the value of that primary identifier".
///
/// WHICH property that is comes from the type's own primary identifier
/// ([`super::primary_identifier::properties`]) — `TopicArn` for an `AWS::SNS::Topic`,
/// `QueueUrl` for an `AWS::SQS::Queue`, `TableName` for an `AWS::DynamoDB::Table` — read
/// through the one catalogue this service answers `GetTemplateSummary`'s
/// `ResourceIdentifierSummary` from, so a row a scan found is addressable by exactly what a
/// `ResourceToImport` would name it by.
///
/// The FIRST of those properties: a type whose identifier is composite is identified by
/// several properties whose values the owning service's listing does not report apart, and
/// none of the catalogued types is one.
fn identifier_of(resource_type: &str, physical_id: &str) -> BTreeMap<String, String> {
    super::primary_identifier::properties(resource_type)
        .into_iter()
        .take(1)
        .map(|property| (property, physical_id.to_string()))
        .collect()
}

/// Every topic ARN Amazon SNS holds in `region`: `ListTopics` is "a list of the requester's
/// topics", `@paginated` over `NextToken`, and each entry carries the topic's own ARN.
async fn sns_topic_arns(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<Vec<String>, Refusal> {
    let mut arns: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let parameters: Vec<(String, String)> = next_token
            .iter()
            .map(|token| ("NextToken".to_string(), token.clone()))
            .collect();
        let answered = call_query(
            ctx,
            SNS_SIGNING_NAME,
            region,
            SNS_API_VERSION,
            "ListTopics",
            &parameters,
        )
        .await?;
        arns.extend(xml_elements(&answered, "TopicArn"));
        next_token = xml_element(&answered, "NextToken").filter(|token| !token.is_empty());
        if next_token.is_none() {
            break;
        }
    }
    Ok(arns)
}

/// The tags one topic carries: `ListTagsForResource` takes the topic's ARN and answers the
/// `Tags` of the resource, each entry a `Key` and a `Value`.
async fn sns_topic_tags(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    topic_arn: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    let answered = call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "ListTagsForResource",
        &[("ResourceArn".to_string(), topic_arn.to_string())],
    )
    .await?;
    Ok(zipped_tags(
        &xml_elements(&answered, "Key"),
        &xml_elements(&answered, "Value"),
    ))
}

/// Every queue url Amazon SQS holds in `region`: `ListQueues` "gets a list of your queues",
/// `@paginated` over `NextToken`, and each entry is the queue's own url — which is the handle
/// every other call about that queue takes.
async fn sqs_queue_urls(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<Vec<String>, Refusal> {
    let mut urls: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = Map::new();
        if let Some(token) = &next_token {
            request.insert("NextToken".to_string(), Value::String(token.clone()));
        }
        let answered = call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.ListQueues"),
            &Value::Object(request),
        )
        .await?;
        urls.extend(reported_strings(&answered, "QueueUrls"));
        next_token = reported_cursor(&answered, "NextToken");
        if next_token.is_none() {
            break;
        }
    }
    Ok(urls)
}

/// The tags one queue carries: `ListQueueTags` takes the queue's url and answers `Tags`, a
/// map of the tag keys to their values.
async fn sqs_queue_tags(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    queue_url: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut request = Map::new();
    request.insert("QueueUrl".to_string(), Value::String(queue_url.to_string()));
    let answered = call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.ListQueueTags"),
        &Value::Object(request),
    )
    .await?;
    Ok(answered
        .get("Tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default())
}

/// Every table name Amazon DynamoDB holds in `region`: `ListTables` "returns an array of
/// table names associated with the current account and endpoint", paged by the LAST name a
/// page evaluated rather than by an opaque cursor (`LastEvaluatedTableName` into
/// `ExclusiveStartTableName`).
async fn dynamodb_table_names(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<Vec<String>, Refusal> {
    let mut names: Vec<String> = Vec::new();
    let mut start_after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = Map::new();
        if let Some(name) = &start_after {
            request.insert(
                "ExclusiveStartTableName".to_string(),
                Value::String(name.clone()),
            );
        }
        let answered = call_json_rpc(
            ctx,
            DYNAMODB_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{DYNAMODB_TARGET_PREFIX}.ListTables"),
            &Value::Object(request),
        )
        .await?;
        names.extend(reported_strings(&answered, "TableNames"));
        start_after = reported_cursor(&answered, "LastEvaluatedTableName");
        if start_after.is_none() {
            break;
        }
    }
    Ok(names)
}

/// The tags one table carries: `ListTagsOfResource` is addressed by the table's ARN, which is
/// the one Amazon DynamoDB reports for it (`DescribeTable`'s `Table.TableArn`) rather than
/// one computed here, and answers `Tags` as entries of a `Key` and a `Value`, `@paginated`
/// over `NextToken`.
async fn dynamodb_table_tags(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    table_name: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut describe = Map::new();
    describe.insert(
        "TableName".to_string(),
        Value::String(table_name.to_string()),
    );
    let described = call_json_rpc(
        ctx,
        DYNAMODB_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{DYNAMODB_TARGET_PREFIX}.DescribeTable"),
        &Value::Object(describe),
    )
    .await?;
    let Some(table_arn) = described
        .get("Table")
        .and_then(|table| table.get("TableArn"))
        .and_then(Value::as_str)
    else {
        // The table answered with no ARN of its own: nothing addresses the tag read, so the
        // row carries no tags rather than a guessed handle's.
        return Ok(BTreeMap::new());
    };
    let mut tags = BTreeMap::new();
    let mut next_token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = Map::new();
        request.insert(
            "ResourceArn".to_string(),
            Value::String(table_arn.to_string()),
        );
        if let Some(token) = &next_token {
            request.insert("NextToken".to_string(), Value::String(token.clone()));
        }
        let answered = call_json_rpc(
            ctx,
            DYNAMODB_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{DYNAMODB_TARGET_PREFIX}.ListTagsOfResource"),
            &Value::Object(request),
        )
        .await?;
        tags.extend(reported_tag_list(&answered));
        next_token = reported_cursor(&answered, "NextToken");
        if next_token.is_none() {
            break;
        }
    }
    Ok(tags)
}

/// One member of an answer document read as a list of STRINGS, which is how each of these
/// listings carries the resources it found; an answer that carries no such member found none.
fn reported_strings(answered: &Value, member: &str) -> Vec<String> {
    answered
        .get(member)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The cursor an answer issues to resume a listing, or `None` where it issued none — which is
/// what says the walk has reached the end of what the service holds. An EMPTY cursor is no
/// cursor: it names no next page.
fn reported_cursor(answered: &Value, member: &str) -> Option<String> {
    answered
        .get(member)
        .and_then(Value::as_str)
        .filter(|cursor| !cursor.is_empty())
        .map(str::to_string)
}

/// The `Tags` of an answer that carries them as a LIST of entries, each a `Key` and a
/// `Value` — the shape both Amazon SNS and Amazon DynamoDB report a resource's tags in. An
/// entry with no key is no tag and is left out; one whose value the answer omits carries the
/// empty value, which is what the answering service reported.
fn reported_tag_list(answered: &Value) -> BTreeMap<String, String> {
    answered
        .get("Tags")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let key = entry.get("Key").and_then(Value::as_str)?;
                    let value = entry.get("Value").and_then(Value::as_str).unwrap_or_default();
                    Some((key.to_string(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tag map an awsQuery answer carries as a list of entries, read as the two PARALLEL
/// lists of its keys and its values and zipped — sound here because the answering service
/// reports both members on every entry of a tag list.
fn zipped_tags(keys: &[String], values: &[String]) -> BTreeMap<String, String> {
    keys.iter()
        .zip(values.iter())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property each catalogued type's rows are identified by, written out rather than
    /// read from the catalogue under test.
    ///
    /// Source: each type's documented "Return values" — "Ref returns the topic ARN" for
    /// `AWS::SNS::Topic`, "Ref returns the queue URL" for `AWS::SQS::Queue`, "Ref returns the
    /// table name" for `AWS::DynamoDB::Table`, each of which is the property a
    /// `ResourceToImport` names a resource of that type by.
    const PUBLISHED_IDENTIFIERS: &[(&str, &str)] = &[
        ("AWS::SNS::Topic", "TopicArn"),
        ("AWS::SQS::Queue", "QueueUrl"),
        ("AWS::DynamoDB::Table", "TableName"),
    ];

    /// Every catalogued type's row is identified by the property that type PUBLISHES, and by
    /// exactly one — so a row a scan discovered is addressable by what a client names that
    /// resource by.
    #[test]
    fn a_discovered_row_is_identified_by_its_types_published_identifier() {
        for entry in CATALOGUE {
            let resource_type = entry.resource_type();
            let expected = PUBLISHED_IDENTIFIERS
                .iter()
                .find(|(catalogued, _)| *catalogued == resource_type)
                .map(|(_, property)| (*property).to_string())
                .unwrap_or_else(|| {
                    panic!(
                        "case {resource_type:?}: this suite publishes no identifier for that \
                         type; add it from the type's documented return values"
                    )
                });
            let identifier = identifier_of(resource_type, "the-physical-id");
            assert_eq!(
                identifier,
                [(expected.clone(), "the-physical-id".to_string())]
                    .into_iter()
                    .collect::<BTreeMap<String, String>>(),
                "case {resource_type:?}: expected the one published identifier \
                 {expected:?} carrying the physical id, got {identifier:?}"
            );
        }
    }

    /// The catalogue names each type once, and every entry names a type this emulator
    /// implements — so no listing is made for a type a scan could not report.
    #[test]
    fn the_catalogue_names_implemented_types_once_each() {
        let mut named: Vec<&str> = CATALOGUE.iter().map(|entry| entry.resource_type()).collect();
        let listed = named.len();
        named.sort_unstable();
        named.dedup();
        assert_eq!(
            named.len(),
            listed,
            "case \"the catalogue\": expected each type named once, got {named:?}"
        );
        for resource_type in &named {
            assert!(
                super::super::template::IMPLEMENTED_RESOURCE_TYPES.contains(resource_type),
                "case {resource_type:?}: expected a type this emulator implements"
            );
        }
    }

    /// A resource the account holds that a live stack also records is ONE row, the stack's:
    /// the merge decides `ManagedByStack` per row, and never reports the same resource twice.
    #[test]
    fn a_stack_record_wins_over_the_same_resource_in_the_inventory() {
        let topic_arn = "arn:aws:sns:us-east-1:000000000000:managed";
        let managed = FoundResource {
            resource_type: super::super::template::SNS_TOPIC.to_string(),
            physical_resource_id: topic_arn.to_string(),
            logical_resource_id: "Alpha".to_string(),
            stack_id: "arn:aws:cloudformation:us-east-1:000000000000:stack/one/1".to_string(),
            resource_identifier: identifier_of(super::super::template::SNS_TOPIC, topic_arn),
            tags: BTreeMap::new(),
        };
        let unmanaged_arn = "arn:aws:sns:us-east-1:000000000000:unmanaged";
        let discovered = vec![
            FoundResource {
                stack_id: String::new(),
                logical_resource_id: String::new(),
                ..managed.clone()
            },
            FoundResource {
                resource_type: super::super::template::SNS_TOPIC.to_string(),
                physical_resource_id: unmanaged_arn.to_string(),
                logical_resource_id: String::new(),
                stack_id: String::new(),
                resource_identifier: identifier_of(
                    super::super::template::SNS_TOPIC,
                    unmanaged_arn,
                ),
                tags: BTreeMap::new(),
            },
        ];

        let rows = merged(vec![managed.clone()], discovered);
        assert_eq!(
            rows.len(),
            2,
            "case \"one managed and one unmanaged topic\": expected two rows, got {rows:?}"
        );
        assert_eq!(
            rows.first(),
            Some(&managed),
            "case \"one managed and one unmanaged topic\": expected the stack's row kept, got \
             {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.physical_resource_id == unmanaged_arn
                && !row.is_managed_by_stack()),
            "case \"one managed and one unmanaged topic\": expected the unmanaged topic \
             reported unmanaged, got {rows:?}"
        );
        assert!(
            rows[0].is_managed_by_stack(),
            "case \"one managed and one unmanaged topic\": expected the stack's row managed, \
             got {rows:?}"
        );
    }
}
