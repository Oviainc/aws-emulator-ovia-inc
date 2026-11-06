// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateNetworkInsightsAccessScope` and
//! `GetNetworkInsightsAccessScopeContent`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope,
//! so every call here carries one.
//!
//! The rule under test is that a create PERSISTS the scope it mints and the content it was
//! given, and that every later reader reports that store: the scope through
//! `DescribeNetworkInsightsAccessScopes`, its paths through
//! `GetNetworkInsightsAccessScopeContent`, and both exactly as the create answered them.
//! Every table row is a distinct content along what that generalises over — which of the two
//! path lists is named, how many paths each carries, which statement a path names (a
//! packet-header statement, a resource statement, both), which members of that statement and
//! how many values each holds, and whether the path goes through resources — so no single
//! recorded call is the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, IDEMPOTENT_PARAMETER_MISMATCH,
    IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE, INVALID_PARAMETER_VALUE, MAX_ACCESS_SCOPE_PATHS,
    MAX_CLIENT_TOKEN_CHARS, NETWORK_INSIGHTS_ACCESS_SCOPE_ID_PREFIX,
    RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, indexed, owned};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for, and the Region the created scopes' ARNs name.
const REGION: &str = "us-west-2";

// ---------------------------------------------------------------------------
// The content a row names, in both directions
// ---------------------------------------------------------------------------

/// "The packet header statement" of a path endpoint
/// (`PacketHeaderStatement`): the address, port, prefix-list and protocol sets the model
/// gives it, each a list of strings a caller chooses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PacketHeader {
    source_addresses: Vec<String>,
    destination_addresses: Vec<String>,
    source_ports: Vec<String>,
    destination_ports: Vec<String>,
    source_prefix_lists: Vec<String>,
    destination_prefix_lists: Vec<String>,
    protocols: Vec<String>,
}

/// "The resource statement" of a path endpoint or a through-resource
/// (`ResourceStatement`): the resources and the resource types it names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Resources {
    resources: Vec<String>,
    resource_types: Vec<String>,
}

/// One endpoint of a path (`PathStatement`): a packet-header statement, a resource
/// statement, or both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Statement {
    packet_header: Option<PacketHeader>,
    resource: Option<Resources>,
}

/// One path of a scope's content (`AccessScopePath`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Path {
    source: Option<Statement>,
    destination: Option<Statement>,
    through_resources: Vec<Resources>,
}

/// One scope's content (`NetworkInsightsAccessScopeContent`): the scope it belongs to and
/// its two path lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Content {
    scope_id: String,
    match_paths: Vec<Path>,
    exclude_paths: Vec<Path>,
}

