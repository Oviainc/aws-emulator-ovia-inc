// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartDashboardRefresh`.
//!
//! Every case goes over the wire through the shared harness
//! (`tests/all/emulator.rs`): an `awsJson1_1` `POST /` with this service's published
//! `X-Amz-Target` prefix, dispatched through the same `MultiServiceRouter` the
//! production router serves, so what is asserted here is what a client sees —
//! including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * a refresh of a stored dashboard mints a `RefreshId` per call — all digits, 10 to
//!   20 of them, never reused — and the dashboard reports the newest one as its last
//!   refresh;
//! * a `DashboardId` is a dashboard name (the caller's Region) or a dashboard ARN (the
//!   Region it carries), and one that names no stored dashboard is refused with
//!   `ResourceNotFoundException`;
//! * a refresh accepts only the query parameters the model documents for the
//!   dashboard's type, and a `MANAGED` dashboard's refresh requires
//!   `$EventDataStoreId$`, naming an event data store that exists and is queryable;
//! * a refused refresh records nothing, and a refresh is not a configuration change.

use serde_json::{json, Value};
use std::time::SystemTime;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that pin which Region an identifier addresses.
const OTHER_REGION: &str = "us-west-2";

/// The id of the event data store the managed-dashboard cases name. `CreateEventDataStore`
/// is still a scaffold stub, so the store is seeded straight into the shared state the
/// handlers read.
const EVENT_DATA_STORE_ID: &str = "11111111-2222-3333-4444-555555555555";

/// One widget, so a row can differ in widget COUNT without repeating the shape.
/// `ViewProperties` is required on `RequestWidget`.
fn widget(index: usize) -> Value {
    json!({
        "QueryStatement": format!("SELECT eventSource FROM EDS WHERE eventName = 'op{index}'"),
        "ViewProperties": {"type": "bar"},
    })
}

/// Whether `value` is a `RefreshId`: `^\d+$` with length 10..20
/// (`smithy_query(query="shape",shape="RefreshId")`).
fn is_refresh_id(value: &str) -> bool {
    (10..=20).contains(&value.chars().count()) && value.chars().all(|c| c.is_ascii_digit())
}

/// The refresh schedule the Highlights dashboard requires: ENABLED, every 6 HOURS.
fn highlights_schedule() -> Value {
    json!({
        "RefreshSchedule": {
            "Frequency": {
                "Unit": state::REFRESH_FREQUENCY_UNIT_HOURS,
                "Value": state::HIGHLIGHTS_REFRESH_FREQUENCY_VALUE,
            },
            "Status": state::REFRESH_SCHEDULE_ENABLED,
        },
    })
}

/// The dashboard calls these cases make, on the shared harness. They carry names of
/// their own: an inherent method may be defined only once per type across this test
/// target, and `dashboard_lifecycle` already defines its own `create` / `get`.
impl Emulator {
    /// Create a dashboard called `name` with `extra` merged into the request.
    async fn create_dashboard(&self, name: &str, extra: Value) -> Reply {
        let mut request = json!({"Name": name});
        if let Some(members) = extra.as_object() {
            for (key, value) in members {
                request[key] = value.clone();
            }
        }
        self.call("CreateDashboard", &request).await
    }

    /// Read a dashboard back by name or ARN.
    async fn read_dashboard(&self, dashboard_id: &str) -> Reply {
        self.call("GetDashboard", &json!({"DashboardId": dashboard_id}))
            .await
    }

    /// Start a refresh of `dashboard_id` with `parameters`.
    async fn refresh(&self, dashboard_id: &str, parameters: &Value) -> Reply {
        self.call(
            "StartDashboardRefresh",
            &json!({"DashboardId": dashboard_id, "QueryParameterValues": parameters}),
        )
        .await
    }

    /// Seed one queryable event data store in `region`, which is what a managed
    /// dashboard's refresh names: `CreateEventDataStore` is still a scaffold stub, so
    /// the record goes straight into the state every handler reads.
    fn seed_event_data_store(&self, region: &str) {
        let state = self.state();
        let mut store = state.lock();
        store.event_data_stores.insert(
            EVENT_DATA_STORE_ID.to_string(),
            state::EventDataStoreRecord {
                id: EVENT_DATA_STORE_ID.to_string(),
                name: "refresh-store".to_string(),
                region: region.to_string(),
                status: state::EventDataStoreState::Enabled,
                created_at: Some(SystemTime::now()),
                ..state::EventDataStoreRecord::default()
            },
        );
    }

    /// Put the seeded event data store into `status`.
    fn set_event_data_store_status(&self, status: state::EventDataStoreState) {
        let state = self.state();
        let mut store = state.lock();
        store
            .event_data_stores
            .get_mut(EVENT_DATA_STORE_ID)
            .expect("the seeded event data store")
            .status = status;
    }
}

