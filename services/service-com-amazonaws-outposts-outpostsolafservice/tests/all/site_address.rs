// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two AWS Outposts site-address operations
//! (`UpdateSiteAddress`, `GetSiteAddress`), driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — so what
//! it asserts is the emulator's HTTP behaviour, not a handler called directly. The
//! request harness is the lifecycle suite's, reused rather than rebuilt.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A site holds one address per value of the modelled `AddressType`. Writing one
//!    stores the WHOLE `Address` the request carried in that type's slot, replacing
//!    whatever was there, leaves the other type's slot alone, and answers the address
//!    as stored under the type it was stored as — which is exactly what every later
//!    read of that type answers. A site carrying no address of a type reports the
//!    optional `Address` member absent rather than failing.
//! 2. Writing the OPERATING address re-derives the three members `Site` projects from
//!    it (`OperatingAddress{CountryCode,StateOrRegion,City}`) and changes nothing else
//!    about the site; writing the SHIPPING address changes no member of `Site` at all.
//! 3. The operating address is where the site itself is, so AWS accepts a change to it
//!    only "before you place an order at the site, or after all Outposts that belong to
//!    the site have been deactivated": while an Outpost belongs to the site the change
//!    is refused 409 `ConflictException`, naming that Outpost, and nothing is written.
//!    A shipping-address change is not held up by it.
//! 4. An identifier that names nothing is 404 `NotFoundException`; one that is no
//!    identifier at all, and a request outside the model's own constraints, is 400
//!    `ValidationException` — on both routes. (A `GET` that carries no `AddressType`
//!    query parameter at all matches no route, so the boundary answers it before this
//!    operation exists to it; that is the framework's routing, not this rule.)
//!
//! The rows differ along the address type (each modelled value), which optional members
//! the address carries (none, every one, one again without them), the length of each
//! member (the modelled minimum and maximum of every one), the country/state/city, the
//! identifier form the request names the site by (bare id and full ARN), and whether the
//! site already held an address of that type.

use serde_json::{json, Value};

use super::site_and_outpost_lifecycle::{
    address, query_value, refusal, segment, text, Outposts, ABSENT_SITE_ID, SITE_ARN_PREFIX,
};

/// Every value the modelled `AddressType` enum declares.
///
/// Source: `smithy_query(query="shape", shape="AddressType")` against this service's
/// model — `["SHIPPING_ADDRESS", "OPERATING_ADDRESS"]`. Every row below sends one of
/// these and asserts the answer echoes that same value, so the two constants the
/// service words its slots by cannot drift from the model's value set without a test
/// here failing; a value outside the set is refused (see the refusal table).
const ADDRESS_TYPES: [&str; 2] = ["OPERATING_ADDRESS", "SHIPPING_ADDRESS"];

/// The `AddressType` naming the address a site is at.
const OPERATING: &str = "OPERATING_ADDRESS";
/// The `AddressType` naming the address a site's hardware ships to.
const SHIPPING: &str = "SHIPPING_ADDRESS";

/// The three members `Site` projects from the site's OPERATING address, each paired
/// with the `Address` member it is read from.
///
/// Source: `smithy_query(query="shape", shape="Site")` — `OperatingAddressCountryCode`
/// (`CountryCode`), `OperatingAddressStateOrRegion` (`StateOrRegion`) and
/// `OperatingAddressCity` (`City`).
const OPERATING_PROJECTION: [(&str, &str); 3] = [
    ("OperatingAddressCountryCode", "CountryCode"),
    ("OperatingAddressStateOrRegion", "StateOrRegion"),
    ("OperatingAddressCity", "City"),
];

// The whole-`Address` baseline every row below starts from is the lifecycle suite's
// shared [`address`] helper: the members the modelled `Address` marks `@required`, at
// the given country/state/city and with no optional member at all.
//
// Source for the member set and for every bound the rows below sit on:
// `smithy_query(query="shape", shape="Address")` — `ContactName` (len 1..255),
// `ContactPhoneNumber` (len 1..20, pattern `^\+[1-9][0-9]{1,18}$`), `AddressLine1`
// (len 1..180), `City` (1..50), `StateOrRegion` (1..50), `PostalCode` (1..20,
// `^[a-zA-Z0-9 -]+$`) and `CountryCode` (len 2..2, `^[A-Z]{2}$`) required;
// `AddressLine2` / `AddressLine3` (0..60), `DistrictOrCounty` (1..60) and
// `Municipality` (0..180) optional.