/// A statement naming only a resource statement over `resource_types`.
fn resource_types(types: &[&str]) -> Statement {
    Statement {
        resource: Some(Resources {
            resource_types: owned(types),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// The request side: one content as ec2Query form fields
// ---------------------------------------------------------------------------

/// `<prefix>.<member>.N` form fields, as ec2Query flattens a list of strings nested inside
/// a structure.
fn values(prefix: &str, member: &str, list: &[String]) -> Vec<Param> {
    let borrowed: Vec<&str> = list.iter().map(String::as_str).collect();
    indexed(&format!("{prefix}.{member}"), &borrowed)
}

/// The form fields of one `ResourceStatementRequest` at `prefix`. The member names are the
/// ones the model binds for the request shape (`ResourceStatementRequest$Resources` is sent
/// as `Resource.N`, `$ResourceTypes` as `ResourceType.N`).
fn resource_params(prefix: &str, resources: &Resources) -> Vec<Param> {
    let mut params = values(prefix, "Resource", &resources.resources);
    params.extend(values(prefix, "ResourceType", &resources.resource_types));
    params
}

/// The form fields of one `PathStatementRequest` at `prefix`.
fn statement_params(prefix: &str, statement: &Statement) -> Vec<Param> {
    let mut params = Vec::new();
    if let Some(header) = &statement.packet_header {
        let at = format!("{prefix}.PacketHeaderStatement");
        params.extend(values(&at, "SourceAddress", &header.source_addresses));
        params.extend(values(
            &at,
            "DestinationAddress",
            &header.destination_addresses,
        ));
        params.extend(values(&at, "SourcePort", &header.source_ports));
        params.extend(values(&at, "DestinationPort", &header.destination_ports));
        params.extend(values(&at, "SourcePrefixList", &header.source_prefix_lists));
        params.extend(values(
            &at,
            "DestinationPrefixList",
            &header.destination_prefix_lists,
        ));
        params.extend(values(&at, "Protocol", &header.protocols));
    }
    if let Some(resource) = &statement.resource {
        params.extend(resource_params(
            &format!("{prefix}.ResourceStatement"),
            resource,
        ));
    }
    params
}

/// The form fields of one path of the `list_member` list (`MatchPath` or `ExcludePath`) at
/// position `index`, counting from 1 as ec2Query does.
fn path_params(list_member: &str, index: usize, path: &Path) -> Vec<Param> {
    let at = format!("{list_member}.{}", index + 1);
    let mut params = Vec::new();
    if let Some(source) = &path.source {
        params.extend(statement_params(&format!("{at}.Source"), source));
    }
    if let Some(destination) = &path.destination {
        params.extend(statement_params(&format!("{at}.Destination"), destination));
    }
    for (through_index, through) in path.through_resources.iter().enumerate() {
        params.extend(resource_params(
            &format!("{at}.ThroughResource.{}.ResourceStatement", through_index + 1),
            through,
        ));
    }
    params
}

/// The form fields of a whole path list.
fn path_list_params(list_member: &str, paths: &[Path]) -> Vec<Param> {
    paths
        .iter()
        .enumerate()
        .flat_map(|(index, path)| path_params(list_member, index, path))
        .collect()
}

/// The form fields for one `TagSpecification` naming the scope being created.
fn tag_specification(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// One create call: the client token, the two path lists and the tags.
fn create_params(
    client_token: &str,
    match_paths: &[Path],
    exclude_paths: &[Path],
    tags: &[(&str, &str)],
) -> Vec<Param> {
    let mut params = vec![param("ClientToken", client_token)];
    params.extend(path_list_params("MatchPath", match_paths));
    params.extend(path_list_params("ExcludePath", exclude_paths));
    params.extend(tag_specification(tags));
    params
}

// ---------------------------------------------------------------------------
// The response side: one content as the service reported it
// ---------------------------------------------------------------------------

/// The `<item>` texts of the `set` nested in `node` — how ec2Query writes a list of
/// strings.
fn reported_values(node: roxmltree::Node<'_, '_>, set: &str) -> Vec<String> {
    set_items(node, set)
        .into_iter()
        .map(|item| item.text().unwrap_or_default().to_string())
        .collect()
}

/// One reported `<packetHeaderStatement>`.
fn reported_packet_header(node: roxmltree::Node<'_, '_>) -> PacketHeader {
    PacketHeader {
        source_addresses: reported_values(node, "sourceAddressSet"),
        destination_addresses: reported_values(node, "destinationAddressSet"),
        source_ports: reported_values(node, "sourcePortSet"),
        destination_ports: reported_values(node, "destinationPortSet"),
        source_prefix_lists: reported_values(node, "sourcePrefixListSet"),
        destination_prefix_lists: reported_values(node, "destinationPrefixListSet"),
        protocols: reported_values(node, "protocolSet"),
    }
}

/// One reported `<resourceStatement>`.
fn reported_resources(node: roxmltree::Node<'_, '_>) -> Resources {
    Resources {
        resources: reported_values(node, "resourceSet"),
        resource_types: reported_values(node, "resourceTypeSet"),
    }
}

/// One reported `<source>` or `<destination>`.
fn reported_statement(node: roxmltree::Node<'_, '_>) -> Statement {
    Statement {
        packet_header: child(node, "packetHeaderStatement").map(reported_packet_header),
        resource: child(node, "resourceStatement").map(reported_resources),
    }
}

/// One reported path.
fn reported_path(node: roxmltree::Node<'_, '_>) -> Path {
    Path {
        source: child(node, "source").map(reported_statement),
        destination: child(node, "destination").map(reported_statement),
        through_resources: set_items(node, "throughResourceSet")
            .into_iter()
            .filter_map(|item| child(item, "resourceStatement"))
            .map(reported_resources)
            .collect(),
    }
}

/// The one `<networkInsightsAccessScopeContent>` a response carries, or a panic naming the
/// case.
fn reported_content(case: &str, body: &str) -> Content {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| {
            node.is_element() && node.tag_name().name() == "networkInsightsAccessScopeContent"
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no <networkInsightsAccessScopeContent> in {body}")
        });
    Content {
        scope_id: child_text(node, "networkInsightsAccessScopeId").unwrap_or_default(),
        match_paths: set_items(node, "matchPathSet")
            .into_iter()
            .map(reported_path)
            .collect(),
        exclude_paths: set_items(node, "excludePathSet")
            .into_iter()
            .map(reported_path)
            .collect(),
    }
}

/// The one `<networkInsightsAccessScope>` a response carries, as the members a reader
/// asserts: the id, the ARN, the two dates and the tags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Scope {
    scope_id: String,
    arn: String,
    created_date: String,
    updated_date: String,
    tags: Vec<(String, String)>,
}

/// The one scope a response carries, or a panic naming the case.
fn reported_scope(case: &str, body: &str) -> Scope {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child(*node, "networkInsightsAccessScopeArn").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no Network Access Scope in {body}"));
    Scope {
        scope_id: child_text(node, "networkInsightsAccessScopeId").unwrap_or_default(),
        arn: child_text(node, "networkInsightsAccessScopeArn").unwrap_or_default(),
        created_date: child_text(node, "createdDate").unwrap_or_default(),
        updated_date: child_text(node, "updatedDate").unwrap_or_default(),
        tags: set_items(node, "tagSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

/// One `CreateNetworkInsightsAccessScope` call, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateNetworkInsightsAccessScope", params).await
}

/// One `GetNetworkInsightsAccessScopeContent` call, as its HTTP status and XML body.
async fn get_content(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "GetNetworkInsightsAccessScopeContent",
        params,
    )
    .await
}

/// The body of a call that must have succeeded.
fn ok(case: &str, status: u16, body: String) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// How many scopes the store holds.
fn stored_scopes(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .network_insights_access_scopes
        .len()
}

// ---------------------------------------------------------------------------
// The table: one row per content shape
// ---------------------------------------------------------------------------

/// One create row: the case name, the `MatchPaths` and `ExcludePaths` it names, and the tags
/// it applies.
struct CreateCase {
    case: &'static str,
    match_paths: Vec<Path>,
    exclude_paths: Vec<Path>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The contents the round-trip table is written against. The rows differ along everything
/// the content generalises over: which list is named, how many paths it carries, which
/// statement a path names and with which members, how many values a statement holds, and
/// whether the path goes through resources.
///
/// The resource-type strings are the CloudFormation-style type names
/// `ResourceStatementRequest$ResourceTypes` documents ("The resource types"); nothing in
/// this operation interprets them, so they travel as the caller's own text.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "neither list named",
            match_paths: Vec::new(),
            exclude_paths: Vec::new(),
            tags: Vec::new(),
        },
        CreateCase {
            case: "one match path, resource statements at both ends",
            match_paths: vec![Path {
                source: Some(resource_types(&["AWS::EC2::InternetGateway"])),
                destination: Some(resource_types(&["AWS::EC2::NetworkInterface"])),
                through_resources: Vec::new(),
            }],
            exclude_paths: Vec::new(),
            tags: vec![("Name", "ingress from the internet")],
        },
        CreateCase {
            case: "one match path, a packet header statement at the destination",
            match_paths: vec![Path {
                source: Some(resource_types(&["AWS::EC2::VPC"])),
                destination: Some(Statement {
                    packet_header: Some(PacketHeader {
                        destination_ports: owned(&["443", "8443"]),
                        protocols: owned(&["tcp"]),
                        ..Default::default()
                    }),
                    resource: None,
                }),
                through_resources: Vec::new(),
            }],
            exclude_paths: Vec::new(),
            tags: Vec::new(),
        },
        CreateCase {
            case: "both statements on one endpoint, every packet header member set",
            match_paths: vec![Path {
                source: Some(Statement {
                    packet_header: Some(PacketHeader {
                        source_addresses: owned(&["10.0.0.0/16", "192.168.0.0/24"]),
                        destination_addresses: owned(&["172.16.0.0/12"]),
                        source_ports: owned(&["1024-65535"]),
                        destination_ports: owned(&["22"]),
                        source_prefix_lists: owned(&["pl-0123456789abcdef0"]),
                        destination_prefix_lists: owned(&["pl-0fedcba9876543210"]),
                        protocols: owned(&["tcp", "udp"]),
                    }),
                    resource: Some(Resources {
                        resources: owned(&["vpc-0123456789abcdef0"]),
                        resource_types: owned(&["AWS::EC2::VPC"]),
                    }),
                }),
                destination: None,
                through_resources: Vec::new(),
            }],
            exclude_paths: Vec::new(),
            tags: vec![("Tier", "prod"), ("Owner", "network team")],
        },
        CreateCase {
            case: "a path through two resources",
            match_paths: vec![Path {
                source: Some(resource_types(&["AWS::EC2::Subnet"])),
                destination: Some(resource_types(&["AWS::EC2::Subnet"])),
                through_resources: vec![
                    Resources {
                        resource_types: owned(&["AWS::EC2::TransitGateway"]),
                        ..Default::default()
                    },
                    Resources {
                        resources: owned(&["nat-0123456789abcdef0"]),
                        resource_types: owned(&["AWS::EC2::NatGateway"]),
                    },
                ],
            }],
            exclude_paths: Vec::new(),
            tags: Vec::new(),
        },
        CreateCase {
            case: "exclude paths only",
            match_paths: Vec::new(),
            exclude_paths: vec![Path {
                source: Some(resource_types(&["AWS::EC2::VPC"])),
                destination: Some(resource_types(&["AWS::EC2::VPC"])),
                through_resources: Vec::new(),
            }],
            tags: Vec::new(),
        },
        CreateCase {
            case: "three match paths and two exclude paths",
            match_paths: vec![
                Path {
                    source: Some(resource_types(&["AWS::EC2::InternetGateway"])),
                    ..Default::default()
                },
                Path {
                    destination: Some(resource_types(&["AWS::EC2::NetworkInterface"])),
                    ..Default::default()
                },
                Path {
                    source: Some(resource_types(&["AWS::EC2::VPC", "AWS::EC2::Subnet"])),
                    destination: Some(Statement {
                        packet_header: Some(PacketHeader {
                            protocols: owned(&["udp"]),
                            ..Default::default()
                        }),
                        resource: None,
                    }),
                    through_resources: Vec::new(),
                },
            ],
            exclude_paths: vec![
                Path {
                    source: Some(resource_types(&["AWS::EC2::VPCPeeringConnection"])),
                    ..Default::default()
                },
                Path {
                    destination: Some(Statement {
                        packet_header: Some(PacketHeader {
                            destination_ports: owned(&["3389"]),
                            ..Default::default()
                        }),
                        resource: None,
                    }),
                    ..Default::default()
                },
            ],
            tags: Vec::new(),
        },
    ]
}

/// A create stores the scope it mints and the content it was given: the id and ARN it
/// answers are the service's own, the content it answers is the paths of the request, the
/// content `GetNetworkInsightsAccessScopeContent` reports afterwards is that same content,
/// and the scope `DescribeNetworkInsightsAccessScopes` reports is that same scope.
#[tokio::test]
async fn a_created_scope_and_its_content_round_trip_through_their_readers() {
    let (router, engine) = super::ec2_query::emulator();

    for (ordinal, row) in create_cases().into_iter().enumerate() {
        let case = row.case;
        let client_token = format!("token-{ordinal}");
        let (status, body) = create(
            &router,
            &create_params(
                &client_token,
                &row.match_paths,
                &row.exclude_paths,
                &row.tags,
            ),
        )
        .await;
        let body = ok(case, status, body);

        // The id is the service's: the `nis-` prefix and the 17 hex digits every long EC2
        // resource id carries, never anything the request named.
        let scope = reported_scope(case, &body);
        assert!(
            is_long_id(NETWORK_INSIGHTS_ACCESS_SCOPE_ID_PREFIX, &scope.scope_id),
            "case {case:?}: expected a {NETWORK_INSIGHTS_ACCESS_SCOPE_ID_PREFIX}-<17 hex> \
             id, got {:?}",
            scope.scope_id
        );
        let expected_arn = ::engine::emulator_arn(
            "ec2",
            REGION,
            &format!(
                "{RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE}/{}",
                scope.scope_id
            ),
        );
        assert_eq!(
            scope.arn, expected_arn,
            "case {case:?}: expected the ARN {expected_arn}, got {:?}",
            scope.arn
        );
        // "The creation date" and "The last updated date": a scope nothing has modified
        // reports the two equal, each in the modelled timestamp's wire form.
        assert!(
            super::ec2_query::as_timestamp(&scope.created_date).is_some(),
            "case {case:?}: expected createdDate in the modelled timestamp form, got {:?}",
            scope.created_date
        );
        assert_eq!(
            scope.updated_date, scope.created_date,
            "case {case:?}: an unmodified scope reports updatedDate equal to createdDate \
             {:?}, got {:?}",
            scope.created_date, scope.updated_date
        );
        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            scope.tags, expected_tags,
            "case {case:?}: expected the tags {expected_tags:?}, got {:?}",
            scope.tags
        );

        // The content the create answered is the request's paths, under the scope it minted.
        let expected_content = Content {
            scope_id: scope.scope_id.clone(),
            match_paths: row.match_paths.clone(),
            exclude_paths: row.exclude_paths.clone(),
        };
        let created_content = reported_content(case, &body);
        assert_eq!(
            created_content, expected_content,
            "case {case:?}: expected the create to answer {expected_content:?}, got \
             {created_content:?}"
        );

        // The get reports the stored content, which is the one the create answered.
        let (status, body) = get_content(
            &router,
            &[param("NetworkInsightsAccessScopeId", &scope.scope_id)],
        )
        .await;
        let body = ok(case, status, body);
        let fetched = reported_content(case, &body);
        assert_eq!(
            fetched, expected_content,
            "case {case:?}: expected the get to report {expected_content:?}, got {fetched:?}"
        );

        // A second get reports the same content: reading one changes nothing.
        let (status, body) = get_content(
            &router,
            &[param("NetworkInsightsAccessScopeId", &scope.scope_id)],
        )
        .await;
        let body = ok(case, status, body);
        let again = reported_content(case, &body);
        assert_eq!(
            again, fetched,
            "case {case:?}: a second get must report the same content, got {again:?}"
        );

        // And the describe reports the scope the create answered, member for member.
        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "DescribeNetworkInsightsAccessScopes",
            &[param("NetworkInsightsAccessScopeId.1", &scope.scope_id)],
        )
        .await;
        let body = ok(case, status, body);
        let described = reported_scope(case, &body);
        assert_eq!(
            described, scope,
            "case {case:?}: expected the describe to report {scope:?}, got {described:?}"
        );
    }

    let held = stored_scopes(&engine);
    let expected = create_cases().len();
    assert_eq!(
        held, expected,
        "every create must store exactly one scope: expected {expected} stored, got {held}"
    );
}

