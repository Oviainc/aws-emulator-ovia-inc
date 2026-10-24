// SPDX-License-Identifier: CC0-1.0

//! `GetGroupQuery`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * the operation reports the query the addressed group was STORED with, verbatim,
//!   beside that group's own stored name;
//! * a group is addressed by name, by its stored ARN, or through the deprecated
//!   `GroupName`, and every form reports the same query, in the region the group
//!   lives in;
//! * reporting changes nothing: repeated calls answer the same query and the group is
//!   unchanged afterwards;
//! * an identifier that names no stored group is `NotFoundException` (404), and a
//!   request this group cannot answer — none named, or a group holding no query at
//!   all — is `BadRequestException` (400).

use serde_json::json;

use super::resource_group_lifecycle::{error_code, Emulator, ACCOUNT};

/// Every value the modelled `QueryType` enum declares — the domain of
/// `ResourceQuery$Type`, and so of the `Type` a stored query can carry.
///
/// Source: `smithy_query(query="shape",shape="QueryType")` on this service's model,
/// which answers `["TAG_FILTERS_1_0", "CLOUDFORMATION_STACK_1_0"]`.
const QUERY_TYPE_VALUES: [&str; 2] = ["TAG_FILTERS_1_0", "CLOUDFORMATION_STACK_1_0"];

/// One group created with a resource query: the query document varies in type, in
/// size and in shape, which is what the reporting rule generalises over.
struct QueryCase {
    label: &'static str,
    /// The region the creating caller's credential scope names, so the group's
    /// stored ARN varies too.
    region: &'static str,
    name: &'static str,
    query_type: &'static str,
    /// `ResourceQuery$Query`, a JSON document carried as a string.
    query: &'static str,
}

/// Groups spanning both declared query types and query documents of different
/// sizes and shapes (empty object, one filter, many filters, a stack identifier),
/// created in different regions.
fn query_cases() -> Vec<QueryCase> {
    vec![
        QueryCase {
            label: "tag query with one tag filter",
            region: "us-east-1",
            name: "one-filter",
            query_type: "TAG_FILTERS_1_0",
            query: r#"{"ResourceTypeFilters":["AWS::AllSupported"],"TagFilters":[{"Key":"Stage","Values":["Test"]}]}"#,
        },
        QueryCase {
            label: "tag query with several filters and several resource types",
            region: "eu-west-1",
            name: "many.filters_2",
            query_type: "TAG_FILTERS_1_0",
            query: r#"{"ResourceTypeFilters":["AWS::EC2::Instance","AWS::S3::Bucket","AWS::Lambda::Function"],"TagFilters":[{"Key":"Stage","Values":["Test","Prod"]},{"Key":"Team","Values":["Platform"]},{"Key":"Owner","Values":[]}]}"#,
        },
        QueryCase {
            label: "tag query whose document selects nothing",
            region: "ap-southeast-2",
            name: "empty-document",
            query_type: "TAG_FILTERS_1_0",
            query: "{}",
        },
        QueryCase {
            label: "cloudformation-stack query naming a stack ARN",
            region: "us-west-2",
            name: "stack.query-3",
            query_type: "CLOUDFORMATION_STACK_1_0",
            query: r#"{"StackIdentifier":"arn:aws:cloudformation:us-west-2:000000000000:stack/demo/0"}"#,
        },
        QueryCase {
            label: "cloudformation-stack query naming a bare stack name",
            region: "us-east-2",
            name: "bare_stack",
            query_type: "CLOUDFORMATION_STACK_1_0",
            query: r#"{"StackIdentifier":"demo-stack"}"#,
        },
    ]
}

/// The ARN every operation of this service reports for the group `name` in `region`.
fn group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:resource-groups:{region}:{ACCOUNT}:group/{name}")
}

/// Every table row's query type is one the model declares, so the table spans the
/// declared domain rather than one value of it.
#[test]
fn the_table_spans_every_declared_query_type() {
    for query_type in QUERY_TYPE_VALUES {
        assert!(
            query_cases().iter().any(|case| case.query_type == query_type),
            "case {query_type:?}: the table must cover every declared QueryType, \
             got {:?}",
            query_cases()
                .iter()
                .map(|case| case.query_type)
                .collect::<Vec<_>>(),
        );
    }
}

