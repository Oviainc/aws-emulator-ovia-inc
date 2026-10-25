// SPDX-License-Identifier: CC0-1.0

//! `ListGroups` filtering, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * every filter name the model declares is answered with a 200 listing, computed
//!   from the stored groups;
//! * a filter selects the groups whose named attribute carries one of its values —
//!   the single-valued `criticality`, `display-name` and `owner`, the service
//!   configuration types a group declares (`configuration-type`), and the resource
//!   types a group's stored query is limited to (`resource-type`);
//! * several filters narrow together, and a filtered listing still pages in name
//!   order;
//! * a filter name the model does not declare is `BadRequestException` (400).

use serde_json::{json, Value};

use super::resource_group_lifecycle::{
    error_code, identifier_names, is_modelled_token, query_encode, Emulator,
};

/// Every value the modelled `GroupFilterName` enum declares — the domain of
/// `GroupFilter$Name`, and so every name `ListGroups` must answer.
///
/// Source: `smithy_query(query="shape",shape="GroupFilterName")` on this service's
/// model, which answers
/// `["resource-type", "configuration-type", "owner", "display-name", "criticality"]`.
const GROUP_FILTER_NAMES: [&str; 5] = [
    "resource-type",
    "configuration-type",
    "owner",
    "display-name",
    "criticality",
];

/// The service-configuration types the `configuration-type` filter documents as its
/// supported values.
///
/// Source: `smithy_query(query="doc",shape="ListGroupsInput",member="Filters")` on
/// this service's model, whose `configuration-type` entry lists exactly these.
const CONFIGURATION_TYPE_VALUES: [&str; 7] = [
    "AWS::ResourceGroups::ApplicationGroup",
    "AWS::AppRegistry::Application",
    "AWS::AppRegistry::ApplicationResourceGroup",
    "AWS::CloudFormation::Stack",
    "AWS::EC2::CapacityReservationPool",
    "AWS::EC2::HostManagement",
    "AWS::NetworkFirewall::RuleGroup",
];

/// One group the filter table selects over: its name and the whole `CreateGroup`
/// request that stores it. The rows differ in what they carry — a one-item
/// configuration, a two-item one, a tag query limited to one or several resource
/// types, a query limited to every supported type, a query limited to none, and a
/// group carrying neither — which is what the filtering rule generalises over.
struct GroupFixture {
    name: &'static str,
    request: fn() -> Value,
}

fn fixtures() -> Vec<GroupFixture> {
    vec![
        GroupFixture {
            name: "all-supported",
            request: || {
                json!({
                    "Name": "all-supported",
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": "{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"
                    }
                })
            },
        },
        GroupFixture {
            name: "app",
            request: || {
                json!({
                    "Name": "app",
                    "Owner": "sre@example.com",
                    "Configuration": [
                        { "Type": "AWS::ResourceGroups::ApplicationGroup" },
                        { "Type": "AWS::AppRegistry::Application" }
                    ]
                })
            },
        },
        GroupFixture {
            name: "firewall",
            request: || {
                json!({
                    "Name": "firewall",
                    "Criticality": 5,
                    "Configuration": [{
                        "Type": "AWS::NetworkFirewall::RuleGroup",
                        "Parameters": [{ "Name": "allowed-resource-types", "Values": ["AWS::EC2::Subnet"] }]
                    }]
                })
            },
        },
        GroupFixture {
            name: "hosts",
            request: || {
                json!({
                    "Name": "hosts",
                    "DisplayName": "Hosts",
                    "Owner": "sre@example.com",
                    "Configuration": [{ "Type": "AWS::EC2::HostManagement" }]
                })
            },
        },
        GroupFixture {
            name: "instances",
            request: || {
                json!({
                    "Name": "instances",
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": "{\"ResourceTypeFilters\":[\"AWS::EC2::Instance\",\"AWS::S3::Bucket\"],\"TagFilters\":[]}"
                    }
                })
            },
        },
        GroupFixture {
            name: "no-types",
            request: || {
                json!({
                    "Name": "no-types",
                    "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "{}" }
                })
            },
        },
        GroupFixture {
            name: "plain",
            request: || json!({ "Name": "plain" }),
        },
        GroupFixture {
            name: "stack-config",
            request: || {
                json!({
                    "Name": "stack-config",
                    "Configuration": [{ "Type": "AWS::CloudFormation::Stack" }]
                })
            },
        },
        GroupFixture {
            name: "stack-query",
            request: || {
                json!({
                    "Name": "stack-query",
                    "ResourceQuery": {
                        "Type": "CLOUDFORMATION_STACK_1_0",
                        "Query": "{\"StackIdentifier\":\"arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0\",\"ResourceTypeFilters\":[\"AWS::EC2::Instance\"]}"
                    }
                })
            },
        },
    ]
}

