// SPDX-License-Identifier: CC0-1.0

//! The REFUSAL contract of this service, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling `resource_group_lifecycle` suite's
//! `Emulator`), so these exercise the whole restJson1 path: routing, boundary
//! validation, the handler, and serialization.
//!
//! An error's HTTP status, its code AND its `Message` are all part of the contract,
//! and a refusal's message is a FIXED sentence whose only variable parts are values
//! taken from the request. The rules under test, each asserted over a TABLE of
//! inputs rather than one row:
//!
//! * a `ResourceQuery` whose `Query` document defines no membership for its own
//!   `Type` is refused, by every operation that takes one, with one fixed sentence,
//!   and nothing is stored;
//! * a `CreateGroup` that names both forms of a group — a query and a service
//!   configuration — is refused with one fixed sentence naming the two members;
//! * a group name starting with the reserved prefix, in any capitalization, is
//!   refused with one fixed sentence that does not repeat the submitted name;
//! * an operation applied to a group whose FORM does not support it is refused with
//!   one fixed sentence that names neither the group nor the operation nor the
//!   forms the service supports;
//! * an incompatible `resource-type` filter is refused with a fixed sentence
//!   followed by the offending types TAKEN FROM THE REQUEST.
//!
//! Every refusal is also judged to have changed nothing a later caller sees.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, static_group, Emulator};

/// The message every operation answers for a `ResourceQuery` whose `Query`
/// document defines no membership for the query's own `Type`.
///
/// The whole sentence is fixed: no part of the request appears in it.
const QUERY_NOT_VALID: &str = "Query not valid: Query format not valid: check JSON syntax";

/// The message `CreateGroup` answers for a request naming both a query and a
/// service configuration. Entirely fixed text.
const BOTH_FORMS: &str = "Please use only 'configuration' or 'query' in the request, but not both.";

/// The message `CreateGroup` answers for a name starting with the reserved prefix,
/// whatever capitalization the request used and whatever the name is.
const RESERVED_PREFIX: &str = "Group name must not start with 'AWS'";

/// The message every operation answers for a group whose form does not support it.
/// Entirely fixed text.
const UNSUPPORTED_TARGET: &str = "This operation does not support the target";

/// The fixed part of the message `ListGroupResources` answers for a
/// `resource-type` filter the addressed group's query does not admit, before the
/// list of types that failed the validation.
const INCOMPATIBLE_FILTERS: &str = "Filters not valid: The resource types specified as filters \
     in the request are incompatible with the group query:";

/// The message every operation answers for an identifier that names no group.
const GROUP_NOT_FOUND: &str = "The specified group does not exist";

/// The two values the modelled `QueryType` declares.
///
/// Source: `smithy_query(query="shape",shape="QueryType")`.
const TAG_QUERY: &str = "TAG_FILTERS_1_0";
const STACK_QUERY: &str = "CLOUDFORMATION_STACK_1_0";

/// A resource ARN a static-membership call can carry.
const MEMBER: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";

/// A role ARN shaped like the modelled `StartTagSyncTaskInput$RoleArn`
/// (`^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
const ROLE: &str = "arn:aws:iam::000000000000:role/ResourceGroupsTagSync";

/// Assert a response is `BadRequestException` (400) carrying exactly `message`.
fn assert_bad_request(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        (status, error_code(body)),
        (400, "BadRequestException"),
        "case {case:?}: expected 400 BadRequestException, got {status} {body}",
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(message),
        "case {case:?}: expected the message {message:?}, got {body}",
    );
}

/// A `ResourceQuery` carrying `document` as its `Query`.
///
/// The model carries the query document as a JSON STRING inside the structure
/// (`smithy_query(query="shape",shape="ResourceQuery")`), which is what the
/// `to_string` below writes.
fn query(query_type: &str, document: &Value) -> Value {
    json!({ "Type": query_type, "Query": document.to_string() })
}

/// A `ResourceQuery` whose `Query` is the literal `document`, for the rows whose
/// document is not JSON at all.
fn raw_query(query_type: &str, document: &str) -> Value {
    json!({ "Type": query_type, "Query": document })
}

