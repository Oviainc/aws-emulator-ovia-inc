// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `NotificationHub` lifecycle: register, list and
//! deregister, driven over HTTP through the same multi-service router the
//! emulator boots in production.
//!
//! Every case asserts the RULE the operations follow, not one recorded request.
//! The table rows vary along what those rules generalise over — which Region the
//! hub is registered in (segment count, segment length, both `@length` bounds of
//! the modelled `Region`), how many hubs the account holds, which transitional
//! status a hub is in when the next call arrives, and which page size the listing
//! is asked for — and the catalogs the rows draw from are the service's own
//! modelled value set (`NOTIFICATION_HUB_STATUS_VALUES`) and documented bound
//! (`MAX_NOTIFICATION_HUBS`), so a model change moves the test with it.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_notifications_notifications::ops::types::NOTIFICATION_HUB_STATUS_VALUES;
use service_com_amazonaws_notifications_notifications::state::{
    HUB_REASON_ACTIVE, HUB_REASON_DEREGISTERING, HUB_REASON_REGISTERING, HUB_TRANSITION_SPAN,
    MAX_NOTIFICATION_HUBS, NOTIFICATION_HUB_STATUS_ACTIVE, NOTIFICATION_HUB_STATUS_DEREGISTERING,
    NOTIFICATION_HUB_STATUS_REGISTERING,
};

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate.
use crate::harness::{encode_path_label, error_code, Emulator};

/// Long enough that a transition the register or deregister call made due has
/// certainly fallen due, without depending on how slow the machine is.
const PAST_THE_TRANSITION: Duration = HUB_TRANSITION_SPAN.saturating_add(Duration::from_millis(400));

/// Regions to register hubs in. Catalog source: the `Region` shape the model
/// declares for `notificationHubRegion` — `@length(2..25)` and
/// `@pattern("^([a-z]{1,4})-([a-z]{1,15}-)+([0-9])$")` — so the rows differ in
/// prefix length, middle-segment length, segment count and total length, and
/// include Regions no issue cited.
const REGIONS: &[(&str, &str)] = &[
    ("a commonplace three-segment Region", "us-east-1"),
    ("a longer middle segment", "ap-southeast-2"),
    ("four segments", "us-gov-east-1"),
    (
        "the longest prefix and middle segment the pattern allows",
        "asia-northeasternmid-1",
    ),
    ("the shortest Region the pattern and length allow", "a-b-1"),
];

/// The hub-specific helpers this suite adds to the shared harness.
impl Emulator {
    async fn register_hub(&self, region: &str) -> (u16, Value) {
        self.send(
            "POST",
            "/notification-hubs",
            Some(json!({ "notificationHubRegion": region })),
        )
        .await
    }

    async fn deregister_hub(&self, region: &str) -> (u16, Value) {
        let uri = format!("/notification-hubs/{}", encode_path_label(region));
        self.send("DELETE", &uri, None).await
    }

    /// List with a raw query string (empty for an unpaged listing).
    async fn list_hubs(&self, query: &str) -> (u16, Value) {
        let uri = if query.is_empty() {
            "/notification-hubs".to_string()
        } else {
            format!("/notification-hubs?{query}")
        };
        self.send("GET", &uri, None).await
    }
}

/// The `notificationHubs` member of a listing, which the model marks required —
/// so it is present even when the account holds no hub.
fn listed_hubs(case: &str, response: &Value) -> Vec<Value> {
    response["notificationHubs"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a listing must answer a notificationHubs list, got {response:?}")
        })
        .clone()
}

/// The one listed hub for `region`, or `None` when the listing does not report
/// that Region.
fn listed_hub(case: &str, response: &Value, region: &str) -> Option<Value> {
    listed_hubs(case, response)
        .into_iter()
        .find(|hub| hub["notificationHubRegion"] == json!(region))
}

/// The `statusSummary.status` of a hub (from a register, deregister or listing
/// entry), checked against the modelled value set.
fn status_of(case: &str, hub: &Value) -> String {
    let status = hub["statusSummary"]["status"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: every hub must carry statusSummary.status, got {hub:?}")
        })
        .to_string();
    assert!(
        // Catalog source: the `NotificationHubStatus` enum shape, re-exported by
        // the service crate.
        NOTIFICATION_HUB_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: status must be one of the modelled NotificationHubStatus values {NOTIFICATION_HUB_STATUS_VALUES:?}, got {status:?}"
    );
    status
}