/// Each create mints a NEW scope id, and the store holds every one of them: two creates with
/// the same content under different tokens are two scopes, not one.
#[tokio::test]
async fn two_creates_of_the_same_content_are_two_scopes() {
    let (router, engine) = super::ec2_query::emulator();
    let paths = vec![Path {
        source: Some(resource_types(&["AWS::EC2::InternetGateway"])),
        ..Default::default()
    }];

    let (status, body) = create(&router, &create_params("token-a", &paths, &[], &[])).await;
    let first = reported_scope("first", &ok("first", status, body));
    let (status, body) = create(&router, &create_params("token-b", &paths, &[], &[])).await;
    let second = reported_scope("second", &ok("second", status, body));
    assert_ne!(
        first.scope_id, second.scope_id,
        "two creates under different tokens must mint two scopes, both got {:?}",
        first.scope_id
    );
    let held = stored_scopes(&engine);
    assert_eq!(
        held, 2,
        "two tokens must leave two scopes stored, got {held}"
    );
}

/// "Unique, case-sensitive identifier that you provide to ensure the idempotency of the
/// request": a replay of the same request answers the scope that token already created and
/// creates no second one, while the same token carrying a DIFFERENT request is refused.
#[tokio::test]
async fn a_replayed_client_token_answers_the_scope_it_created() {
    let (router, engine) = super::ec2_query::emulator();
    let paths = vec![Path {
        source: Some(resource_types(&["AWS::EC2::VPC"])),
        destination: Some(resource_types(&["AWS::EC2::NetworkInterface"])),
        through_resources: Vec::new(),
    }];
    let tags = vec![("Name", "replayed")];
    let params = create_params("one-token", &paths, &[], &tags);

    let (status, body) = create(&router, &params).await;
    let body = ok("first call", status, body);
    let created = reported_scope("first call", &body);
    let created_content = reported_content("first call", &body);

    let (status, body) = create(&router, &params).await;
    let body = ok("replay", status, body);
    assert_eq!(
        reported_scope("replay", &body),
        created,
        "a replay must answer the scope the token created"
    );
    assert_eq!(
        reported_content("replay", &body),
        created_content,
        "a replay must answer the content the token created"
    );
    let held = stored_scopes(&engine);
    assert_eq!(held, 1, "a replay must create no second scope, got {held}");

    // One token, a different request: refused rather than answered with a scope the caller
    // did not ask for. Each row differs from the first call in ONE thing.
    let different: Vec<(&str, Vec<Param>)> = vec![
        (
            "different match paths",
            create_params(
                "one-token",
                &[Path {
                    source: Some(resource_types(&["AWS::EC2::Subnet"])),
                    ..Default::default()
                }],
                &[],
                &tags,
            ),
        ),
        (
            "an exclude path the first call did not name",
            create_params(
                "one-token",
                &paths,
                &[Path {
                    source: Some(resource_types(&["AWS::EC2::VPC"])),
                    ..Default::default()
                }],
                &tags,
            ),
        ),
        (
            "no paths at all",
            create_params("one-token", &[], &[], &tags),
        ),
        (
            "different tags",
            create_params("one-token", &paths, &[], &[("Name", "something else")]),
        ),
    ];
    for (case, params) in different {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected {IDEMPOTENT_PARAMETER_MISMATCH}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
            "case {case:?}: expected the message {IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE:?}, \
             got {message:?}"
        );
    }
    let held = stored_scopes(&engine);
    assert_eq!(
        held, 1,
        "a refused replay must store nothing, got {held} scopes"
    );
}