/// Query documents that define NO membership for their own `Type`: a stack query
/// that names no stack, and a tag query whose `TagFilters` is not the documented
/// collection `[ { "Key": .., "Values": [..] } ]`
/// (`smithy_query(query="doc",shape="ResourceQuery",member="Query")`), plus a
/// document that is not a JSON object at all.
fn undefined_membership_queries() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a stack query naming no stack, limited by type",
            query(STACK_QUERY, &json!({ "ResourceTypeFilters": ["AWS::AllSupported"] })),
        ),
        ("a stack query with an empty document", query(STACK_QUERY, &json!({}))),
        (
            "a stack query whose stack is not a string",
            query(STACK_QUERY, &json!({ "StackIdentifier": 7 })),
        ),
        (
            "a stack query whose stack is null",
            query(STACK_QUERY, &json!({ "StackIdentifier": null })),
        ),
        (
            "a stack query whose stack is the empty string",
            query(STACK_QUERY, &json!({ "StackIdentifier": "" })),
        ),
        (
            "a stack query whose stack is only separators",
            query(STACK_QUERY, &json!({ "StackIdentifier": "   " })),
        ),
        (
            "a stack query naming its stack in a list",
            query(STACK_QUERY, &json!({ "StackIdentifier": ["demo"] })),
        ),
        ("a document that is not JSON", raw_query(TAG_QUERY, "Stage=Test")),
        ("a document that is a JSON array", raw_query(TAG_QUERY, "[]")),
        ("a document that is a JSON string", raw_query(TAG_QUERY, "\"{}\"")),
        ("an empty document", raw_query(STACK_QUERY, "")),
        (
            "a tag query whose TagFilters is not an array",
            query(TAG_QUERY, &json!({ "TagFilters": { "Key": "Stage" } })),
        ),
        (
            "a tag query whose TagFilters entry names no key",
            query(TAG_QUERY, &json!({ "TagFilters": [{ "Values": ["Test"] }] })),
        ),
        (
            "a tag query whose TagFilters key is not a string",
            query(TAG_QUERY, &json!({ "TagFilters": [{ "Key": 7 }] })),
        ),
        (
            "a tag query whose TagFilters values are not an array",
            query(TAG_QUERY, &json!({ "TagFilters": [{ "Key": "Stage", "Values": "Test" }] })),
        ),
        (
            "a tag query with a non-string tag value",
            query(TAG_QUERY, &json!({ "TagFilters": [{ "Key": "Stage", "Values": [7] }] })),
        ),
    ]
}

/// Query documents that DO define a membership, so every operation serves them:
/// the other side of the same rule.
fn defined_membership_queries() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a stack query naming a stack ARN",
            query(
                STACK_QUERY,
                &json!({
                    "StackIdentifier": "arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0"
                }),
            ),
        ),
        (
            "a stack query naming a bare stack name",
            query(STACK_QUERY, &json!({ "StackIdentifier": "demo-stack" })),
        ),
        (
            "a stack query naming a stack and a type limit",
            query(
                STACK_QUERY,
                &json!({
                    "StackIdentifier": "demo-stack",
                    "ResourceTypeFilters": ["AWS::EC2::Instance"]
                }),
            ),
        ),
        ("a tag query with an empty document", query(TAG_QUERY, &json!({}))),
        (
            "a tag query with no filter at all",
            query(TAG_QUERY, &json!({ "TagFilters": [] })),
        ),
        (
            "a tag query filtering on a key with no value",
            query(TAG_QUERY, &json!({ "TagFilters": [{ "Key": "Stage" }] })),
        ),
        (
            "a tag query filtering on two keys and several values",
            query(
                TAG_QUERY,
                &json!({
                    "ResourceTypeFilters": ["AWS::AllSupported"],
                    "TagFilters": [
                        { "Key": "Stage", "Values": ["Test", "Prod"] },
                        { "Key": "Team", "Values": ["Platform"] }
                    ]
                }),
            ),
        ),
    ]
}