/// Whether a rendered timestamp is a real instant: epoch seconds past 2020, or
/// an RFC 3339 string in this century. This framework renders a timestamp member
/// in either encoding depending on the schema, so both are accepted — but a zero
/// or a placeholder is not.
fn is_an_instant(value: &Value) -> bool {
    match value {
        Value::Number(_) => value.as_f64().is_some_and(|secs| secs > 1_600_000_000.0),
        Value::String(rendered) => {
            rendered.len() >= 20
                && rendered.starts_with("20")
                && rendered[..4].bytes().all(|b| b.is_ascii_digit())
                && rendered.contains('T')
        }
        _ => false,
    }
}

/// Whether `later` is at or after `earlier`, for whichever of the two encodings
/// the framework used (numbers compare numerically, RFC 3339 strings in the same
/// zone compare lexicographically).
fn is_at_or_after(later: &Value, earlier: &Value) -> bool {
    match (later, earlier) {
        (Value::Number(_), Value::Number(_)) => {
            later.as_f64().unwrap_or_default() >= earlier.as_f64().unwrap_or_default()
        }
        (Value::String(left), Value::String(right)) => left >= right,
        _ => false,
    }
}

/// RULE: a register persists one hub per Region in the transitional status the
/// model documents (`REGISTERING`), a listing reports exactly the stored hubs,
/// and the clock — not a read — moves a registering hub to `ACTIVE` and stamps
/// its `lastActivationTime`.
#[tokio::test]
async fn register_persists_a_hub_the_listing_reports_until_the_clock_activates_it() {
    for (case, region) in REGIONS {
        // One account per row: the account holds at most
        // `MAX_NOTIFICATION_HUBS` hubs, so a row must not spend another row's
        // capacity.
        let emulator = Emulator::new();

        // Before the register the account holds no hub, and the required
        // member is still present.
        let (status, empty) = emulator.list_hubs("").await;
        assert_eq!(status, 200, "case {case:?}: a listing answers 200, got {empty:?}");
        assert!(
            listed_hubs(case, &empty).is_empty(),
            "case {case:?}: an account with no hub lists none, got {empty:?}"
        );
        assert!(
            empty.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a listing with nothing behind it carries no nextToken, got {empty:?}"
        );

        let (status, registered) = emulator.register_hub(region).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a register answers the modelled 201, got {registered:?}"
        );
        assert_eq!(
            registered["notificationHubRegion"],
            json!(region),
            "case {case:?}: the response names the Region registered, got {registered:?}"
        );
        assert_eq!(
            status_of(case, &registered),
            NOTIFICATION_HUB_STATUS_REGISTERING,
            "case {case:?}: a fresh hub is REGISTERING, got {registered:?}"
        );
        assert_eq!(
            registered["statusSummary"]["reason"],
            json!(HUB_REASON_REGISTERING),
            "case {case:?}: the reason accompanies the status, got {registered:?}"
        );
        assert!(
            is_an_instant(&registered["creationTime"]),
            "case {case:?}: creationTime is the instant the hub was registered, got {registered:?}"
        );
        assert!(
            registered.get("lastActivationTime").is_none_or(Value::is_null),
            "case {case:?}: a hub that has never been active has no lastActivationTime, got {registered:?}"
        );

        // The transitional status is observable: a caller reading straight after
        // the register still sees REGISTERING, and the listing reports the
        // stored record member for member.
        let (status, listed) = emulator.list_hubs("").await;
        assert_eq!(status, 200, "case {case:?}: a listing answers 200, got {listed:?}");
        let entry = listed_hub(case, &listed, region)
            .unwrap_or_else(|| panic!("case {case:?}: the registered hub must be listed, got {listed:?}"));
        assert_eq!(
            listed_hubs(case, &listed).len(),
            1,
            "case {case:?}: one register makes exactly one hub, got {listed:?}"
        );
        assert_eq!(
            status_of(case, &entry),
            NOTIFICATION_HUB_STATUS_REGISTERING,
            "case {case:?}: the listing reports the stored transitional status, got {entry:?}"
        );
        assert_eq!(
            entry["creationTime"], registered["creationTime"],
            "case {case:?}: the listing reports the stored creationTime, got {entry:?}"
        );

        // A read changes nothing a later reader sees.
        let (_, again) = emulator.list_hubs("").await;
        assert_eq!(
            listed_hub(case, &again, region),
            Some(entry.clone()),
            "case {case:?}: a listing must not change what the next listing reports, got {again:?}"
        );

        // Only the clock moves the hub on.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, settled) = emulator.list_hubs("").await;
        assert_eq!(status, 200, "case {case:?}: a listing answers 200, got {settled:?}");
        let entry = listed_hub(case, &settled, region)
            .unwrap_or_else(|| panic!("case {case:?}: an activated hub stays listed, got {settled:?}"));
        assert_eq!(
            status_of(case, &entry),
            NOTIFICATION_HUB_STATUS_ACTIVE,
            "case {case:?}: the clock activates a registering hub, got {entry:?}"
        );
        assert_eq!(
            entry["statusSummary"]["reason"],
            json!(HUB_REASON_ACTIVE),
            "case {case:?}: the reason moves with the status, got {entry:?}"
        );
        assert_eq!(
            entry["creationTime"], registered["creationTime"],
            "case {case:?}: creationTime is stamped once and never rewritten, got {entry:?}"
        );
        assert!(
            is_an_instant(&entry["lastActivationTime"]),
            "case {case:?}: an activated hub carries lastActivationTime, got {entry:?}"
        );
        assert!(
            is_at_or_after(&entry["lastActivationTime"], &entry["creationTime"]),
            "case {case:?}: lastActivationTime is at or after creationTime, got {entry:?}"
        );

        // The one page size the model permits (`@range{min:3,max:3}`) reports
        // the same listing, with the required member present.
        let (status, paged) = emulator
            .list_hubs(&format!("maxResults={MAX_NOTIFICATION_HUBS}"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the permitted page size is accepted, got {paged:?}"
        );
        assert_eq!(
            listed_hubs(case, &paged),
            listed_hubs(case, &settled),
            "case {case:?}: a page that holds every hub reports the whole listing, got {paged:?}"
        );
        assert!(
            paged.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: the final page carries no nextToken, got {paged:?}"
        );
    }
}

