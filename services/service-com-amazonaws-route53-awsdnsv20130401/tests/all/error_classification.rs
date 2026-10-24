// SPDX-License-Identifier: CC0-1.0

//! How Route 53 CLASSIFIES a client fault: the error code a client branches on, and the
//! wording of a not-found built from the request.
//!
//! Two rules are pinned here, each over a TABLE that spans the class it generalises over
//! rather than the one request that exposed it:
//!
//! * a request refused against the MODEL's own rules — a value outside an enum's value
//!   set, a value outside a `@length` bound, a required member the request left out — is
//!   refused with the one input error this service's model declares, `InvalidInput`
//!   (HTTP 400), stating the per-member envelope built from the request, whatever
//!   operation and member it was about, and a request inside those rules is not refused
//!   at all;
//! * a change id that names no change is refused `NoSuchChange` (404) with the message
//!   built FROM the request — `Could not find resource with ID: <the id submitted>` —
//!   while an id that DOES name a change reads it back.
//!
//! The harness (`router`, `call`, `Reply`) and the request builders come from
//! `hosted_zone_records`, so these suites drive the service through the same router a
//! socket-served request goes through.

use super::hosted_zone_records::{call, create_zone_body, create_zone_ok, router};

/// The one input error the Route 53 model declares for an input it cannot use —
/// `InvalidInput`, HTTP 400 (`smithy_query(query="shape",shape="InvalidInput")`). The
/// model declares no shape of any other name for that class.
const INVALID_INPUT: &str = "InvalidInput";

/// One request that breaks a rule the model declares.
struct BadRequest {
    /// What this row is, named in every assertion it drives.
    case: String,
    method: &'static str,
    uri: String,
    body: String,
    /// The member the refusal must name, as the request named it. Matched
    /// case-insensitively, because the clause spells a member path in lowerCamel while
    /// the model spells the member itself in UpperCamel.
    member: &'static str,
}

/// One request that breaks NO rule the model declares, and the status it must answer.
struct GoodRequest {
    case: &'static str,
    uri: String,
    /// What the request must answer: 200 for an implemented operation.
    status: u16,
}

/// A string of `length` ASCII letters, for a row that is one character past a `@length`
/// maximum or one short of its minimum.
fn filler(length: usize) -> String {
    "a".repeat(length)
}