/// A document that defines no membership for its type is refused by EVERY
/// operation that takes a `ResourceQuery`, with the one fixed sentence — and the
/// create that was refused stored nothing, so a following read answers 404.
#[tokio::test]
async fn a_query_that_defines_no_membership_is_refused_everywhere_and_stores_nothing() {
    let emulator = Emulator::new();
    // The group an update, a listing and a task are applied to: it keeps the query
    // it was created with through every refusal below.
    let kept = query(TAG_QUERY, &json!({ "TagFilters": [{ "Key": "Stage", "Values": ["Test"] }] }));
    emulator
        .ok("/groups", &json!({ "Name": "queried", "ResourceQuery": kept }))
        .await;
    let stored = emulator
        .ok("/get-group-query", &json!({ "Group": "queried" }))
        .await;

    for (index, (label, document)) in undefined_membership_queries().into_iter().enumerate() {
        let name = format!("refused-{index}");
        // Every operation that takes a `ResourceQuery`, with the request each one
        // needs beside it.
        let calls = [
            ("CreateGroup", "/groups", json!({ "Name": name, "ResourceQuery": document })),
            (
                "UpdateGroupQuery",
                "/update-group-query",
                json!({ "Group": "queried", "ResourceQuery": document }),
            ),
            ("SearchResources", "/resources/search", json!({ "ResourceQuery": document })),
            (
                "StartTagSyncTask",
                "/start-tag-sync-task",
                json!({ "Group": "queried", "RoleArn": ROLE, "ResourceQuery": document }),
            ),
        ];
        for (operation, path, body) in calls {
            let case = format!("{operation} with {label}");
            let (status, answered) = emulator.call(path, &body).await;
            assert_bad_request(&case, status, &answered, QUERY_NOT_VALID);
        }
        // The refused create stored nothing: the group it would have created does
        // not exist.
        let (status, read) = emulator.call("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            (status, error_code(&read), read["Message"].as_str()),
            (404, "NotFoundException", Some(GROUP_NOT_FOUND)),
            "case {label:?}: expected the refused create to have stored no group, \
             got {status} {read}",
        );
    }
    // The refused updates left the stored query exactly as it was.
    assert_eq!(
        emulator
            .ok("/get-group-query", &json!({ "Group": "queried" }))
            .await,
        stored,
        "expected the refusals to leave the stored query alone",
    );
}

/// The other side of the rule: a document that DOES define a membership is stored
/// and read back verbatim, so the refusals above are about the document and not
/// about the operation.
#[tokio::test]
async fn a_query_that_defines_a_membership_is_stored_and_read_back() {
    let emulator = Emulator::new();
    for (index, (label, document)) in defined_membership_queries().into_iter().enumerate() {
        let name = format!("served-{index}");
        let created = emulator
            .ok("/groups", &json!({ "Name": name, "ResourceQuery": document }))
            .await;
        assert_eq!(
            created["ResourceQuery"], document,
            "case {label:?}: expected the create to report the query it stored, got {created}",
        );
        // Read back through an implemented reader: the round trip is what proves the
        // document was stored rather than echoed.
        let read = emulator
            .ok("/get-group-query", &json!({ "Group": name }))
            .await;
        assert_eq!(
            read["GroupQuery"]["ResourceQuery"], document,
            "case {label:?}: expected GetGroupQuery to report the stored query, got {read}",
        );
        // And accepted by a search and a tag-sync task, the other takers of a query.
        let searched = emulator
            .ok("/resources/search", &json!({ "ResourceQuery": document }))
            .await;
        assert!(
            searched["ResourceIdentifiers"].is_array(),
            "case {label:?}: expected the search to be answered, got {searched}",
        );
        let started = emulator
            .ok(
                "/start-tag-sync-task",
                &json!({ "Group": name, "RoleArn": ROLE, "ResourceQuery": document }),
            )
            .await;
        assert!(
            started["TaskArn"].is_string(),
            "case {label:?}: expected the task to be started, got {started}",
        );
    }
}

