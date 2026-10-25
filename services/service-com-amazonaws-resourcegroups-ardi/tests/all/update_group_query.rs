// SPDX-License-Identifier: CC0-1.0

//! `UpdateGroupQuery`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * the call REPLACES the addressed group's stored resource query with the one the
//!   request carries, stores the document verbatim, and reports the group's query as
//!   the store then holds it — which is what a later `GetGroupQuery` reports;
//! * a group is addressed by name, by its stored ARN, or through the deprecated
//!   `GroupName`, and every form updates the same group, in the region it lives in;
//! * a group contains either a resource query or a service configuration, never
//!   both, so a group whose stored configuration carries an item is refused with
//!   `BadRequestException` (400) and keeps its configuration;
//! * an identifier that names no stored group of the request's region is
//!   `NotFoundException` (404), and a request this operation cannot serve — naming no
//!   group, or carrying a query this service does not store — is `BadRequestException`
//!   (400); either way the stored query is left exactly as it was.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, Emulator, ACCOUNT};

/// Every value the modelled `QueryType` enum declares — the domain of
/// `ResourceQuery$Type`, and so of the `Type` an updated query can carry.
///
/// Source: `smithy_query(query="shape",shape="QueryType")` on this service's model,
/// which answers `["TAG_FILTERS_1_0", "CLOUDFORMATION_STACK_1_0"]`.
const QUERY_TYPE_VALUES: [&str; 2] = ["TAG_FILTERS_1_0", "CLOUDFORMATION_STACK_1_0"];

/// One group and one replacement of its resource query: the documents vary in type,
/// in size and in shape, which is what the replacement rule generalises over.
struct UpdateCase {
    label: &'static str,
    /// The region the creating caller's credential scope names, so the group's
    /// stored ARN — and the endpoint the update must be sent to — vary too.
    region: &'static str,
    name: &'static str,
    /// The `Type`/`Query` pair the group is created with.
    stored: (&'static str, &'static str),
    /// The `Type`/`Query` pair the update carries.
    updated: (&'static str, &'static str),
}

/// Replacements spanning both declared query types (in both directions), query
/// documents of different sizes and shapes (an empty object, one filter, many
/// filters, a stack identifier), in different regions.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "one tag filter replaced by several",
            region: "us-east-1",
            name: "more-filters",
            stored: (
                "TAG_FILTERS_1_0",
                r#"{"ResourceTypeFilters":["AWS::AllSupported"],"TagFilters":[{"Key":"Stage","Values":["Test"]}]}"#,
            ),
            updated: (
                "TAG_FILTERS_1_0",
                r#"{"ResourceTypeFilters":["AWS::EC2::Instance","AWS::S3::Bucket","AWS::Lambda::Function"],"TagFilters":[{"Key":"Stage","Values":["Test","Prod"]},{"Key":"Team","Values":["Platform"]},{"Key":"Owner","Values":[]}]}"#,
            ),
        },
        UpdateCase {
            label: "several tag filters replaced by a document that selects nothing",
            region: "eu-west-1",
            name: "narrowed.query_1",
            stored: (
                "TAG_FILTERS_1_0",
                r#"{"ResourceTypeFilters":["AWS::AllSupported"],"TagFilters":[{"Key":"Stage","Values":["Test","Prod"]},{"Key":"Team","Values":["Platform"]}]}"#,
            ),
            updated: ("TAG_FILTERS_1_0", "{}"),
        },
        UpdateCase {
            label: "a tag query replaced by a cloudformation-stack query",
            region: "ap-southeast-2",
            name: "tag-to-stack",
            stored: (
                "TAG_FILTERS_1_0",
                r#"{"ResourceTypeFilters":["AWS::AllSupported"]}"#,
            ),
            updated: (
                "CLOUDFORMATION_STACK_1_0",
                r#"{"StackIdentifier":"arn:aws:cloudformation:ap-southeast-2:000000000000:stack/demo/0"}"#,
            ),
        },
        UpdateCase {
            label: "a cloudformation-stack query replaced by a tag query",
            region: "us-west-2",
            name: "stack_to-tag.4",
            stored: (
                "CLOUDFORMATION_STACK_1_0",
                r#"{"StackIdentifier":"demo-stack"}"#,
            ),
            updated: (
                "TAG_FILTERS_1_0",
                r#"{"ResourceTypeFilters":["AWS::EC2::Instance"],"TagFilters":[{"Key":"Stage","Values":["Prod"]}]}"#,
            ),
        },
        UpdateCase {
            label: "a stack query replaced by another stack",
            region: "us-east-2",
            name: "another_stack",
            stored: (
                "CLOUDFORMATION_STACK_1_0",
                r#"{"StackIdentifier":"first-stack"}"#,
            ),
            updated: (
                "CLOUDFORMATION_STACK_1_0",
                r#"{"StackIdentifier":"second-stack","ResourceTypeFilters":["AWS::S3::Bucket"]}"#,
            ),
        },
    ]
}

