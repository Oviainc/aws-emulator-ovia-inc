// SPDX-License-Identifier: CC0-1.0

//! `GetAccountSettings` and `UpdateAccountSettings`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing — which is what a Unit-input operation needs
//! proving, since an unwired one is answered by the framework instead of a handler —
//! boundary validation, the handler, and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * `GetAccountSettings` reports the optional-feature settings the store holds for
//!   the region the request was signed for, and a region nothing has been written
//!   for reports the feature off;
//! * reporting changes nothing a later caller sees, whatever body the request
//!   carries (a Unit input has none);
//! * `UpdateAccountSettings` persists the status it is asked for as the DESIRED
//!   status and reports the switch in flight, and the status asked for becomes the
//!   one in effect only once the span the write started has elapsed — the span is
//!   never zero, so a caller reading straight afterwards still sees it in flight;
//! * the settings a region reports are the ones written through that region;
//! * a desired status the model does not admit is `BadRequestException` (400) and
//!   changes nothing, and an update that names no desired status changes nothing.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroups_ardi::ops::types::{
    GROUP_LIFECYCLE_EVENTS_DESIRED_STATUS_VALUES, GROUP_LIFECYCLE_EVENTS_STATUS_VALUES,
};
use service_com_amazonaws_resourcegroups_ardi::state::{
    ComAmazonawsResourcegroupsArdiState, LIFECYCLE_EVENTS_ACTIVE, LIFECYCLE_EVENTS_INACTIVE,
    LIFECYCLE_EVENTS_IN_PROGRESS, MAX_ACCOUNT_SETTINGS_REGIONS,
};

use super::resource_group_lifecycle::{error_code, Emulator};

/// `POST /get-account-settings`, with `body` as the request payload — the operation's
/// input is `smithy.api#Unit`, so a client may send nothing at all or an empty
/// document, and both must be served the same way.
async fn get_settings(emulator: &Emulator, region: &str, body: Option<&Value>) -> (u16, Value) {
    emulator
        .request_in(region, "POST", "/get-account-settings", body)
        .await
}

/// `POST /update-account-settings` with `body` as the request payload.
async fn update_settings(emulator: &Emulator, region: &str, body: &Value) -> (u16, Value) {
    emulator
        .request_in(region, "POST", "/update-account-settings", Some(body))
        .await
}

/// The three members of an `AccountSettings` document, as a response carries them:
/// the desired status, the status in effect, and the status message.
fn settings_of(case: &str, call: &str, status: u16, body: &Value) -> (String, String, Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: {call} expected 200, got {status} {body}",
    );
    let settings = body.get("AccountSettings").unwrap_or_else(|| {
        panic!("case {case:?}: {call} must report an AccountSettings document, got {body}")
    });
    let member = |name: &str| -> String {
        settings
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {call} must report {name} as a string, got {body}")
            })
            .to_string()
    };
    (
        member("GroupLifecycleEventsDesiredStatus"),
        member("GroupLifecycleEventsStatus"),
        settings
            .get("GroupLifecycleEventsStatusMessage")
            .cloned()
            .unwrap_or(Value::Null),
    )
}

/// The settings `GetAccountSettings` reports in `region`, which must be a 200
/// carrying the document.
async fn reported(emulator: &Emulator, region: &str, case: &str) -> (String, String, Value) {
    let (status, body) = get_settings(emulator, region, None).await;
    settings_of(case, "GetAccountSettings", status, &body)
}

/// The instant the switch `region` has in flight is owed, or `None` when nothing is
/// in flight there.
fn due_at(emulator: &Emulator, region: &str) -> Option<SystemTime> {
    emulator
        .engine()
        .state::<ComAmazonawsResourcegroupsArdiState>()
        .lock()
        .account_settings
        .get(region)
        .and_then(|record| record.pending.as_ref())
        .map(|pending| pending.due_at)
}