/// RULE: there is at most one hub per Region, and the register is `@idempotent` —
/// repeating it for a Region that already holds a hub reports that hub instead
/// of minting a second one or restarting its registration.
#[tokio::test]
async fn registering_a_region_twice_reports_the_one_hub_it_already_holds() {
    for (case, region) in REGIONS {
        let emulator = Emulator::new();
        let (_, first) = emulator.register_hub(region).await;

        // Repeated while the hub is still REGISTERING.
        let (status, repeated) = emulator.register_hub(region).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a repeated register stays successful, got {repeated:?}"
        );
        assert_eq!(
            repeated, first,
            "case {case:?}: a repeated register reports the hub already registered, got {repeated:?}"
        );

        // And once the clock has activated it: the reported status moves with
        // the record, but no second hub appears and creationTime is untouched.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, after) = emulator.register_hub(region).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a register of an active Region stays successful, got {after:?}"
        );
        assert_eq!(
            status_of(case, &after),
            NOTIFICATION_HUB_STATUS_ACTIVE,
            "case {case:?}: the repeated register reports the stored status, got {after:?}"
        );
        assert_eq!(
            after["creationTime"], first["creationTime"],
            "case {case:?}: a repeated register does not re-create the hub, got {after:?}"
        );

        let (_, listed) = emulator.list_hubs("").await;
        assert_eq!(
            listed_hubs(case, &listed).len(),
            1,
            "case {case:?}: three registers of one Region leave one hub, got {listed:?}"
        );
    }
}

