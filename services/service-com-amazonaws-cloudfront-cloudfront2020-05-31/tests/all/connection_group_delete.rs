// SPDX-License-Identifier: CC0-1.0

//! `DeleteConnectionGroup` — removing a connection group under its version token, end to
//! end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! * a delete that satisfies the group's precondition REMOVES the one stored record, so
//!   nothing the group was reachable by resolves to it afterwards — not its id, its name,
//!   its ARN or its routing endpoint — the listing stops reporting it, a second delete of
//!   the same id is an entity that does not exist, and the name is free for another create
//!   to claim;
//! * the answer is the modelled 204 with no document at all;
//! * a group that still carries `Enabled` has not "been disabled yet", so the delete is
//!   refused with `ResourceNotDisabled` (409) until an update turns it off;
//! * a group a stored distribution tenant is still routed through is in use, so the delete
//!   is refused with `CannotDeleteEntityWhileInUse` (409) until the tenant is routed
//!   somewhere else;
//! * `DeleteConnectionGroup$Id` is "The ID of the connection group to delete", so an id no
//!   stored group carries is `EntityNotFound` (404) — including the other identifiers a
//!   READ admits;
//! * the `If-Match` is the group's version: a value that is no version token is
//!   `InvalidIfMatchVersion` (400) and a well-formed token that is not the current one is
//!   `PreconditionFailed` (412);
//! * every refusal leaves the group exactly as it was — still readable, under the same
//!   version token — and leaves every other group alone.
//!
//! The rows differ along what those rules generalise over: whether the create named
//! `Enabled` at all and which value it named, whether the group was disabled by the create
//! or by a later update, whether `Ipv6Enabled` was named, how many tags the group carries,
//! whether it is associated with an Anycast static IP list, and whether the delete lands
//! while the group is still propagating or after the clock has deployed it.
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! [`crate::distribution_lifecycle`] suite's; the create and the read are
//! [`crate::connection_groups`]'s, the update is [`crate::connection_group_update`]'s, the
//! read by routing endpoint is [`crate::connection_group_by_routing_endpoint`]'s and the
//! tenant update is [`crate::distribution_tenant_update`]'s — reused rather than copied.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::IP_ADDRESS_TYPE_IPV4;
use smithy_server_runtime::MultiServiceRouter;

use crate::anycast_ip_list_lifecycle::{create_anycast_ip_list, Create as AnycastCreate};
use crate::connection_group_by_routing_endpoint::get_by_routing_endpoint;
use crate::connection_group_update::update_connection_group;
use crate::connection_groups::{
    create_connection_group, get_connection_group, list_body, list_connection_groups, summary_ids,
};
use crate::distribution_lifecycle::{call, emulator, past_propagation, Answer, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, encode_label, managed_certificate_request,
    multi_tenant_distribution,
};
use crate::distribution_tenant_update::{update_tenant, update_xml};

/// The sentence CloudFront answers `ResourceNotDisabled` with for a CONNECTION GROUP —
/// written out as the LITERAL rather than read back off the crate's own constant, so a
/// change to that constant fails this case.
const MODELLED_RESOURCE_NOT_DISABLED_MESSAGE: &str =
    "You must disable the specified connection group before you can delete it.";

/// The sentence CloudFront answers `CannotDeleteEntityWhileInUse` with, as the model
/// documents the error shape (`smithy_query(query="doc",shape="CannotDeleteEntityWhileInUse")`),
/// written out as the literal for the same reason.
const MODELLED_ENTITY_IN_USE_MESSAGE: &str = "The entity cannot be deleted while it is in use.";

