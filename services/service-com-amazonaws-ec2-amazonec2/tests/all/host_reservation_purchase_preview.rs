// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetHostReservationPurchasePreview`, driven over the wire the service
//! actually speaks (`ec2Query`, through the crate's shared [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. THE PREVIEW PRICES THE PUBLISHED OFFERING for the Dedicated Hosts the request names:
//!    the purchase it reports carries that offering's term, instance family, payment option
//!    and prices, and every host the request named; the totals are those prices over the
//!    hosts the reservation would cover, and the currency is the one the offerings are quoted
//!    in. The offering is the one `DescribeHostReservationOfferings` publishes under that id,
//!    so a preview and the catalogue cannot quote one offering differently;
//! 2. IT PURCHASES NOTHING: "this is a preview of the PurchaseHostReservation action and does
//!    not result in the offering being purchased" — no reservation id is reported, nothing is
//!    stored, and two previews of one request answer the same prices;
//! 3. "YOU MUST HAVE ACTIVE DEDICATED HOSTS IN YOUR ACCOUNT": every host id this operation
//!    cannot resolve to a host the account holds — a value outside the host id form, one
//!    inside it the account holds no host under, and one it has released — is refused
//!    `InvalidHostId.Malformed`;
//! 4. THE CONFIGURATIONS MUST MATCH: an offering id no published offering carries, and one
//!    for another instance family than the hosts', are each refused `InvalidParameterValue`.
//!
//! Each table row is a distinct input along what its rule generalises over — the instance
//! family, the payment option and the term of the offering, how many hosts the reservation
//! covers, and every way a host id or an offering can fail to be one this preview can price
//! — so no row is the specification.

use ec2::ops::describe_host_reservation_offerings::{
    HOST_OFFERING_CURRENCY_CODE, format_price_milli, host_offering_by_id,
};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, indexed, member};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The zone the hosts this suite allocates sit in.
const ZONE: &str = "us-east-1a";

/// A host id inside the form EC2 issues that this account holds no Dedicated Host under.
const UNHELD_HOST_ID: &str = "h-0123456789abcdef0";

/// Allocate `quantity` Dedicated Hosts of `instance_family`, answering their ids.
async fn allocate(
    router: &MultiServiceRouter,
    instance_family: &str,
    quantity: usize,
) -> Vec<String> {
    let params = vec![
        param("AvailabilityZone", ZONE),
        param("InstanceFamily", instance_family),
        param("Quantity", &quantity.to_string()),
    ];
    let (status, body) = call(router, REGION, "AllocateHosts", &params).await;
    assert_eq!(
        status, 200,
        "AllocateHosts must succeed, got {status} with body {body}"
    );
    let ids = super::ec2_query::elements(&body, "item");
    assert_eq!(
        ids.len(),
        quantity,
        "AllocateHosts must report the {quantity} host(s) it allocated: {body}"
    );
    ids
}

/// The id of a published offering for `instance_family` on `payment_option` whose term is
/// `duration_seconds` — read out of the catalogue the describe publishes, so this suite names
/// no id of its own.
async fn published_offering(
    router: &MultiServiceRouter,
    instance_family: &str,
    payment_option: &str,
    duration_seconds: i32,
) -> String {
    let mut params = vec![
        param("MinDuration", &duration_seconds.to_string()),
        param("MaxDuration", &duration_seconds.to_string()),
    ];
    params.extend(super::ec2_query::filters(&[
        ("instance-family", &[instance_family]),
        ("payment-option", &[payment_option]),
    ]));
    let (status, body) = call(
        router,
        REGION,
        "DescribeHostReservationOfferings",
        &params,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeHostReservationOfferings must succeed, got {status} with body {body}"
    );
    super::ec2_query::elements(&body, "offeringId")
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!("the catalogue must publish an offering for {instance_family}: {body}")
        })
}