/// RULE: the account holds at most `MAX_NOTIFICATION_HUBS` hubs, so a register
/// past that ceiling is refused with the modelled quota error instead of growing
/// the store — and the capacity a removed hub frees is available again.
#[tokio::test]
async fn a_register_past_the_documented_cap_is_refused_with_the_quota_error() {
    let emulator = Emulator::new();
    let regions: Vec<&str> = REGIONS.iter().map(|(_, region)| *region).collect();
    assert!(
        regions.len() > MAX_NOTIFICATION_HUBS,
        "the Region catalog must be able to exceed the cap, got {regions:?}"
    );

    for region in regions.iter().take(MAX_NOTIFICATION_HUBS) {
        let (status, response) = emulator.register_hub(region).await;
        assert_eq!(
            status, 201,
            "case {region:?}: a register below the cap succeeds, got {response:?}"
        );
    }

    for region in regions.iter().skip(MAX_NOTIFICATION_HUBS) {
        let (status, refused) = emulator.register_hub(region).await;
        assert_eq!(
            status, 402,
            "case {region:?}: a register past the cap answers the modelled 402, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ServiceQuotaExceededException"),
            "case {region:?}: a register past the cap is a ServiceQuotaExceededException, got {refused:?}"
        );
    }

    let (_, listed) = emulator.list_hubs("").await;
    assert_eq!(
        listed_hubs("the cap", &listed).len(),
        MAX_NOTIFICATION_HUBS,
        "a refused register does not grow the store, got {listed:?}"
    );

    // Deregistering one frees its slot once the removal falls due, and the
    // Region refused above is then admitted.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (status, _) = emulator.deregister_hub(regions[0]).await;
    assert_eq!(status, 200, "a deregister of an active hub answers 200");
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let freed = regions[MAX_NOTIFICATION_HUBS];
    let (status, response) = emulator.register_hub(freed).await;
    assert_eq!(
        status, 201,
        "case {freed:?}: the slot a removed hub freed is available, got {response:?}"
    );
    let (_, listed) = emulator.list_hubs("").await;
    assert_eq!(
        listed_hubs("the freed slot", &listed).len(),
        MAX_NOTIFICATION_HUBS,
        "the account still holds exactly the hubs it registered, got {listed:?}"
    );
    assert!(
        listed_hub("the freed slot", &listed, regions[0]).is_none(),
        "the deregistered Region is gone, got {listed:?}"
    );
}

/// RULE: a deregister moves the hub to the transitional status the model
/// documents (`DEREGISTERING`), from where the clock — not the call — removes it;
/// while it is in that status its Region cannot be registered again, and once it
/// is gone every operation keyed by that Region reports it as not found.
#[tokio::test]
async fn deregister_moves_the_hub_to_deregistering_and_the_clock_removes_it() {
    for (case, region) in REGIONS {
        let emulator = Emulator::new();
        emulator.register_hub(region).await;
        tokio::time::sleep(PAST_THE_TRANSITION).await;

        let (status, deregistered) = emulator.deregister_hub(region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a deregister answers the modelled 200, got {deregistered:?}"
        );
        assert_eq!(
            deregistered["notificationHubRegion"],
            json!(region),
            "case {case:?}: the response names the Region deregistered, got {deregistered:?}"
        );
        assert_eq!(
            status_of(case, &deregistered),
            NOTIFICATION_HUB_STATUS_DEREGISTERING,
            "case {case:?}: a deregistering hub carries DEREGISTERING, got {deregistered:?}"
        );
        assert_eq!(
            deregistered["statusSummary"]["reason"],
            json!(HUB_REASON_DEREGISTERING),
            "case {case:?}: the reason moves with the status, got {deregistered:?}"
        );

        // The transitional status is observable, and repeating the deregister is
        // idempotent rather than a failure or a second removal clock.
        let (_, listed) = emulator.list_hubs("").await;
        let entry = listed_hub(case, &listed, region).unwrap_or_else(|| {
            panic!("case {case:?}: a deregistering hub is still listed, got {listed:?}")
        });
        assert_eq!(
            status_of(case, &entry),
            NOTIFICATION_HUB_STATUS_DEREGISTERING,
            "case {case:?}: the listing reports the stored transitional status, got {entry:?}"
        );
        let (status, repeated) = emulator.deregister_hub(region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated deregister stays successful, got {repeated:?}"
        );
        assert_eq!(
            repeated, deregistered,
            "case {case:?}: a repeated deregister reports the same summary, got {repeated:?}"
        );

        // "Cannot register a Notification Hub in the same region as one in this
        // status", from the model's own documentation for DEREGISTERING.
        let (status, conflict) = emulator.register_hub(region).await;
        assert_eq!(
            status, 409,
            "case {case:?}: registering a deregistering Region answers the modelled 409, got {conflict:?}"
        );
        assert!(
            error_code(&conflict).contains("ConflictException"),
            "case {case:?}: that refusal is a ConflictException, got {conflict:?}"
        );

        // The clock removes the record, and the Region is then unknown.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, gone) = emulator.list_hubs("").await;
        assert_eq!(status, 200, "case {case:?}: a listing answers 200, got {gone:?}");
        assert!(
            listed_hubs(case, &gone).is_empty(),
            "case {case:?}: a removed hub is reported by nobody, got {gone:?}"
        );
        let (status, missing) = emulator.deregister_hub(region).await;
        assert_eq!(
            status, 404,
            "case {case:?}: deregistering a Region holding no hub answers the modelled 404, got {missing:?}"
        );
        assert!(
            error_code(&missing).contains("ResourceNotFoundException"),
            "case {case:?}: that refusal is a ResourceNotFoundException, got {missing:?}"
        );

        // And the Region can be registered again, as a new hub.
        let (status, again) = emulator.register_hub(region).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a removed Region can be registered again, got {again:?}"
        );
        assert_eq!(
            status_of(case, &again),
            NOTIFICATION_HUB_STATUS_REGISTERING,
            "case {case:?}: the re-registered hub starts over at REGISTERING, got {again:?}"
        );
    }
}

