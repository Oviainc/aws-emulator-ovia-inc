// SPDX-License-Identifier: CC0-1.0

//! `CreateGUISessionAccessDetails` — the two Amazon DCV URLs a virtual
//! computer's graphical user interface session is reached through, driven over
//! the wire through the same front door a client uses.
//!
//! The rule under test: the call REPORTS the session the named virtual computer
//! holds and opens none — "Use StartGUISession to open the session" is this
//! operation's own documentation — so a running computer whose session nobody has
//! opened reports the modelled `notStarted` with no URLs, however often it is
//! read, and the hour AWS documents a session stays active for begins at the
//! start, never at a read. Once a session has been opened the call reports the
//! pair of URLs it is reached through — a primary one for the computer's
//! application session and a secondary one for its operating system, both
//! addressed by the one token the store minted for that session and both under
//! the endpoint the request arrived at — `starting` until the clock owes the
//! session `started`, with `percentageComplete` following that span, and a second
//! call reports the SAME pair rather than a second session. A computer that is
//! still coming up reports `settingUpInstance` and no URLs; one that is stopping
//! or stopped reports `notStarted` and no URLs, because an Amazon DCV session
//! runs ON the computer — and the clock releases that session the instant the
//! computer leaves `running`, so a stop and a start lead to a new session
//! whether or not anybody looked in between. The hour AWS documents a session
//! stays active for ends in `startExpired`. A name nothing is stored under is
//! refused with the service's `DoesNotExist` envelope.
//!
//! Each wire case is a TABLE whose rows differ along what the rule generalises
//! over: the virtual computer's blueprint platform, its bundle size, its Region
//! and its IP address type — none of which the answer depends on, which is
//! exactly what the rows prove.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has, and opening a
//! session is the `StartGUISession` helper `gui_session_start.rs` already has.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::STATUS_VALUES;
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    elapsed_percentage, now_ms, StoredGuiSession, GUI_SESSION_ACTIVE_MILLIS,
    GUI_SESSION_NAME_APPLICATION, GUI_SESSION_NAME_OPERATING_SYSTEM, GUI_SESSION_PATH_PREFIX,
    GUI_SESSION_STATUS_NOT_STARTED, GUI_SESSION_STATUS_SETTING_UP_INSTANCE,
    GUI_SESSION_STATUS_STARTED, GUI_SESSION_STATUS_STARTING, GUI_SESSION_STATUS_START_EXPIRED,
    STATE_TRANSITION_MILLIS,
};

use crate::gui_session_start::{start_session, stored_session};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, wait_for_state, Emulator};

/// The endpoint the suite pretends to have reached the emulator on. A session
/// URL is an address the caller must be able to reach, so it is built from THIS
/// value rather than from an AWS hostname.
const HOST: &str = "lightsail.emulator.test";

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist, and the wire error code that carries it.
const DOES_NOT_EXIST: &str = "DoesNotExist";
/// See [`DOES_NOT_EXIST`].
const NOT_FOUND: &str = "NotFoundException";

/// One virtual computer to create a GUI session for. The rows differ in
/// blueprint platform, bundle size, Region and IP address type.
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
        name: "gui-amazon-linux",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        ip_address_type: None,
    },
    Row {
        case: "ubuntu, ipv4 only, smallest bundle",
        name: "gui_ubuntu_2",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        ip_address_type: Some("ipv4"),
    },
    Row {
        case: "windows, ipv6 only, so the computer has no public IPv4 address",
        name: "gui-windows-3",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        blueprint_id: "windows_server_2022",
        bundle_id: "large_3_0",
        ip_address_type: Some("ipv6"),
    },
    Row {
        case: "application blueprint, dualstack, largest bundle in the table",
        name: "gui-wordpress-4",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        blueprint_id: "wordpress",
        bundle_id: "4xlarge_3_0",
        ip_address_type: Some("dualstack"),
    },
];