/// The ARN every operation of this service reports for the group `name` in `region`.
fn group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:resource-groups:{region}:{ACCOUNT}:group/{name}")
}

/// The wire form of a `Type`/`Query` pair.
fn query(pair: (&str, &str)) -> Value {
    json!({ "Type": pair.0, "Query": pair.1 })
}

/// The `GroupQuery` an operation reports for the group `name` queried by `pair`.
fn group_query(name: &str, pair: (&str, &str)) -> Value {
    json!({ "GroupName": name, "ResourceQuery": query(pair) })
}

/// Every table row's query types are ones the model declares, and the table covers
/// each declared value on both sides of an update, so it spans the declared domain
/// rather than one value of it.
#[test]
fn the_table_spans_every_declared_query_type() {
    for query_type in QUERY_TYPE_VALUES {
        for (side, stored_side) in [("stored", true), ("updated", false)] {
            assert!(
                update_cases().iter().any(|case| {
                    let (declared, _) = if stored_side { case.stored } else { case.updated };
                    declared == query_type
                }),
                "case {query_type:?} ({side}): the table must cover every declared \
                 QueryType on both sides of an update, got {:?}",
                update_cases()
                    .iter()
                    .map(|case| (case.stored.0, case.updated.0))
                    .collect::<Vec<_>>(),
            );
        }
    }
}

/// The update replaces the stored query with the one the request carries, reports
/// the query the store then holds beside the group's own stored name, and leaves the
/// group's identity alone. A later `GetGroupQuery` reports exactly that query, and
/// repeating the call reports the same thing again.
#[tokio::test]
async fn an_update_replaces_the_stored_query_and_reads_back() {
    let emulator = Emulator::new();
    for case in update_cases() {
        let label = case.label;
        emulator
            .ok_in(
                case.region,
                "/groups",
                &json!({ "Name": case.name, "ResourceQuery": query(case.stored) }),
            )
            .await;
        // The group starts out queried by the document it was created with.
        let before = emulator
            .ok_in(case.region, "/get-group-query", &json!({ "Group": case.name }))
            .await;
        assert_eq!(
            before["GroupQuery"],
            group_query(case.name, case.stored),
            "case {label:?}: expected the created query to be reported first, got {before}",
        );

        let expected = group_query(case.name, case.updated);
        let answered = emulator
            .ok_in(
                case.region,
                "/update-group-query",
                &json!({ "Group": case.name, "ResourceQuery": query(case.updated) }),
            )
            .await;
        assert_eq!(
            answered["GroupQuery"], expected,
            "case {label:?}: expected GroupQuery {expected}, got {answered}",
        );

        // The store, not the response, is the thing under test: the reader answers
        // the replaced query verbatim.
        let read_back = emulator
            .ok_in(case.region, "/get-group-query", &json!({ "Group": case.name }))
            .await;
        assert_eq!(
            read_back["GroupQuery"], expected,
            "case {label:?}: expected GetGroupQuery to report {expected}, got {read_back}",
        );

        // The group's identity is not updatable, so the replacement left it alone.
        let group = emulator
            .ok_in(case.region, "/get-group", &json!({ "Group": case.name }))
            .await;
        let arn = group_arn(case.region, case.name);
        assert_eq!(
            (
                group["Group"]["Name"].as_str(),
                group["Group"]["GroupArn"].as_str()
            ),
            (Some(case.name), Some(arn.as_str())),
            "case {label:?}: expected the group to still be {} {arn}, got {group}",
            case.name,
        );

        // Writing the same query again is the same state: the second call reports
        // what the first one stored.
        let again = emulator
            .ok_in(
                case.region,
                "/update-group-query",
                &json!({ "Group": case.name, "ResourceQuery": query(case.updated) }),
            )
            .await;
        assert_eq!(
            again["GroupQuery"], expected,
            "case {label:?}: expected a repeated update to report {expected}, got {again}",
        );
    }
}

