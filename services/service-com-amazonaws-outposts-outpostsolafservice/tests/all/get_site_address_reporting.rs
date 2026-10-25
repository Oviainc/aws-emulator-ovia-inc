// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetSiteAddress` as a SERVED, REPORTING operation, driven
//! over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — so what
//! it asserts is the emulator's HTTP behaviour, not a handler called directly. The
//! request harness is the lifecycle suite's, reused rather than rebuilt.
//!
//! What [`crate::site_address`] states is WHICH address a read answers. This suite
//! states the two properties that are about the read itself, and pins them over a set
//! of inputs:
//!
//! 1. The operation is SERVED: every call to it — accepted or refused — is answered
//!    from the model's own outcomes (200, or the modelled 404 / 400), never with the
//!    unimplemented-operation channel's HTTP 501 `NotImplemented`.
//! 2. The operation only REPORTS: the spec has it get the site address of the
//!    specified site, so no call to it — however many, in whichever order, whichever
//!    type or identifier form it names, and whether it was accepted or refused —
//!    changes anything a later caller observes: not the address it answers, not the
//!    other type's address, not the site record, not the region's site inventory.
//!
//! The rows differ along the address type (each modelled value), the identifier form
//! the request names the site by (bare id and full ARN), whether the site holds an
//! address of the requested type (both, operating only, neither), how the site's
//! address got there (`CreateSite` or `UpdateSiteAddress`), and — for rule 1 — the
//! outcome the model declares for the request (served, 404, 400).

use serde_json::{json, Value};

use super::site_and_outpost_lifecycle::{
    address, query_value, segment, text, Outposts, ABSENT_SITE_ID, SITE_ARN_PREFIX,
};

/// Every value the modelled `AddressType` enum declares.
///
/// Source: `smithy_query(query="shape", shape="AddressType")` against this service's
/// model — `["SHIPPING_ADDRESS", "OPERATING_ADDRESS"]`.
const ADDRESS_TYPES: [&str; 2] = ["OPERATING_ADDRESS", "SHIPPING_ADDRESS"];

/// The HTTP status the unimplemented-operation channel answers with, and the wire
/// error code it carries. No row below may ever see these: an operation that is served
/// answers from the model's own outcomes.
const NOT_IMPLEMENTED_STATUS: u16 = 501;
/// See [`NOT_IMPLEMENTED_STATUS`].
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// `GET /sites/{SiteId}/address?AddressType=…` as a path, for the two identifier forms
/// the modelled `SiteId` pattern accepts.
fn read_path(identifier: &str, address_type: &str) -> String {
    format!(
        "/sites/{}/address?AddressType={}",
        segment(identifier),
        query_value(address_type)
    )
}

/// Everything a later caller can observe about one site through the implemented
/// readers: the site record itself and both of its addresses.
///
/// This is the value rule 2 compares before and after a batch of reads; it is built
/// only from calls over the wire, so a read that quietly moved stored state shows up
/// here rather than needing the store to be inspected.
async fn observable(outposts: &Outposts, site_id: &str) -> Value {
    let mut view = json!({ "Site": outposts.get_site(site_id).await });
    for address_type in ADDRESS_TYPES {
        view[address_type] = outposts
            .ok("GET", &read_path(site_id, address_type), None)
            .await;
    }
    view
}

/// Rule 1: every `GetSiteAddress` call is answered from the model's own outcomes, so
/// the operation is no longer the 501 scaffold — over a table spanning the served rows
/// (each modelled `AddressType`, each identifier form, a site holding that address and
/// a site holding none) and the refused rows the model declares (an identifier that
/// names no site, a value that is no identifier, a type outside the modelled enum).
#[tokio::test]
async fn every_get_site_address_call_is_served_from_the_modelled_outcomes() {
    let outposts = Outposts::new();
    let furnished = outposts
        .create_site(&json!({
            "Name": "both addresses",
            "OperatingAddress": address("US", "WA", "Seattle"),
            "ShippingAddress": address("US", "WA", "Kirkland"),
        }))
        .await;
    let furnished_id = text(&furnished, "SiteId");
    let furnished_arn = text(&furnished, "SiteArn");
    let bare = outposts.create_site(&json!({ "Name": "no addresses" })).await;
    let bare_id = text(&bare, "SiteId");
    let bare_arn = text(&bare, "SiteArn");

    let mut cases: Vec<(String, String, u16)> = Vec::new();
    for address_type in ADDRESS_TYPES {
        for (held, identifier) in [
            ("holds it, by id", furnished_id.as_str()),
            ("holds it, by ARN", furnished_arn.as_str()),
            ("holds none, by id", bare_id.as_str()),
            ("holds none, by ARN", bare_arn.as_str()),
        ] {
            cases.push((
                format!("{address_type}, site {held}"),
                read_path(identifier, address_type),
                200,
            ));
        }
        // The modelled refusals: a well-formed identifier naming no site is the
        // operation's 404, and a value outside the model's own constraints is its 400.
        cases.push((
            format!("{address_type}, id nothing holds"),
            read_path(ABSENT_SITE_ID, address_type),
            404,
        ));
        cases.push((
            format!("{address_type}, ARN nothing holds"),
            read_path(
                &format!("{SITE_ARN_PREFIX}{ABSENT_SITE_ID}"),
                address_type,
            ),
            404,
        ));
        cases.push((
            format!("{address_type}, value that is no site identifier"),
            read_path("not-a-site-id", address_type),
            400,
        ));
    }
    cases.push((
        "type outside the modelled enum".to_string(),
        read_path(&furnished_id, "BILLING_ADDRESS"),
        400,
    ));

    for (case, path, expected) in &cases {
        let reply = outposts.call("GET", path, None).await;
        assert_ne!(
            reply.status, NOT_IMPLEMENTED_STATUS,
            "case {case:?}: GetSiteAddress must be served, not answered with the \
             unimplemented-operation status, got {} {:?}",
            reply.status, reply.body
        );
        assert_ne!(
            reply.error_code, NOT_IMPLEMENTED_CODE,
            "case {case:?}: GetSiteAddress must be served, not answered with the \
             unimplemented-operation error code, got {} {:?}",
            reply.status, reply.body
        );
        assert_ne!(
            reply.errortype_header, NOT_IMPLEMENTED_CODE,
            "case {case:?}: GetSiteAddress must be served, not answered with the \
             unimplemented-operation error code, got {:?}",
            reply.errortype_header
        );
        assert_eq!(
            reply.status, *expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?}",
            reply.status, reply.body
        );
    }

    // A served read answers the operation's own output members, which the 501 channel
    // has none of: the site it read, and the type it was asked for.
    for address_type in ADDRESS_TYPES {
        let read = outposts
            .ok("GET", &read_path(&furnished_arn, address_type), None)
            .await;
        assert_eq!(
            (text(&read, "SiteId"), text(&read, "AddressType")),
            (furnished_id.clone(), address_type.to_string()),
            "case {address_type:?}: a served read names the site it read and the type \
             it was asked for, got {read:?}"
        );
    }
}