/// Names nothing created, differing along the length, the case, the digits and
/// the separators. Every one satisfies the modelled `ResourceName` pattern
/// `^\w[\w\-]*\w$`, so the centralized walk passes it through to the handler.
const MISSING_NAMES: &[&str] = &[
    "ab",
    "never-created",
    "Probe_7",
    "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
];

/// The `CreateInstances` body for `row`, naming one instance.
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

/// The `CreateGUISessionAccessDetails` answer for `name`, at `host` in `region`.
async fn access_details(
    emulator: &Emulator,
    host: Option<&str>,
    region: Option<&str>,
    name: &str,
) -> Value {
    emulator
        .ok_at(
            host,
            region,
            "CreateGUISessionAccessDetails",
            json!({"resourceName": name}),
        )
        .await
}

/// Poll `CreateGUISessionAccessDetails` until it reports `started`, answering the
/// body it reported it in.
async fn wait_until_session_started(
    emulator: &Emulator,
    host: Option<&str>,
    region: Option<&str>,
    name: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let body = access_details(emulator, host, region, name).await;
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

/// Assert `body` carries the modelled status `want` with no sessions and no
/// failure reason, and answer its `percentageComplete`.
fn assert_status_without_sessions(case: &str, body: &Value, want: &str) -> i64 {
    assert_eq!(
        body["status"].as_str(),
        Some(want),
        "case {case}: expected the status {want:?}, got {body:?}"
    );
    assert!(
        body["sessions"].is_null(),
        "case {case}: expected no session URL while the status is {want:?}, got {body:?}"
    );
    assert!(
        body["failureReason"].is_null(),
        "case {case}: nothing failed, so no failure reason is reported, got {body:?}"
    );
    body["percentageComplete"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: expected a percentageComplete, got {body:?}"))
}

/// Assert `body` carries exactly the pair of sessions the rule describes for the
/// virtual computer `resource_name`, answering the token both URLs address the
/// session by — `None` when the request named no endpoint, in which case no URL
/// may be announced at all.
fn assert_session_pair(
    case: &str,
    body: &Value,
    resource_name: &str,
    host: Option<&str>,
) -> Option<String> {
    let entries = body["sessions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a sessions list, got {body:?}"));
    assert_eq!(
        entries.len(),
        2,
        "case {case}: the operation creates two URLs, got {entries:?}"
    );
    let names: Vec<&str> = entries
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or("<no name>"))
        .collect();
    assert_eq!(
        names,
        vec![
            GUI_SESSION_NAME_APPLICATION,
            GUI_SESSION_NAME_OPERATING_SYSTEM
        ],
        "case {case}: expected the application session then the operating-system one, \
         got {entries:?}"
    );
    assert_eq!(
        entries[0]["isPrimary"].as_bool(),
        Some(true),
        "case {case}: the application session is the primary one, got {entries:?}"
    );
    assert_eq!(
        entries[1]["isPrimary"].as_bool(),
        Some(false),
        "case {case}: the operating-system session is not the primary one, got {entries:?}"
    );
    assert!(
        body["failureReason"].is_null(),
        "case {case}: nothing failed, so no failure reason is reported, got {body:?}"
    );

    let Some(host) = host else {
        for entry in entries {
            assert!(
                entry["url"].is_null(),
                "case {case}: a request naming no endpoint announces no URL, got {entry:?}"
            );
        }
        return None;
    };

    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for (entry, session_name) in entries.iter().zip([
        GUI_SESSION_NAME_APPLICATION,
        GUI_SESSION_NAME_OPERATING_SYSTEM,
    ]) {
        let url = entry["url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a session URL, got {entry:?}"));
        let prefix = format!("https://{host}{GUI_SESSION_PATH_PREFIX}/{resource_name}/");
        let rest = url.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected a URL under {prefix:?}, got {url:?}")
        });
        let (token, tail) = rest
            .split_once('/')
            .unwrap_or_else(|| panic!("case {case}: expected a token and a session name in the \
                                      URL, got {url:?}"));
        assert!(
            is_uuid_shaped(token),
            "case {case}: expected a uuid-shaped session token the store minted, got {token:?}"
        );
        assert_eq!(
            tail, session_name,
            "case {case}: expected the URL to address the {session_name} session, got {url:?}"
        );
        tokens.insert(token.to_string());
    }
    assert_eq!(
        tokens.len(),
        1,
        "case {case}: both URLs address ONE session, got {tokens:?}"
    );
    tokens.into_iter().next()
}

