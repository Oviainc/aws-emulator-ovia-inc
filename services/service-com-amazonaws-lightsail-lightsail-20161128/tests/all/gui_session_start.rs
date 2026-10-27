// SPDX-License-Identifier: CC0-1.0

//! `StartGUISession` — starting the Amazon DCV graphical user interface session
//! of a virtual computer, driven over the wire through the same front door a
//! client uses.
//!
//! The rule under test: the call resolves `resourceName` against the ONE stored
//! instance record and starts the Amazon DCV session ON that computer — a
//! computer that holds no session, or one whose active window has run out, gets a
//! session started on it under a token the store minted, and one that already
//! holds a live session keeps exactly that session, because the session is the
//! resource and a second start neither mints a second one nor restarts the window
//! AWS documents it stays active for. The session comes up one transition span
//! after the computer it runs on is serving, so a start on a computer that is
//! still coming up is a session that comes up with it. Every accepted call
//! appends exactly one `StartGUISession` entry to the operation log, located
//! where the computer is, non-terminal for as long as the session is still coming
//! up. A computer that is stopping or stopped has nothing to run a session on, so
//! the request is refused and nothing is recorded, and a name the store does not
//! hold is refused with the service's `DoesNotExist` envelope.
//!
//! Each wire case is a TABLE whose rows differ along what the rule generalises
//! over: the computer's blueprint platform, its bundle size, its Region and its
//! IP address type — none of which the answer depends on, which is exactly what
//! the rows prove. The clock case is a table over the instant the computer the
//! session runs on becomes ready, each published bound and one instant either
//! side of it.
//!
//! The session's two URLs are `CreateGUISessionAccessDetails`' answer (pinned in
//! `gui_session_access_details.rs`), which is also the read this suite proves the
//! start through.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::STATUS_VALUES;
use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    StoredGuiSession, GUI_SESSION_ACTIVE_MILLIS, GUI_SESSION_STATUS_SETTING_UP_INSTANCE,
    GUI_SESSION_STATUS_STARTED, GUI_SESSION_STATUS_STARTING, GUI_SESSION_STATUS_START_EXPIRED,
    STATE_TRANSITION_MILLIS,
};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, wait_for_state, Emulator};

/// The operation under test.
const OPERATION: &str = "StartGUISession";

/// The `operationType` every log entry this suite is about carries, the modelled
/// `OperationType` enum's own `StartGUISession` member.
const OPERATION_TYPE: &str = "StartGUISession";

/// The endpoint the suite pretends to have reached the emulator on, so the
/// session URLs the read-back reports are addresses the caller could reach.
const HOST: &str = "lightsail.emulator.test";

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist, and the wire error code that carries it.
const DOES_NOT_EXIST: &str = "DoesNotExist";
/// See [`DOES_NOT_EXIST`].
const NOT_FOUND: &str = "NotFoundException";
/// The wire error of the service's refusal of a request it cannot serve for the
/// resource it names.
const INVALID_INPUT: &str = "InvalidInputException";

/// One virtual computer to start a GUI session on. The rows differ in blueprint
/// platform, bundle size, Region and IP address type.
#[derive(Debug)]
struct Row {
    case: &'static str,
    name: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    /// `None` leaves `ipAddressType` out of the request, which the service
    /// defaults to `dualstack`.
    ip_address_type: Option<&'static str>,
}

/// The table every wire case here runs. Blueprint ids and bundle ids are the
/// catalogs `GetBlueprints` and `GetBundles` publish (`crate::state::BLUEPRINTS`
/// and `crate::state::BUNDLE_SPECS`).
const ROWS: &[Row] = &[
    Row {
        case: "amazon linux, no ipAddressType (dualstack by default)",
        name: "start-gui-amazon-linux",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        ip_address_type: None,
    },
    Row {
        case: "ubuntu, ipv4 only, smallest bundle",
        name: "start_gui_ubuntu_2",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        ip_address_type: Some("ipv4"),
    },
    Row {
        case: "windows, ipv6 only, so the computer has no public IPv4 address",
        name: "start-gui-windows-3",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        blueprint_id: "windows_server_2022",
        bundle_id: "large_3_0",
        ip_address_type: Some("ipv6"),
    },
    Row {
        case: "application blueprint, dualstack, largest bundle in the table",
        name: "start-gui-wordpress-4",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        blueprint_id: "wordpress",
        bundle_id: "4xlarge_3_0",
        ip_address_type: Some("dualstack"),
    },
];