/// A create naming BOTH forms of a group is refused with the one fixed sentence,
/// over every combination of a well-formed query and a service configuration, and
/// stores nothing.
#[tokio::test]
async fn a_create_naming_both_a_query_and_a_configuration_is_refused() {
    let emulator = Emulator::new();
    let configurations: [(&str, Value); 3] = [
        (
            "a generic configuration with parameters",
            json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": "allowed-resource-types", "Values": ["AWS::EC2::Host"] }]
            }]),
        ),
        (
            "a host-management configuration",
            json!([{ "Type": "AWS::EC2::HostManagement" }]),
        ),
        (
            "two configuration items",
            json!([
                { "Type": "AWS::ResourceGroups::ApplicationGroup" },
                { "Type": "AWS::ResourceGroups::Generic" }
            ]),
        ),
    ];
    let queries = defined_membership_queries();
    for (query_index, (query_label, document)) in queries.into_iter().enumerate() {
        for (config_index, (config_label, configuration)) in configurations.iter().enumerate() {
            let name = format!("both-{query_index}-{config_index}");
            let case = format!("{query_label} beside {config_label}");
            let (status, answered) = emulator
                .call(
                    "/groups",
                    &json!({
                        "Name": name,
                        "ResourceQuery": document,
                        "Configuration": configuration
                    }),
                )
                .await;
            assert_bad_request(&case, status, &answered, BOTH_FORMS);
            let (status, read) = emulator.call("/get-group", &json!({ "Group": name })).await;
            assert_eq!(
                (status, error_code(&read)),
                (404, "NotFoundException"),
                "case {case:?}: expected the refused create to have stored no group, \
                 got {status} {read}",
            );
        }
    }
    // One form alone is served, and so is a `Configuration` that carries no item at
    // all: the refusal is about a group CONTAINING both.
    let served: [(&str, Value); 3] = [
        (
            "a query alone",
            json!({ "Name": "query-only", "ResourceQuery": query(TAG_QUERY, &json!({})) }),
        ),
        ("a configuration alone", static_group("configuration-only")),
        (
            "an empty configuration beside a query",
            json!({
                "Name": "empty-configuration",
                "ResourceQuery": query(TAG_QUERY, &json!({})),
                "Configuration": []
            }),
        ),
    ];
    for (label, request) in served {
        let created = emulator.ok("/groups", &request).await;
        assert!(
            created["Group"]["GroupArn"].is_string(),
            "case {label:?}: expected the create to be served, got {created}",
        );
    }
}

/// Names whose first three characters are the reserved prefix in some
/// capitalization, and names that are not reserved at all.
///
/// Source: `smithy_query(query="doc",shape="CreateGroupInput",member="Name")` —
/// "The name cannot start with AWS, aws, or any other possible capitalization;
/// these are reserved" — so the rule is on the PREFIX, in any case, and every
/// character the modelled pattern `^[a-zA-Z0-9_\.-]+$` admits may follow it.
fn name_cases() -> Vec<(&'static str, &'static str, bool)> {
    vec![
        ("exactly the prefix, lowercase", "aws", true),
        ("exactly the prefix, uppercase", "AWS", true),
        ("the prefix, lowercase, with a suffix", "aws-reserved", true),
        ("the prefix, uppercase, with a suffix", "AWS-reserved", true),
        ("the prefix, leading capital", "Aws-reserved", true),
        ("the prefix, middle capital", "aWs-reserved", true),
        ("the prefix, trailing capital", "awS-reserved", true),
        ("the prefix, two capitals", "AwS-reserved", true),
        ("the prefix run on into a word", "awsome-group", true),
        ("the prefix before a separator this pattern admits", "aws_1.2", true),
        ("a shorter name that is only a part of the prefix", "aw", false),
        ("a one-character name", "a", false),
        ("the prefix without its first letter", "ws-group", false),
        ("the prefix later in the name", "my-aws-group", false),
        ("a name that merely contains the letters", "sawstooth", false),
    ]
}

/// Every capitalization of the reserved prefix is refused with the SAME fixed
/// sentence, which does not repeat the submitted name, and nothing is stored; a
/// name that is not reserved is served.
#[tokio::test]
async fn a_reserved_name_prefix_is_refused_with_one_fixed_sentence() {
    let emulator = Emulator::new();
    for (label, name, reserved) in name_cases() {
        let (status, answered) = emulator.call("/groups", &json!({ "Name": name })).await;
        if reserved {
            assert_bad_request(label, status, &answered, RESERVED_PREFIX);
            let message = answered["Message"].as_str().unwrap_or_default();
            // The sentence is the same for every row, so it cannot carry the name
            // this row sent — tested on the names that are not themselves a part of
            // the sentence (the bare prefix is).
            if name.len() > "aws".len() {
                assert!(
                    !message.contains(name),
                    "case {label:?}: the refusal must not repeat the submitted name, \
                     got {message:?}",
                );
            }
            let (status, read) = emulator.call("/get-group", &json!({ "Group": name })).await;
            assert_eq!(
                (status, error_code(&read)),
                (404, "NotFoundException"),
                "case {label:?}: expected the refused create to have stored no group, \
                 got {status} {read}",
            );
        } else {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the create to be served, got {status} {answered}",
            );
            let read = emulator.ok("/get-group", &json!({ "Group": name })).await;
            assert_eq!(
                read["Group"]["Name"].as_str(),
                Some(name),
                "case {label:?}: expected the group to be stored under its name, got {read}",
            );
        }
    }
}