/// Whether the store holds a settings record for `region` at all — a region nothing
/// has been written for must have none, so neither a read nor a no-op write creates
/// one.
fn has_record(emulator: &Emulator, region: &str) -> bool {
    emulator
        .engine()
        .state::<ComAmazonawsResourcegroupsArdiState>()
        .lock()
        .account_settings
        .contains_key(region)
}

/// Move the switch `region` has in flight into the past, so the clock now owes it.
///
/// The due instant is what the handler reads, so moving it is how this suite reaches
/// the state a caller reaches by waiting — without sleeping for the span.
fn make_switch_due(emulator: &Emulator, region: &str) {
    let state = emulator
        .engine()
        .state::<ComAmazonawsResourcegroupsArdiState>();
    let mut store = state.lock();
    let pending = store
        .account_settings
        .get_mut(region)
        .and_then(|record| record.pending.as_mut())
        .expect("a switch is in flight");
    pending.due_at = SystemTime::now() - Duration::from_millis(1);
}

/// Every status this service writes is a member of the enum the model generated its
/// value list from, so the statuses on the wire cannot drift from the model.
#[test]
fn every_status_this_service_writes_is_a_modelled_enum_member() {
    for (case, status, admitted) in [
        (
            "the status in effect while a switch is in flight",
            LIFECYCLE_EVENTS_IN_PROGRESS,
            GROUP_LIFECYCLE_EVENTS_STATUS_VALUES,
        ),
        (
            "the status of a feature that is on",
            LIFECYCLE_EVENTS_ACTIVE,
            GROUP_LIFECYCLE_EVENTS_DESIRED_STATUS_VALUES,
        ),
        (
            "the status of a feature that is off",
            LIFECYCLE_EVENTS_INACTIVE,
            GROUP_LIFECYCLE_EVENTS_DESIRED_STATUS_VALUES,
        ),
    ] {
        assert!(
            admitted.contains(&status),
            "case {case:?}: expected {status} among the modelled values {admitted:?}",
        );
    }
}

/// An account nothing has been written for reports the feature off, in every region
/// and whatever body the request carries — and reading it twice reports the same
/// thing, because reporting is not a mutation.
#[tokio::test]
async fn an_account_that_has_asked_for_nothing_reports_the_feature_off() {
    let emulator = Emulator::new();
    let empty = json!({});
    // A Unit input is carried by no body at all, and by the empty document an SDK
    // client sends for an operation with no members: both are the same request.
    let bodies: [(&str, Option<&Value>); 2] =
        [("no body at all", None), ("an empty document", Some(&empty))];
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        for (form, body) in bodies {
            let case = format!("{region} with {form}");
            let (status, answered) = get_settings(&emulator, region, body).await;
            let (desired, effective, message) =
                settings_of(&case, "GetAccountSettings", status, &answered);
            assert_eq!(
                (desired.as_str(), effective.as_str()),
                (LIFECYCLE_EVENTS_INACTIVE, LIFECYCLE_EVENTS_INACTIVE),
                "case {case:?}: expected the feature off, got {answered}",
            );
            assert_eq!(
                message,
                Value::Null,
                "case {case:?}: nothing failed, so no status message may be reported, \
                 got {answered}",
            );
            // A read must not have created a record, so the next read answers the
            // same and nothing is in flight.
            assert!(
                !has_record(&emulator, region),
                "case {case:?}: a read must store no record",
            );
            assert_eq!(
                due_at(&emulator, region),
                None,
                "case {case:?}: a read must start no switch",
            );
            let again = reported(&emulator, region, &case).await;
            assert_eq!(
                (again.0.as_str(), again.1.as_str()),
                (LIFECYCLE_EVENTS_INACTIVE, LIFECYCLE_EVENTS_INACTIVE),
                "case {case:?}: a second read must report the same settings, got {again:?}",
            );
        }
    }
}