/// RULE: a refresh of a stored dashboard mints a `RefreshId` of its own and the
/// dashboard then reports that id as its last refresh.
///
/// The rows differ in widget count, in whether a refresh schedule is configured, in the
/// dashboard type, in how the dashboard is addressed and in which query parameters the
/// refresh carries.
#[tokio::test]
async fn a_refresh_mints_an_id_the_dashboard_then_reports() {
    let emulator = Emulator::new();
    emulator.seed_event_data_store(HOME_REGION);

    // (case, name, create request extras, refresh query parameters)
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        (
            "a dashboard with no widgets and no parameters",
            "bare-dashboard",
            json!({}),
            json!({}),
        ),
        (
            "one widget, refreshed over a period",
            "one-widget",
            json!({"Widgets": [widget(0)]}),
            json!({"$Period$": "OneDay"}),
        ),
        (
            "three widgets, refreshed over a time range",
            "three-widgets",
            json!({"Widgets": [widget(0), widget(1), widget(2)]}),
            json!({
                "$StartTime$": "2026-01-01T00:00:00Z",
                "$EndTime$": "2026-01-02T00:00:00Z",
            }),
        ),
        (
            "the widget-count maximum on a scheduled dashboard",
            "ten-widgets",
            json!({
                "Widgets": (0..state::CUSTOM_DASHBOARD_WIDGET_LIMIT)
                    .map(widget)
                    .collect::<Vec<_>>(),
                "RefreshSchedule": {
                    "Frequency": {"Unit": "HOURS", "Value": 24},
                    "Status": "ENABLED",
                },
            }),
            json!({"$Period$": "OneWeek"}),
        ),
        (
            "the CloudTrail-managed Highlights dashboard",
            state::HIGHLIGHTS_DASHBOARD_NAME,
            highlights_schedule(),
            json!({"$EventDataStoreId$": EVENT_DATA_STORE_ID}),
        ),
    ];

    // Every id this suite mints, so no two refreshes can share one.
    let mut minted: Vec<String> = Vec::new();

    for (case, name, extra, parameters) in rows {
        emulator.create_dashboard(name, extra).await.expect_ok(case);
        let before = emulator.read_dashboard(name).await;
        let before = before.expect_ok(case).clone();
        assert!(
            before.get("LastRefreshId").is_none(),
            "case {case:?}: a dashboard that was never refreshed reports no LastRefreshId, \
             got {before:?}"
        );

        // Addressed both ways a `DashboardId` may be written, so two refreshes run.
        for (addressed_as, identifier) in [
            ("by name", name.to_string()),
            ("by ARN", state::dashboard_arn(HOME_REGION, name)),
        ] {
            let case = format!("{case} refreshed {addressed_as}");
            let reply = emulator.refresh(&identifier, &parameters).await;
            let body = reply.expect_ok(&case).clone();
            let id = body["RefreshId"].as_str().unwrap_or_default().to_string();
            assert!(
                is_refresh_id(&id),
                "case {case:?}: expected a RefreshId of 10 to 20 digits, got {body:?}"
            );
            assert!(
                !minted.contains(&id),
                "case {case:?}: RefreshId {id} was already minted for another refresh"
            );
            minted.push(id.clone());

            let after = emulator.read_dashboard(name).await;
            let after = after.expect_ok(&case).clone();
            assert_eq!(
                after["LastRefreshId"],
                json!(id),
                "case {case:?}: expected LastRefreshId {id}, got {after:?}"
            );
            assert!(
                after.get("LastRefreshFailureReason").is_none(),
                "case {case:?}: no refresh failed, so no LastRefreshFailureReason may be \
                 reported, got {after:?}"
            );
            // A refresh is not a configuration change.
            assert_eq!(
                (&after["UpdatedTimestamp"], &after["CreatedTimestamp"]),
                (&before["UpdatedTimestamp"], &before["CreatedTimestamp"]),
                "case {case:?}: a refresh moved the dashboard's timestamps, got {after:?}"
            );
            // Reading the dashboard changes nothing a later caller sees.
            let again = emulator.read_dashboard(name).await;
            assert_eq!(
                again.expect_ok(&case),
                &after,
                "case {case:?}: a second read reports a different dashboard, got {again:?}"
            );
        }
    }
}