/// Every identifier form the model admits addresses the same group: a bare name,
/// the group's stored ARN, and the deprecated `GroupName` — with `Group` winning
/// when both members are sent. Each form writes a distinct document, so the one the
/// store holds afterwards says which group the form reached.
#[tokio::test]
async fn every_identifier_form_updates_the_same_group() {
    let emulator = Emulator::new();
    let name = "addressed";
    let arn = group_arn("us-east-1", name);
    emulator
        .ok(
            "/groups",
            &json!({
                "Name": name,
                "ResourceQuery": query(("TAG_FILTERS_1_0", "{}")),
            }),
        )
        .await;
    let forms = [
        (
            "Group as a name",
            json!({ "Group": name }),
            r#"{"TagFilters":[{"Key":"By","Values":["name"]}]}"#,
        ),
        (
            "Group as the stored ARN",
            json!({ "Group": arn }),
            r#"{"TagFilters":[{"Key":"By","Values":["arn"]}]}"#,
        ),
        (
            "deprecated GroupName",
            json!({ "GroupName": name }),
            r#"{"TagFilters":[{"Key":"By","Values":["group-name"]}]}"#,
        ),
        (
            "both members, Group winning",
            json!({ "Group": name, "GroupName": "a-group-that-does-not-exist" }),
            r#"{"TagFilters":[{"Key":"By","Values":["both"]}]}"#,
        ),
    ];
    for (form, identifier, document) in forms {
        let pair = ("TAG_FILTERS_1_0", document);
        let mut body = identifier.clone();
        body["ResourceQuery"] = query(pair);
        let answered = emulator.ok("/update-group-query", &body).await;
        let expected = group_query(name, pair);
        assert_eq!(
            answered["GroupQuery"], expected,
            "case {form:?}: expected GroupQuery {expected}, got {answered}",
        );
        let read_back = emulator
            .ok("/get-group-query", &json!({ "Group": name }))
            .await;
        assert_eq!(
            read_back["GroupQuery"], expected,
            "case {form:?}: expected GetGroupQuery to report {expected}, got {read_back}",
        );
    }

    // A group belongs to the region whose endpoint created it, so a caller signing
    // for another region addresses no group at all — and writes nothing.
    let last = emulator
        .ok("/get-group-query", &json!({ "Group": name }))
        .await;
    for region in ["eu-central-1", "us-west-2"] {
        let (status, elsewhere) = emulator
            .call_in(
                region,
                "/update-group-query",
                &json!({
                    "Group": name,
                    "ResourceQuery": query(("TAG_FILTERS_1_0", r#"{"TagFilters":[]}"#)),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&elsewhere)),
            (404, "NotFoundException"),
            "case {region:?}: expected 404 NotFoundException from another region, \
             got {status} {elsewhere}",
        );
    }
    let unchanged = emulator
        .ok("/get-group-query", &json!({ "Group": name }))
        .await;
    assert_eq!(
        unchanged["GroupQuery"], last["GroupQuery"],
        "expected a refused cross-region update to leave the query {}, got {unchanged}",
        last["GroupQuery"],
    );
}

/// A group contains either a resource query or a service configuration, never both,
/// so a group whose stored configuration carries an item cannot be given a query:
/// that is `BadRequestException` (400), and the group keeps the configuration it had.
#[tokio::test]
async fn a_group_that_holds_a_configuration_is_refused() {
    let emulator = Emulator::new();
    // Configuration types this emulator's `CreateGroup` stores, including one group
    // configured with two items and one configured as an application group.
    let configured_groups = [
        (
            "a group configured for host management",
            "host-managed",
            json!([{ "Type": "AWS::EC2::HostManagement" }]),
        ),
        (
            "a generic group",
            "generic",
            json!([{ "Type": "AWS::ResourceGroups::Generic" }]),
        ),
        (
            "a group configured with two items",
            "two-items",
            json!([
                { "Type": "AWS::EC2::HostManagement" },
                { "Type": "AWS::ResourceGroups::Generic" }
            ]),
        ),
        (
            "an application group",
            "application",
            json!([{ "Type": "AWS::ResourceGroups::ApplicationGroup" }]),
        ),
    ];
    for (label, name, configuration) in configured_groups {
        emulator
            .ok(
                "/groups",
                &json!({ "Name": name, "Configuration": configuration }),
            )
            .await;
        for identifier in [json!({ "Group": name }), json!({ "GroupName": name })] {
            let mut body = identifier.clone();
            body["ResourceQuery"] = query(("TAG_FILTERS_1_0", "{}"));
            let (status, answered) = emulator.call("/update-group-query", &body).await;
            assert_eq!(
                (status, error_code(&answered)),
                (400, "BadRequestException"),
                "case {label:?} ({body}): expected 400 BadRequestException, \
                 got {status} {answered}",
            );
        }
        // The refusal stored nothing: the group still holds its configuration, and
        // still has no query to report.
        let read_back = emulator
            .ok("/get-group-configuration", &json!({ "Group": name }))
            .await;
        assert_eq!(
            read_back["GroupConfiguration"]["Configuration"], configuration,
            "case {label:?}: expected the configuration {configuration} to be kept, \
             got {read_back}",
        );
        let (status, queried) = emulator
            .call("/get-group-query", &json!({ "Group": name }))
            .await;
        assert_eq!(
            (status, error_code(&queried)),
            (400, "BadRequestException"),
            "case {label:?}: expected the group to still hold no query, \
             got {status} {queried}",
        );
    }
}

/// A group that carries neither a query nor a configuration is not caught by the
/// mutual-exclusion rule, so the update stores its query and a reader reports it.
#[tokio::test]
async fn a_group_that_holds_neither_gains_the_query() {
    let emulator = Emulator::new();
    let groups_without_either = [
        (
            "a group created with no query and no configuration",
            "plain",
            json!({ "Name": "plain" }),
            ("TAG_FILTERS_1_0", r#"{"ResourceTypeFilters":["AWS::AllSupported"]}"#),
        ),
        (
            "a group created with an empty configuration list",
            "empty-configuration",
            json!({ "Name": "empty-configuration", "Configuration": [] }),
            (
                "CLOUDFORMATION_STACK_1_0",
                r#"{"StackIdentifier":"late-stack"}"#,
            ),
        ),
    ];
    for (label, name, create, pair) in groups_without_either {
        emulator.ok("/groups", &create).await;
        // Nothing queries the group yet.
        let (status, before) = emulator
            .call("/get-group-query", &json!({ "Group": name }))
            .await;
        assert_eq!(
            (status, error_code(&before)),
            (400, "BadRequestException"),
            "case {label:?}: expected no query before the update, got {status} {before}",
        );
        let expected = group_query(name, pair);
        let answered = emulator
            .ok(
                "/update-group-query",
                &json!({ "Group": name, "ResourceQuery": query(pair) }),
            )
            .await;
        assert_eq!(
            answered["GroupQuery"], expected,
            "case {label:?}: expected GroupQuery {expected}, got {answered}",
        );
        let read_back = emulator
            .ok("/get-group-query", &json!({ "Group": name }))
            .await;
        assert_eq!(
            read_back["GroupQuery"], expected,
            "case {label:?}: expected GetGroupQuery to report {expected}, got {read_back}",
        );
    }
}

/// An identifier that names no stored group of the request's region is
/// `NotFoundException` (404).
#[tokio::test]
async fn an_identifier_that_names_no_stored_group_is_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({ "Name": "present", "ResourceQuery": query(("TAG_FILTERS_1_0", "{}")) }),
        )
        .await;
    let cases = [
        ("a name nothing was created under", "absent".to_string()),
        (
            "an ARN for the right name in another region",
            group_arn("eu-west-1", "present"),
        ),
        (
            "an ARN for the right name in another account",
            "arn:aws:resource-groups:us-east-1:111111111111:group/present".to_string(),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:ec2:us-east-1:{ACCOUNT}:group/present"),
        ),
        ("a name-shaped identifier with a colon", "a:b".to_string()),
    ];
    for (label, identifier) in cases {
        for member in ["Group", "GroupName"] {
            let body = json!({
                member: identifier,
                "ResourceQuery": query(("TAG_FILTERS_1_0", r#"{"TagFilters":[]}"#)),
            });
            let (status, answered) = emulator.call("/update-group-query", &body).await;
            assert_eq!(
                (status, error_code(&answered)),
                (404, "NotFoundException"),
                "case {label:?} ({body}): expected 404 NotFoundException, \
                 got {status} {answered}",
            );
        }
    }
    // Nothing above wrote anything to the group that does exist.
    let read_back = emulator
        .ok("/get-group-query", &json!({ "Group": "present" }))
        .await;
    assert_eq!(
        read_back["GroupQuery"],
        group_query("present", ("TAG_FILTERS_1_0", "{}")),
        "expected the stored query to be untouched, got {read_back}",
    );
}

/// A request this operation cannot serve is `BadRequestException` (400): one naming
/// no group, and one carrying a query this service does not store — a `Type` the
/// model does not declare, or a `Query` that is not a JSON object. The stored query
/// survives every refusal.
#[tokio::test]
async fn a_request_the_operation_cannot_serve_is_bad_request() {
    let emulator = Emulator::new();
    let name = "refusals";
    let stored = ("TAG_FILTERS_1_0", r#"{"ResourceTypeFilters":["AWS::AllSupported"]}"#);
    emulator
        .ok(
            "/groups",
            &json!({ "Name": name, "ResourceQuery": query(stored) }),
        )
        .await;
    let cases = [
        (
            "no group named at all",
            json!({ "ResourceQuery": query(("TAG_FILTERS_1_0", "{}")) }),
        ),
        (
            "a Type the model does not declare",
            json!({ "Group": name, "ResourceQuery": query(("TAG_FILTERS_9_9", "{}")) }),
        ),
        (
            "a Type that is the empty string",
            json!({ "Group": name, "ResourceQuery": query(("", "{}")) }),
        ),
        (
            "a Query that is not JSON",
            json!({ "Group": name, "ResourceQuery": query(("TAG_FILTERS_1_0", "not json at all")) }),
        ),
        (
            "a Query that is JSON but not an object",
            json!({ "Group": name, "ResourceQuery": query(("TAG_FILTERS_1_0", "[]")) }),
        ),
        (
            "a Query that is a JSON number",
            json!({ "Group": name, "ResourceQuery": query(("CLOUDFORMATION_STACK_1_0", "42")) }),
        ),
        (
            "no ResourceQuery, which the model requires",
            json!({ "Group": name }),
        ),
    ];
    for (label, body) in cases {
        let (status, answered) = emulator.call("/update-group-query", &body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?} ({body}): expected 400 BadRequestException, \
             got {status} {answered}",
        );
    }
    let read_back = emulator
        .ok("/get-group-query", &json!({ "Group": name }))
        .await;
    assert_eq!(
        read_back["GroupQuery"],
        group_query(name, stored),
        "expected every refusal to leave the stored query alone, got {read_back}",
    );
}
