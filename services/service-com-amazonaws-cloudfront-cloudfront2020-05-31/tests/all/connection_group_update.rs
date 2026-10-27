// SPDX-License-Identifier: CC0-1.0

//! `UpdateConnectionGroup` — changing the settings a connection group runs under, end to
//! end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! * the update submits `Enabled`, `Ipv6Enabled` and `AnycastIpListId`, all optional, so a
//!   member the request names becomes the one the group runs under and a member it leaves
//!   out is left exactly as it was — and every later read answers from that one record;
//! * the group's identity (`Id`, `Arn`, `Name`, `RoutingEndpoint`, `CreatedTime`,
//!   `IsDefault`) is untouched — the update submits none of it, and the name "can't be
//!   updated after you create the connection group";
//! * an update is a mutation: it mints a fresh version token, so the token the caller
//!   matched on no longer satisfies an `If-Match`, and the group re-enters its transient
//!   status with the CLOCK owing it the terminal one;
//! * an `AnycastIpListId` names an Anycast static IP list, so a value this emulator cannot
//!   resolve to one is an invalid argument — the rule the create applies to the same
//!   member;
//! * an id no group carries is `EntityNotFound` (404), an `If-Match` that is no version
//!   token is `InvalidIfMatchVersion` (400) and one that is well formed but not the
//!   current version is `PreconditionFailed` (412) — and each refusal leaves the group
//!   exactly as it was.
//!
//! The rows differ along what those rules generalise over: which optional members the
//! create named at all, which the update names at all, and both values each boolean can
//! take — including an update that names the value the group already carries.
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! [`crate::distribution_lifecycle`] suite's, and the connection group create and read are
//! [`crate::connection_groups`]'s, reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    IP_ADDRESS_TYPE_IPV4, STATUS_DEPLOYED, STATUS_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::anycast_ip_list_lifecycle::{create_anycast_ip_list, Create as AnycastCreate};
use crate::connection_groups::{create_connection_group, get_connection_group};
use crate::distribution_tenant_lifecycle::encode_label;
use crate::distribution_lifecycle::{
    call, emulator, is_version_token, past_propagation, Answer, API_VERSION,
};

/// `PUT /2020-05-31/connection-group/{Id}` with the members the row names, under
/// `if_match`.
///
/// `IfMatch` is a required member, so every call states one: what the rows differ along is
/// the value, not whether the header is there.
///
/// `pub(crate)` so the sibling delete suite disables a group over this one wire path rather
/// than growing a copy of it.
pub(crate) async fn update_connection_group(
    router: &MultiServiceRouter,
    id: &str,
    if_match: &str,
    anycast_ip_list_id: Option<&str>,
    enabled: Option<bool>,
    ipv6_enabled: Option<bool>,
) -> Answer {
    let mut body = format!(
        "<UpdateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(anycast_ip_list_id) = anycast_ip_list_id {
        body.push_str(&format!(
            "<AnycastIpListId>{anycast_ip_list_id}</AnycastIpListId>"
        ));
    }
    if let Some(enabled) = enabled {
        body.push_str(&format!("<Enabled>{enabled}</Enabled>"));
    }
    if let Some(ipv6_enabled) = ipv6_enabled {
        body.push_str(&format!("<Ipv6Enabled>{ipv6_enabled}</Ipv6Enabled>"));
    }
    body.push_str("</UpdateConnectionGroupRequest>");
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/connection-group/{}", encode_label(id)),
        Some(&body),
        Some(if_match),
    )
    .await
}