/// Every desired status the model admits, asked for in turn, so the rows differ
/// along the value asked for AND along what it was before: turning the feature on
/// from off, off again from on, and asking for the status that is already in effect.
///
/// Source of the values: `GroupLifecycleEventsDesiredStatus`
/// (`smithy_query(query="shape",shape="GroupLifecycleEventsDesiredStatus")`), whose
/// members are `ACTIVE` and `INACTIVE`.
fn switch_sequence() -> Vec<&'static str> {
    vec![
        LIFECYCLE_EVENTS_ACTIVE,
        LIFECYCLE_EVENTS_INACTIVE,
        LIFECYCLE_EVENTS_ACTIVE,
        LIFECYCLE_EVENTS_ACTIVE,
    ]
}

/// A status asked for is persisted as the desired status at once, is in effect only
/// once the span the write started has elapsed, and is then reported by every later
/// read.
#[tokio::test]
async fn a_requested_status_is_reported_in_flight_and_then_in_effect() {
    for region in ["us-east-1", "eu-west-1"] {
        let emulator = Emulator::new();
        for desired in switch_sequence() {
            let case = format!("{desired} in {region}");
            let (status, answered) = update_settings(
                &emulator,
                region,
                &json!({ "GroupLifecycleEventsDesiredStatus": desired }),
            )
            .await;
            let (written_desired, written_effective, written_message) =
                settings_of(&case, "UpdateAccountSettings", status, &answered);
            assert_eq!(
                (written_desired.as_str(), written_effective.as_str()),
                (desired, LIFECYCLE_EVENTS_IN_PROGRESS),
                "case {case:?}: expected the write to report {desired} desired and the switch \
                 in flight, got {answered}",
            );
            assert_eq!(
                written_message,
                Value::Null,
                "case {case:?}: nothing failed, so no status message may be reported, \
                 got {answered}",
            );

            // The switch is owed at a stored instant that is still in the future when
            // the write answers: the span is never zero, so a caller acting straight
            // after the call sees the transient status.
            let due = due_at(&emulator, region)
                .unwrap_or_else(|| panic!("case {case:?}: expected a switch in flight"));
            assert!(
                due > SystemTime::now(),
                "case {case:?}: expected the switch to fall due later than the write, \
                 got {due:?}",
            );

            // A read before the span elapses reports the same in-flight document.
            let in_flight = reported(&emulator, region, &case).await;
            assert_eq!(
                (in_flight.0.as_str(), in_flight.1.as_str()),
                (desired, LIFECYCLE_EVENTS_IN_PROGRESS),
                "case {case:?}: expected the read to report the switch in flight, \
                 got {in_flight:?}",
            );

            // Once the clock owes the switch, the status asked for is the one in
            // effect — and stays so for every later read.
            make_switch_due(&emulator, region);
            for read in 0..2 {
                let settled = reported(&emulator, region, &case).await;
                assert_eq!(
                    (settled.0.as_str(), settled.1.as_str()),
                    (desired, desired),
                    "case {case:?} (read {read}): expected {desired} both desired and in \
                     effect, got {settled:?}",
                );
            }
            assert_eq!(
                due_at(&emulator, region),
                None,
                "case {case:?}: a settled switch is no longer in flight",
            );
        }
    }
}

/// The settings a region reports are the ones written through that region: this
/// service is regional, so another region keeps the feature off until it is written
/// itself, and writing it there leaves the first region alone.
#[tokio::test]
async fn settings_are_reported_by_the_region_they_were_written_through() {
    let emulator = Emulator::new();
    let written = "us-east-1";
    let (status, body) = update_settings(
        &emulator,
        written,
        &json!({ "GroupLifecycleEventsDesiredStatus": LIFECYCLE_EVENTS_ACTIVE }),
    )
    .await;
    settings_of("the written region", "UpdateAccountSettings", status, &body);
    make_switch_due(&emulator, written);

    for region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
        let case = format!("read from {region}");
        let (desired, effective, _) = reported(&emulator, region, &case).await;
        let expected = if region == written {
            LIFECYCLE_EVENTS_ACTIVE
        } else {
            LIFECYCLE_EVENTS_INACTIVE
        };
        assert_eq!(
            (desired.as_str(), effective.as_str()),
            (expected, expected),
            "case {case:?}: expected {expected} in {region}, got {desired}/{effective}",
        );
    }

    // A write in another region is that region's setting, and not this one's.
    let other = "eu-central-1";
    let (status, body) = update_settings(
        &emulator,
        other,
        &json!({ "GroupLifecycleEventsDesiredStatus": LIFECYCLE_EVENTS_INACTIVE }),
    )
    .await;
    settings_of("the other region", "UpdateAccountSettings", status, &body);
    let (desired, effective, _) = reported(&emulator, written, "the first region after").await;
    assert_eq!(
        (desired.as_str(), effective.as_str()),
        (LIFECYCLE_EVENTS_ACTIVE, LIFECYCLE_EVENTS_ACTIVE),
        "a write in {other} must leave {written} at {LIFECYCLE_EVENTS_ACTIVE}, \
         got {desired}/{effective}",
    );
}