/// Names nothing is created under, differing along the length, the case, the
/// digits and the separators. Every one satisfies the modelled `ResourceName`
/// pattern `^\w[\w\-]*\w$`, so the centralized walk passes it to the handler.
const MISSING_NAMES: &[&str] = &[
    "ab",
    "never-created",
    "Probe_7",
    "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
];

/// The `CreateInstances` body for `row`, naming one computer.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = json!({
        "instanceNames": [name],
        "availabilityZone": row.availability_zone,
        "blueprintId": row.blueprint_id,
        "bundleId": row.bundle_id,
    });
    if let Some(ip_address_type) = row.ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    body
}

/// Start the GUI session of `name` in `region`, requiring success.
async fn start_session(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator
        .ok_at(
            Some(HOST),
            Some(region),
            OPERATION,
            json!({"resourceName": name}),
        )
        .await
}

/// The `CreateGUISessionAccessDetails` answer for `name` in `region` — the read
/// this suite proves a start through.
async fn access_details(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator
        .ok_at(
            Some(HOST),
            Some(region),
            "CreateGUISessionAccessDetails",
            json!({"resourceName": name}),
        )
        .await
}

/// The token both URLs of the session `name` holds address it by, as the access
/// details report them — `None` when no session is reported at all.
fn reported_token(case: &str, body: &Value) -> Option<String> {
    let entries = body["sessions"].as_array()?;
    let mut tokens: Vec<String> = Vec::new();
    for entry in entries {
        let url = entry["url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a session URL, got {entry:?}"));
        // `https://<host><prefix>/<resourceName>/<token>/<sessionName>`
        let mut parts = url.rsplit('/');
        let _session_name = parts.next();
        let token = parts
            .next()
            .unwrap_or_else(|| panic!("case {case}: expected a token in the URL, got {url:?}"));
        assert!(
            is_uuid_shaped(token),
            "case {case}: expected a uuid-shaped session token the store minted, got {token:?}"
        );
        tokens.push(token.to_string());
    }
    assert!(
        !tokens.is_empty(),
        "case {case}: a reported session carries its URLs, got {body:?}"
    );
    assert!(
        tokens.windows(2).all(|pair| pair[0] == pair[1]),
        "case {case}: every URL of one session addresses ONE session, got {tokens:?}"
    );
    tokens.into_iter().next()
}

/// The modelled `Status` the access details report for `name`, checked against
/// the modelled `Status` enum.
async fn reported_status(case: &str, emulator: &Emulator, region: &str, name: &str) -> String {
    let body = access_details(emulator, region, name).await;
    let status = body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a status, got {body:?}"))
        .to_string();
    assert!(
        STATUS_VALUES.contains(&status.as_str()),
        "case {case}: expected a modelled Status member, got {status:?}"
    );
    status
}