/// Assert the reported status is a member of the modelled `Status` enum.
fn assert_modelled_status(case: &str, body: &Value) -> String {
    let status = body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a status, got {body:?}"));
    assert!(
        STATUS_VALUES.contains(&status),
        "case {case}: expected a modelled Status member, got {status:?}"
    );
    status.to_string()
}

// ---------------------------------------------------------------------------
// A running virtual computer
// ---------------------------------------------------------------------------

/// RULE: the session a running virtual computer holds is reported with the pair
/// of Amazon DCV URLs it is reached through — primary (application) and secondary
/// (operating system), both addressed by the one store-minted token of that
/// session and under the endpoint the request arrived at — `starting` until the
/// clock owes the session `started`, and a later call while that session is live
/// reports the SAME pair rather than a second session. Before the session is
/// opened there is nothing to report URLs for, so the answer is `notStarted`.
#[tokio::test]
async fn reports_one_pair_of_dcv_urls_for_the_session_a_computer_holds() {
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

        // Nobody has opened a session on this computer yet.
        let unopened = access_details(&emulator, Some(HOST), Some(row.region), row.name).await;
        assert_eq!(
            assert_status_without_sessions(case, &unopened, GUI_SESSION_STATUS_NOT_STARTED),
            0,
            "case {case}: nothing is coming up on a computer whose session is not open, \
             got {unopened:?}"
        );

        start_session(&emulator, row.region, row.name).await;

        let body = access_details(&emulator, Some(HOST), Some(row.region), row.name).await;
        assert_eq!(
            body["resourceName"].as_str(),
            Some(row.name),
            "case {case}: the answer names the virtual computer the request named, got {body:?}"
        );
        let status = assert_modelled_status(case, &body);
        assert!(
            status == GUI_SESSION_STATUS_STARTING || status == GUI_SESSION_STATUS_STARTED,
            "case {case}: the session opened on a running computer is coming up or up, \
             got {status:?}"
        );
        let percentage = body["percentageComplete"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: expected a percentageComplete, got {body:?}"));
        assert!(
            (0..=100).contains(&percentage),
            "case {case}: expected a percentage in 0..=100, got {percentage}"
        );
        if status == GUI_SESSION_STATUS_STARTING {
            assert!(
                percentage < 100,
                "case {case}: a session still coming up is not complete, got {percentage}"
            );
        }
        let created = assert_session_pair(case, &body, row.name, Some(HOST))
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {body:?}"));

        // The clock, not the poll, is what starts the session.
        let started = wait_until_session_started(&emulator, Some(HOST), Some(row.region), row.name)
            .await;
        assert_eq!(
            started["percentageComplete"].as_i64(),
            Some(100),
            "case {case}: a started session is complete, got {started:?}"
        );
        let after_start = assert_session_pair(case, &started, row.name, Some(HOST))
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {started:?}"));
        assert_eq!(
            after_start, created,
            "case {case}: the session that came up is the one the start opened, not a second one"
        );

        // A further call hands back the same live session: the documented hour
        // it is active for is not restarted, and no second session is minted.
        let again = access_details(&emulator, Some(HOST), Some(row.region), row.name).await;
        assert_eq!(
            assert_modelled_status(case, &again),
            GUI_SESSION_STATUS_STARTED,
            "case {case}: the live session is still up, got {again:?}"
        );
        let again_token = assert_session_pair(case, &again, row.name, Some(HOST))
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {again:?}"));
        assert_eq!(
            again_token, created,
            "case {case}: a second call reports the live session, it does not open another"
        );
    }
}