/// RULE: a `DashboardId` that names no stored dashboard is refused with
/// `ResourceNotFoundException` — whichever of the two forms it is written in, and
/// whether it could name a dashboard at all.
#[tokio::test]
async fn a_refresh_of_an_identifier_naming_no_dashboard_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create_dashboard("present-dashboard", json!({"Widgets": [widget(0)]}))
        .await
        .expect_ok("the dashboard that does exist");

    let rows: Vec<(&str, String)> = vec![
        ("an unused name", "absent-dashboard".to_string()),
        (
            "an unused ARN",
            state::dashboard_arn(HOME_REGION, "absent-dashboard"),
        ),
        (
            "an existing name under another Region's ARN",
            state::dashboard_arn(OTHER_REGION, "present-dashboard"),
        ),
        ("a name too short to be one", "ab".to_string()),
        (
            // The modelled `DashboardArn` pattern admits a dot; a `DashboardName`
            // does not, so this can name no dashboard.
            "a name carrying characters a dashboard name cannot",
            "present.dashboard".to_string(),
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:us-east-1:dashboard/present-dashboard".to_string(),
        ),
        (
            "an ARN of another resource type",
            format!(
                "arn:aws:cloudtrail:{HOME_REGION}:{}:trail/present-dashboard",
                engine::emulator_account_id()
            ),
        ),
        (
            "an ARN missing its account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::dashboard/present-dashboard"),
        ),
    ];
    for (case, identifier) in rows {
        let reply = emulator.refresh(&identifier, &json!({})).await;
        reply.expect_error(case, 400, "ResourceNotFoundException");
        // CloudTrail's own fixed not-found text, the same for every identifier that
        // names nothing: it reports no part of the request.
        assert_eq!(
            reply.body["Message"],
            json!(state::DASHBOARD_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the not-found message, got {reply:?}"
        );
    }

    // The control row: the dashboard that does exist is refreshed from the Region it
    // lives in, by name and by its own ARN.
    for identifier in [
        "present-dashboard".to_string(),
        state::dashboard_arn(HOME_REGION, "present-dashboard"),
    ] {
        let reply = emulator.refresh(&identifier, &json!({})).await;
        reply.expect_ok(&format!("the stored dashboard as {identifier}"));
    }
    // A bare name is the caller's own Region, so the same name is a different
    // dashboard elsewhere — and there is none there.
    let elsewhere = emulator
        .call_in(
            "StartDashboardRefresh",
            OTHER_REGION,
            &json!({"DashboardId": "present-dashboard"}),
        )
        .await;
    elsewhere.expect_error(
        "the same name from another Region",
        400,
        "ResourceNotFoundException",
    );
}