/// Create one connection group named `name`, naming whichever booleans the row states, and
/// report `(Id, ETag)`.
async fn created(
    router: &MultiServiceRouter,
    name: &str,
    enabled: Option<bool>,
    ipv6_enabled: Option<bool>,
) -> (String, String) {
    let mut body = format!(
        "<CreateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>"
    );
    if let Some(ipv6_enabled) = ipv6_enabled {
        body.push_str(&format!("<Ipv6Enabled>{ipv6_enabled}</Ipv6Enabled>"));
    }
    if let Some(enabled) = enabled {
        body.push_str(&format!("<Enabled>{enabled}</Enabled>"));
    }
    body.push_str("</CreateConnectionGroupRequest>");
    let answer = create_connection_group(router, &body).await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected the create to answer 201, got {} ({})",
        answer.status, answer.raw
    );
    (
        answer.root().text_at("Id").unwrap_or_default().to_string(),
        answer.header("etag").to_string(),
    )
}

/// One row: which booleans the create named, which the update names, and what each member
/// must read as once the update has landed.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    created_enabled: Option<bool>,
    created_ipv6: Option<bool>,
    updated_enabled: Option<bool>,
    updated_ipv6: Option<bool>,
}

impl Row {
    /// `Enabled` after the update: the value the update states, else the one the create
    /// stored, else the default a create that names none takes.
    fn expected_enabled(&self) -> bool {
        self.updated_enabled
            .or(self.created_enabled)
            .unwrap_or(true)
    }

    /// `Ipv6Enabled` after the update, on the same rule. The model pins the create's
    /// default literally — "Enable IPv6 for the connection group. The default is `true`".
    fn expected_ipv6(&self) -> bool {
        self.updated_ipv6.or(self.created_ipv6).unwrap_or(true)
    }
}

/// Every combination of "the create named it / did not" and "the update names it / does
/// not", over both values each boolean can take.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "a create and an update naming nothing",
            created_enabled: None,
            created_ipv6: None,
            updated_enabled: None,
            updated_ipv6: None,
        },
        Row {
            case: "a create naming nothing, an update turning both off",
            created_enabled: None,
            created_ipv6: None,
            updated_enabled: Some(false),
            updated_ipv6: Some(false),
        },
        Row {
            case: "a create naming both, an update naming neither",
            created_enabled: Some(false),
            created_ipv6: Some(false),
            updated_enabled: None,
            updated_ipv6: None,
        },
        Row {
            case: "a create naming both, an update turning both on",
            created_enabled: Some(false),
            created_ipv6: Some(false),
            updated_enabled: Some(true),
            updated_ipv6: Some(true),
        },
        Row {
            case: "an update naming only Enabled",
            created_enabled: Some(true),
            created_ipv6: Some(false),
            updated_enabled: Some(false),
            updated_ipv6: None,
        },
        Row {
            case: "an update naming only Ipv6Enabled",
            created_enabled: Some(false),
            created_ipv6: Some(true),
            updated_enabled: None,
            updated_ipv6: Some(false),
        },
        Row {
            case: "an update restating the values the group already carries",
            created_enabled: Some(true),
            created_ipv6: Some(true),
            updated_enabled: Some(true),
            updated_ipv6: Some(true),
        },
    ];
    // The two one-sided creates, so a member the create left out and the update states is
    // covered beside one both state.
    rows.push(Row {
        case: "a create naming only Enabled, an update naming only Ipv6Enabled",
        created_enabled: Some(false),
        created_ipv6: None,
        updated_enabled: None,
        updated_ipv6: Some(false),
    });
    rows.push(Row {
        case: "a create naming only Ipv6Enabled, an update naming only Enabled",
        created_enabled: None,
        created_ipv6: Some(false),
        updated_enabled: Some(false),
        updated_ipv6: None,
    });
    rows
}