/// RULE: this call REPORTS a virtual computer's GUI session and opens none —
/// "Use StartGUISession to open the session" is its own modelled documentation —
/// so a running computer whose session nobody has opened answers the modelled
/// `notStarted` with no URLs however often it is read, holds no session after
/// those reads, and gets the window AWS documents a session stays active for only
/// from the start that opens one.
///
/// The rows cross the table of computers with the number of times the access
/// details are read before anything opens a session: polling this operation is
/// what a caller does, and no number of reads may open a session.
#[tokio::test]
async fn reading_the_access_details_never_opens_the_session() {
    let emulator = Emulator::new();

    /// How many reads a row makes before the session is opened: one single call,
    /// and then a caller polling.
    const READ_COUNTS: &[usize] = &[1, 2, 5];

    for row in ROWS {
        for reads in READ_COUNTS {
            let case = format!("{} ({reads} read(s) before any start)", row.case);
            let name = format!("{}-unopened-{reads}", row.name);
            emulator
                .ok_at(
                    Some(HOST),
                    Some(row.region),
                    "CreateInstances",
                    create_body(row, &name),
                )
                .await;
            wait_for_state(&emulator, &name, "running").await;

            for read in 1..=*reads {
                let read_case = format!("{case}, read {read}");
                let body = access_details(&emulator, Some(HOST), Some(row.region), &name).await;
                let percentage = assert_status_without_sessions(
                    &read_case,
                    &body,
                    GUI_SESSION_STATUS_NOT_STARTED,
                );
                assert_eq!(
                    percentage, 0,
                    "case {read_case}: nothing is coming up on a computer whose session is not \
                     open, got {percentage}"
                );
                assert!(
                    stored_session(&emulator, &name).is_none(),
                    "case {read_case}: the read opened a session, got {:?}",
                    stored_session(&emulator, &name)
                );
            }

            // The start is what opens the session, so the session the store ends
            // up holding was created no earlier than this instant — the reads
            // above cannot have created it.
            let before_start_ms = now_ms();
            start_session(&emulator, row.region, &name).await;
            let session = stored_session(&emulator, &name)
                .unwrap_or_else(|| panic!("case {case}: the start must open a session"));
            assert!(
                session.created_at_ms >= before_start_ms,
                "case {case}: the session was opened by the start, not by a read, got {session:?}"
            );
            assert_eq!(
                session.expires_at_ms(),
                session.starts_at_ms + GUI_SESSION_ACTIVE_MILLIS,
                "case {case}: the documented active window runs from the instant the session \
                 the start opened comes up, got {session:?}"
            );

            // From the start on, the call reports that session and its URLs.
            let opened = access_details(&emulator, Some(HOST), Some(row.region), &name).await;
            let token = assert_session_pair(&case, &opened, &name, Some(HOST))
                .unwrap_or_else(|| panic!("case {case}: expected a session token, got {opened:?}"));
            assert_eq!(
                token, session.token,
                "case {case}: the reported session is the one the start opened, got {opened:?}"
            );
        }
    }
}

/// RULE: a session URL has to be an address the caller can reach, so a request
/// that names no endpoint at all announces the session pair without URLs rather
/// than an address nothing serves.
#[tokio::test]
async fn a_request_naming_no_endpoint_announces_no_url() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = format!("{}-hostless", row.name);
        emulator
            .ok_at(
                None,
                Some(row.region),
                "CreateInstances",
                create_body(row, &name),
            )
            .await;
        wait_for_state(&emulator, &name, "running").await;
        start_session(&emulator, row.region, &name).await;

        let body = access_details(&emulator, None, Some(row.region), &name).await;
        assert_modelled_status(case, &body);
        assert_eq!(
            assert_session_pair(case, &body, &name, None),
            None,
            "case {case}: no endpoint was named, so no URL may be announced"
        );
    }
}

// ---------------------------------------------------------------------------
// A virtual computer that is not running
// ---------------------------------------------------------------------------