/// RULE: a refresh accepts only the query parameters the model documents for the
/// dashboard's type, a managed dashboard's refresh requires `$EventDataStoreId$`, and a
/// refused refresh records nothing.
#[tokio::test]
async fn a_refresh_takes_only_the_query_parameters_its_dashboard_type_documents() {
    let emulator = Emulator::new();
    emulator.seed_event_data_store(HOME_REGION);
    emulator
        .create_dashboard("custom-dashboard", json!({"Widgets": [widget(0)]}))
        .await
        .expect_ok("the custom dashboard");
    emulator
        .create_dashboard(state::HIGHLIGHTS_DASHBOARD_NAME, highlights_schedule())
        .await
        .expect_ok("the managed dashboard");

    // Accepted for a CUSTOM dashboard: "$StartTime$, $EndTime$, and $Period$", alone,
    // together, and none at all. A MANAGED dashboard takes those three plus the
    // required `$EventDataStoreId$`.
    let accepted: Vec<(&str, &str, Value)> = vec![
        ("custom, no parameters", "custom-dashboard", json!({})),
        (
            "custom, $StartTime$",
            "custom-dashboard",
            json!({"$StartTime$": "2026-01-01T00:00:00Z"}),
        ),
        (
            "custom, $EndTime$",
            "custom-dashboard",
            json!({"$EndTime$": "2026-01-02T00:00:00Z"}),
        ),
        (
            "custom, $Period$",
            "custom-dashboard",
            json!({"$Period$": "OneDay"}),
        ),
        (
            "custom, all three",
            "custom-dashboard",
            json!({
                "$StartTime$": "2026-01-01T00:00:00Z",
                "$EndTime$": "2026-01-02T00:00:00Z",
                "$Period$": "OneWeek",
            }),
        ),
        (
            "managed, the required $EventDataStoreId$",
            state::HIGHLIGHTS_DASHBOARD_NAME,
            json!({"$EventDataStoreId$": EVENT_DATA_STORE_ID}),
        ),
        (
            "managed, the store named by its ARN",
            state::HIGHLIGHTS_DASHBOARD_NAME,
            json!({
                "$EventDataStoreId$":
                    state::event_data_store_arn(HOME_REGION, EVENT_DATA_STORE_ID),
            }),
        ),
        (
            "managed, with a period as well",
            state::HIGHLIGHTS_DASHBOARD_NAME,
            json!({"$EventDataStoreId$": EVENT_DATA_STORE_ID, "$Period$": "OneDay"}),
        ),
    ];
    // The last refresh each dashboard accepted, so a later refusal can be shown to
    // have recorded nothing.
    let mut last_accepted: std::collections::BTreeMap<&str, String> =
        std::collections::BTreeMap::new();
    for (case, dashboard, parameters) in accepted {
        let reply = emulator.refresh(dashboard, &parameters).await;
        let body = reply.expect_ok(case).clone();
        let id = body["RefreshId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_refresh_id(&id),
            "case {case:?}: expected the refresh to be accepted with a RefreshId, got {body:?}"
        );
        last_accepted.insert(dashboard, id);
    }

    // (case, dashboard, parameters, expected error code)
    let refused: Vec<(&str, &str, Value, &str)> = vec![
        (
            "custom, $EventDataStoreId$ — only a managed dashboard's refresh takes it",
            "custom-dashboard",
            json!({"$EventDataStoreId$": EVENT_DATA_STORE_ID}),
            "ValidationException",
        ),
        (
            "custom, a query parameter the model does not document",
            "custom-dashboard",
            json!({"$Interval$": "OneHour"}),
            "ValidationException",
        ),
        (
            "custom, one documented and one undocumented parameter",
            "custom-dashboard",
            json!({"$Period$": "OneDay", "$Window$": "OneHour"}),
            "ValidationException",
        ),
        (
            "managed, without the required $EventDataStoreId$",
            state::HIGHLIGHTS_DASHBOARD_NAME,
            json!({"$Period$": "OneDay"}),
            "ValidationException",
        ),
        (
            "managed, no parameters at all",
            state::HIGHLIGHTS_DASHBOARD_NAME,
            json!({}),
            "ValidationException",
        ),
        (
            "managed, an $EventDataStoreId$ naming no event data store",
            state::HIGHLIGHTS_DASHBOARD_NAME,
            json!({"$EventDataStoreId$": "99999999-8888-7777-6666-555555555555"}),
            "EventDataStoreNotFoundException",
        ),
        (
            "managed, an event data store ARN of another Region",
            state::HIGHLIGHTS_DASHBOARD_NAME,
            json!({
                "$EventDataStoreId$":
                    state::event_data_store_arn(OTHER_REGION, EVENT_DATA_STORE_ID),
            }),
            "EventDataStoreNotFoundException",
        ),
    ];
    for (case, dashboard, parameters, code) in refused {
        let reply = emulator.refresh(dashboard, &parameters).await;
        reply.expect_error(case, 400, code);
    }

    // A refused refresh is not a refresh: each dashboard still reports the last one it
    // accepted.
    for (dashboard, id) in last_accepted {
        let fetched = emulator.read_dashboard(dashboard).await;
        assert_eq!(
            fetched.expect_ok(dashboard)["LastRefreshId"],
            json!(id),
            "case {dashboard:?} after the refused refreshes: expected LastRefreshId {id}, \
             got {fetched:?}"
        );
    }
}

/// RULE: a managed dashboard's refresh runs its queries against the event data store
/// the request names, so a store that is no longer queryable is refused as inactive.
#[tokio::test]
async fn a_refresh_naming_an_inactive_event_data_store_is_refused() {
    let emulator = Emulator::new();
    emulator.seed_event_data_store(HOME_REGION);
    emulator
        .create_dashboard(state::HIGHLIGHTS_DASHBOARD_NAME, highlights_schedule())
        .await
        .expect_ok("the managed dashboard");
    let parameters = json!({"$EventDataStoreId$": EVENT_DATA_STORE_ID});

    // Every state a store can be in, and whether a refresh against it runs. A store
    // that has stopped ingesting still serves queries over what it holds; only one
    // awaiting deletion holds no queryable lake.
    let states = [
        state::EventDataStoreState::Created,
        state::EventDataStoreState::Enabled,
        state::EventDataStoreState::StartingIngestion,
        state::EventDataStoreState::StoppingIngestion,
        state::EventDataStoreState::StoppedIngestion,
        state::EventDataStoreState::PendingDeletion,
    ];
    for store_state in states {
        emulator.set_event_data_store_status(store_state);
        let case = format!("an event data store that is {}", store_state.as_wire());
        let reply = emulator
            .refresh(state::HIGHLIGHTS_DASHBOARD_NAME, &parameters)
            .await;
        if store_state.is_queryable() {
            let body = reply.expect_ok(&case).clone();
            assert!(
                body["RefreshId"].as_str().is_some_and(is_refresh_id),
                "case {case:?}: expected a RefreshId, got {body:?}"
            );
        } else {
            reply.expect_error(&case, 400, "InactiveEventDataStoreException");
        }
    }
}