#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_keeps_the_ones_it_does_not() {
    let router = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let case = format!("{} ({index})", row.case);
        let name = format!("update-row-{index:02}");
        let (id, created_etag) =
            created(&router, &name, row.created_enabled, row.created_ipv6).await;

        // The record the create stored, which the update must change only where it says so.
        let before = get_connection_group(&router, &id).await;
        assert_eq!(
            before.status, 200,
            "case {case}: expected the read of the created group to answer 200, got {} ({})",
            before.status, before.raw
        );

        let updated = update_connection_group(
            &router,
            &id,
            &created_etag,
            None,
            row.updated_enabled,
            row.updated_ipv6,
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200, got {} ({})",
            updated.status, updated.raw
        );

        // A fresh version token, so the one the caller matched on is spent.
        let new_etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&new_etag),
            "case {case}: expected a minted version token, got {new_etag:?}"
        );
        assert_ne!(
            new_etag, created_etag,
            "case {case}: an update must mint a new version token, got {new_etag}"
        );

        // The members the update names, and the ones it leaves alone.
        assert_eq!(
            updated.root().text_at("Enabled"),
            Some(row.expected_enabled().to_string().as_str()),
            "case {case}: expected Enabled {}, got {}",
            row.expected_enabled(),
            updated.raw
        );
        assert_eq!(
            updated.root().text_at("Ipv6Enabled"),
            Some(row.expected_ipv6().to_string().as_str()),
            "case {case}: expected Ipv6Enabled {}, got {}",
            row.expected_ipv6(),
            updated.raw
        );

        // The identity the create minted stands: an update submits none of it.
        for member in ["Id", "Arn", "Name", "RoutingEndpoint", "CreatedTime", "IsDefault"] {
            assert_eq!(
                updated.root().text_at(member),
                before.root().text_at(member),
                "case {case}: {member} must stand, got {}",
                updated.raw
            );
        }

        // An update has to reach the edge locations, so the group re-enters its transient
        // status and the CLOCK owes it the terminal one.
        assert_eq!(
            updated.root().text_at("Status"),
            Some(STATUS_IN_PROGRESS),
            "case {case}: expected {STATUS_IN_PROGRESS}, got {}",
            updated.raw
        );

        // The round trip: a read answers from the record the update wrote.
        let read = get_connection_group(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the read to answer 200, got {} ({})",
            read.status, read.raw
        );
        for member in [
            "Id",
            "Arn",
            "Name",
            "RoutingEndpoint",
            "CreatedTime",
            "IsDefault",
            "Enabled",
            "Ipv6Enabled",
            "LastModifiedTime",
        ] {
            assert_eq!(
                read.root().text_at(member),
                updated.root().text_at(member),
                "case {case}: the read must answer the updated record's {member}, got {}",
                read.raw
            );
        }
        assert_eq!(
            read.header("etag"),
            new_etag,
            "case {case}: the read must report the minted token, got {}",
            read.raw
        );

        // The token the update spent no longer satisfies an `If-Match`, and the one it
        // minted does.
        let stale = update_connection_group(
            &router,
            &id,
            &created_etag,
            None,
            Some(row.expected_enabled()),
            None,
        )
        .await;
        assert_eq!(
            stale.status, 412,
            "case {case}: expected the spent token to fail the precondition, got {} ({})",
            stale.status, stale.raw
        );

        // Past the span the clock owes, the group is deployed again — without another call
        // doing it.
        tokio::time::sleep(past_propagation()).await;
        let settled = get_connection_group(&router, &id).await;
        assert_eq!(
            settled.root().text_at("Status"),
            Some(STATUS_DEPLOYED),
            "case {case}: expected the clock to deploy the group, got {}",
            settled.raw
        );
    }
}