/// Desired statuses the model does not admit, each refused with HTTP 400
/// `BadRequestException` — and refused without changing what is stored.
///
/// The rows differ along WHY the value is not one a caller may ask for: a status of
/// the effective-status enum that is not a desired status, a member of neither enum,
/// the right value in the wrong case, and one carrying stray whitespace.
fn refused_statuses() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a status of the effective enum that is not a desired status",
            json!({ "GroupLifecycleEventsDesiredStatus": LIFECYCLE_EVENTS_IN_PROGRESS }),
        ),
        (
            "the ERROR status, which a caller cannot ask for either",
            json!({ "GroupLifecycleEventsDesiredStatus": "ERROR" }),
        ),
        (
            "a value in neither enum",
            json!({ "GroupLifecycleEventsDesiredStatus": "ENABLED" }),
        ),
        (
            "the right value in the wrong case",
            json!({ "GroupLifecycleEventsDesiredStatus": "active" }),
        ),
        (
            "the right value with trailing whitespace",
            json!({ "GroupLifecycleEventsDesiredStatus": "ACTIVE " }),
        ),
        (
            "the empty string, which names no status",
            json!({ "GroupLifecycleEventsDesiredStatus": "" }),
        ),
    ]
}

/// A desired status the model does not admit is refused, and the refusal leaves the
/// stored settings exactly as they were.
#[tokio::test]
async fn a_desired_status_the_model_does_not_admit_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let (status, body) = update_settings(
        &emulator,
        region,
        &json!({ "GroupLifecycleEventsDesiredStatus": LIFECYCLE_EVENTS_ACTIVE }),
    )
    .await;
    settings_of("the accepted write", "UpdateAccountSettings", status, &body);
    make_switch_due(&emulator, region);

    for (case, request) in refused_statuses() {
        let (status, body) = update_settings(&emulator, region, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {status} {body}",
        );
        let (desired, effective, _) = reported(&emulator, region, case).await;
        assert_eq!(
            (desired.as_str(), effective.as_str()),
            (LIFECYCLE_EVENTS_ACTIVE, LIFECYCLE_EVENTS_ACTIVE),
            "case {case:?}: a refused update must change nothing, got {desired}/{effective}",
        );
        assert_eq!(
            due_at(&emulator, region),
            None,
            "case {case:?}: a refused update must start no switch",
        );
    }
}

/// The region at `index` of a deterministic series of distinct, well-formed region
/// names, for driving the store's own ceiling on how many regions it holds settings
/// for rather than one hand-picked name.
///
/// The shape is the one the modelled ARN patterns use, `[a-z]{2}(-[a-z]+)+-\d{1}`
/// (`smithy_query(query="shape",shape="GroupArn")`), which is what this service
/// accepts as the region of a credential scope.
fn nth_region(index: usize) -> String {
    let letter = |value: usize| char::from(b'a' + (value % 26) as u8);
    format!("zz-{}{}-1", letter(index / 26), letter(index))
}

