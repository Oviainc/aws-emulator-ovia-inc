// SPDX-License-Identifier: CC0-1.0

//! `GetConnectionGroupByRoutingEndpoint`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one the operation states: it "gets information
//! about a connection group by using the endpoint that you specify", so the `RoutingEndpoint`
//! argument is matched against the endpoint STORED on each connection group — the one this
//! service assigned at creation — and the answer is the same stored record, and the same
//! version token, that a read by identifier reports. A routing endpoint no stored connection
//! group carries names no entity, and the read reports: it changes nothing a later caller
//! sees.
//!
//! The rows differ along everything the rule generalises over: which optional members the
//! create named at all, the two values each boolean can take, how many tags the group
//! carries, how many groups the account holds, whether the group was created by a CALL or
//! minted by the service itself for a distribution tenant that named none, the case the
//! endpoint is asked for in, and — for the refusals — whether the value is another
//! identifier of a stored group (its id, its name, its ARN), another CloudFront name this
//! service minted (a distribution's edge domain), a well-formed endpoint nothing carries, a
//! name in a customer's own zone, or no endpoint shape at all.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds, and the creates go through the same
//! wire helper [`crate::connection_groups`] uses.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    STATUS_DEPLOYED, STATUS_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_groups::{create_connection_group, get_connection_group};
use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, is_version_token, past_propagation, Answer,
    ACCOUNT_ID, API_VERSION,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, escape, managed_certificate_request, multi_tenant_distribution,
};
use crate::realtime_log_config_list::encode_query;
use crate::response_headers_policy_lifecycle::text;