/// `MatchPaths` and `ExcludePaths` are caller-sized lists, so each is accepted up to the
/// bound this emulator holds a stored content to and refused one past it — with nothing
/// stored for the refused call.
#[tokio::test]
async fn a_path_list_past_the_bound_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let path = Path {
        source: Some(resource_types(&["AWS::EC2::VPC"])),
        ..Default::default()
    };
    let at_bound = vec![path.clone(); MAX_ACCESS_SCOPE_PATHS];
    let past_bound = vec![path; MAX_ACCESS_SCOPE_PATHS + 1];

    for (ordinal, (case, member, paths)) in [
        ("matchPaths at the bound", "MatchPath", &at_bound),
        ("excludePaths at the bound", "ExcludePath", &at_bound),
    ]
    .into_iter()
    .enumerate()
    {
        let mut params = vec![param("ClientToken", &format!("bound-{ordinal}"))];
        params.extend(path_list_params(member, paths));
        let (status, body) = create(&router, &params).await;
        let body = ok(case, status, body);
        let reported = reported_content(case, &body);
        let reported_count = if member == "MatchPath" {
            reported.match_paths.len()
        } else {
            reported.exclude_paths.len()
        };
        assert_eq!(
            reported_count, MAX_ACCESS_SCOPE_PATHS,
            "case {case:?}: expected {MAX_ACCESS_SCOPE_PATHS} paths reported, got \
             {reported_count}"
        );
    }

    for (case, member, paths) in [
        ("matchPaths past the bound", "MatchPath", &past_bound),
        ("excludePaths past the bound", "ExcludePath", &past_bound),
    ] {
        let mut params = vec![param("ClientToken", "past-the-bound")];
        params.extend(path_list_params(member, paths));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {code:?} in {body}"
        );
    }

    let held = stored_scopes(&engine);
    assert_eq!(
        held, 2,
        "only the two calls at the bound may have stored a scope, got {held}"
    );
}