#[tokio::test]
async fn a_request_outside_the_models_rules_is_refused_as_invalid_input() {
    // The rule: a request the boundary refuses against the model's own rules comes back
    // as `InvalidInput` (400) — the input error the Route 53 model declares and every
    // operation lists — and never under a code no shape of this model carries. The rows
    // span the KINDS of rule (an enum's value set, a `@length` maximum, a `@length`
    // minimum, an absent required member), the operations (implemented and scaffolded
    // alike, since the boundary refuses in front of both), the members, and whether the
    // value travels in the query string or in the body.
    let router = router();
    let zone_id = create_zone_ok(&router, "classify.example.com", "ref-classify").await;

    let rows = vec![
        // `HostedZoneType`'s value set is exactly [PrivateHostedZone]
        // (`smithy_query(query="shape",shape="HostedZoneType")`).
        BadRequest {
            case: "a hostedzonetype outside the enum's value set".to_string(),
            method: "GET",
            uri: "/2013-04-01/hostedzone?hostedzonetype=SharedHostedZone".to_string(),
            body: String::new(),
            member: "hostedZoneType",
        },
        BadRequest {
            case: "a modelled hostedzonetype in the wrong case".to_string(),
            method: "GET",
            uri: "/2013-04-01/hostedzone?hostedzonetype=privatehostedzone".to_string(),
            body: String::new(),
            member: "hostedZoneType",
        },
        // `RRType`'s value set is the DNS record types the model names (SOA, A, TXT, NS,
        // CNAME, MX, NAPTR, PTR, SRV, SPF, AAAA, CAA, DS, TLSA, SSHFP, SVCB, HTTPS) —
        // `smithy_query(query="shape",shape="RRType")`. `ANY` is not one of them.
        BadRequest {
            case: "a record type outside the RRType value set".to_string(),
            method: "GET",
            uri: format!("/2013-04-01/hostedzone/{zone_id}/rrset?type=ANY"),
            body: String::new(),
            member: "startRecordType",
        },
        // `ListHostedZones$Marker` targets `PageMarker`, len 0..64.
        BadRequest {
            case: "a marker one character past its length maximum".to_string(),
            method: "GET",
            uri: format!("/2013-04-01/hostedzone?marker={}", filler(65)),
            body: String::new(),
            member: "marker",
        },
        // `ListHostedZones$DelegationSetId` targets `ResourceId`, len 0..32.
        BadRequest {
            case: "a delegationsetid one character past its length maximum".to_string(),
            method: "GET",
            uri: format!("/2013-04-01/hostedzone?delegationsetid={}", filler(33)),
            body: String::new(),
            member: "delegationSetId",
        },
        // `ListResourceRecordSets$StartRecordIdentifier`, len 1..128 — a different
        // operation, and a bound on a member the previous rows never touch.
        BadRequest {
            case: "a record-set identifier past its length maximum".to_string(),
            method: "GET",
            uri: format!(
                "/2013-04-01/hostedzone/{zone_id}/rrset?identifier={}",
                filler(129)
            ),
            body: String::new(),
            member: "startRecordIdentifier",
        },
        // `ListGeoLocations$StartContinentCode`, len 2..2 — a MINIMUM, on an operation
        // this emulator has not implemented yet: the boundary refuses in front of the
        // handler, so the classification is the same there.
        BadRequest {
            case: "a continent code one character short of its length minimum".to_string(),
            method: "GET",
            uri: "/2013-04-01/geolocations?startcontinentcode=E".to_string(),
            body: String::new(),
            member: "startContinentCode",
        },
        BadRequest {
            case: "a continent code one character past its length maximum".to_string(),
            method: "GET",
            uri: "/2013-04-01/geolocations?startcontinentcode=EUR".to_string(),
            body: String::new(),
            member: "startContinentCode",
        },
        // `ListHostedZonesByName$HostedZoneId` targets `ResourceId`, len 0..32.
        BadRequest {
            case: "a hostedzoneid past its length maximum".to_string(),
            method: "GET",
            uri: format!("/2013-04-01/hostedzonesbyname?hostedzoneid={}", filler(33)),
            body: String::new(),
            member: "hostedZoneId",
        },
        // A required member the request leaves out is the same class of refusal to a
        // caller, and it travels in the BODY rather than the query string.
        BadRequest {
            case: "a create with no CallerReference at all".to_string(),
            method: "POST",
            uri: "/2013-04-01/hostedzone".to_string(),
            body: "<CreateHostedZoneRequest \
                   xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
                   <Name>missing-ref.example.com</Name></CreateHostedZoneRequest>"
                .to_string(),
            member: "callerReference",
        },
        BadRequest {
            case: "a create with no Name at all".to_string(),
            method: "POST",
            uri: "/2013-04-01/hostedzone".to_string(),
            body: "<CreateHostedZoneRequest \
                   xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
                   <CallerReference>ref-no-name</CallerReference></CreateHostedZoneRequest>"
                .to_string(),
            member: "name",
        },
    ];

    for row in &rows {
        let reply = call(&router, row.method, &row.uri, &row.body).await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected 400, got {} with body {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            INVALID_INPUT,
            "case {:?}: expected {INVALID_INPUT}, got body {}",
            row.case,
            reply.body
        );
        let message = reply.first("Error/Message").unwrap_or_default();
        // The message is built FROM the request: the per-member envelope naming the
        // member and the requirement its value failed.
        assert!(
            message.starts_with("1 validation error detected: ")
                && message.contains("failed to satisfy constraint: "),
            "case {:?}: the refusal must state the per-member envelope, got {message:?}",
            row.case
        );
        assert!(
            message
                .to_lowercase()
                .contains(&format!("at '{}'", row.member).to_lowercase()),
            "case {:?}: the refusal must name the member it was about ({}), got {message:?}",
            row.case,
            row.member
        );
        assert!(
            !reply.body.contains("ValidationException"),
            "case {:?}: the refusal must not name an error shape this model does not \
             declare, got {}",
            row.case,
            reply.body
        );
    }

    // The whole message for one row, to pin the envelope the clauses sit in: the value
    // the request sent, the member path it sat at, and the requirement it failed.
    let refused = call(
        &router,
        "GET",
        "/2013-04-01/hostedzone?hostedzonetype=SharedHostedZone",
        "",
    )
    .await;
    assert_eq!(
        refused.first("Error/Message"),
        Some(
            "1 validation error detected: Value 'SharedHostedZone' at 'hostedZoneType' failed \
             to satisfy constraint: Member must satisfy enum value set: [PrivateHostedZone]"
        ),
        "the refusal must state the value, the member path and the requirement, got {:?}",
        refused.first("Error/Message")
    );
}

