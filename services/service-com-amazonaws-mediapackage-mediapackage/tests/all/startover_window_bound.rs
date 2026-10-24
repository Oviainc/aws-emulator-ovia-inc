// SPDX-License-Identifier: CC0-1.0

//! `StartoverWindowSeconds` is bounded by the smallest window MediaPackage will retain.
//!
//! The rule under test: an origin endpoint's startover window is either DISABLED (the
//! member left out, or sent as the zero the service fills an absent one with) or at least
//! `state::MIN_ENABLED_STARTOVER_WINDOW_SECONDS` seconds — the minimum this service
//! publishes for the member — so a `CreateOriginEndpoint` or `UpdateOriginEndpoint`
//! carrying any other value is refused with `UnprocessableEntityException` (HTTP 422,
//! `smithy_query(query="operation",operation="CreateOriginEndpoint")`) keyed by that member
//! and stating the minimum, and persists nothing: the create stores no endpoint and the
//! update leaves the stored one exactly as it was. Every accepted value is stored and read
//! back unchanged through `DescribeOriginEndpoint`.
//!
//! The bound is the service's own, not a modelled constraint: the model targets the member
//! at the plain `__integer` and declares no `@range`
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#CreateOriginEndpointRequest",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
//! and the constants below are read off the service rather than restated, so the suite
//! cannot drift from what it enforces.
//!
//! `tests/all/origin_endpoint_durations.rs` covers the floor every OTHER
//! duration-in-seconds member shares, and `tests/all/harvest_startover.rs` what an enabled
//! window MEANS for a harvest; this suite is the member's accepted domain.
//!
//! The rows differ along what the rule generalises over: the value's sign and magnitude
//! (the smallest `__integer`, values far below and just below the minimum, the minimum
//! itself, just above it, and up to the largest `__integer`), whether the member is sent at
//! all, which operation carries it (create or update) and which packaging configuration the
//! endpoint uses. Each published bound is a row, with one step past it.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — and the harness (`call`, `emulator`, `error_code`, …) is the one
//! `tests/all/channel_lifecycle.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;

use crate::channel_lifecycle::{call, create_channel, emulator, error_code};

/// The channel every endpoint below packages.
const CHANNEL_ID: &str = "startover-bound-channel";

/// The smallest window with startover playback enabled, and so the smallest value a
/// request may carry beyond the "disabled" zero.
const MINIMUM: i32 = state::MIN_ENABLED_STARTOVER_WINDOW_SECONDS;

/// The value an absent member is stored and rendered as — startover playback disabled.
const DISABLED: i32 = state::DEFAULT_STARTOVER_WINDOW_SECONDS;

/// Values the service must REFUSE: the smallest `__integer`, the value the issue reported,
/// one step below zero, one step above the disabled zero, a span clients reach for (a
/// minute, two minutes) and the step just below the minimum.
const REFUSED: &[i32] = &[i32::MIN, -1_000, -100, -1, 1, 60, 120, MINIMUM - 1];

/// Values the service must ACCEPT and store: the disabled zero, the minimum itself, one
/// step above it, a ten-minute window, a day, the fortnight the console offers and the
/// largest the member's `__integer` can carry.
const ACCEPTED: &[i32] = &[DISABLED, MINIMUM, MINIMUM + 1, 600, 86_400, 1_209_600, i32::MAX];

/// The packaging configurations an endpoint may carry — exactly one per endpoint — so the
/// rule is asserted across them rather than on one.
const PACKAGES: &[&str] = &["hlsPackage", "dashPackage", "mssPackage", "cmafPackage"];

/// The requirement a refused value states: the member's own minimum, as the service words
/// every violated numeric bound.
fn requirement() -> String {
    format!("must be greater than or equal to {MINIMUM}")
}

/// A `CreateOriginEndpoint` body for `id` with `package`, carrying `seconds` when it is
/// `Some` and leaving the member out when it is `None`.
fn create_body(id: &str, package: &str, seconds: Option<i32>) -> Value {
    let mut body = json!({ "channelId": CHANNEL_ID, "id": id });
    body[package] = json!({});
    if let Some(seconds) = seconds {
        body["startoverWindowSeconds"] = json!(seconds);
    }
    body
}

/// An `UpdateOriginEndpoint` body with `package` and, when `seconds` is `Some`, that
/// window. `UpdateOriginEndpoint` carries the id as an `@httpLabel` and models no
/// `ChannelId`, so neither travels in its body.
fn update_body(package: &str, seconds: Option<i32>) -> Value {
    let mut body = json!({});
    body[package] = json!({});
    if let Some(seconds) = seconds {
        body["startoverWindowSeconds"] = json!(seconds);
    }
    body
}

/// The package a row uses, so the rows cycle through every packaging configuration.
fn package_for(index: usize) -> &'static str {
    PACKAGES[index % PACKAGES.len()]
}

/// Assert `refusal` is this service's 422, keyed by the member and stating the minimum.
fn assert_refused(case: &str, status: u16, refusal: &Value) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    assert_eq!(
        refusal.pointer(&format!("/errors/{}", state::STARTOVER_WINDOW_MEMBER)),
        Some(&json!(requirement())),
        "case {case:?}: the refusal must be keyed by {:?} and state the minimum, got \
         {refusal:?}",
        state::STARTOVER_WINDOW_MEMBER
    );
    assert_eq!(
        refusal.get("message").and_then(Value::as_str),
        Some(format!("{} {}", state::STARTOVER_WINDOW_MEMBER, requirement()).as_str()),
        "case {case:?}: the refusal's sentence must name the member and the minimum, got \
         {refusal:?}"
    );
}