/// A client token longer than this crate stores is refused, and the one at the bound is
/// accepted: the token is retained for the life of the store, so its length is bounded.
#[tokio::test]
async fn a_client_token_past_the_bound_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    for (case, length, expected_status) in [
        ("at the bound", MAX_CLIENT_TOKEN_CHARS, 200),
        ("one past the bound", MAX_CLIENT_TOKEN_CHARS + 1, 400),
    ] {
        let token = "t".repeat(length);
        let (status, body) = create(&router, &create_params(&token, &[], &[], &[])).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            let code = error_code(&body);
            assert_eq!(
                code, INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {code:?} in {body}"
            );
        }
    }

    let held = stored_scopes(&engine);
    assert_eq!(
        held, 1,
        "only the accepted token may have stored a scope, got {held}"
    );
}

/// "Checks whether you have the required permissions for the action, without actually making
/// the request": a dry run of a request that would have succeeded reports
/// `DryRunOperation` and stores nothing, on either operation.
#[tokio::test]
async fn a_dry_run_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let paths = vec![Path {
        source: Some(resource_types(&["AWS::EC2::VPC"])),
        ..Default::default()
    }];

    let mut params = create_params("dry-run-token", &paths, &[], &[]);
    params.push(param("DryRun", "true"));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 412,
        "a dry run must be refused, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, DRY_RUN_OPERATION,
        "a dry run must answer {DRY_RUN_OPERATION}, got {code:?} in {body}"
    );
    let held = stored_scopes(&engine);
    assert_eq!(held, 0, "a dry run must store nothing, got {held} scopes");

    // The same token now creates the scope: the dry run recorded nothing, not even the
    // token.
    let (status, body) = create(
        &router,
        &create_params("dry-run-token", &paths, &[], &[]),
    )
    .await;
    let body = ok("after the dry run", status, body);
    let scope = reported_scope("after the dry run", &body);

    // A dry run of the get is refused the same way, having reported no content.
    let (status, body) = get_content(
        &router,
        &[
            param("NetworkInsightsAccessScopeId", &scope.scope_id),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 412,
        "a dry-run get must be refused, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, DRY_RUN_OPERATION,
        "a dry-run get must answer {DRY_RUN_OPERATION}, got {code:?} in {body}"
    );
}