/// An emulator holding every fixture group.
async fn populated() -> Emulator {
    let emulator = Emulator::new();
    for fixture in fixtures() {
        emulator.ok("/groups", &(fixture.request)()).await;
    }
    emulator
}

/// The names a `ListGroups` call carrying `filters` reports, requiring the modelled
/// 200 success rather than any refusal.
async fn filtered(emulator: &Emulator, filters: &Value) -> Vec<String> {
    let page = emulator.ok("/groups-list", &json!({ "Filters": filters })).await;
    // The deprecated `Groups` member is reported from the same page, so the two
    // cannot disagree about what the filter selected.
    let deprecated: Vec<String> = page["Groups"]
        .as_array()
        .expect("Groups is a list")
        .iter()
        .map(|group| group["Name"].as_str().expect("a group name").to_string())
        .collect();
    let identifiers = identifier_names(&page);
    assert_eq!(
        identifiers, deprecated,
        "case {filters}: expected GroupIdentifiers and Groups to agree, got {page}",
    );
    identifiers
}

/// Every filter name the model declares is answered with a 200 listing — no filter
/// name is refused as unsupported.
#[tokio::test]
async fn every_declared_filter_name_is_answered_with_a_listing() {
    let emulator = populated().await;
    // A value in the domain of each name: a resource type, a configuration type, an
    // owner, a display name and a criticality rank.
    let values = [
        ("resource-type", "AWS::EC2::Instance"),
        ("configuration-type", "AWS::EC2::HostManagement"),
        ("owner", "sre@example.com"),
        ("display-name", "Hosts"),
        ("criticality", "5"),
    ];
    for name in GROUP_FILTER_NAMES {
        let value = values
            .iter()
            .find(|(declared, _)| *declared == name)
            .map(|(_, value)| *value)
            .unwrap_or_else(|| panic!("case {name:?}: the table must carry a value for every declared filter name"));
        let (status, body) = emulator
            .call(
                "/groups-list",
                &json!({ "Filters": [{ "Name": name, "Values": [value] }] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name}={value:?}: expected a 200 listing, got {status} {body}",
        );
        assert!(
            body["GroupIdentifiers"].is_array(),
            "case {name}={value:?}: expected GroupIdentifiers in the listing, got {body}",
        );
    }
}

/// A `configuration-type` filter selects the groups whose stored service
/// configuration declares one of its values, and nothing else.
#[tokio::test]
async fn a_configuration_type_filter_selects_the_groups_declaring_that_type() {
    let emulator = populated().await;
    // (filter values, the groups the filter must select, in name order)
    let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (vec!["AWS::EC2::HostManagement"], vec!["hosts"]),
        // An entry of the documented catalog the fixtures configure on one group.
        (vec!["AWS::NetworkFirewall::RuleGroup"], vec!["firewall"]),
        // Either item of a two-item configuration selects the group.
        (vec!["AWS::ResourceGroups::ApplicationGroup"], vec!["app"]),
        (vec!["AWS::AppRegistry::Application"], vec!["app"]),
        // Several values select the union, still in name order.
        (
            vec!["AWS::CloudFormation::Stack", "AWS::EC2::HostManagement"],
            vec!["hosts", "stack-config"],
        ),
        // A declared configuration type nothing is configured with selects nothing.
        (vec!["AWS::EC2::CapacityReservationPool"], vec![]),
        // A RESOURCE type is not a configuration type: the group whose query names
        // it declares no configuration at all.
        (vec!["AWS::EC2::Instance"], vec![]),
    ];
    for (values, expected) in cases {
        let answered = filtered(
            &emulator,
            &json!([{ "Name": "configuration-type", "Values": values }]),
        )
        .await;
        let want: Vec<String> = expected.iter().map(|n| (*n).to_string()).collect();
        assert_eq!(
            answered, want,
            "case configuration-type={values:?}: expected {want:?}, got {answered:?}",
        );
    }
}