/// `base` with every member of `patch` written over it: how a row states "the same
/// address, plus these members".
fn with(base: &Value, patch: Value) -> Value {
    let mut merged = base.as_object().cloned().unwrap_or_default();
    for (member, value) in patch.as_object().into_iter().flatten() {
        merged.insert(member.clone(), value.clone());
    }
    Value::Object(merged)
}

/// `PUT /sites/{SiteId}/address`, returning the whole output it answered with.
async fn update_site_address(
    outposts: &Outposts,
    identifier: &str,
    address_type: &str,
    address: &Value,
) -> Value {
    outposts
        .ok(
            "PUT",
            &format!("/sites/{}/address", segment(identifier)),
            Some(&json!({ "AddressType": address_type, "Address": address })),
        )
        .await
}

/// `GET /sites/{SiteId}/address?AddressType=…`, returning the whole output.
async fn get_site_address(outposts: &Outposts, identifier: &str, address_type: &str) -> Value {
    outposts
        .ok(
            "GET",
            &format!(
                "/sites/{}/address?AddressType={}",
                segment(identifier),
                query_value(address_type)
            ),
            None,
        )
        .await
}

/// What a read of `address_type` must answer: the site's bare id, the type that was
/// asked for, and the address stored for it — with the member absent when none is.
fn expected_read(site_id: &str, address_type: &str, stored: Option<&Value>) -> Value {
    let mut output = json!({ "SiteId": site_id, "AddressType": address_type });
    if let Some(address) = stored {
        output["Address"] = address.clone();
    }
    output
}