// ---------------------------------------------------------------------------
// Calling the operation
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/connection-group/{Id}` under the `If-Match` the case states.
async fn delete_connection_group(
    router: &MultiServiceRouter,
    id: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/connection-group/{}", encode_label(id)),
        None,
        if_match,
    )
    .await
}

/// A `CreateConnectionGroupRequest` naming `name`, whichever booleans the row states, the
/// Anycast static IP list it states, and `tags` tags.
fn create_xml(
    name: &str,
    enabled: Option<bool>,
    ipv6_enabled: Option<bool>,
    anycast_ip_list_id: Option<&str>,
    tags: usize,
) -> String {
    let mut out = format!(
        "<CreateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><Name>{name}</Name>"
    );
    if let Some(enabled) = enabled {
        out.push_str(&format!("<Enabled>{enabled}</Enabled>"));
    }
    if let Some(ipv6_enabled) = ipv6_enabled {
        out.push_str(&format!("<Ipv6Enabled>{ipv6_enabled}</Ipv6Enabled>"));
    }
    if let Some(anycast_ip_list_id) = anycast_ip_list_id {
        out.push_str(&format!(
            "<AnycastIpListId>{anycast_ip_list_id}</AnycastIpListId>"
        ));
    }
    if tags > 0 {
        out.push_str("<Tags><Items>");
        for index in 0..tags {
            out.push_str(&format!(
                "<Tag><Key>tag-{index}</Key><Value>value-{index}</Value></Tag>"
            ));
        }
        out.push_str("</Items></Tags>");
    }
    out.push_str("</CreateConnectionGroupRequest>");
    out
}

/// The identity a created group reports, which the delete has to make unreachable.
#[derive(Debug, Clone)]
struct Group {
    id: String,
    name: String,
    arn: String,
    routing_endpoint: String,
    etag: String,
}

/// Create one connection group and report its identity.
async fn created(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    enabled: Option<bool>,
    ipv6_enabled: Option<bool>,
    anycast_ip_list_id: Option<&str>,
    tags: usize,
) -> Group {
    let answer = create_connection_group(
        router,
        &create_xml(name, enabled, ipv6_enabled, anycast_ip_list_id, tags),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected the create to answer 201, got {} ({})",
        answer.status, answer.raw
    );
    let text = |member: &str| {
        answer
            .root()
            .text_at(member)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the created group carries no {member}: {}",
                    answer.raw
                )
            })
            .to_string()
    };
    Group {
        id: text("Id"),
        name: text("Name"),
        arn: text("Arn"),
        routing_endpoint: text("RoutingEndpoint"),
        etag: answer.header("etag").to_string(),
    }
}

/// Turn `Enabled` off, and report the version token the update minted.
async fn disabled(router: &MultiServiceRouter, case: &str, group: &Group) -> String {
    let answer =
        update_connection_group(router, &group.id, &group.etag, None, Some(false), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the update that disables the group to answer 200, got {} ({})",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.root().text_at("Enabled"),
        Some("false"),
        "case {case}: expected the update to disable the group, got {}",
        answer.raw
    );
    answer.header("etag").to_string()
}

/// Assert that nothing the group was reachable by reaches it any more.
async fn assert_gone(router: &MultiServiceRouter, case: &str, group: &Group) {
    for (identifier_case, identifier) in [
        ("its id", group.id.as_str()),
        ("its name", group.name.as_str()),
        ("its ARN", group.arn.as_str()),
    ] {
        let read = get_connection_group(router, identifier).await;
        assert_eq!(
            read.status, 404,
            "case {case} ({identifier_case}): expected HTTP 404 after the delete, got {} ({})",
            read.status, read.raw
        );
        assert_eq!(
            read.error_code(),
            "EntityNotFound",
            "case {case} ({identifier_case}): expected EntityNotFound, got {:?} ({})",
            read.error_code(),
            read.raw
        );
    }

    let by_endpoint = get_by_routing_endpoint(router, &group.routing_endpoint).await;
    assert_eq!(
        by_endpoint.status, 404,
        "case {case} (its routing endpoint): expected HTTP 404 after the delete, got {} ({})",
        by_endpoint.status, by_endpoint.raw
    );

    // A second delete of the same id names no entity either: the record is gone rather
    // than tombstoned.
    let again = delete_connection_group(router, &group.id, Some(&group.etag)).await;
    assert_eq!(
        again.status, 404,
        "case {case}: expected a second delete to answer HTTP 404, got {} ({})",
        again.status, again.raw
    );
    assert_eq!(
        again.error_code(),
        "EntityNotFound",
        "case {case}: expected EntityNotFound, got {:?} ({})",
        again.error_code(),
        again.raw
    );
}

/// The ids the listing reports, as a set.
async fn listed_ids(router: &MultiServiceRouter, case: &str) -> BTreeSet<String> {
    let answer = list_connection_groups(router, &list_body(None, None, None)).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the listing to answer 200, got {} ({})",
        answer.status, answer.raw
    );
    summary_ids(&answer).into_iter().collect()
}

// ---------------------------------------------------------------------------
// The deletes under test
// ---------------------------------------------------------------------------

/// One row: the group the delete removes, and when it lands.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// `Enabled` as the create names it, or `None` to leave the member out entirely.
    created_enabled: Option<bool>,
    /// `Ipv6Enabled` as the create names it, or `None` to leave it out entirely.
    created_ipv6: Option<bool>,
    /// How many tags the group carries.
    tags: usize,
    /// Whether the group is associated with an Anycast static IP list.
    anycast: bool,
    /// Whether the caller waits for the clock to deploy the group before deleting it.
    settled: bool,
}

impl Row {
    /// Whether the group is enabled once the create has landed: the value the create
    /// named, else the one a create that names none takes.
    fn enabled_after_create(&self) -> bool {
        self.created_enabled.unwrap_or(true)
    }
}

/// Every combination the rules generalise over: a group disabled by its own create beside
/// one disabled by a later update, both values each boolean can take, a group with no tags
/// beside groups carrying one and several, a group associated with an Anycast static IP list
/// beside one that is not, and a delete landing while the group propagates beside one
/// landing after the clock deployed it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a group created disabled, naming nothing else",
            created_enabled: Some(false),
            created_ipv6: None,
            tags: 0,
            anycast: false,
            settled: false,
        },
        Row {
            case: "a group created enabled by default and disabled by an update",
            created_enabled: None,
            created_ipv6: Some(false),
            tags: 1,
            anycast: false,
            settled: false,
        },
        Row {
            case: "a group created disabled, carrying three tags, deleted once deployed",
            created_enabled: Some(false),
            created_ipv6: Some(true),
            tags: 3,
            anycast: false,
            settled: true,
        },
        Row {
            case: "a group associated with an Anycast static IP list",
            created_enabled: Some(false),
            created_ipv6: None,
            tags: 0,
            anycast: true,
            settled: false,
        },
        Row {
            case: "a group created enabled explicitly and disabled by an update",
            created_enabled: Some(true),
            created_ipv6: Some(true),
            tags: 2,
            anycast: false,
            settled: true,
        },
        Row {
            case: "a group created disabled with IPv6 off",
            created_enabled: Some(false),
            created_ipv6: Some(false),
            tags: 0,
            anycast: false,
            settled: true,
        },
        Row {
            case: "a group created enabled with an Anycast static IP list, then disabled",
            created_enabled: Some(true),
            created_ipv6: None,
            tags: 1,
            anycast: true,
            settled: false,
        },
    ]
}

#[tokio::test]
async fn a_delete_removes_the_one_record_every_read_answered_from() {
    let router = emulator();

    // One Anycast static IP list the rows that associate one all name, created over the
    // same wire path its own suite drives.
    let list = create_anycast_ip_list(
        &router,
        &AnycastCreate {
            case: "the list the delete rows associate",
            name: "delete-anycast-list".to_string(),
            // `IpCount` is one of the counts this service admits, as its own create
            // refusal names them.
            ip_count: 3,
            ip_address_type: Some(IP_ADDRESS_TYPE_IPV4),
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        }
        .xml(),
    )
    .await;
    assert_eq!(
        list.status, 202,
        "case the Anycast static IP list: expected the create to answer 202, got {} ({})",
        list.status, list.raw
    );
    let list_id = list
        .root()
        .text_at("Id")
        .expect("the created list carries an Id")
        .to_string();

    // A group no row deletes: every delete below has to leave it exactly as it is.
    let control = created(
        &router,
        "the control group",
        "delete-control",
        Some(false),
        None,
        None,
        1,
    )
    .await;
    let mut survivors: BTreeSet<String> = BTreeSet::from([control.id.clone()]);

    for (index, row) in rows().into_iter().enumerate() {
        let case = format!("{} ({index})", row.case);
        let name = format!("delete-row-{index:02}");
        let mut group = created(
            &router,
            &case,
            &name,
            row.created_enabled,
            row.created_ipv6,
            row.anycast.then_some(list_id.as_str()),
            row.tags,
        )
        .await;

        // A group that is still enabled is refused until an update turns it off, whatever
        // else the row named.
        if row.enabled_after_create() {
            let refused = delete_connection_group(&router, &group.id, Some(&group.etag)).await;
            assert_eq!(
                refused.status, 409,
                "case {case}: expected an enabled group to be refused with HTTP 409, got {} ({})",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "ResourceNotDisabled",
                "case {case}: expected ResourceNotDisabled, got {:?} ({})",
                refused.error_code(),
                refused.raw
            );
            group.etag = disabled(&router, &case, &group).await;
        }

        if row.settled {
            tokio::time::sleep(past_propagation()).await;
        }

        let deleted = delete_connection_group(&router, &group.id, Some(&group.etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected the modelled HTTP 204, got {} ({})",
            deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {case}: expected no document at all, got {:?}",
            deleted.raw
        );

        // Nothing the group was reachable by reaches it any more.
        assert_gone(&router, &case, &group).await;

        // The collection holds exactly the groups no delete removed.
        assert_eq!(
            listed_ids(&router, &case).await,
            survivors,
            "case {case}: expected the listing to hold exactly the groups no delete removed"
        );

        // The group every delete had to leave alone is untouched, under the token its own
        // create minted.
        let read_control = get_connection_group(&router, &control.id).await;
        assert_eq!(
            read_control.status, 200,
            "case {case}: the group no delete names must still be readable, got {} ({})",
            read_control.status, read_control.raw
        );
        assert_eq!(
            read_control.header("etag"),
            control.etag,
            "case {case}: the group no delete names must keep its version token, got {}",
            read_control.raw
        );

        // "This name can't be updated after you create the connection group" bounds a NAME
        // to one group for its whole life, so the deleted group's name is free again — and
        // what claims it is a DIFFERENT record.
        let reclaimed = created(
            &router,
            &format!("{case}: the name after the delete"),
            &name,
            Some(false),
            None,
            None,
            0,
        )
        .await;
        assert_ne!(
            reclaimed.id, group.id,
            "case {case}: a create claiming the freed name mints a new id, got {}",
            reclaimed.id
        );
        survivors.insert(reclaimed.id.clone());
    }
}

#[tokio::test]
async fn a_delete_is_refused_on_an_absent_id_and_on_a_version_that_is_not_current() {
    let router = emulator();
    let group = created(
        &router,
        "the group every refusal names",
        "delete-refusals",
        Some(false),
        Some(true),
        None,
        2,
    )
    .await;

    // `DeleteConnectionGroup$Id` is "The ID of the connection group to delete", so nothing
    // but the minted id names the group to delete: every other form a READ admits is an
    // entity this store holds nothing for under that id.
    for (case, candidate) in [
        ("an id never minted", "cg_0000000000000".to_string()),
        ("the stored id upper-cased", group.id.to_uppercase()),
        ("the group's name", group.name.clone()),
        ("the group's ARN", group.arn.clone()),
        ("the group's routing endpoint", group.routing_endpoint.clone()),
    ] {
        let answer = delete_connection_group(&router, &candidate, Some(&group.etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
    }

    // The precondition: a value that is no version token at all, and one that is well
    // formed but not this group's current version.
    for (case, if_match, status, code) in [
        ("a token of no version shape", "not-a-version", 400, "InvalidIfMatchVersion"),
        ("an empty token", "", 400, "InvalidIfMatchVersion"),
        ("a token in lowercase", "e1pdk09esldghd", 400, "InvalidIfMatchVersion"),
        (
            "a well-formed token that is not current",
            "E1PDK09ESLDGHD",
            412,
            "PreconditionFailed",
        ),
    ] {
        let answer = delete_connection_group(&router, &group.id, Some(if_match)).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status}, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code}, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
    }

    // Every refusal above left the group exactly as it was, under the token its create
    // minted — and the current token still deletes it.
    let read = get_connection_group(&router, &group.id).await;
    assert_eq!(
        read.status, 200,
        "case after the refusals: the group must still be readable, got {} ({})",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        group.etag,
        "case after the refusals: a refused delete must spend no version token, got {}",
        read.raw
    );
    let deleted = delete_connection_group(&router, &group.id, Some(&group.etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case the current token after the refusals: expected HTTP 204, got {} ({})",
        deleted.status, deleted.raw
    );
    assert_gone(&router, "after the refusals", &group).await;
}

#[tokio::test]
async fn an_enabled_group_is_refused_until_an_update_disables_it() {
    let router = emulator();

    // Both ways a group ends up enabled: a create that names `Enabled` and one that leaves
    // the member out and takes its default.
    for (index, created_enabled) in [None, Some(true)].into_iter().enumerate() {
        let case = format!("an enabled group ({index})");
        let group = created(
            &router,
            &case,
            &format!("delete-enabled-{index}"),
            created_enabled,
            None,
            None,
            0,
        )
        .await;

        let refused = delete_connection_group(&router, &group.id, Some(&group.etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409, got {} ({})",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotDisabled",
            "case {case}: expected ResourceNotDisabled, got {:?} ({})",
            refused.error_code(),
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message"),
            Some(MODELLED_RESOURCE_NOT_DISABLED_MESSAGE),
            "case {case}: expected the sentence CloudFront serves for this resource kind, \
             got {}",
            refused.raw
        );

        // The refusal stored nothing: the group is readable, still enabled, under the same
        // token.
        let read = get_connection_group(&router, &group.id).await;
        assert_eq!(
            read.root().text_at("Enabled"),
            Some("true"),
            "case {case}: a refused delete must leave the group enabled, got {}",
            read.raw
        );
        assert_eq!(
            read.header("etag"),
            group.etag,
            "case {case}: a refused delete must spend no version token, got {}",
            read.raw
        );

        // Disabling it is what lifts the refusal.
        let etag = disabled(&router, &case, &group).await;
        let deleted = delete_connection_group(&router, &group.id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected the disabled group to be deleted, got {} ({})",
            deleted.status, deleted.raw
        );
        assert_gone(
            &router,
            &case,
            &Group {
                etag,
                ..group.clone()
            },
        )
        .await;
    }
}

#[tokio::test]
async fn a_group_a_tenant_is_routed_through_is_refused_until_the_tenant_moves() {
    let router = emulator();
    let case = "a group a tenant is routed through";

    // Two groups: the one the tenant is created on, and the one it moves to.
    let routed = created(&router, case, "delete-in-use", Some(false), None, None, 0).await;
    let elsewhere = created(&router, case, "delete-in-use-other", Some(false), None, None, 0).await;

    let distribution_id = multi_tenant_distribution(&router, "ref-connection-group-delete").await;
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution_id}</DistributionId>\
         <Name>delete-in-use-tenant</Name>\
         <Domains><member><Domain>delete-in-use-tenant.example.com</Domain></member></Domains>\
         <Parameters><member><Name>tenantName</Name><Value>acme</Value></member></Parameters>\
         <ConnectionGroupId>{}</ConnectionGroupId>{}\
         </CreateDistributionTenantRequest>",
        routed.id,
        managed_certificate_request()
    );
    let tenant = create_distribution_tenant(&router, &body).await;
    assert_eq!(
        tenant.status, 201,
        "case {case}: expected the tenant create to answer 201, got {} ({})",
        tenant.status, tenant.raw
    );
    assert_eq!(
        tenant.root().text_at("ConnectionGroupId"),
        Some(routed.id.as_str()),
        "case {case}: the tenant must be routed through the group it named, got {}",
        tenant.raw
    );
    let tenant_id = tenant
        .root()
        .text_at("Id")
        .expect("the created tenant carries an Id")
        .to_string();
    let tenant_etag = tenant.header("etag").to_string();

    // The group is in use, so the delete is refused — the group is disabled, so this is
    // the association rather than the group's own state talking.
    let refused = delete_connection_group(&router, &routed.id, Some(&routed.etag)).await;
    assert_eq!(
        refused.status, 409,
        "case {case}: expected HTTP 409, got {} ({})",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "CannotDeleteEntityWhileInUse",
        "case {case}: expected CannotDeleteEntityWhileInUse, got {:?} ({})",
        refused.error_code(),
        refused.raw
    );
    assert_eq!(
        refused.root().find_text("Message"),
        Some(MODELLED_ENTITY_IN_USE_MESSAGE),
        "case {case}: expected the sentence the model states for the error shape, got {}",
        refused.raw
    );

    // The refusal stored nothing.
    let read = get_connection_group(&router, &routed.id).await;
    assert_eq!(
        read.status, 200,
        "case {case}: a refused delete must leave the group readable, got {} ({})",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        routed.etag,
        "case {case}: a refused delete must spend no version token, got {}",
        read.raw
    );

    // Routing the tenant somewhere else is what lifts the refusal.
    let moved = update_tenant(
        &router,
        &tenant_id,
        &update_xml(&format!(
            "<ConnectionGroupId>{}</ConnectionGroupId>",
            elsewhere.id
        )),
        Some(&tenant_etag),
    )
    .await;
    assert_eq!(
        moved.status, 200,
        "case {case}: expected the tenant update to answer 200, got {} ({})",
        moved.status, moved.raw
    );
    assert_eq!(
        moved.root().text_at("ConnectionGroupId"),
        Some(elsewhere.id.as_str()),
        "case {case}: the tenant must be routed through the group the update named, got {}",
        moved.raw
    );

    let deleted = delete_connection_group(&router, &routed.id, Some(&routed.etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case {case}: expected the group the tenant left to be deleted, got {} ({})",
        deleted.status, deleted.raw
    );
    assert_gone(&router, case, &routed).await;

    // And the group the tenant moved ONTO is now the one in use.
    let refused = delete_connection_group(&router, &elsewhere.id, Some(&elsewhere.etag)).await;
    assert_eq!(
        refused.error_code(),
        "CannotDeleteEntityWhileInUse",
        "case {case}: the group the tenant moved onto is in use, got {:?} ({})",
        refused.error_code(),
        refused.raw
    );
}