/// The store's own ceiling on how many regions it holds settings for: writes up to
/// it are served, one more NEW region is refused and stores nothing, and a region
/// already stored stays writable at the ceiling.
#[tokio::test]
async fn the_regions_per_store_ceiling_is_enforced() {
    let emulator = Emulator::new();
    for index in 0..MAX_ACCOUNT_SETTINGS_REGIONS {
        let region = nth_region(index);
        let case = format!("region {index} of the ceiling");
        let (status, body) = update_settings(
            &emulator,
            &region,
            &json!({ "GroupLifecycleEventsDesiredStatus": LIFECYCLE_EVENTS_ACTIVE }),
        )
        .await;
        settings_of(&case, "UpdateAccountSettings", status, &body);
    }

    let case = "one new region past the ceiling";
    let past = nth_region(MAX_ACCOUNT_SETTINGS_REGIONS);
    let (status, body) = update_settings(
        &emulator,
        &past,
        &json!({ "GroupLifecycleEventsDesiredStatus": LIFECYCLE_EVENTS_ACTIVE }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected 400 BadRequestException, got {status} {body}",
    );
    let (desired, effective, _) = reported(&emulator, &past, case).await;
    assert_eq!(
        (desired.as_str(), effective.as_str()),
        (LIFECYCLE_EVENTS_INACTIVE, LIFECYCLE_EVENTS_INACTIVE),
        "case {case:?}: a refused write must store nothing, got {desired}/{effective}",
    );

    // A region the store already holds is still writable at the ceiling: the bound
    // is on how many it holds, not on changing a setting it holds.
    let case = "a stored region at the ceiling";
    let stored = nth_region(0);
    let (status, body) = update_settings(
        &emulator,
        &stored,
        &json!({ "GroupLifecycleEventsDesiredStatus": LIFECYCLE_EVENTS_INACTIVE }),
    )
    .await;
    let (desired, effective, _) = settings_of(case, "UpdateAccountSettings", status, &body);
    assert_eq!(
        (desired.as_str(), effective.as_str()),
        (LIFECYCLE_EVENTS_INACTIVE, LIFECYCLE_EVENTS_IN_PROGRESS),
        "case {case:?}: expected the switch accepted and in flight, got {body}",
    );
}

/// An update that names no desired status asks for no change: it reports the settings
/// the store holds, and starts no switch — before anything has been written and after
/// a status is in effect alike.
#[tokio::test]
async fn an_update_that_names_no_desired_status_changes_nothing() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let case = "before anything was written";
    let (status, body) = update_settings(&emulator, region, &json!({})).await;
    let (desired, effective, _) = settings_of(case, "UpdateAccountSettings", status, &body);
    assert_eq!(
        (desired.as_str(), effective.as_str()),
        (LIFECYCLE_EVENTS_INACTIVE, LIFECYCLE_EVENTS_INACTIVE),
        "case {case:?}: expected the feature off, got {body}",
    );
    assert_eq!(
        due_at(&emulator, region),
        None,
        "case {case:?}: an update that asks for nothing must start no switch",
    );
    assert!(
        !has_record(&emulator, region),
        "case {case:?}: an update that asks for nothing must store no record",
    );

    let (status, body) = update_settings(
        &emulator,
        region,
        &json!({ "GroupLifecycleEventsDesiredStatus": LIFECYCLE_EVENTS_ACTIVE }),
    )
    .await;
    settings_of("the accepted write", "UpdateAccountSettings", status, &body);
    make_switch_due(&emulator, region);

    let case = "after a status is in effect";
    let (status, body) = update_settings(&emulator, region, &json!({})).await;
    let (desired, effective, _) = settings_of(case, "UpdateAccountSettings", status, &body);
    assert_eq!(
        (desired.as_str(), effective.as_str()),
        (LIFECYCLE_EVENTS_ACTIVE, LIFECYCLE_EVENTS_ACTIVE),
        "case {case:?}: expected the stored settings reported unchanged, got {body}",
    );
    assert_eq!(
        due_at(&emulator, region),
        None,
        "case {case:?}: an update that asks for nothing must start no switch",
    );
}