/// Assert `operations` is exactly one `StartGUISession` log entry against `name`
/// in `region`, terminal exactly when `terminal` says so, and answer its id.
fn assert_start_operation(
    case: &str,
    operations: &Value,
    name: &str,
    region: &str,
    terminal: bool,
) -> String {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations list, got {operations:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: the call acts on one virtual computer, so it logs one operation, \
         got {operations:?}"
    );
    let entry = &entries[0];
    assert_eq!(
        entry["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected operationType {OPERATION_TYPE}, got {entry:?}"
    );
    assert_eq!(
        entry["resourceName"].as_str(),
        Some(name),
        "case {case}: expected resourceName {name}, got {entry:?}"
    );
    assert_eq!(
        entry["resourceType"].as_str(),
        Some("Instance"),
        "case {case}: expected resourceType Instance, got {entry:?}"
    );
    assert_eq!(
        entry["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: the entry is located where the computer is, got {entry:?}"
    );
    let want = if terminal {
        (Some("Succeeded"), Some(true))
    } else {
        (Some("Started"), Some(false))
    };
    assert_eq!(
        (entry["status"].as_str(), entry["isTerminal"].as_bool()),
        want,
        "case {case}: expected a {} entry, got {entry:?}",
        if terminal { "terminal Succeeded" } else { "non-terminal Started" }
    );
    let id = entry["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a minted operation id, got {entry:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id.to_string()
}

/// Poll the access details of `name` until they report `started`, answering the
/// body they reported it in.
async fn wait_until_session_started(emulator: &Emulator, region: &str, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let body = access_details(emulator, region, name).await;
        if body["status"].as_str() == Some(GUI_SESSION_STATUS_STARTED) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for the GUI session of {name} to start: the clock never settled it, \
             last {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Poll `GetOperation` for `id` until the entry is terminal, answering it.
async fn wait_until_operation_terminal(emulator: &Emulator, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let entry = emulator
            .ok("GetOperation", json!({"operationId": id}))
            .await["operation"]
            .clone();
        if entry["isTerminal"].as_bool() == Some(true) {
            return entry;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for the log entry {id} to settle: the clock never settled it, \
             last {entry:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The GUI session the store holds for the computer `name`, read through the same
/// public state API a handler uses — so a case can assert the window AWS
/// documents a session stays active for was not restarted, which no reported
/// member carries.
fn stored_session(emulator: &Emulator, name: &str) -> Option<StoredGuiSession> {
    lightsail_state::with_state(emulator.engine(), |store| {
        store.instance(name).and_then(|instance| instance.gui_session.clone())
    })
    .expect("the suite's engine is the shared in-memory one")
}

/// Every `operationType` the operation log reports for the resource `name`.
async fn logged_types_for(emulator: &Emulator, region: &str, name: &str) -> Vec<String> {
    let body = emulator
        .ok_at(
            Some(HOST),
            Some(region),
            "GetOperationsForResource",
            json!({"resourceName": name}),
        )
        .await;
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the log of {name}: expected a list, got {body:?}"))
        .iter()
        .filter_map(|entry| entry["operationType"].as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// Starting a session
// ---------------------------------------------------------------------------

/// RULE: a start on a running virtual computer that holds no session starts one
/// ON that computer — addressed by a token the store minted, reported by the
/// access details, and `started` once the clock owes it — and logs exactly one
/// `StartGUISession` entry against the computer, non-terminal while the session
/// is still coming up and settled by the clock, readable back by its id.
#[tokio::test]
async fn starting_a_session_records_it_on_the_virtual_computer() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        emulator
            .ok_at(
                Some(HOST),
                Some(row.region),
                "CreateInstances",
                create_body(row, row.name),
            )
            .await;
        wait_for_state(&emulator, row.name, "running").await;
        assert!(
            stored_session(&emulator, row.name).is_none(),
            "case {case}: nothing has started a session yet, got {:?}",
            stored_session(&emulator, row.name)
        );

        let answered = start_session(&emulator, row.region, row.name).await;
        let id = assert_start_operation(case, &answered["operations"], row.name, row.region, false);

        // The read-back: the session the start created is the one the access
        // details report, under the token the store minted for it.
        let details = access_details(&emulator, row.region, row.name).await;
        let status = details["status"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a status, got {details:?}"));
        assert!(
            status == GUI_SESSION_STATUS_STARTING || status == GUI_SESSION_STATUS_STARTED,
            "case {case}: the session the start created is coming up or up, got {status:?}"
        );
        let token = reported_token(case, &details)
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {details:?}"));
        assert_eq!(
            stored_session(&emulator, row.name).map(|session| session.token),
            Some(token.clone()),
            "case {case}: the reported token is the one the store holds for the computer"
        );

        // The clock, not the poll, is what brings the session up and settles the
        // entry that recorded the start.
        let started = wait_until_session_started(&emulator, row.region, row.name).await;
        assert_eq!(
            reported_token(case, &started),
            Some(token),
            "case {case}: the session that came up is the one the start created, got {started:?}"
        );
        let settled = wait_until_operation_terminal(&emulator, &id).await;
        assert_eq!(
            (
                settled["operationType"].as_str(),
                settled["resourceName"].as_str(),
                settled["status"].as_str(),
            ),
            (Some(OPERATION_TYPE), Some(row.name), Some("Succeeded")),
            "case {case}: GetOperation must report the settled entry the start logged, \
             got {settled:?}"
        );
    }
}

/// RULE: the session is the resource, so a start while the session is still
/// inside its active window keeps exactly that session — the same token, and the
/// same window, which the documented hour is NOT restarted from — and the entry
/// recording that start is terminal, because there is nothing left to come up.
#[tokio::test]
async fn a_second_start_keeps_the_live_session_and_its_window() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = format!("{}-again", row.name);
        emulator
            .ok_at(
                Some(HOST),
                Some(row.region),
                "CreateInstances",
                create_body(row, &name),
            )
            .await;
        wait_for_state(&emulator, &name, "running").await;

        start_session(&emulator, row.region, &name).await;
        let started = wait_until_session_started(&emulator, row.region, &name).await;
        let first = reported_token(case, &started)
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {started:?}"));
        let before = stored_session(&emulator, &name)
            .unwrap_or_else(|| panic!("case {case}: the store must hold the started session"));

        let answered = start_session(&emulator, row.region, &name).await;
        // Already up, so the start it records is complete as the call returns.
        assert_start_operation(case, &answered["operations"], &name, row.region, true);

        let again = access_details(&emulator, row.region, &name).await;
        assert_eq!(
            reported_token(case, &again),
            Some(first),
            "case {case}: a second start reports the live session, it does not mint another, \
             got {again:?}"
        );
        let after = stored_session(&emulator, &name)
            .unwrap_or_else(|| panic!("case {case}: the store must still hold the session"));
        assert_eq!(
            (after.starts_at_ms, after.expires_at_ms()),
            (before.starts_at_ms, before.expires_at_ms()),
            "case {case}: the documented active window is not restarted by a second start, \
             got {after:?} after {before:?}"
        );
        // Both starts are in the log: the call was served twice.
        let logged = logged_types_for(&emulator, row.region, &name).await;
        assert_eq!(
            logged
                .iter()
                .filter(|logged| logged.as_str() == OPERATION_TYPE)
                .count(),
            2,
            "case {case}: each accepted start appends one entry, got {logged:?}"
        );
    }
}

/// RULE: the window AWS documents a session stays active for is what makes the
/// session's address temporary, so a start once that window has run out resumes
/// the computer's session as a NEW one — a token the store minted for this start,
/// and a fresh window — rather than handing back the address the expired window
/// belonged to. "Use this action to resume the session after it expires" is this
/// operation's own documentation.
///
/// The expiry is reached by ageing the session the store holds through the same
/// public state API a handler uses: the hour AWS documents is not something a
/// suite can wait out, and the alternative — shortening it — would change the
/// documented value the rule is about.
#[tokio::test]
async fn a_start_after_the_window_ran_out_resumes_the_session_under_a_new_token() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = format!("{}-expired", row.name);
        emulator
            .ok_at(
                Some(HOST),
                Some(row.region),
                "CreateInstances",
                create_body(row, &name),
            )
            .await;
        wait_for_state(&emulator, &name, "running").await;

        start_session(&emulator, row.region, &name).await;
        let started = wait_until_session_started(&emulator, row.region, &name).await;
        let first = reported_token(case, &started)
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {started:?}"));
        let before = stored_session(&emulator, &name)
            .unwrap_or_else(|| panic!("case {case}: the store must hold the started session"));

        // Age the session past the documented window, leaving the computer
        // running: the clock releases a session only when the computer stops.
        let aged_by = GUI_SESSION_ACTIVE_MILLIS + STATE_TRANSITION_MILLIS;
        lightsail_state::with_state(emulator.engine(), |store| {
            let session = store
                .instance_mut(&name)
                .and_then(|instance| instance.gui_session.as_mut())
                .expect("the fixture must age the session the start created");
            session.created_at_ms -= aged_by;
            session.starts_at_ms -= aged_by;
        })
        .expect("the suite's engine is the shared in-memory one");
        let expired = reported_status(case, &emulator, row.region, &name).await;
        assert_eq!(
            expired, GUI_SESSION_STATUS_START_EXPIRED,
            "case {case}: the fixture must leave the session past its window, got {expired:?}"
        );

        let answered = start_session(&emulator, row.region, &name).await;
        // Resuming starts a session that is coming up again, so the entry
        // recording it waits on the clock.
        assert_start_operation(case, &answered["operations"], &name, row.region, false);

        let resumed = access_details(&emulator, row.region, &name).await;
        let second = reported_token(case, &resumed)
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {resumed:?}"));
        assert_ne!(
            second, first,
            "case {case}: the window that ran out is what the old address belonged to, so the \
             resumed session is addressed by a new token, got {resumed:?}"
        );
        let after = stored_session(&emulator, &name)
            .unwrap_or_else(|| panic!("case {case}: the store must hold the resumed session"));
        assert!(
            after.starts_at_ms > before.starts_at_ms
                && after.expires_at_ms() == after.starts_at_ms + GUI_SESSION_ACTIVE_MILLIS,
            "case {case}: the resumed session gets a fresh window of the documented length, \
             got {after:?} after {before:?}"
        );
        // And it comes up by the clock, like any other start.
        let up = wait_until_session_started(&emulator, row.region, &name).await;
        assert_eq!(
            reported_token(case, &up),
            Some(second),
            "case {case}: the session that came up is the one the resume created, got {up:?}"
        );
    }
}

/// RULE: a session runs ON the virtual computer, so one started while that
/// computer is still coming up comes up WITH it — the session survives the
/// computer's own transition and is reported once the computer is serving, rather
/// than being released or reported up before anything could run it.
#[tokio::test]
async fn a_session_started_while_the_computer_comes_up_comes_up_with_it() {
    let emulator = Emulator::new();

    /// How many freshly created computers one row may try to catch while it is
    /// still coming up.
    const ATTEMPTS: usize = 8;

    for row in ROWS {
        let case = row.case;
        let mut caught: Option<String> = None;
        let mut last = Value::Null;
        for attempt in 1..=ATTEMPTS {
            let name = format!("{}-pending-{attempt}", row.name);
            // No wait: the computer is `pending` for as long as the clock owes it
            // `running`, which is the window this case is about.
            emulator
                .ok_at(
                    Some(HOST),
                    Some(row.region),
                    "CreateInstances",
                    create_body(row, &name),
                )
                .await;
            let answered = start_session(&emulator, row.region, &name).await;
            assert_start_operation(case, &answered["operations"], &name, row.region, false);

            // The access details report the COMPUTER while it is still being set
            // up, which is what tells this case the start landed inside the
            // window it is about.
            let status = reported_status(case, &emulator, row.region, &name).await;
            if status != GUI_SESSION_STATUS_SETTING_UP_INSTANCE {
                last = json!({"name": name, "status": status});
                continue;
            }
            assert!(
                stored_session(&emulator, &name).is_some(),
                "case {case}: the start recorded a session on the computer that is coming up"
            );
            caught = Some(name);
            break;
        }
        let name = caught.unwrap_or_else(|| {
            panic!(
                "case {case}: none of {ATTEMPTS} freshly created computers was caught while \
                 coming up, last {last:?}"
            )
        });

        // Once the computer is serving, the session the start created is the one
        // reported, and the clock brings it up from there.
        wait_for_state(&emulator, &name, "running").await;
        let started = wait_until_session_started(&emulator, row.region, &name).await;
        let token = reported_token(case, &started)
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {started:?}"));
        assert_eq!(
            stored_session(&emulator, &name).map(|session| session.token),
            Some(token),
            "case {case}: the session that came up is the one started while the computer was \
             coming up"
        );
    }
}

// ---------------------------------------------------------------------------
// A computer that cannot host a session
// ---------------------------------------------------------------------------

/// RULE: an Amazon DCV session runs ON the virtual computer, so a computer that
/// is stopping or stopped has nothing to run one on: the request is refused with
/// the service's modelled 400, whose message names the computer and the state it
/// is in, and nothing is recorded — no session on the computer and no entry in
/// its log.
#[tokio::test]
async fn a_computer_that_is_not_running_cannot_host_a_session() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = format!("{}-stopped", row.name);
        emulator
            .ok_at(
                Some(HOST),
                Some(row.region),
                "CreateInstances",
                create_body(row, &name),
            )
            .await;
        wait_for_state(&emulator, &name, "running").await;
        emulator
            .ok_at(
                Some(HOST),
                Some(row.region),
                "StopInstance",
                json!({"instanceName": name}),
            )
            .await;

        // Caught on its way down, then once it is down: neither state can run a
        // session.
        for phase in ["on the way down", "down"] {
            if phase == "down" {
                wait_for_state(&emulator, &name, "stopped").await;
            }
            let (status, body) = emulator
                .call_at(
                    Some(HOST),
                    Some(row.region),
                    OPERATION,
                    json!({"resourceName": name}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}, {phase}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_INPUT,
                "case {case}, {phase}: expected the wire error {INVALID_INPUT}, got {body:?}"
            );
            // The message names the computer the request named and the state the
            // store holds it in — whichever of the two it has reached by now, so
            // the case reads the message rather than the clock.
            let message = body["message"].as_str().unwrap_or("<no message>");
            let prefix = format!("The instance {name} is ");
            let rest = message.strip_prefix(&prefix).unwrap_or_else(|| {
                panic!(
                    "case {case}, {phase}: expected a message naming the computer, got {message:?}"
                )
            });
            assert!(
                rest.starts_with("stopping:") || rest.starts_with("stopped:"),
                "case {case}, {phase}: expected the message to name the state the computer is \
                 in, got {message:?}"
            );
            assert!(
                stored_session(&emulator, &name).is_none(),
                "case {case}, {phase}: a refused start records no session, got {:?}",
                stored_session(&emulator, &name)
            );
            let logged = logged_types_for(&emulator, row.region, &name).await;
            assert!(
                !logged.iter().any(|logged| logged == OPERATION_TYPE),
                "case {case}, {phase}: a refused start logs nothing, got {logged:?}"
            );
        }
    }
}

/// RULE: a name no virtual computer is stored under is refused with Lightsail's
/// `DoesNotExist` envelope — HTTP 400, the wire error `NotFoundException`, the
/// body code `DoesNotExist` and a message naming the resource type and the
/// identifier the request carried.
#[tokio::test]
async fn an_unresolvable_name_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();

    for name in MISSING_NAMES {
        let case = format!("{OPERATION} on {name:?}");
        let (status, body) = emulator
            .call(OPERATION, json!({"resourceName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case}: expected the wire error {NOT_FOUND}, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Instance does not exist: {name}").as_str()),
            "case {case}: expected a message naming the resource type and the name, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// One instant the virtual computer a session is started on becomes ready at, and
/// the instant the session is therefore up.
#[derive(Debug)]
struct ReadyCase {
    case: &'static str,
    ready_at_offset_ms: i64,
}

/// RULE: a session cannot be up before the computer it runs on is, so it comes up
/// one transition span after the LATER of the instant it was started at and the
/// instant that computer is serving — and from there it is `started` for the hour
/// AWS documents ("The session will be active for 1 hour", the modelled
/// `StartGUISession` documentation) and `startExpired` after it. The rows span a
/// computer that is already serving, one whose transition is due at the instant of
/// the start, and ones still coming up.
#[test]
fn a_session_comes_up_one_span_after_the_computer_it_runs_on() {
    // An arbitrary instant to start a session at: the rule is about spans, not
    // about dates.
    const STARTED_AT_MS: i64 = 1_700_000_000_000;

    let cases = [
        ReadyCase {
            case: "the computer has been serving for an hour",
            ready_at_offset_ms: -GUI_SESSION_ACTIVE_MILLIS,
        },
        ReadyCase {
            case: "the computer became ready one instant before the start",
            ready_at_offset_ms: -1,
        },
        ReadyCase {
            case: "the computer is ready at the instant of the start",
            ready_at_offset_ms: 0,
        },
        ReadyCase {
            case: "the computer is one instant from ready",
            ready_at_offset_ms: 1,
        },
        ReadyCase {
            case: "the computer is a transition span from ready",
            ready_at_offset_ms: STATE_TRANSITION_MILLIS,
        },
        ReadyCase {
            case: "the computer is ten minutes from ready",
            ready_at_offset_ms: 10 * 60 * 1_000,
        },
    ];

    for case in &cases {
        let ready_at_ms = STARTED_AT_MS.saturating_add(case.ready_at_offset_ms);
        let session = StoredGuiSession::starting_once_ready(
            "01234567-89ab-cdef-0123-456789abcdef".to_string(),
            STARTED_AT_MS,
            ready_at_ms,
        );
        let want_starts_at = ready_at_ms.max(STARTED_AT_MS) + STATE_TRANSITION_MILLIS;
        assert_eq!(
            session.starts_at_ms, want_starts_at,
            "case {}: expected the session up at {want_starts_at}, got {session:?}",
            case.case
        );
        assert_eq!(
            session.expires_at_ms(),
            want_starts_at + GUI_SESSION_ACTIVE_MILLIS,
            "case {}: expected the documented hour from the instant it is up, got {session:?}",
            case.case
        );

        // The status at each bound of the session's life: coming up, up, and past
        // its window.
        for (at_ms, want) in [
            (STARTED_AT_MS, GUI_SESSION_STATUS_STARTING),
            (want_starts_at - 1, GUI_SESSION_STATUS_STARTING),
            (want_starts_at, GUI_SESSION_STATUS_STARTED),
            (
                want_starts_at + GUI_SESSION_ACTIVE_MILLIS - 1,
                GUI_SESSION_STATUS_STARTED,
            ),
            (
                want_starts_at + GUI_SESSION_ACTIVE_MILLIS,
                GUI_SESSION_STATUS_START_EXPIRED,
            ),
        ] {
            let got = session.status_at(at_ms);
            assert!(
                STATUS_VALUES.contains(&got),
                "case {}: expected a modelled Status member at {at_ms}, got {got:?}",
                case.case
            );
            assert_eq!(
                got, want,
                "case {}: expected {want:?} at {at_ms}, got {got:?}",
                case.case
            );
        }
    }
}