/// Rules 1 + 2, over a table of addresses applied in sequence to ONE site: each row's
/// whole address lands in its own type's slot, the other type's slot is untouched, both
/// reads answer what was stored, and the `Site` projection follows the operating address
/// while nothing else about the site moves.
#[tokio::test]
async fn site_addresses_round_trip_per_type_and_project_into_the_site() {
    let outposts = Outposts::new();
    let created = outposts
        .create_site(&json!({
            "Name": "address site",
            "Description": "created description",
            "Notes": "created notes",
            "Tags": { "Env": "dev" },
        }))
        .await;
    let site_id = text(&created, "SiteId");
    let site_arn = text(&created, "SiteArn");

    // Every optional member the modelled `Address` declares, at a plain value.
    let optional = json!({
        "AddressLine2": "Floor 7",
        "AddressLine3": "Cage 12",
        "DistrictOrCounty": "King",
        "Municipality": "Seattle",
    });
    // Each member at the modelled MAXIMUM its shape declares. `ContactPhoneNumber`'s
    // maximum is both its `length(max: 20)` and the longest string its pattern
    // `^\+[1-9][0-9]{1,18}$` admits: `+`, a leading non-zero digit and 18 more.
    let at_maximum = json!({
        "ContactName": "n".repeat(255),
        "ContactPhoneNumber": format!("+1{}", "0".repeat(18)),
        "AddressLine1": "a".repeat(180),
        "AddressLine2": "b".repeat(60),
        "AddressLine3": "c".repeat(60),
        "City": "y".repeat(50),
        "StateOrRegion": "s".repeat(50),
        "DistrictOrCounty": "d".repeat(60),
        "PostalCode": "p".repeat(20),
        "CountryCode": "ZA",
        "Municipality": "m".repeat(180),
    });
    // Each member at the modelled MINIMUM its shape declares. The optional members
    // whose `length(min: 0)` the pattern `^\S[\S ]*$` contradicts sit at one
    // character, the shortest value that satisfies both.
    let at_minimum = json!({
        "ContactName": "A",
        "ContactPhoneNumber": "+12",
        "AddressLine1": "B",
        "AddressLine2": "C",
        "AddressLine3": "D",
        "City": "E",
        "StateOrRegion": "F",
        "DistrictOrCounty": "G",
        "PostalCode": "0",
        "CountryCode": "DE",
        "Municipality": "H",
    });

    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "operating, required members only, by id",
            OPERATING,
            site_id.as_str(),
            address("US", "WA", "Seattle"),
        ),
        (
            "shipping, required members only, by id",
            SHIPPING,
            site_id.as_str(),
            address("US", "OR", "Portland"),
        ),
        (
            "operating, every optional member too, named by ARN",
            OPERATING,
            site_arn.as_str(),
            with(&address("US", "WA", "Seattle"), optional.clone()),
        ),
        (
            "shipping, every optional member too, named by ARN",
            SHIPPING,
            site_arn.as_str(),
            with(&address("IE", "Leinster", "Dublin"), optional),
        ),
        (
            "operating, every member at its modelled maximum",
            OPERATING,
            site_id.as_str(),
            at_maximum,
        ),
        (
            "shipping, every member at its modelled minimum",
            SHIPPING,
            site_id.as_str(),
            at_minimum,
        ),
        (
            "operating, replaced again in another country, optional members dropped",
            OPERATING,
            site_id.as_str(),
            address("JP", "Tokyo", "Chiyoda"),
        ),
        (
            "shipping, replaced again, optional members dropped",
            SHIPPING,
            site_arn.as_str(),
            address("BR", "Sao Paulo", "Osasco"),
        ),
    ];

    let mut stored: [Option<Value>; 2] = [None, None];
    for (case, address_type, identifier, address) in &cases {
        let answered = update_site_address(&outposts, identifier, address_type, address).await;
        assert_eq!(
            answered,
            json!({ "AddressType": address_type, "Address": address }),
            "case {case:?}: UpdateSiteAddress must answer the address it stored under the type it stored it as, got {answered:?}"
        );

        let slot = ADDRESS_TYPES
            .iter()
            .position(|known| known == address_type)
            .expect("every case names a modelled AddressType");
        stored[slot] = Some(address.clone());

        // Both types are read back after every row, so a write that landed in the
        // wrong slot — or leaked into the other one — shows up immediately, by
        // either identifier form.
        for (index, known) in ADDRESS_TYPES.iter().enumerate() {
            let expected = expected_read(&site_id, known, stored[index].as_ref());
            for named_by in [site_id.as_str(), site_arn.as_str()] {
                let read = get_site_address(&outposts, named_by, known).await;
                assert_eq!(
                    read, expected,
                    "case {case:?}: GetSiteAddress({known}) by {named_by} must answer the stored address, got {read:?}"
                );
            }
        }

        // Rule 2: the site itself carries the projection of the OPERATING address and
        // is otherwise exactly the record the create answered.
        let mut expected_site = created.clone();
        for (projected, member) in OPERATING_PROJECTION {
            if let Some(operating) = stored[0].as_ref() {
                expected_site[projected] = operating[member].clone();
            }
        }
        let site = outposts.get_site(&site_id).await;
        assert_eq!(
            site, expected_site,
            "case {case:?}: the site must project the operating address and change nothing else, got {site:?}"
        );
    }
}

/// Rule 1's absent-address half: a site that holds no address of the requested type is
/// reported with the optional `Address` member absent — for a site created with neither
/// address and for one created with only the other — and the first write of that type
/// makes it appear.
#[tokio::test]
async fn a_type_the_site_holds_no_address_for_is_reported_absent() {
    let outposts = Outposts::new();
    let bare = outposts.create_site(&json!({ "Name": "no addresses" })).await;
    let bare_id = text(&bare, "SiteId");
    let operating_only = outposts
        .create_site(&json!({
            "Name": "operating only",
            "OperatingAddress": address("US", "WA", "Seattle"),
        }))
        .await;
    let operating_only_id = text(&operating_only, "SiteId");

    let cases: Vec<(&str, &str, &str)> = vec![
        ("no address at all, operating", bare_id.as_str(), OPERATING),
        ("no address at all, shipping", bare_id.as_str(), SHIPPING),
        (
            "operating address only, shipping asked for",
            operating_only_id.as_str(),
            SHIPPING,
        ),
    ];
    for (case, site, address_type) in &cases {
        let read = get_site_address(&outposts, site, address_type).await;
        assert_eq!(
            read,
            expected_read(site, address_type, None),
            "case {case:?}: a site with no address of that type reports the member absent, got {read:?}"
        );
    }

    // The create's own address is readable the same way, and the first write of the
    // other type makes that one appear.
    let created_operating = address("US", "WA", "Seattle");
    let read = get_site_address(&outposts, &operating_only_id, OPERATING).await;
    assert_eq!(
        read,
        expected_read(&operating_only_id, OPERATING, Some(&created_operating)),
        "case \"address stored by CreateSite\": GetSiteAddress must answer it, got {read:?}"
    );
    let first = address("US", "OR", "Portland");
    update_site_address(&outposts, &operating_only_id, SHIPPING, &first).await;
    let read = get_site_address(&outposts, &operating_only_id, SHIPPING).await;
    assert_eq!(
        read,
        expected_read(&operating_only_id, SHIPPING, Some(&first)),
        "case \"first shipping address\": the written address must be readable, got {read:?}"
    );
}