/// The query a group was created with is reported verbatim, beside the group's own
/// stored name, whichever identifier form addresses it — and reporting it changes
/// nothing.
#[tokio::test]
async fn a_stored_query_is_reported_for_every_identifier_form() {
    let emulator = Emulator::new();
    for case in query_cases() {
        let label = case.label;
        let sent = json!({ "Type": case.query_type, "Query": case.query });
        let created = emulator
            .call_in(
                case.region,
                "/groups",
                &json!({
                    "Name": case.name,
                    "ResourceQuery": sent,
                }),
            )
            .await;
        assert_eq!(
            created.0, 200,
            "case {label:?}: expected CreateGroup to answer 200, got {} {}",
            created.0, created.1,
        );
        let arn = group_arn(case.region, case.name);
        assert_eq!(
            created.1["Group"]["GroupArn"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected the created group's ARN to be {arn}, got {}",
            created.1,
        );

        // The identifier forms the model admits: a bare name, the stored ARN, and
        // the same two through the deprecated member.
        let identifiers = [
            ("Group as a name", json!({ "Group": case.name })),
            ("Group as the stored ARN", json!({ "Group": arn })),
            ("deprecated GroupName", json!({ "GroupName": case.name })),
            (
                "both members, Group winning",
                json!({ "Group": case.name, "GroupName": "a-group-that-does-not-exist" }),
            ),
        ];
        let expected = json!({
            "GroupName": case.name,
            "ResourceQuery": { "Type": case.query_type, "Query": case.query },
        });
        for (form, body) in identifiers {
            let (status, answered) = emulator.call_in(case.region, "/get-group-query", &body).await;
            assert_eq!(
                status, 200,
                "case {label:?} ({form}): expected 200, got {status} {answered}",
            );
            assert_eq!(
                answered["GroupQuery"], expected,
                "case {label:?} ({form}): expected GroupQuery {expected}, got {answered}",
            );
        }

        // A group belongs to the region whose endpoint created it, so a caller
        // signing for another region addresses no group at all.
        let (status, elsewhere) = emulator
            .call_in("eu-central-1", "/get-group-query", &json!({ "Group": case.name }))
            .await;
        assert_eq!(
            (status, error_code(&elsewhere)),
            (404, "NotFoundException"),
            "case {label:?}: expected 404 NotFoundException from another region, \
             got {status} {elsewhere}",
        );

        // Reporting is not a mutation: the group still reads back as itself, and
        // its query is unchanged.
        let group = emulator
            .ok_in(case.region, "/get-group", &json!({ "Group": case.name }))
            .await;
        assert_eq!(
            group["Group"]["GroupArn"].as_str(),
            Some(group_arn(case.region, case.name).as_str()),
            "case {label:?}: expected GetGroup to still report the group, got {group}",
        );
        let again = emulator
            .ok_in(case.region, "/get-group-query", &json!({ "Group": case.name }))
            .await;
        assert_eq!(
            again["GroupQuery"], expected,
            "case {label:?}: expected a repeated read to answer the same query, got {again}",
        );
    }
}

/// An identifier that names no stored group is `NotFoundException` (404) —
/// including an ARN whose group name exists locally but whose region or account
/// does not match the stored ARN.
#[tokio::test]
async fn an_identifier_that_names_no_stored_group_is_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({
                "Name": "present",
                "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "{}" }
            }),
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
        for body in [
            json!({ "Group": identifier }),
            json!({ "GroupName": identifier }),
        ] {
            let (status, answered) = emulator.call("/get-group-query", &body).await;
            assert_eq!(
                (status, error_code(&answered)),
                (404, "NotFoundException"),
                "case {label:?} ({body}): expected 404 NotFoundException, \
                 got {status} {answered}",
            );
        }
    }
}

/// A request this operation cannot serve is `BadRequestException` (400): one that
/// names no group at all, and one that names a group holding no query, because a
/// group contains either a query or a service configuration and a 200 with an
/// empty body would report the group as queried by nothing.
#[tokio::test]
async fn a_request_the_group_cannot_answer_is_bad_request() {
    let emulator = Emulator::new();
    let groups_without_a_query = [
        (
            "a group created with a service configuration",
            "configured",
            json!({
                "Name": "configured",
                "Configuration": [{ "Type": "AWS::EC2::HostManagement" }]
            }),
        ),
        (
            "a group created with neither a query nor a configuration",
            "plain",
            json!({ "Name": "plain" }),
        ),
        (
            "a group created with an empty configuration list",
            "empty-configuration",
            json!({ "Name": "empty-configuration", "Configuration": [] }),
        ),
    ];
    for (label, name, request) in groups_without_a_query {
        emulator.ok("/groups", &request).await;
        for body in [
            json!({ "Group": name }),
            json!({ "Group": group_arn("us-east-1", name) }),
            json!({ "GroupName": name }),
        ] {
            let (status, answered) = emulator.call("/get-group-query", &body).await;
            assert_eq!(
                (status, error_code(&answered)),
                (400, "BadRequestException"),
                "case {label:?} ({body}): expected 400 BadRequestException, \
                 got {status} {answered}",
            );
        }
        // The refusal left the group alone.
        let group = emulator.ok("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            group["Group"]["Name"].as_str(),
            Some(name),
            "case {label:?}: expected the group to be untouched, got {group}",
        );
    }

    // A request naming no group addresses nothing. (A member sent as JSON `null`
    // is refused at the boundary, before the handler, so the handler's own refusal
    // is the one for a member that is simply absent.)
    for body in [json!({}), json!({ "Tags": { "Stage": "Test" } })] {
        let (status, answered) = emulator.call("/get-group-query", &body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {body}: expected 400 BadRequestException, got {status} {answered}",
        );
    }
}