#[tokio::test]
async fn an_update_resolves_the_anycast_ip_list_it_names_and_refuses_one_it_cannot() {
    let router = emulator();
    let (id, created_etag) = created(&router, "update-anycast", None, None).await;

    // An id this emulator cannot resolve to an Anycast static IP list is an invalid
    // argument, whatever shape it takes — the rule the create applies to the same member.
    let mut etag = created_etag.clone();
    for (case, candidate) in [
        ("a list id in CloudFront's shape", "ipl-00000000000000000"),
        ("a value of no list shape at all", "not a valid id !!"),
        ("the empty value", ""),
    ] {
        let answer =
            update_connection_group(&router, &id, &etag, Some(candidate), None, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
    }

    // A refused update spends no version token and stores nothing.
    let after_refusals = get_connection_group(&router, &id).await;
    assert_eq!(
        after_refusals.header("etag"),
        etag,
        "case after the refused updates: a refused update must spend no version token, got {}",
        after_refusals.raw
    );
    assert_eq!(
        after_refusals.root().text_at("AnycastIpListId"),
        None,
        "case after the refused updates: no Anycast static IP list must be stored, got {}",
        after_refusals.raw
    );

    // An id this emulator DOES resolve is held as submitted, and the read answers it back.
    let list = create_anycast_ip_list(
        &router,
        &AnycastCreate {
            case: "the list an update associates",
            name: "update-anycast-list".to_string(),
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
        "case the resolvable list: expected the create to answer 202, got {} ({})",
        list.status, list.raw
    );
    let list_id = list.root().text_at("Id").unwrap_or_default().to_string();

    let associated =
        update_connection_group(&router, &id, &etag, Some(&list_id), None, None).await;
    assert_eq!(
        associated.status, 200,
        "case the resolvable list: expected HTTP 200, got {} ({})",
        associated.status, associated.raw
    );
    assert_eq!(
        associated.root().text_at("AnycastIpListId"),
        Some(list_id.as_str()),
        "case the resolvable list: the id is held as submitted, got {}",
        associated.raw
    );
    etag = associated.header("etag").to_string();

    let read = get_connection_group(&router, &id).await;
    assert_eq!(
        read.root().text_at("AnycastIpListId"),
        Some(list_id.as_str()),
        "case the resolvable list: the read must answer from the updated record, got {}",
        read.raw
    );

    // An update that names no list leaves the one the group is associated with.
    let kept = update_connection_group(&router, &id, &etag, None, Some(false), None).await;
    assert_eq!(
        kept.root().text_at("AnycastIpListId"),
        Some(list_id.as_str()),
        "case a later update naming no list: the association must stand, got {}",
        kept.raw
    );
}

#[tokio::test]
async fn an_update_is_refused_on_an_absent_group_and_on_a_version_that_is_not_current() {
    let router = emulator();
    let (id, etag) = created(&router, "update-refusals", Some(true), Some(true)).await;
    let before = get_connection_group(&router, &id).await;
    let arn = before.root().text_at("Arn").unwrap_or_default().to_string();

    // `UpdateConnectionGroup$Id` is "The ID of the connection group", so nothing but the
    // minted id names the group to update: every other form is an entity this store does
    // not hold under that id.
    for (case, candidate) in [
        ("an id never minted", "cg_0000000000000000000000000".to_string()),
        ("the stored id upper-cased", id.to_uppercase()),
        ("the group's name", "update-refusals".to_string()),
        ("the group's ARN", arn),
    ] {
        let answer =
            update_connection_group(&router, &candidate, &etag, None, Some(false), None)
                .await;
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

    // The precondition: a token that is no version token at all, and one that is well
    // formed but not the current version.
    for (case, if_match, status, code) in [
        (
            "a token of no version shape",
            "not-a-version",
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "an empty token",
            "",
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a well-formed token that is not current",
            "E1PDK09ESLDGHD",
            412,
            "PreconditionFailed",
        ),
    ] {
        let answer =
            update_connection_group(&router, &id, if_match, None, Some(false), None).await;
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

    // Every refusal above left the group exactly as it was: the settings it was created
    // with, under the token the create minted.
    let read = get_connection_group(&router, &id).await;
    assert_eq!(
        read.root().text_at("Enabled"),
        Some("true"),
        "case refusals: the group must still carry the settings it was created with, got {}",
        read.raw
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case refusals: a refused update must spend no version token, got {}",
        read.raw
    );
}