/// A `resource-type` filter selects the groups whose stored resource query is
/// limited to one of its values, and nothing else.
#[tokio::test]
async fn a_resource_type_filter_selects_the_groups_whose_query_names_that_type() {
    let emulator = populated().await;
    // (filter values, the groups the filter must select, in name order)
    let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
        // Both query types carry the element, so both answer the filter.
        (vec!["AWS::EC2::Instance"], vec!["instances", "stack-query"]),
        (vec!["AWS::S3::Bucket"], vec!["instances"]),
        (vec!["AWS::AllSupported"], vec!["all-supported"]),
        (
            vec!["AWS::AllSupported", "AWS::S3::Bucket"],
            vec!["all-supported", "instances"],
        ),
        // A type no stored query names selects nothing, and neither does a
        // CONFIGURATION type.
        (vec!["AWS::Lambda::Function"], vec![]),
        (vec!["AWS::EC2::HostManagement"], vec![]),
    ];
    for (values, expected) in cases {
        let answered = filtered(
            &emulator,
            &json!([{ "Name": "resource-type", "Values": values }]),
        )
        .await;
        let want: Vec<String> = expected.iter().map(|n| (*n).to_string()).collect();
        assert_eq!(
            answered, want,
            "case resource-type={values:?}: expected {want:?}, got {answered:?}",
        );
    }
}

/// Several filters narrow together: a group is reported only when it satisfies
/// every one of them.
#[tokio::test]
async fn filters_narrow_together() {
    let emulator = populated().await;
    // (the whole Filters list, the groups it must select, in name order)
    let cases: Vec<(Value, Vec<&str>)> = vec![
        (
            json!([
                { "Name": "configuration-type", "Values": ["AWS::EC2::HostManagement"] },
                { "Name": "owner", "Values": ["sre@example.com"] }
            ]),
            vec!["hosts"],
        ),
        (
            json!([
                { "Name": "configuration-type", "Values": ["AWS::EC2::HostManagement"] },
                { "Name": "owner", "Values": ["dba@example.com"] }
            ]),
            vec![],
        ),
        (
            json!([
                { "Name": "configuration-type", "Values": ["AWS::NetworkFirewall::RuleGroup"] },
                { "Name": "criticality", "Values": ["5"] }
            ]),
            vec!["firewall"],
        ),
        (
            json!([
                { "Name": "resource-type", "Values": ["AWS::EC2::Instance"] },
                { "Name": "configuration-type", "Values": ["AWS::EC2::HostManagement"] }
            ]),
            vec![],
        ),
        // An empty filter list narrows nothing.
        (json!([]), fixtures().iter().map(|f| f.name).collect()),
    ];
    for (filters, expected) in cases {
        let answered = filtered(&emulator, &filters).await;
        let want: Vec<String> = expected.iter().map(|n| (*n).to_string()).collect();
        assert_eq!(
            answered, want,
            "case {filters}: expected {want:?}, got {answered:?}",
        );
    }
}

/// A filtered listing pages through the groups the filter selected exactly once,
/// in name order, and is a read: the groups are unchanged afterwards.
#[tokio::test]
async fn a_filtered_listing_pages_in_name_order_and_changes_nothing() {
    let emulator = populated().await;
    let filters = json!([{
        "Name": "configuration-type",
        "Values": ["AWS::CloudFormation::Stack", "AWS::EC2::HostManagement"]
    }]);
    let expected = vec!["hosts".to_string(), "stack-config".to_string()];

    for page_size in [1usize, 2, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            // `MaxResults` and `NextToken` are bound to the query string.
            let path = match &token {
                Some(t) => format!(
                    "/groups-list?maxResults={page_size}&nextToken={}",
                    query_encode(t)
                ),
                None => format!("/groups-list?maxResults={page_size}"),
            };
            let page = emulator.ok(&path, &json!({ "Filters": filters })).await;
            let names = identifier_names(&page);
            assert!(
                names.len() <= page_size,
                "case page_size={page_size}: a page carried {} items, got {page}",
                names.len(),
            );
            seen.extend(names);
            token = page["NextToken"].as_str().map(str::to_string);
            if let Some(t) = &token {
                assert!(
                    is_modelled_token(t),
                    "case page_size={page_size}: NextToken {t:?} is not the modelled form",
                );
            }
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case page_size={page_size}: paging did not terminate, saw {seen:?}",
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case page_size={page_size}: expected the selected groups once each in \
             name order, got {seen:?}",
        );
    }

    // Filtering reports; it does not change what a later caller sees.
    let after = filtered(&emulator, &json!([])).await;
    let every_name: Vec<String> = fixtures().iter().map(|f| f.name.to_string()).collect();
    assert_eq!(
        after, every_name,
        "expected every group to survive the filtered listings, got {after:?}",
    );
}