/// Rule 2: reading changes nothing a later caller sees. A batch of reads spanning both
/// types, both identifier forms, repeated calls and the refused routes leaves every
/// site's record, both of its addresses and the region's whole site inventory exactly
/// as they were — for a site holding both addresses, one holding only the operating
/// address written after the fact, and one holding none.
#[tokio::test]
async fn reading_a_site_address_changes_nothing_a_later_caller_sees() {
    let outposts = Outposts::new();
    let furnished = outposts
        .create_site(&json!({
            "Name": "both addresses",
            "Description": "created description",
            "Tags": { "Env": "dev" },
            "OperatingAddress": address("US", "WA", "Seattle"),
            "ShippingAddress": address("IE", "Leinster", "Dublin"),
        }))
        .await;
    // The second site's address arrives through `UpdateSiteAddress` rather than the
    // create, so the property is pinned on both ways an address gets stored.
    let updated = outposts.create_site(&json!({ "Name": "updated later" })).await;
    let updated_id = text(&updated, "SiteId");
    outposts
        .ok(
            "PUT",
            &format!("/sites/{}/address", segment(&updated_id)),
            Some(&json!({
                "AddressType": "OPERATING_ADDRESS",
                "Address": address("JP", "Tokyo", "Chiyoda"),
            })),
        )
        .await;
    let bare = outposts.create_site(&json!({ "Name": "no addresses" })).await;

    let sites: Vec<(&str, String, String)> = vec![
        (
            "both addresses from the create",
            text(&furnished, "SiteId"),
            text(&furnished, "SiteArn"),
        ),
        (
            "operating address written after the create",
            updated_id.clone(),
            text(&updated, "SiteArn"),
        ),
        (
            "no address at all",
            text(&bare, "SiteId"),
            text(&bare, "SiteArn"),
        ),
    ];

    let mut before = Vec::new();
    for (_, site_id, _) in &sites {
        before.push(observable(&outposts, site_id).await);
    }
    let inventory_before = outposts.list_sites("").await;

    // The batch of reads whose effect is under test: every type, both identifier
    // forms, each site, twice over — plus the refused routes, which must not create or
    // move anything either.
    for _ in 0..2 {
        for (_, site_id, site_arn) in &sites {
            for address_type in ADDRESS_TYPES {
                for named_by in [site_id.as_str(), site_arn.as_str()] {
                    outposts
                        .ok("GET", &read_path(named_by, address_type), None)
                        .await;
                }
            }
        }
        for (case, path) in [
            ("id nothing holds", read_path(ABSENT_SITE_ID, "OPERATING_ADDRESS")),
            (
                "ARN nothing holds",
                read_path(
                    &format!("{SITE_ARN_PREFIX}{ABSENT_SITE_ID}"),
                    "SHIPPING_ADDRESS",
                ),
            ),
            (
                "value that is no site identifier",
                read_path("not-a-site-id", "OPERATING_ADDRESS"),
            ),
            (
                "type outside the modelled enum",
                read_path(&updated_id, "BILLING_ADDRESS"),
            ),
        ] {
            let reply = outposts.call("GET", &path, None).await;
            assert!(
                reply.status == 404 || reply.status == 400,
                "case {case:?}: a refused read must be refused by the model's own \
                 outcomes, got {} {:?}",
                reply.status,
                reply.body
            );
        }
    }

    for (index, (case, site_id, _)) in sites.iter().enumerate() {
        let after = observable(&outposts, site_id).await;
        assert_eq!(
            after, before[index],
            "case {case:?}: reading the site address must leave the site and both of \
             its addresses exactly as they were, got {after:?}"
        );
    }
    let inventory_after = outposts.list_sites("").await;
    assert_eq!(
        inventory_after, inventory_before,
        "case \"region inventory\": reading site addresses must add, remove and move \
         nothing in the region's sites, got {inventory_after:?}"
    );
}