/// RULE: a virtual computer that is still being set up reports
/// `settingUpInstance` with the progress of the transition the clock owes it, and
/// no session URLs: an Amazon DCV session runs ON the computer, so there is
/// nothing to reach yet.
///
/// The window is the `STATE_TRANSITION_MILLIS` the clock owes a new instance, so
/// the call has to land inside it. Rather than racing a loaded host once, each
/// row gets several ATTEMPTS, each on a freshly created computer, and the case
/// requires that at least one of them caught the window — a loop that cannot
/// flake into a pass, because the assertions run on the attempt that did catch
/// it.
#[tokio::test]
async fn reports_setting_up_instance_while_the_virtual_computer_comes_up() {
    let emulator = Emulator::new();

    /// How many freshly created computers one row may try to catch while it is
    /// still coming up.
    const ATTEMPTS: usize = 8;

    for row in ROWS {
        let case = row.case;
        let mut caught = false;
        let mut last = Value::Null;
        for attempt in 1..=ATTEMPTS {
            let name = format!("{}-pending-{attempt}", row.name);
            // No wait: the computer is `pending` for as long as the clock owes
            // it `running`, which is the window this case is about.
            emulator
                .ok_at(
                    Some(HOST),
                    Some(row.region),
                    "CreateInstances",
                    create_body(row, &name),
                )
                .await;

            let body = access_details(&emulator, Some(HOST), Some(row.region), &name).await;
            assert_modelled_status(case, &body);
            if body["status"].as_str() != Some(GUI_SESSION_STATUS_SETTING_UP_INSTANCE) {
                last = body;
                continue;
            }
            let percentage =
                assert_status_without_sessions(case, &body, GUI_SESSION_STATUS_SETTING_UP_INSTANCE);
            assert!(
                (0..100).contains(&percentage),
                "case {case}: a computer still being set up is not complete, got {percentage}"
            );
            caught = true;
            break;
        }
        assert!(
            caught,
            "case {case}: none of {ATTEMPTS} freshly created computers was caught while coming \
             up, last {last:?}"
        );
    }
}

/// RULE: a virtual computer that is stopping or stopped has no GUI session and
/// this call cannot bring one up on it, so it reports `notStarted` with no URLs
/// — and the session it held is released, so the computer reports `notStarted`
/// once it is running again and a start there opens a NEW session rather than
/// handing back URLs that reach nothing.
#[tokio::test]
async fn a_virtual_computer_that_is_not_running_has_no_session() {
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
        start_session(&emulator, row.region, &name).await;
        let before = wait_until_session_started(&emulator, Some(HOST), Some(row.region), &name)
            .await;
        let first = assert_session_pair(case, &before, &name, Some(HOST))
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {before:?}"));

        emulator
            .ok_at(
                Some(HOST),
                Some(row.region),
                "StopInstance",
                json!({"instanceName": name}),
            )
            .await;
        // Caught on its way down, then once it is down: neither state has a
        // session on it.
        let stopping = access_details(&emulator, Some(HOST), Some(row.region), &name).await;
        let stopping_percentage = assert_status_without_sessions(
            &format!("{case}, on the way down"),
            &stopping,
            GUI_SESSION_STATUS_NOT_STARTED,
        );
        assert_eq!(
            stopping_percentage, 0,
            "case {case}, on the way down: nothing is coming up, got {stopping_percentage}"
        );
        wait_for_state(&emulator, &name, "stopped").await;
        let stopped = access_details(&emulator, Some(HOST), Some(row.region), &name).await;
        assert_status_without_sessions(
            &format!("{case}, stopped"),
            &stopped,
            GUI_SESSION_STATUS_NOT_STARTED,
        );

        emulator
            .ok_at(
                Some(HOST),
                Some(row.region),
                "StartInstance",
                json!({"instanceName": name}),
            )
            .await;
        wait_for_state(&emulator, &name, "running").await;
        // The released session is gone and this call opens none, so the computer
        // is back to holding no session at all.
        let running_again = access_details(&emulator, Some(HOST), Some(row.region), &name).await;
        assert_status_without_sessions(
            &format!("{case}, running again"),
            &running_again,
            GUI_SESSION_STATUS_NOT_STARTED,
        );
        start_session(&emulator, row.region, &name).await;
        let after = access_details(&emulator, Some(HOST), Some(row.region), &name).await;
        let second = assert_session_pair(case, &after, &name, Some(HOST))
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {after:?}"));
        assert_ne!(
            second, first,
            "case {case}: the session the stop released is gone, so a start opens a new one"
        );
    }
}