/// One group the unsupported-target table judges operations against: the
/// `CreateGroup` request that stores it, and which FORM it holds — a resource
/// query, a service configuration, and whether that configuration is one the
/// static-membership operations serve.
struct FormCase {
    label: &'static str,
    name: &'static str,
    request: Value,
    /// Whether the group holds a resource query, which is what the query-reading
    /// operations report and what excludes a configuration from it.
    queried: bool,
    /// Whether the group contains a service configuration, which is what the
    /// configuration-reading operations report and what excludes a query from it.
    configured: bool,
    /// Whether the group's configuration is one the static-membership operations
    /// serve.
    static_membership: bool,
}

fn form_cases() -> Vec<FormCase> {
    vec![
        FormCase {
            label: "a tag-filters query group",
            name: "tag-queried",
            request: json!({
                "Name": "tag-queried",
                "ResourceQuery": { "Type": TAG_QUERY, "Query": "{\"TagFilters\":[]}" }
            }),
            queried: true,
            configured: false,
            static_membership: false,
        },
        FormCase {
            label: "a CloudFormation-stack query group",
            name: "stack-queried",
            request: json!({
                "Name": "stack-queried",
                "ResourceQuery": {
                    "Type": STACK_QUERY,
                    "Query": "{\"StackIdentifier\":\"demo-stack\"}"
                }
            }),
            queried: true,
            configured: false,
            static_membership: false,
        },
        FormCase {
            label: "a host-management configuration group",
            name: "host-managed",
            request: static_group("host-managed"),
            queried: false,
            configured: true,
            static_membership: true,
        },
        FormCase {
            label: "an application configuration group",
            name: "application",
            request: json!({
                "Name": "application",
                "Configuration": [{ "Type": "AWS::ResourceGroups::ApplicationGroup" }]
            }),
            queried: false,
            configured: true,
            static_membership: true,
        },
        FormCase {
            label: "a group configured as a type static membership does not serve",
            name: "generic",
            request: json!({
                "Name": "generic",
                "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }]
            }),
            queried: false,
            configured: true,
            static_membership: false,
        },
        FormCase {
            label: "a group holding neither a query nor a configuration",
            name: "bare",
            request: json!({ "Name": "bare" }),
            queried: false,
            configured: false,
            static_membership: false,
        },
    ]
}

/// An operation applied to a group whose FORM does not support it answers the one
/// fixed sentence — over every form of group crossed with every operation that
/// depends on a form — and leaves the group exactly as it was. A group whose form
/// DOES support the operation is served, which is what makes the cross product
/// cover both sides of the rule.
///
/// Each pair is judged on its own store, since a served call changes the form of
/// the group it is applied to and the next pair must judge the form its row
/// declares.
#[tokio::test]
async fn an_operation_a_group_form_does_not_support_answers_one_fixed_sentence() {
    for case in form_cases() {
        let group = case.name;
        // Every form-dependent operation: the path, its request, and whether THIS
        // group's form supports it.
        let calls: [(&str, &str, Value, bool); 6] = [
            (
                "GetGroupQuery",
                "/get-group-query",
                json!({ "Group": group }),
                case.queried,
            ),
            (
                "UpdateGroupQuery",
                "/update-group-query",
                json!({
                    "Group": group,
                    "ResourceQuery": { "Type": TAG_QUERY, "Query": "{\"TagFilters\":[]}" }
                }),
                // A group that contains no configuration takes a query, whether or
                // not it already holds one.
                !case.configured,
            ),
            (
                "GetGroupConfiguration",
                "/get-group-configuration",
                json!({ "Group": group }),
                case.configured,
            ),
            (
                "PutGroupConfiguration",
                "/put-group-configuration",
                json!({
                    "Group": group,
                    "Configuration": [{ "Type": "AWS::EC2::HostManagement" }]
                }),
                // A group that is not queried takes a configuration, whether or not
                // it already holds one.
                !case.queried,
            ),
            (
                "GroupResources",
                "/group-resources",
                json!({ "Group": group, "ResourceArns": [MEMBER] }),
                case.static_membership,
            ),
            (
                "UngroupResources",
                "/ungroup-resources",
                json!({ "Group": group, "ResourceArns": [MEMBER] }),
                case.static_membership,
            ),
        ];
        for (operation, path, body, supported) in calls {
            let label = format!("{operation} on {}", case.label);
            let emulator = Emulator::new();
            emulator.ok("/groups", &case.request).await;
            let before = emulator.ok("/get-group", &json!({ "Group": group })).await;
            let (status, answered) = emulator.call(path, &body).await;
            if supported {
                assert!(
                    (200..300).contains(&status),
                    "case {label:?}: this group's form supports the operation, so \
                     expected a success, got {status} {answered}",
                );
            } else {
                assert_bad_request(&label, status, &answered, UNSUPPORTED_TARGET);
                let after = emulator.ok("/get-group", &json!({ "Group": group })).await;
                assert_eq!(
                    after, before,
                    "case {label:?}: expected the refusal to leave the group alone, \
                     got {after} after {before}",
                );
            }
        }
    }
}