/// One `GetHostReservationPurchasePreview` call naming `host_ids` and `offering_id`.
async fn preview(
    router: &MultiServiceRouter,
    host_ids: &[&str],
    offering_id: &str,
) -> (u16, String) {
    let mut params: Vec<Param> = indexed("HostIdSet", host_ids);
    params.push(param("OfferingId", offering_id));
    call(
        router,
        REGION,
        "GetHostReservationPurchasePreview",
        &params,
    )
    .await
}

/// One preview that must be served.
async fn previewed(
    router: &MultiServiceRouter,
    case: &str,
    host_ids: &[&str],
    offering_id: &str,
) -> String {
    let (status, body) = preview(router, host_ids, offering_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetHostReservationPurchasePreview must succeed, got {status}: {body}"
    );
    body
}

/// How many Dedicated Host reservations the store holds — a preview must leave this at zero,
/// since nothing in this emulator stores one yet.
fn stored_hosts(engine: &engine::InMemoryEngine) -> usize {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store.dedicated_hosts.len()
}

/// RULE 1 and RULE 2: the preview prices the published offering over the hosts the request
/// names, and purchases nothing.
#[tokio::test]
async fn the_preview_prices_the_published_offering_over_the_named_hosts() {
    // Rows differ in the instance family, the payment option (which decides how the one term
    // total is split between upfront and hourly), the term, and how many hosts the
    // reservation covers.
    let one_year = 365 * 24 * 60 * 60;
    let rows: [(&str, &str, &str, i32, usize); 4] = [
        ("one m5 host, All Upfront, one year", "m5", "AllUpfront", one_year, 1),
        (
            "two m5 hosts, No Upfront, one year",
            "m5",
            "NoUpfront",
            one_year,
            2,
        ),
        (
            "three c5 hosts, Partial Upfront, one year",
            "c5",
            "PartialUpfront",
            one_year,
            3,
        ),
        (
            "one t3 host, All Upfront, three years",
            "t3",
            "AllUpfront",
            3 * one_year,
            1,
        ),
    ];

    for (case, instance_family, payment_option, duration_seconds, hosts) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let host_ids = allocate(&router, instance_family, hosts).await;
        let named: Vec<&str> = host_ids.iter().map(String::as_str).collect();
        let offering_id =
            published_offering(&router, instance_family, payment_option, duration_seconds).await;
        let offering = host_offering_by_id(&offering_id)
            .unwrap_or_else(|| panic!("case {case:?}: the catalogue published {offering_id}"));

        let body = previewed(&router, case, &named, &offering_id).await;

        assert_eq!(
            member(&body, "currencyCode").as_deref(),
            Some(HOST_OFFERING_CURRENCY_CODE),
            "case {case:?}: expected the currency the offerings are quoted in, got {body}"
        );
        assert_eq!(
            super::ec2_query::element(&body, "instanceFamily").as_deref(),
            Some(instance_family),
            "case {case:?}: the purchase must cover the hosts' instance family, got {body}"
        );
        assert_eq!(
            super::ec2_query::element(&body, "paymentOption").as_deref(),
            Some(payment_option),
            "case {case:?}: the purchase must carry the offering's payment option, got {body}"
        );
        assert_eq!(
            super::ec2_query::element(&body, "duration").as_deref(),
            Some(duration_seconds.to_string().as_str()),
            "case {case:?}: the purchase must carry the offering's term, got {body}"
        );
        assert_eq!(
            super::ec2_query::element(&body, "hourlyPrice").as_deref(),
            Some(format_price_milli(offering.hourly_price_milli).as_str()),
            "case {case:?}: the purchase must carry the offering's hourly price, got {body}"
        );
        assert_eq!(
            super::ec2_query::element(&body, "upfrontPrice").as_deref(),
            Some(format_price_milli(offering.upfront_price_milli).as_str()),
            "case {case:?}: the purchase must carry the offering's upfront price, got {body}"
        );

        // Every host the request named is covered by the one reservation.
        let covered = super::ec2_query::elements(&body, "hostIdSet")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected a hostIdSet in {body}"));
        for host_id in &host_ids {
            assert!(
                covered.contains(host_id.as_str()),
                "case {case:?}: expected the purchase to cover {host_id}, got {covered}"
            );
        }

        // The TOTALS: the offering's prices over the hosts the reservation would cover.
        let hosts = i64::try_from(hosts).expect("a small count");
        assert_eq!(
            member(&body, "totalHourlyPrice").as_deref(),
            Some(format_price_milli(offering.hourly_price_milli * hosts).as_str()),
            "case {case:?}: expected the hourly price over every host, got {body}"
        );
        assert_eq!(
            member(&body, "totalUpfrontPrice").as_deref(),
            Some(format_price_milli(offering.upfront_price_milli * hosts).as_str()),
            "case {case:?}: expected the upfront price over every host, got {body}"
        );

        // RULE 2: nothing was purchased — no reservation id, and the store is as it was.
        assert!(
            super::ec2_query::element(&body, "hostReservationId").is_none(),
            "case {case:?}: a preview purchases nothing, so it names no reservation: {body}"
        );
        let again = previewed(&router, case, &named, &offering_id).await;
        assert_eq!(
            member(&again, "totalUpfrontPrice"),
            member(&body, "totalUpfrontPrice"),
            "case {case:?}: two previews of one request must answer the same prices"
        );
        assert_eq!(
            stored_hosts(&engine),
            usize::try_from(hosts).expect("a small count"),
            "case {case:?}: a preview must leave the account's hosts as they were"
        );
    }
}