/// A sanitised id fragment for a value, so each row addresses its own endpoint.
fn id_fragment(seconds: i32) -> String {
    format!("{seconds}").replace('-', "minus")
}

#[tokio::test]
async fn a_window_below_the_minimum_is_refused_on_create_and_stores_no_endpoint() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for (index, seconds) in REFUSED.iter().enumerate() {
        let package = package_for(index);
        let id = format!("refused-{}-{package}", id_fragment(*seconds));
        let case = format!("create startoverWindowSeconds={seconds} with {package}");
        let (status, refusal) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(&id, package, Some(*seconds))),
        )
        .await;
        assert_refused(&case, status, &refusal);

        // Nothing was persisted: the endpoint the refused create named is unreadable.
        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused create must store no endpoint, got {status} \
             {described:?}"
        );
        assert_eq!(
            error_code(&described),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {described:?}"
        );
    }
}

#[tokio::test]
async fn an_accepted_window_is_stored_and_read_back() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    // The member left out is the first row: the service fills in the disabled window and
    // reports that, which is the value a caller can send back unchanged.
    let rows: Vec<Option<i32>> = std::iter::once(None)
        .chain(ACCEPTED.iter().copied().map(Some))
        .collect();

    for (index, seconds) in rows.iter().enumerate() {
        let package = package_for(index);
        let label = seconds.map(id_fragment).unwrap_or_else(|| "absent".to_string());
        let id = format!("stored-{label}-{package}");
        let case = format!("create startoverWindowSeconds={seconds:?} with {package}");
        let (status, created) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(&id, package, *seconds)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        let expected = seconds.unwrap_or(DISABLED);
        assert_eq!(
            created.get("startoverWindowSeconds"),
            Some(&json!(expected)),
            "case {case:?}: the stored endpoint must report {expected}, got {created:?}"
        );

        // Persisted: the later read renders the same record.
        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: the read must render the stored record, got {described:?}"
        );

        // The value the service reported is itself acceptable: sending it back unchanged
        // through the update path is accepted and changes nothing.
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{id}"),
            Some(&update_body(package, Some(expected))),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: writing back the reported window expected 200, got {status} \
             {updated:?}"
        );
        assert_eq!(
            updated.get("startoverWindowSeconds"),
            Some(&json!(expected)),
            "case {case:?}: the written endpoint must still report {expected}, got \
             {updated:?}"
        );
    }
}

#[tokio::test]
async fn a_window_below_the_minimum_is_refused_on_update_and_changes_nothing() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    // One endpoint per refused value, each created with an accepted window, so a refused
    // update is compared against a record nothing else touched.
    for (index, seconds) in REFUSED.iter().enumerate() {
        let package = package_for(index);
        let id = format!("updated-{}-{package}", id_fragment(*seconds));
        let stored_window = ACCEPTED[index % ACCEPTED.len()];
        let (status, created) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(&id, package, Some(stored_window))),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {id:?}: CreateOriginEndpoint expected 200, got {status} {created:?}"
        );

        let case = format!("update startoverWindowSeconds={seconds} on {id}");
        let (status, refusal) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{id}"),
            Some(&update_body(package, Some(*seconds))),
        )
        .await;
        assert_refused(&case, status, &refusal);

        // The stored endpoint is exactly the one the create stored.
        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: a refused update must leave the stored record alone, got \
             {described:?}"
        );
    }
}

#[tokio::test]
async fn the_bound_is_the_same_whichever_operation_carries_the_window() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    // Walking the span either side of the minimum, one step at a time, on both write
    // paths: the create and the update accept and refuse exactly the same values.
    let endpoint = "bound-walk";
    let (status, created) = call(
        &router,
        "POST",
        "/origin_endpoints",
        Some(&create_body(endpoint, "hlsPackage", Some(MINIMUM))),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the walked endpoint expected 200, got {status} {created:?}"
    );

    for seconds in [MINIMUM - 2, MINIMUM - 1, MINIMUM, MINIMUM + 1, MINIMUM + 2] {
        let acceptable = seconds >= MINIMUM;
        let case = format!("startoverWindowSeconds={seconds}");

        let (status, body) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(
                &format!("walk-{}", id_fragment(seconds)),
                "hlsPackage",
                Some(seconds),
            )),
        )
        .await;
        if acceptable {
            assert_eq!(
                status, 200,
                "case {case:?}: the create must accept it, got {status} {body:?}"
            );
        } else {
            assert_refused(&format!("create {case}"), status, &body);
        }

        let (status, body) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{endpoint}"),
            Some(&update_body("hlsPackage", Some(seconds))),
        )
        .await;
        if acceptable {
            assert_eq!(
                status, 200,
                "case {case:?}: the update must accept it, got {status} {body:?}"
            );
            assert_eq!(
                body.get("startoverWindowSeconds"),
                Some(&json!(seconds)),
                "case {case:?}: the update must store it, got {body:?}"
            );
        } else {
            assert_refused(&format!("update {case}"), status, &body);
        }
    }
}