/// One row of the incompatible-filter table: the types the addressed group's query
/// admits, the whole `Filters` list the request sends, and the types the refusal
/// must name, in order.
struct FilterCase {
    label: &'static str,
    admits: &'static [&'static str],
    filters: Value,
    unadmitted: &'static [&'static str],
}

fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            label: "one unadmitted type",
            admits: &["AWS::SQS::Queue"],
            filters: json!([{ "Name": "resource-type", "Values": ["AWS::EC2::Instance"] }]),
            unadmitted: &["AWS::EC2::Instance"],
        },
        FilterCase {
            label: "an admitted type beside an unadmitted one",
            admits: &["AWS::S3::Bucket", "AWS::EC2::Volume"],
            filters: json!([{
                "Name": "resource-type",
                "Values": ["AWS::S3::Bucket", "AWS::Lambda::Function"]
            }]),
            unadmitted: &["AWS::Lambda::Function"],
        },
        FilterCase {
            label: "three unadmitted types in one entry",
            admits: &["AWS::EC2::Instance"],
            filters: json!([{
                "Name": "resource-type",
                "Values": ["AWS::SQS::Queue", "AWS::S3::Bucket", "AWS::DynamoDB::Table"]
            }]),
            unadmitted: &["AWS::SQS::Queue", "AWS::S3::Bucket", "AWS::DynamoDB::Table"],
        },
        FilterCase {
            label: "unadmitted types spread over two entries, one repeated",
            admits: &["AWS::EC2::Instance"],
            filters: json!([
                { "Name": "resource-type", "Values": ["AWS::S3::Bucket"] },
                { "Name": "resource-type", "Values": ["AWS::SQS::Queue", "AWS::S3::Bucket"] }
            ]),
            unadmitted: &["AWS::S3::Bucket", "AWS::SQS::Queue"],
        },
        FilterCase {
            label: "an admitted type spelled in another case",
            admits: &["AWS::S3::Bucket"],
            filters: json!([{ "Name": "resource-type", "Values": ["AWS::S3::bucket"] }]),
            unadmitted: &["AWS::S3::bucket"],
        },
    ]
}

/// The refusal is the fixed sentence followed by the types THE REQUEST named that
/// the group's query does not admit — each once, in request order — and nothing
/// else: the whole message is asserted, over rows differing in how many types fail,
/// where they sit in the request and what the group admits.
#[tokio::test]
async fn an_incompatible_resource_type_filter_reports_the_request_s_own_types() {
    let emulator = Emulator::new();
    for (index, case) in filter_cases().into_iter().enumerate() {
        let name = format!("filtered-{index}");
        let document = json!({ "ResourceTypeFilters": case.admits });
        emulator
            .ok(
                "/groups",
                &json!({ "Name": name, "ResourceQuery": query(TAG_QUERY, &document) }),
            )
            .await;
        let (status, answered) = emulator
            .call(
                "/list-group-resources",
                &json!({ "Group": name, "Filters": case.filters }),
            )
            .await;
        let expected = format!("{INCOMPATIBLE_FILTERS} {}", case.unadmitted.join(", "));
        assert_bad_request(case.label, status, &answered, &expected);
        // A filter the query admits is answered with a listing, so the refusal is
        // about the incompatible value and not about the group.
        let admitted = emulator
            .ok(
                "/list-group-resources",
                &json!({
                    "Group": name,
                    "Filters": [{ "Name": "resource-type", "Values": [case.admits[0]] }]
                }),
            )
            .await;
        assert!(
            admitted["Resources"].is_array(),
            "case {:?}: expected an admitted filter to be answered with a listing, \
             got {admitted}",
            case.label,
        );
    }
}