/// RULE 3: every host id this preview cannot resolve to an active host of the account is
/// refused.
#[tokio::test]
async fn a_host_the_account_does_not_hold_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let held = allocate(&router, "m5", 1).await.remove(0);
    let released = allocate(&router, "m5", 1).await.remove(0);
    let (status, body) = call(
        &router,
        REGION,
        "ReleaseHosts",
        &indexed("HostId", &[released.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "ReleaseHosts must succeed, got {status} with body {body}"
    );
    let offering_id = published_offering(&router, "m5", "AllUpfront", 365 * 24 * 60 * 60).await;

    // The rows differ in HOW the host id fails to name an active host of the account: a
    // well-formed id nothing was allocated under, a host the account gave back, the short id
    // form, another resource's prefix, and something that is not an id at all. Every one of
    // them is refused the malformed half, this operation's own answer for an id it cannot
    // resolve.
    let rows: [(&str, Vec<&str>); 6] = [
        ("a well-formed id nothing holds", vec![UNHELD_HOST_ID]),
        ("a released host", vec![released.as_str()]),
        ("a released host beside a held one", vec![held.as_str(), released.as_str()]),
        ("the short id form", vec!["h-1234567"]),
        ("another resource's prefix", vec!["i-0123456789abcdef0"]),
        ("not an id at all", vec!["not-a-host"]),
    ];

    for (case, host_ids) in rows {
        let (status, body) = preview(&router, &host_ids, &offering_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidHostId.Malformed",
            "case {case:?}: expected InvalidHostId.Malformed, got {body}"
        );
    }

    // A request that names no host at all carries no `HostIdSet`, which the modelled
    // `@required` member is refused for before this operation runs.
    let (status, body) = preview(&router, &[], &offering_id).await;
    assert_eq!(
        status, 400,
        "no host at all: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "MalformedInput",
        "no host at all: expected the boundary validator's refusal, got {body}"
    );
}

/// RULE 4: the offering must be one the catalogue publishes, and "with configurations that
/// match those of your Dedicated Host".
#[tokio::test]
async fn an_offering_that_cannot_be_applied_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let host = allocate(&router, "m5", 1).await.remove(0);
    let other_family = published_offering(&router, "c5", "AllUpfront", 365 * 24 * 60 * 60).await;

    // Rows differ in why the offering cannot be applied: it is of another instance family
    // than the host, it is an id no published offering carries, and it is not an id at all.
    let rows: [(&str, &str); 3] = [
        ("an offering of another instance family", other_family.as_str()),
        (
            "an id no published offering carries",
            "hro-0ffffffffffffffff",
        ),
        ("not an offering id at all", "not-an-offering"),
    ];

    for (case, offering_id) in rows {
        let (status, body) = preview(&router, &[host.as_str()], offering_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}