/// Rule 3: while an Outpost belongs to the site, a change to the OPERATING address is
/// refused 409 `ConflictException` naming that Outpost, and the stored address is left
/// as it was; a change to the SHIPPING address goes through. Once the Outpost is gone
/// the operating change is accepted and reads back.
#[tokio::test]
async fn the_operating_address_is_held_while_an_outpost_belongs_to_the_site() {
    let outposts = Outposts::new();
    let before = address("US", "WA", "Seattle");
    let site = outposts
        .create_site(&json!({
            "Name": "occupied site",
            "OperatingAddress": before,
        }))
        .await;
    let site_id = text(&site, "SiteId");
    let site_arn = text(&site, "SiteArn");
    let outpost = outposts
        .create_outpost(&json!({ "Name": "resident", "SiteId": site_id }))
        .await;
    let outpost_id = text(&outpost, "OutpostId");

    let attempt = address("IE", "Leinster", "Dublin");
    // Both identifier forms are refused the same way: the conflict is about the site's
    // Outposts, not about how the request spelled the site.
    for named_by in [site_id.as_str(), site_arn.as_str()] {
        let reply = outposts
            .call(
                "PUT",
                &format!("/sites/{}/address", segment(named_by)),
                Some(&json!({ "AddressType": OPERATING, "Address": attempt })),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (409, "ConflictException"),
            "case {named_by:?}: an operating-address change must be refused while an Outpost belongs to the site, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            (
                text(&reply.body, "ResourceType"),
                text(&reply.body, "ResourceId")
            ),
            ("OUTPOST".to_string(), outpost_id.clone()),
            "case {named_by:?}: the conflict must name the Outpost holding the address, got {:?}",
            reply.body
        );
    }
    let read = get_site_address(&outposts, &site_id, OPERATING).await;
    assert_eq!(
        read,
        expected_read(&site_id, OPERATING, Some(&before)),
        "case \"refused operating change\": the stored address must be untouched, got {read:?}"
    );

    // The shipping address is not held up by an Outpost at the site.
    let shipping = address("US", "OR", "Portland");
    update_site_address(&outposts, &site_id, SHIPPING, &shipping).await;
    let read = get_site_address(&outposts, &site_id, SHIPPING).await;
    assert_eq!(
        read,
        expected_read(&site_id, SHIPPING, Some(&shipping)),
        "case \"shipping change with an Outpost at the site\": it must be stored, got {read:?}"
    );

    // With no Outpost left at the site the operating change is accepted, and the site's
    // projection follows it.
    outposts
        .ok("DELETE", &format!("/outposts/{outpost_id}"), None)
        .await;
    let answered = update_site_address(&outposts, &site_id, OPERATING, &attempt).await;
    assert_eq!(
        answered,
        json!({ "AddressType": OPERATING, "Address": attempt }),
        "case \"operating change after the Outpost is gone\": it must be accepted, got {answered:?}"
    );
    let read = get_site_address(&outposts, &site_id, OPERATING).await;
    assert_eq!(
        read,
        expected_read(&site_id, OPERATING, Some(&attempt)),
        "case \"operating change after the Outpost is gone\": it must read back, got {read:?}"
    );
    let site = outposts.get_site(&site_id).await;
    for (projected, member) in OPERATING_PROJECTION {
        assert_eq!(
            site.get(projected),
            attempt.get(member),
            "case \"operating change after the Outpost is gone\": Site must project {projected} from the new address, got {site:?}"
        );
    }
}