/// RULE: the session dies with the computer it runs on at the instant that
/// computer leaves `running` — the clock owes the release, not whoever happens to
/// look — so a stop and a start with NO call in between leaves the computer
/// holding no session, and the next start opens a NEW one rather than handing
/// back the pre-stop URLs with the remains of their hour.
#[tokio::test]
async fn a_stop_and_start_with_no_call_in_between_releases_the_session() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = format!("{}-cycled", row.name);
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
        let before = wait_until_session_started(&emulator, Some(HOST), Some(row.region), &name)
            .await;
        let first = assert_session_pair(case, &before, &name, Some(HOST))
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {before:?}"));

        // Stop, let it settle, start, let it settle — without asking for access
        // details anywhere in the window the computer was not running.
        emulator
            .ok_at(
                Some(HOST),
                Some(row.region),
                "StopInstance",
                json!({"instanceName": name}),
            )
            .await;
        wait_for_state(&emulator, &name, "stopped").await;
        emulator
            .ok_at(
                Some(HOST),
                Some(row.region),
                "StartInstance",
                json!({"instanceName": name}),
            )
            .await;
        wait_for_state(&emulator, &name, "running").await;

        let released = access_details(&emulator, Some(HOST), Some(row.region), &name).await;
        assert_status_without_sessions(case, &released, GUI_SESSION_STATUS_NOT_STARTED);
        start_session(&emulator, row.region, &name).await;
        let after = access_details(&emulator, Some(HOST), Some(row.region), &name).await;
        let second = assert_session_pair(case, &after, &name, Some(HOST))
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {after:?}"));
        assert_ne!(
            second, first,
            "case {case}: the stop released the session, so the computer gets a new one \
             whether or not anybody looked while it was down"
        );
    }
}

// ---------------------------------------------------------------------------
// A name nothing is stored under
// ---------------------------------------------------------------------------