/// RULE: "Cannot deregister a Notification Hub in this status" — the model's
/// documentation for `REGISTERING` — so a deregister that arrives before the hub
/// has initialized is refused with the modelled conflict and changes nothing;
/// once the clock has activated the hub the same call succeeds.
#[tokio::test]
async fn a_deregister_is_refused_while_the_hub_is_still_registering() {
    for (case, region) in REGIONS {
        let emulator = Emulator::new();
        let (_, registered) = emulator.register_hub(region).await;
        assert_eq!(
            status_of(case, &registered),
            NOTIFICATION_HUB_STATUS_REGISTERING,
            "case {case:?}: the hub under test must still be registering, got {registered:?}"
        );

        let (status, refused) = emulator.deregister_hub(region).await;
        assert_eq!(
            status, 409,
            "case {case:?}: deregistering a registering hub answers the modelled 409, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ConflictException"),
            "case {case:?}: that refusal is a ConflictException, got {refused:?}"
        );

        // The refusal changed nothing: the hub is still on its way to ACTIVE.
        let (_, listed) = emulator.list_hubs("").await;
        let entry = listed_hub(case, &listed, region).unwrap_or_else(|| {
            panic!("case {case:?}: a refused deregister leaves the hub in place, got {listed:?}")
        });
        assert!(
            [
                NOTIFICATION_HUB_STATUS_REGISTERING,
                NOTIFICATION_HUB_STATUS_ACTIVE
            ]
            .contains(&status_of(case, &entry).as_str()),
            "case {case:?}: the hub is registering or activated, never deregistering, got {entry:?}"
        );

        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (status, accepted) = emulator.deregister_hub(region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same deregister succeeds once the hub is active, got {accepted:?}"
        );
        assert_eq!(
            status_of(case, &accepted),
            NOTIFICATION_HUB_STATUS_DEREGISTERING,
            "case {case:?}: the accepted deregister moves the hub on, got {accepted:?}"
        );
    }
}

/// RULE: a `nextToken` this service never issued is refused with a 400 rather
/// than silently restarting the listing.
#[tokio::test]
async fn a_listing_refuses_a_page_token_it_did_not_issue() {
    let emulator = Emulator::new();
    emulator.register_hub(REGIONS[0].1).await;

    // Rows differ in how the token is wrong: not hex at all, hex of the wrong
    // width, and a well-formed-but-foreign 16-byte token (a tag this process
    // never minted). Every row still satisfies the modelled `NextToken` length
    // (1..4096) and pattern (`^[\w+-/=]+$`), so the refusal comes from the
    // handler, not from the boundary validator.
    let forged = [
        ("not hex at all", "notatoken"),
        ("hex of the wrong width", "00ff00ff"),
        (
            "a well-formed token carrying a tag this process never minted",
            "0123456789abcdef0000000000000001",
        ),
    ];
    for (case, token) in forged {
        let (status, refused) = emulator.list_hubs(&format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a forged nextToken is refused with 400, got {refused:?}"
        );
        assert!(
            error_code(&refused).contains("ValidationException"),
            "case {case:?}: that refusal is a ValidationException, got {refused:?}"
        );
    }

    // The listing itself still works afterwards.
    let (status, listed) = emulator.list_hubs("").await;
    assert_eq!(status, 200, "a listing after a refusal answers 200, got {listed:?}");
    assert_eq!(
        listed_hubs("after a refusal", &listed).len(),
        1,
        "a refused listing changes nothing, got {listed:?}"
    );
}