/// Rule 4 on both site-address routes: a well-formed identifier that names no site is
/// 404 `NotFoundException` (in either identifier form), a value that is no site
/// identifier is 400 `ValidationException`, and so is a request the model's own
/// constraints refuse — a missing required `Address` member, an `AddressType` outside
/// the modelled enum, and a missing required `AddressType`.
#[tokio::test]
async fn site_address_calls_on_absent_or_malformed_requests_are_refused() {
    let outposts = Outposts::new();
    let absent_site_arn = format!("{SITE_ARN_PREFIX}{ABSENT_SITE_ID}");
    let valid = address("US", "WA", "Seattle");

    let cases = vec![
        refusal(
            "GetSiteAddress on an id nothing holds",
            "GET",
            format!("/sites/{ABSENT_SITE_ID}/address?AddressType={OPERATING}"),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetSiteAddress on an ARN nothing holds",
            "GET",
            format!(
                "/sites/{}/address?AddressType={SHIPPING}",
                segment(&absent_site_arn)
            ),
            404,
            "NotFoundException",
        ),
        refusal(
            "UpdateSiteAddress on an id nothing holds",
            "PUT",
            format!("/sites/{ABSENT_SITE_ID}/address"),
            404,
            "NotFoundException",
        )
        .with_body(json!({ "AddressType": OPERATING, "Address": valid })),
        refusal(
            "UpdateSiteAddress on an ARN nothing holds",
            "PUT",
            format!("/sites/{}/address", segment(&absent_site_arn)),
            404,
            "NotFoundException",
        )
        .with_body(json!({ "AddressType": SHIPPING, "Address": valid })),
        refusal(
            "GetSiteAddress on a value that is no site identifier",
            "GET",
            format!("/sites/not-a-site-id/address?AddressType={OPERATING}"),
            400,
            "ValidationException",
        ),
        refusal(
            "GetSiteAddress on an id whose hex is too short",
            "GET",
            format!("/sites/os-0000000000000000/address?AddressType={OPERATING}"),
            400,
            "ValidationException",
        ),
        refusal(
            "GetSiteAddress with an AddressType outside the modelled enum",
            "GET",
            format!("/sites/{ABSENT_SITE_ID}/address?AddressType=BILLING_ADDRESS"),
            400,
            "ValidationException",
        ),
        refusal(
            "UpdateSiteAddress on a value that is no site identifier",
            "PUT",
            "/sites/not-a-site-id/address".to_string(),
            400,
            "ValidationException",
        )
        .with_body(json!({ "AddressType": OPERATING, "Address": valid })),
        refusal(
            "UpdateSiteAddress with an AddressType outside the modelled enum",
            "PUT",
            format!("/sites/{ABSENT_SITE_ID}/address"),
            400,
            "ValidationException",
        )
        .with_body(json!({ "AddressType": "BILLING_ADDRESS", "Address": valid })),
        refusal(
            "UpdateSiteAddress with no AddressType",
            "PUT",
            format!("/sites/{ABSENT_SITE_ID}/address"),
            400,
            "ValidationException",
        )
        .with_body(json!({ "Address": valid })),
        refusal(
            "UpdateSiteAddress with no Address",
            "PUT",
            format!("/sites/{ABSENT_SITE_ID}/address"),
            400,
            "ValidationException",
        )
        .with_body(json!({ "AddressType": OPERATING })),
        refusal(
            "UpdateSiteAddress with an Address missing a required member",
            "PUT",
            format!("/sites/{ABSENT_SITE_ID}/address"),
            400,
            "ValidationException",
        )
        .with_body(json!({
            "AddressType": OPERATING,
            "Address": { "ContactName": "Sam Site", "City": "Seattle" },
        })),
        refusal(
            "UpdateSiteAddress with a CountryCode outside the modelled pattern",
            "PUT",
            format!("/sites/{ABSENT_SITE_ID}/address"),
            400,
            "ValidationException",
        )
        .with_body(json!({
            "AddressType": OPERATING,
            "Address": with(&valid, json!({ "CountryCode": "usa" })),
        })),
    ];

    outposts.expect_refusals(&cases).await;

    // A refused call created nothing: the site the cases named still does not exist.
    let reply = outposts
        .call(
            "GET",
            &format!("/sites/{ABSENT_SITE_ID}/address?AddressType={OPERATING}"),
            None,
        )
        .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (404, "NotFoundException"),
        "a refused site-address call must not have created the site, got {} {:?}",
        reply.status,
        reply.body
    );
}