/// An id the get cannot resolve to a scope of the account is refused rather than answered
/// with an empty content — a value of a Network Access Scope id's own form that names no
/// scope of the account and a value that is not such an id at all are refused the one way,
/// with this resource's own `Invalid<Resource>Id.NotFound`. A dry run of either is refused
/// the same way, because a dry run of a request that would have been refused is refused with
/// that reason.
#[tokio::test]
async fn the_get_refuses_an_id_naming_no_scope() {
    let (router, _engine) = super::ec2_query::emulator();
    // One stored scope, so a refusal is about the id named rather than about an empty store.
    let (status, body) = create(&router, &create_params("seed-token", &[], &[], &[])).await;
    let body = ok("seed", status, body);
    let stored = reported_scope("seed", &body).scope_id;

    // Every id this operation cannot resolve is refused the one way, so the rows differ
    // along the ways a value fails to resolve: a value of this id's own form that the
    // account holds no scope under (in both the 17- and the 8-hexadecimal-digit form EC2
    // mints), another resource's prefix, the analysis prefix that merely starts the same
    // way, too few digits, a digit that is not hexadecimal, and text that is not an id.
    let refused = [
        ("a well-formed long id nothing holds", "nis-0123456789abcdef0"),
        ("a well-formed short id nothing holds", "nis-01234567"),
        ("another resource's id", "vpc-0123456789abcdef0"),
        ("the analysis id form", "nisa-0123456789abcdef0"),
        ("too few digits", "nis-0123"),
        ("a digit that is not hexadecimal", "nis-0123456789abcdefg"),
        ("not an id at all", "scope"),
    ];
    for (case, scope_id) in refused {
        let expected_code = ec2::state::INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND;
        let expected_message = format!("{scope_id} does not exist");
        for dry_run in [false, true] {
            let mut params = vec![param("NetworkInsightsAccessScopeId", scope_id)];
            if dry_run {
                params.push(param("DryRun", "true"));
            }
            let (status, body) = get_content(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?} (dry run {dry_run}): expected HTTP 400, got {status} with \
                 body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} (dry run {dry_run}): expected {expected_code}, got {code:?} \
                 in {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected_message,
                "case {case:?} (dry run {dry_run}): expected the message \
                 {expected_message:?}, got {message:?}"
            );
        }
    }

    // The scope the account DOES hold is still readable, so the refusals above narrowed
    // nothing.
    let (status, body) = get_content(&router, &[param("NetworkInsightsAccessScopeId", &stored)]).await;
    let body = ok("the stored scope", status, body);
    let reported = reported_content("the stored scope", &body);
    assert_eq!(
        reported.scope_id, stored,
        "the stored scope must still report its own content, got {reported:?}"
    );
}