// ---------------------------------------------------------------------------
// Calling the operation
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/connection-group?RoutingEndpoint=<endpoint>` — the operation binds its
/// one input member to a query string parameter
/// (`smithy_query(query="operation",operation="GetConnectionGroupByRoutingEndpoint")`).
///
/// The value is percent-encoded by the encoder [`crate::realtime_log_config_list`] already
/// builds for a query-bound member, so a value of any shape reaches the handler as written.
async fn get_by_routing_endpoint(router: &MultiServiceRouter, endpoint: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/connection-group?RoutingEndpoint={}",
            encode_query(endpoint)
        ),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The groups every case reads
// ---------------------------------------------------------------------------

/// One `CreateConnectionGroupRequest` a row below submits.
#[derive(Debug, Clone)]
struct Create {
    case: &'static str,
    name: &'static str,
    /// `Ipv6Enabled`, or `None` to leave the optional member out entirely.
    ipv6_enabled: Option<bool>,
    /// `Enabled`, or `None` to leave the optional member out entirely.
    enabled: Option<bool>,
    tags: Vec<(&'static str, &'static str)>,
}

impl Create {
    fn xml(&self) -> String {
        let mut out = format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{}</Name>",
            self.name
        );
        if let Some(ipv6_enabled) = self.ipv6_enabled {
            out.push_str(&format!("<Ipv6Enabled>{ipv6_enabled}</Ipv6Enabled>"));
        }
        if let Some(enabled) = self.enabled {
            out.push_str(&format!("<Enabled>{enabled}</Enabled>"));
        }
        if !self.tags.is_empty() {
            out.push_str("<Tags><Items>");
            for (key, value) in &self.tags {
                out.push_str(&format!(
                    "<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"
                ));
            }
            out.push_str("</Items></Tags>");
        }
        out.push_str("</CreateConnectionGroupRequest>");
        out
    }
}

/// The creates every case below runs, differing along which optional members the request
/// names at all, the two values each boolean can take, and how many tags the group carries.
fn creates() -> Vec<Create> {
    vec![
        Create {
            case: "every optional member left out",
            name: "endpoint-plain-group",
            ipv6_enabled: None,
            enabled: None,
            tags: Vec::new(),
        },
        Create {
            case: "ipv6 turned off",
            name: "endpoint-ipv4-only-group",
            ipv6_enabled: Some(false),
            enabled: None,
            tags: Vec::new(),
        },
        Create {
            case: "a disabled group",
            name: "endpoint-disabled-group",
            ipv6_enabled: None,
            enabled: Some(false),
            tags: Vec::new(),
        },
        Create {
            case: "both booleans named, one tag",
            name: "endpoint-tagged-group",
            ipv6_enabled: Some(true),
            enabled: Some(true),
            tags: vec![("stage", "prod")],
        },
        Create {
            case: "several tags",
            name: "endpoint-many-tags-group",
            ipv6_enabled: Some(false),
            enabled: Some(true),
            tags: vec![("stage", "dev"), ("team", "edge")],
        },
    ]
}

/// What one create stored, as the cases below address it.
#[derive(Debug, Clone)]
struct Stored {
    case: String,
    id: String,
    name: String,
    arn: String,
    routing_endpoint: String,
}

/// Run `create` against `router` and answer the identity it minted.
async fn create_group(router: &MultiServiceRouter, create: &Create) -> Stored {
    let answer = create_connection_group(router, &create.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        create.case, answer.status, answer.raw
    );
    let group = answer.root();
    Stored {
        case: create.case.to_string(),
        id: text(group, "Id", create.case, "the created group", &answer.raw),
        name: text(group, "Name", create.case, "the created group", &answer.raw),
        arn: text(group, "Arn", create.case, "the created group", &answer.raw),
        routing_endpoint: text(
            group,
            "RoutingEndpoint",
            create.case,
            "the created group",
            &answer.raw,
        ),
    }
}

/// Create a distribution tenant that names NO connection group, so the service mints the
/// account's DEFAULT one for it, and answer that group as the cases address it.
///
/// The group this returns was created by the SERVICE rather than by a call, which is the
/// other way a connection group comes to exist.
async fn default_group(router: &MultiServiceRouter) -> Stored {
    let distribution_id = multi_tenant_distribution(router, "ref-connection-group-endpoint").await;
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution_id}</DistributionId>\
         <Name>endpoint-default-tenant</Name>\
         <Domains><member><Domain>{}</Domain></member></Domains>\
         <Parameters><member><Name>tenantName</Name><Value>endpoint-default-tenant</Value>\
         </member></Parameters>{}\
         </CreateDistributionTenantRequest>",
        escape("default-tenant.example.com"),
        // The domain is a name in a customer's zone, so the create asks CloudFront to
        // validate it — the one shared helper every sibling suite's tenant create uses.
        managed_certificate_request()
    );
    let created = create_distribution_tenant(router, &body).await;
    assert_eq!(
        created.status, 201,
        "case the default group: expected the tenant to be created, got {} with {}",
        created.status, created.raw
    );
    let tenant = created
        .root()
        .descendant("DistributionTenant")
        .expect("case the default group: the create carries no DistributionTenant");
    let group_id = text(
        tenant,
        "ConnectionGroupId",
        "the default group",
        "the created tenant",
        &created.raw,
    );

    let read = get_connection_group(router, &group_id).await;
    assert_eq!(
        read.status, 200,
        "case the default group: expected HTTP 200 reading {group_id}, got {} with {}",
        read.status, read.raw
    );
    Stored {
        case: "the group the service minted for a tenant".to_string(),
        id: group_id,
        name: text(
            read.root(),
            "Name",
            "the default group",
            "the minted group",
            &read.raw,
        ),
        arn: text(
            read.root(),
            "Arn",
            "the default group",
            "the minted group",
            &read.raw,
        ),
        routing_endpoint: text(
            read.root(),
            "RoutingEndpoint",
            "the default group",
            "the minted group",
            &read.raw,
        ),
    }
}

/// The members a read of one group reports from the stored record, so the two reads of that
/// record are compared member by member rather than by the raw document.
const REPORTED_MEMBERS: [&str; 10] = [
    "Id",
    "Name",
    "Arn",
    "RoutingEndpoint",
    "Enabled",
    "Ipv6Enabled",
    "Status",
    "IsDefault",
    "CreatedTime",
    "LastModifiedTime",
];

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// The endpoint specified is matched against the routing endpoint STORED on each group, and
/// the answer is that group's stored record and version token — the very ones a read by
/// identifier reports. DNS names are case-insensitive, so the case the endpoint is asked for
/// in changes nothing.
#[tokio::test]
async fn a_group_is_read_by_the_routing_endpoint_assigned_to_it() {
    let router = emulator();
    let mut stored: Vec<Stored> = Vec::new();
    for create in creates() {
        stored.push(create_group(&router, &create).await);
    }
    stored.push(default_group(&router).await);

    // Every group the account holds carries its OWN endpoint: a lookup by endpoint can only
    // be a lookup of one record.
    for (index, group) in stored.iter().enumerate() {
        for other in stored.iter().skip(index + 1) {
            assert_ne!(
                group.routing_endpoint.to_lowercase(),
                other.routing_endpoint.to_lowercase(),
                "case {} and {}: two groups share the routing endpoint {}",
                group.case,
                other.case,
                group.routing_endpoint
            );
        }
    }

    // Past the propagation the clock owes, so every group has reached its terminal status
    // and the two reads below cannot straddle that transition.
    tokio::time::sleep(past_propagation()).await;

    for group in &stored {
        let by_identifier = get_connection_group(&router, &group.id).await;
        assert_eq!(
            by_identifier.status, 200,
            "case {}: expected HTTP 200 reading by id, got {} with {}",
            group.case, by_identifier.status, by_identifier.raw
        );
        let expected_etag = by_identifier.header("etag").to_string();
        assert!(
            is_version_token(&expected_etag),
            "case {}: expected a minted version token in the ETag header, got {expected_etag:?}",
            group.case
        );
        assert_eq!(
            by_identifier.root().text_at("Status"),
            Some(STATUS_DEPLOYED),
            "case {}: expected the clock to have deployed the group, got {}",
            group.case,
            by_identifier.raw
        );
        // Each member compared below is REPORTED by the read it is compared against, so a
        // comparison of the two answers cannot pass on two absences.
        for member in REPORTED_MEMBERS {
            assert!(
                by_identifier.root().text_at(member).is_some(),
                "case {}: the read by id must report {member}, got {}",
                group.case,
                by_identifier.raw
            );
        }

        // The case the endpoint is written in: as minted, upper and mixed.
        for (case, endpoint) in [
            ("as minted", group.routing_endpoint.clone()),
            ("in upper case", group.routing_endpoint.to_ascii_uppercase()),
            ("in mixed case", mixed_case(&group.routing_endpoint)),
        ] {
            let answer = get_by_routing_endpoint(&router, &endpoint).await;
            assert_eq!(
                answer.status, 200,
                "case {} ({case}): expected HTTP 200, got {} with {}",
                group.case, answer.status, answer.raw
            );
            assert_eq!(
                answer.header("etag"),
                expected_etag,
                "case {} ({case}): expected the stored version token {expected_etag}, got {:?}",
                group.case,
                answer.header("etag")
            );
            for member in REPORTED_MEMBERS {
                assert_eq!(
                    answer.root().text_at(member),
                    by_identifier.root().text_at(member),
                    "case {} ({case}): {member} must come from the stored record, got {}",
                    group.case,
                    answer.raw
                );
            }
            // The record's identity is the one the create minted, not anything read off the
            // request.
            assert_eq!(
                answer.root().text_at("Id"),
                Some(group.id.as_str()),
                "case {} ({case}): expected the group {}, got {}",
                group.case,
                group.id,
                answer.raw
            );
            assert_eq!(
                answer.root().text_at("Arn"),
                Some(group.arn.as_str()),
                "case {} ({case}): expected the stored ARN, got {}",
                group.case,
                answer.raw
            );
            // The tag set travels with the group, so a group created with tags reports them
            // through this read too.
            assert_eq!(
                tag_pairs(&answer),
                tag_pairs(&by_identifier),
                "case {} ({case}): the tag set must be the stored one, got {}",
                group.case,
                answer.raw
            );
        }

        // The read REPORTS: the record a later caller sees is untouched by it.
        let after = get_connection_group(&router, &group.id).await;
        assert_eq!(
            after.header("etag"),
            expected_etag,
            "case {}: a read by routing endpoint must not spend a version token, got {:?}",
            group.case,
            after.header("etag")
        );
        for member in REPORTED_MEMBERS {
            assert_eq!(
                after.root().text_at(member),
                by_identifier.root().text_at(member),
                "case {}: a read by routing endpoint must not change {member}, got {}",
                group.case,
                after.raw
            );
        }
    }
}

/// The status this read reports is the group's status at this instant: a group CloudFront has
/// just created is `InProgress`, and the CLOCK — not this call — owes it `Deployed`.
#[tokio::test]
async fn the_read_reports_the_status_the_clock_owes() {
    let router = emulator();
    let group = create_group(&router, &creates()[0]).await;

    let fresh = get_by_routing_endpoint(&router, &group.routing_endpoint).await;
    assert_eq!(
        fresh.status, 200,
        "case a freshly created group: expected HTTP 200, got {} with {}",
        fresh.status, fresh.raw
    );
    assert_eq!(
        fresh.root().text_at("Status"),
        Some(STATUS_IN_PROGRESS),
        "case a freshly created group: expected {STATUS_IN_PROGRESS}, got {}",
        fresh.raw
    );

    tokio::time::sleep(past_propagation()).await;

    let settled = get_by_routing_endpoint(&router, &group.routing_endpoint).await;
    assert_eq!(
        settled.root().text_at("Status"),
        Some(STATUS_DEPLOYED),
        "case the same group past its propagation: expected {STATUS_DEPLOYED}, got {}",
        settled.raw
    );
}

/// A `RoutingEndpoint` no stored connection group carries names no entity: HTTP 404,
/// `EntityNotFound` — whether the value is another identifier of a group that DOES exist,
/// another CloudFront name this service minted, a well-formed endpoint nothing carries, a
/// name in a customer's own zone, or no endpoint shape at all. None of the refusals stores
/// anything either.
#[tokio::test]
async fn a_routing_endpoint_no_group_carries_is_not_found() {
    let router = emulator();
    let mut stored: Vec<Stored> = Vec::new();
    for create in creates() {
        stored.push(create_group(&router, &create).await);
    }
    let group = stored[0].clone();

    // Another CloudFront name this service minted, which is no connection group's routing
    // endpoint: a standard distribution's own edge domain.
    let created = create_distribution(
        &router,
        &config("standard", "ref-connection-group-endpoint-absent").xml(),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case the setup distribution: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let distribution_domain = text(
        created.root(),
        "DomainName",
        "a distribution's edge domain",
        "the created distribution",
        &created.raw,
    );

    // The documented example endpoint: minted endpoints are allocated, so this one is
    // carried by no group here — which the guard below pins.
    let unassigned = "d111111abcdef8.cloudfront.net".to_string();
    assert!(
        stored
            .iter()
            .all(|group| !group.routing_endpoint.eq_ignore_ascii_case(&unassigned)),
        "the fixture must not have been assigned {unassigned}"
    );

    let cases: Vec<(&str, String)> = vec![
        ("a well-formed endpoint nothing carries", unassigned),
        (
            "an endpoint under another cloudfront label",
            "dzzzzzzzzzzzzz.cloudfront.net".to_string(),
        ),
        ("the group's own id", group.id.clone()),
        ("the group's own name", group.name.clone()),
        ("the group's own ARN", group.arn.clone()),
        ("a distribution's edge domain", distribution_domain),
        (
            "an ARN no group carries",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:connection-group/cg_ZZZZZZZZZZZZZ"),
        ),
        (
            "a name in the customer's own zone",
            "shop.example.com".to_string(),
        ),
        (
            "the endpoint's leading label alone",
            group
                .routing_endpoint
                .split('.')
                .next()
                .expect("an endpoint carries at least one label")
                .to_string(),
        ),
        (
            "a child of the endpoint",
            format!("edge.{}", group.routing_endpoint),
        ),
        ("a value of no endpoint shape", "not an endpoint".to_string()),
        ("the empty endpoint", String::new()),
    ];

    for (case, endpoint) in cases {
        let answer = get_by_routing_endpoint(&router, &endpoint).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {}",
            answer.raw
        );
    }

    // The control: each stored group is still read by its own endpoint, so the refusals were
    // about the value asked for and nothing else.
    for group in &stored {
        let answer = get_by_routing_endpoint(&router, &group.routing_endpoint).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected the group to still be readable, got {} with {}",
            group.case, answer.status, answer.raw
        );
        assert_eq!(
            answer.root().text_at("Id"),
            Some(group.id.as_str()),
            "case {}: expected the group {}, got {}",
            group.case,
            group.id,
            answer.raw
        );
    }
}

/// The `(Key, Value)` pairs one answer reported for the group's tag set, in order.
fn tag_pairs(answer: &Answer) -> Vec<(String, String)> {
    let Some(items) = answer.root().at("Tags/Items") else {
        return Vec::new();
    };
    items
        .children_named("Tag")
        .into_iter()
        .map(|tag| {
            (
                tag.text_at("Key").unwrap_or_default().to_string(),
                tag.text_at("Value").unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `value` with every other ASCII letter upper-cased, so a case asks for a name in neither
/// the case it was minted in nor all upper case.
fn mixed_case(value: &str) -> String {
    value
        .chars()
        .enumerate()
        .map(|(index, c)| {
            if index % 2 == 0 {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}