/// RULE: a name no virtual computer is stored under is refused with Lightsail's
/// `DoesNotExist` envelope — HTTP 400, the wire error `NotFoundException`, the
/// body code `DoesNotExist` and a message naming the resource type and the
/// identifier the request carried.
#[tokio::test]
async fn an_unresolvable_name_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();

    for name in MISSING_NAMES {
        let case = format!("CreateGUISessionAccessDetails on {name:?}");
        let (status, body) = emulator
            .call(
                "CreateGUISessionAccessDetails",
                json!({"resourceName": name}),
            )
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

/// One instant in a session's life, and what the session reports at it.
#[derive(Debug)]
struct ClockCase {
    case: &'static str,
    at_ms: i64,
    status: &'static str,
    percentage_complete: i32,
    live: bool,
}

/// RULE: a session's status and progress are a function of the clock alone —
/// `starting` while it is coming up, `started` for the hour AWS documents it
/// stays active for ("The session will be active for 1 hour", the modelled
/// `StartGUISession` documentation), and `startExpired` from the instant that
/// hour runs out. The rows walk one session's whole life, each published bound
/// and one instant either side of it.
#[test]
fn a_sessions_status_and_progress_follow_the_clock() {
    // An arbitrary creation instant: the rule is about spans, not about dates.
    const CREATED_AT_MS: i64 = 1_700_000_000_000;

    let session = StoredGuiSession::starting(
        "01234567-89ab-cdef-0123-456789abcdef".to_string(),
        CREATED_AT_MS,
    );
    assert_eq!(
        session.starts_at_ms,
        CREATED_AT_MS + STATE_TRANSITION_MILLIS,
        "case the start span: a created session comes up after the transition span, \
         got {session:?}"
    );
    assert_eq!(
        session.expires_at_ms(),
        session.starts_at_ms + GUI_SESSION_ACTIVE_MILLIS,
        "case the active window: a session stays active for the documented hour, got {session:?}"
    );
    let starts_at = session.starts_at_ms;
    let expires_at = session.expires_at_ms();

    let cases = [
        ClockCase {
            case: "the instant the access was created",
            at_ms: CREATED_AT_MS,
            status: GUI_SESSION_STATUS_STARTING,
            percentage_complete: 0,
            live: true,
        },
        ClockCase {
            case: "halfway through coming up",
            at_ms: CREATED_AT_MS + STATE_TRANSITION_MILLIS / 2,
            status: GUI_SESSION_STATUS_STARTING,
            percentage_complete: 50,
            live: true,
        },
        ClockCase {
            case: "the last instant before it is up",
            at_ms: starts_at - 1,
            status: GUI_SESSION_STATUS_STARTING,
            percentage_complete: 99,
            live: true,
        },
        ClockCase {
            case: "the instant it is up",
            at_ms: starts_at,
            status: GUI_SESSION_STATUS_STARTED,
            percentage_complete: 100,
            live: true,
        },
        ClockCase {
            case: "halfway through its active hour",
            at_ms: starts_at + GUI_SESSION_ACTIVE_MILLIS / 2,
            status: GUI_SESSION_STATUS_STARTED,
            percentage_complete: 100,
            live: true,
        },
        ClockCase {
            case: "the last instant of its active hour",
            at_ms: expires_at - 1,
            status: GUI_SESSION_STATUS_STARTED,
            percentage_complete: 100,
            live: true,
        },
        ClockCase {
            case: "the instant the hour runs out",
            at_ms: expires_at,
            status: GUI_SESSION_STATUS_START_EXPIRED,
            percentage_complete: 100,
            live: false,
        },
        ClockCase {
            case: "an hour past the hour",
            at_ms: expires_at + GUI_SESSION_ACTIVE_MILLIS,
            status: GUI_SESSION_STATUS_START_EXPIRED,
            percentage_complete: 100,
            live: false,
        },
    ];

    for case in &cases {
        let status = session.status_at(case.at_ms);
        assert!(
            STATUS_VALUES.contains(&status),
            "case {}: expected a modelled Status member, got {status:?}",
            case.case
        );
        assert_eq!(
            status, case.status,
            "case {}: expected {:?}, got {status:?}",
            case.case, case.status
        );
        let percentage = session.percentage_complete_at(case.at_ms);
        assert_eq!(
            percentage, case.percentage_complete,
            "case {}: expected {}% complete, got {percentage}",
            case.case, case.percentage_complete
        );
        let live = session.is_live_at(case.at_ms);
        assert_eq!(
            live, case.live,
            "case {}: expected live={}, got {live}",
            case.case, case.live
        );
    }
}

/// RULE: the progress a `percentageComplete` reports is how far the instant is
/// through the span it measures, bounded by that span at both ends — a span that
/// has not begun is 0, one that is over is 100, and a span of no length has
/// nothing left to wait for.
#[test]
fn elapsed_percentage_is_bounded_by_the_span_it_measures() {
    // case, span start, span end, the instant, the percentage.
    let rows: &[(&str, i64, i64, i64, i32)] = &[
        ("before the span begins", 100, 200, 50, 0),
        ("the first instant of the span", 100, 200, 100, 0),
        ("a quarter through", 100, 200, 125, 25),
        ("the last instant of the span", 100, 200, 199, 99),
        ("the instant the span ends", 100, 200, 200, 100),
        ("past the end of the span", 100, 200, 10_000, 100),
        ("a span of no length is complete", 100, 100, 100, 100),
        ("a span that ends before it begins is complete", 200, 100, 150, 100),
        ("a long span, measured at its midpoint", 0, 3_600_000, 1_800_000, 50),
    ];

    for (case, start_ms, end_ms, at_ms, want) in rows {
        let got = elapsed_percentage(*start_ms, *end_ms, *at_ms);
        assert_eq!(
            got, *want,
            "case {case}: expected {want}% of {start_ms}..{end_ms} at {at_ms}, got {got}"
        );
    }
}