/// A filter name the model does not declare is `BadRequestException` (400): the
/// request names an attribute no group has, so it cannot be answered with a
/// listing.
#[tokio::test]
async fn an_undeclared_filter_name_is_bad_request() {
    let emulator = populated().await;
    for name in [
        "Resource-Type",
        "CONFIGURATION-TYPE",
        "group-name",
        "tag",
        "name",
    ] {
        assert!(
            !GROUP_FILTER_NAMES.contains(&name),
            "case {name:?}: this case must name a filter the model does not declare",
        );
        let (status, body) = emulator
            .call(
                "/groups-list",
                &json!({ "Filters": [{ "Name": name, "Values": ["AWS::EC2::Instance"] }] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {name:?}: expected 400 BadRequestException, got {status} {body}",
        );
    }
}

/// The longest `GroupConfigurationType` a group can be created with.
///
/// Source: `smithy_query(query="shape",shape="GroupConfigurationType")`, whose
/// `length` is `0..40` — a modelled constraint the boundary refuses before any
/// handler runs, so a longer catalog entry can never reach the store.
const MAX_CONFIGURATION_TYPE_LENGTH: usize = 40;

/// The documented `configuration-type` catalog is answerable: creating a group with
/// each supported type and filtering on it reports that group, for every entry of
/// the catalog rather than the one the fixtures use. An entry past the modelled
/// `GroupConfigurationType` length is refused at the boundary instead, so no group
/// can carry it.
#[tokio::test]
async fn every_documented_configuration_type_is_filterable() {
    let emulator = Emulator::new();
    for (index, configuration_type) in CONFIGURATION_TYPE_VALUES.iter().enumerate() {
        let name = format!("configured-{index}");
        let request = json!({ "Name": name, "Configuration": [{ "Type": configuration_type }] });
        if configuration_type.len() > MAX_CONFIGURATION_TYPE_LENGTH {
            let (status, body) = emulator.call("/groups", &request).await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "BadRequestException"),
                "case {configuration_type}: expected 400 BadRequestException for a type \
                 past the modelled length, got {status} {body}",
            );
            continue;
        }
        emulator.ok("/groups", &request).await;
        let answered = filtered(
            &emulator,
            &json!([{ "Name": "configuration-type", "Values": [configuration_type] }]),
        )
        .await;
        assert_eq!(
            answered,
            vec![name.clone()],
            "case {configuration_type}: expected [{name}], got {answered:?}",
        );
    }
}

/// The most `Filters` entries one call is served with: a request at the bound is
/// answered, and one past it is `BadRequestException` (400).
///
/// Not an AWS quota — it is this emulator's bound on the work one request asks
/// for, since every entry is applied to every group of the region.
const MAX_FILTERS: usize = 10;

#[tokio::test]
async fn a_request_past_the_filter_bound_is_bad_request() {
    let emulator = populated().await;
    let entry = json!({ "Name": "configuration-type", "Values": ["AWS::EC2::HostManagement"] });
    for count in [1usize, MAX_FILTERS] {
        let answered = filtered(
            &emulator,
            &Value::Array(vec![entry.clone(); count]),
        )
        .await;
        assert_eq!(
            answered,
            vec!["hosts".to_string()],
            "case {count} filters: expected [\"hosts\"], got {answered:?}",
        );
    }
    let (status, body) = emulator
        .call(
            "/groups-list",
            &json!({ "Filters": Value::Array(vec![entry; MAX_FILTERS + 1]) }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {} filters: expected 400 BadRequestException, got {status} {body}",
        MAX_FILTERS + 1,
    );
}