#[tokio::test]
async fn a_request_inside_the_models_rules_is_not_refused() {
    // The other half of the same rule: the classification changed, not what is refused.
    // Each row is a value the model DOES admit — the enum's one member, a marker exactly
    // at its length maximum, a record type inside `RRType` — and must be served.
    let router = router();
    let zone_id = create_zone_ok(&router, "accepted.example.com", "ref-accepted").await;

    let rows = [
        GoodRequest {
            case: "the hostedzonetype the enum names",
            uri: "/2013-04-01/hostedzone?hostedzonetype=PrivateHostedZone".to_string(),
            status: 200,
        },
        GoodRequest {
            case: "a marker exactly at its length maximum",
            uri: format!("/2013-04-01/hostedzone?marker={}", filler(64)),
            status: 200,
        },
        GoodRequest {
            case: "no optional query member at all",
            uri: "/2013-04-01/hostedzone".to_string(),
            status: 200,
        },
        GoodRequest {
            case: "a record type inside the RRType value set",
            uri: format!("/2013-04-01/hostedzone/{zone_id}/rrset?type=NS&name=accepted.example.com"),
            status: 200,
        },
        GoodRequest {
            case: "a record-set identifier exactly at its length maximum",
            uri: format!(
                "/2013-04-01/hostedzone/{zone_id}/rrset?identifier={}&type=A&name=a.accepted.example.com",
                filler(128)
            ),
            status: 200,
        },
    ];

    for row in &rows {
        let reply = call(&router, "GET", &row.uri, "").await;
        assert_eq!(
            reply.status, row.status,
            "case {:?}: expected {}, got {} with body {}",
            row.case, row.status, reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn an_unknown_change_id_is_reported_in_awss_resource_lookup_form() {
    // The rule: a change id that names no change is refused `NoSuchChange` (404) with the
    // message built FROM the request — `Could not find resource with ID: <the id the
    // request submitted>` — so a caller can read back WHICH id was missing. The rows span
    // the id's form and length: the minted shape, a shorter and a longer well-formed id,
    // a lowercase one, and one that looks nothing like a change id.
    let router = router();
    let ids = [
        "C00000000000000",
        "C1234567890ABCDEF12345",
        "CABC123",
        "c00000000000000",
        "BOGUS",
        "0",
    ];

    for id in ids {
        let reply = call(&router, "GET", &format!("/2013-04-01/change/{id}"), "").await;
        assert_eq!(
            reply.status, 404,
            "case {id:?}: expected 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchChange",
            "case {id:?}: expected NoSuchChange, got {}",
            reply.body
        );
        assert_eq!(
            reply.first("Error/Message"),
            Some(format!("Could not find resource with ID: {id}").as_str()),
            "case {id:?}: the message must name the submitted id in AWS's resource-lookup \
             form, got {:?}",
            reply.first("Error/Message")
        );
    }
}

#[tokio::test]
async fn a_change_id_that_names_a_change_reads_it_back() {
    // The refusal above is read off state, not off the id's shape: the change ids a
    // mutating call hands back are served, in both the bare and the `/change/`-prefixed
    // form, while the ids beside them are not. The rows differ in the operation that
    // minted the change.
    let router = router();
    let created = call(
        &router,
        "POST",
        "/2013-04-01/hostedzone",
        &create_zone_body("minted.example.com", "ref-minted", ""),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "the zone must be created, got {} with body {}",
        created.status, created.body
    );
    let change_id = created
        .first("ChangeInfo/Id")
        .unwrap_or_else(|| panic!("the create must report a change; body {}", created.body))
        .to_string();
    let bare = change_id.trim_start_matches("/change/").to_string();

    for (case, form) in [("the bare id", bare.clone()), ("the prefixed id", change_id)] {
        let reply = call(
            &router,
            "GET",
            &format!("/2013-04-01/change/{}", form.replace('/', "%2F")),
            "",
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.first("ChangeInfo/Status").is_some(),
            "case {case:?}: the change must report its status, got {}",
            reply.body
        );
    }

    // One character off the minted id names no change, and says so about THAT id.
    let neighbour = format!("{bare}X");
    let missing = call(
        &router,
        "GET",
        &format!("/2013-04-01/change/{neighbour}"),
        "",
    )
    .await;
    assert_eq!(
        missing.status, 404,
        "a neighbouring id must name no change, got {} with body {}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.first("Error/Message"),
        Some(format!("Could not find resource with ID: {neighbour}").as_str()),
        "the message must name the submitted id, got {:?}",
        missing.first("Error/Message")
    );
}
