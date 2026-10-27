// SPDX-License-Identifier: CC0-1.0

//! What `DescribePortal` reports, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules this suite pins:
//!
//! * a read answers from the portal its `portalId` names — in a store holding
//!   several portals, every member it reports is the one that portal's accepted
//!   create left, never a neighbour's;
//! * the read is REPORTING: reading a portal any number of times changes nothing a
//!   later caller sees, so the only member two reads of one portal differ in is the
//!   state the clock has made due;
//! * an id at the modelled `ID` length that no portal is stored under is the
//!   modelled not-found refusal, worded the way this service words the absence of a
//!   resource, while an id outside that length is refused as a bad request rather
//!   than reported absent or answered.
//!
//! The sibling `portal_lifecycle` suite covers the WRITE (what a create persists and
//! how the owed transition settles) and `portal_delete` the removal; this one is
//! about the read itself over a store of several portals, so it reuses that slice's
//! HTTP harness rather than growing a second.

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use super::asset_lifecycle::{call, error_code, is_modelled_id, router};

/// Where every portal request goes, as `OPERATIONS.md` reports the route:
/// `POST /portals`, `GET /portals/{portalId}`.
const PORTALS: &str = "/portals";

/// One row of the read table: the create body a portal is stored from, beside what
/// a read of that portal must then report.
#[derive(Debug)]
struct ReadCase {
    label: &'static str,
    /// The `CreatePortal` body, carrying a different combination of the optional
    /// members on every row so each read is checked against a different stored
    /// portal rather than against a renamed copy of one.
    body: Value,
    /// The `portalAuthMode` the read must report: the value the create sent, or the
    /// documented default (`SSO`) when it sent none.
    expected_auth_mode: &'static str,
}

/// A service-role ARN within the modelled `IamArn` pattern.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::123456789012:role/{name}")
}

/// The rows. They differ along every dimension the stored portal has that a read
/// reports: the `AuthMode` value (absent / `SSO` / `IAM` — the enum's whole value
/// list, `smithy_query(query="shape", shape="AuthMode")`), a description or none,
/// the `PortalType` values (`SITEWISE_PORTAL_V1` | `SITEWISE_PORTAL_V2`, likewise
/// the whole list) with the configuration each names, alarms with and without the
/// optional notification Lambda, and a notification sender email or none.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            label: "the required members alone",
            body: json!({
                "portalName": "floor-one",
                "portalContactEmail": "floor-one@example.com",
                "roleArn": role_arn("floor-one"),
            }),
            // "Default: SSO" — `portalAuthMode`'s own documentation.
            expected_auth_mode: "SSO",
        },
        ReadCase {
            label: "an IAM portal with a description",
            body: json!({
                "portalName": "floor-two",
                "portalContactEmail": "floor-two@example.com",
                "roleArn": role_arn("floor-two"),
                "portalAuthMode": "IAM",
                "portalDescription": "the IAM-authenticated portal",
            }),
            expected_auth_mode: "IAM",
        },
        ReadCase {
            label: "a classic portal with alarms and a sender email",
            body: json!({
                "portalName": "floor-three",
                "portalContactEmail": "floor-three@example.com",
                "roleArn": role_arn("floor-three"),
                "portalAuthMode": "SSO",
                "portalType": "SITEWISE_PORTAL_V1",
                "portalTypeConfiguration": {
                    "SITEWISE_PORTAL_V1": { "portalTools": ["DASHBOARD"] },
                },
                "alarms": {
                    "alarmRoleArn": role_arn("floor-three-alarms"),
                    "notificationLambdaArn":
                        "arn:aws:lambda:us-west-2:123456789012:function:floor-three",
                },
                "notificationSenderEmail": "alarms@example.com",
            }),
            expected_auth_mode: "SSO",
        },
        ReadCase {
            label: "an AI-aware portal with alarms carrying no notification Lambda",
            body: json!({
                "portalName": "floor-four",
                "portalContactEmail": "floor-four@example.com",
                "roleArn": role_arn("floor-four"),
                "portalType": "SITEWISE_PORTAL_V2",
                "portalTypeConfiguration": {
                    "SITEWISE_PORTAL_V2": { "portalTools": ["ASSISTANT", "DASHBOARD"] },
                },
                "alarms": { "alarmRoleArn": role_arn("floor-four-alarms") },
            }),
            expected_auth_mode: "SSO",
        },
    ]
}

/// The members a read reports that the create carried verbatim, so a row that sent
/// one must read it back and a row that sent none must have nobody report it.
const ECHOED_MEMBERS: [&str; 5] = [
    "portalDescription",
    "notificationSenderEmail",
    "alarms",
    "portalType",
    "portalTypeConfiguration",
];

/// `reference` with `later`'s `portalStatus` in place of its own: what a second
/// read of one portal must equal, since the state the clock owes is the only member
/// two reads of a portal nothing wrote to may differ in.
fn with_status_of(reference: &Value, later: &Value) -> Value {
    let mut expected = reference.clone();
    expected["portalStatus"] = later["portalStatus"].clone();
    expected
}

/// Assert `body` reports the `PortalState` the clock owes a portal whose create was
/// accepted at `accepted_at`.
///
/// A read that RETURNED before the owed span had elapsed certainly landed inside it,
/// so it must still report the transient state; one that may have landed after the
/// span may report either, so the clock's own work is never read as a failure. The
/// `PortalState` values are `CREATING`, `PENDING`, `UPDATING`, `DELETING`, `ACTIVE`
/// and `FAILED` (`smithy_query(query="shape", shape="PortalState")`); an accepted
/// create owes only the first and the last of those two.
fn assert_owed_state(accepted_at: std::time::Instant, body: &Value, case: &str) {
    let state = body["portalStatus"]["state"].as_str().unwrap_or_default();
    if accepted_at.elapsed() < ACTIVATION_DELAY {
        assert_eq!(
            state, "CREATING",
            "{case}: a read must not settle the owed transition early, got {body}"
        );
    } else {
        assert!(
            matches!(state, "CREATING" | "ACTIVE"),
            "{case}: a portal whose create was accepted is creating or active, got {body}"
        );
    }
}

#[tokio::test]
async fn a_read_reports_the_portal_its_id_names_among_several_stored_portals() {
    let router = router();

    // Store every row first, so each read below happens against a store that holds
    // all of them and has to select by the id the request names.
    let cases = read_cases();
    let mut stored: Vec<(ReadCase, String)> = Vec::new();
    for case in cases {
        let (status, created) = call(&router, "POST", PORTALS, Some(case.body.clone())).await;
        assert_eq!(
            status, 202,
            "case {:?}: CreatePortal expected 202, got {status} {created}",
            case.label
        );
        let id = created["portalId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_modelled_id(&id),
            "case {:?}: the minted portalId must be a modelled id, got {created}",
            case.label
        );
        stored.push((case, id));
    }

    // Let the transition each create owes fall due, so the reads below are about a
    // settled portal; the transient state is the `portal_lifecycle` suite's subject.
    tokio::time::sleep(ACTIVATION_DELAY + std::time::Duration::from_millis(50)).await;

    // Read in the reverse of the creation order: a read that answered from the last
    // write rather than from the id would pass in creation order and fail here.
    for (case, id) in stored.iter().rev() {
        let (status, read) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribePortal expected 200, got {status} {read}",
            case.label
        );
        assert_eq!(
            read["portalId"].as_str().unwrap_or_default(),
            id.as_str(),
            "case {:?}: the read must report the portal it was asked for, got {read}",
            case.label
        );
        for member in ["portalName", "portalContactEmail", "roleArn"] {
            assert_eq!(
                &read[member], &case.body[member],
                "case {:?}: {member} must be the one the create stored, got {read}",
                case.label
            );
        }
        assert_eq!(
            read["portalAuthMode"].as_str().unwrap_or_default(),
            case.expected_auth_mode,
            "case {:?}: unexpected portalAuthMode, got {read}",
            case.label
        );
        for member in ECHOED_MEMBERS {
            match case.body.get(member) {
                Some(sent) => assert_eq!(
                    &read[member], sent,
                    "case {:?}: {member} must read back as it was stored, got {read}",
                    case.label
                ),
                None => assert!(
                    read.get(member).is_none_or(Value::is_null),
                    "case {:?}: {member} was never stored, so nothing reports it: {read}",
                    case.label
                ),
            }
        }
        // The identity members carry this portal's own id, not a neighbour's.
        assert!(
            read["portalArn"]
                .as_str()
                .unwrap_or_default()
                .ends_with(&format!(":portal/{id}")),
            "case {:?}: the ARN must carry this portal's id, got {read}",
            case.label
        );
        assert!(
            read["portalStartUrl"]
                .as_str()
                .unwrap_or_default()
                .contains(id.as_str()),
            "case {:?}: the start URL must carry this portal's id, got {read}",
            case.label
        );
        // "ACTIVE" — the `PortalState` a portal settles into once the transition the
        // create owes falls due (smithy_query query="shape" shape="PortalState").
        assert_eq!(
            read["portalStatus"]["state"].as_str().unwrap_or_default(),
            "ACTIVE",
            "case {:?}: a settled portal reports ACTIVE, got {read}",
            case.label
        );
        assert_eq!(
            read["portalCreationDate"], read["portalLastUpdateDate"],
            "case {:?}: a portal no update has touched reports one instant for both, got {read}",
            case.label
        );
    }
}

#[tokio::test]
async fn reading_a_portal_changes_nothing_a_later_caller_sees() {
    let router = router();

    // Taken BEFORE the create is issued, so it is never later than the instant the
    // portal's owed transition is counted from: a read that returns while this has
    // not run for the owed span cannot have seen the transition fall due.
    let accepted_at = std::time::Instant::now();
    let (status, created) = call(
        &router,
        "POST",
        PORTALS,
        Some(json!({
            "portalName": "the-polled-portal",
            "portalContactEmail": "polled@example.com",
            "roleArn": role_arn("polled"),
            "portalDescription": "read over and over",
        })),
    )
    .await;
    assert_eq!(status, 202, "CreatePortal got {status} {created}");
    let id = created["portalId"].as_str().unwrap_or_default().to_string();
    let uri = format!("{PORTALS}/{id}");

    // Poll the portal and compare every read with the first: no number of reads may
    // settle the owed transition early, and none may move a member. A read that
    // RETURNED before the owed span had elapsed certainly landed inside it, so it
    // must report the transient state; one that may have landed after the span may
    // report either state, so the clock's own work is never a failure.
    let (status, first) = call(&router, "GET", &uri, None).await;
    assert_eq!(status, 200, "the first read got {status} {first}");
    assert_owed_state(accepted_at, &first, "the first read");
    for poll in 0..4 {
        let (status, read) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 200, "poll {poll}: got {status} {read}");
        assert_owed_state(accepted_at, &read, &format!("poll {poll}"));
        assert_eq!(
            read,
            with_status_of(&first, &read),
            "poll {poll}: a read must report the portal the read before it did, got {read}"
        );
    }

    tokio::time::sleep(ACTIVATION_DELAY + std::time::Duration::from_millis(50)).await;
    let (status, settled) = call(&router, "GET", &uri, None).await;
    assert_eq!(status, 200, "the settled read got {status} {settled}");
    assert_eq!(
        settled["portalStatus"]["state"].as_str().unwrap_or_default(),
        "ACTIVE",
        "the owed transition must have fallen due by itself, got {settled}"
    );
    assert_eq!(
        settled,
        with_status_of(&first, &settled),
        "the state is the only member the clock may move, got {settled}"
    );

    // And a settled portal stays settled, however often it is read.
    for poll in 0..4 {
        let (status, read) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 200, "settled poll {poll}: got {status} {read}");
        assert_eq!(
            read, settled,
            "settled poll {poll}: a read must report the same portal, got {read}"
        );
    }
}

#[tokio::test]
async fn an_id_no_portal_is_stored_under_is_refused_rather_than_answered() {
    let router = router();

    let (status, created) = call(
        &router,
        "POST",
        PORTALS,
        Some(json!({
            "portalName": "the-one-stored-portal",
            "portalContactEmail": "stored@example.com",
            "roleArn": role_arn("the-one-stored-portal"),
        })),
    )
    .await;
    assert_eq!(status, 202, "CreatePortal got {status} {created}");
    let id = created["portalId"].as_str().unwrap_or_default().to_string();

    // (case, the id the read addresses, the status it must answer). `portalId`
    // targets `ID`, len 36..36
    // (`smithy_query(query="operation", operation="DescribePortal")`), so a request
    // at the published length that nothing is stored under addresses an ABSENT
    // portal, while one at either side of that bound is a bad request no accepted
    // create can ever have stored. (The `portal_lifecycle` suite covers the ABSENT
    // side over several ids; one row carries it here as the contrast the bound rows
    // are read against.)
    let cases: Vec<(&str, &str, u16)> = vec![
        (
            "a modelled id nothing is stored under",
            "4f2c1b7a-9d3e-4a58-8b61-0e2d4c6a8b10",
            404,
        ),
        (
            "one character short of the modelled length",
            "4f2c1b7a-9d3e-4a58-8b61-0e2d4c6a8b1",
            400,
        ),
        (
            "one character past the modelled length",
            "4f2c1b7a-9d3e-4a58-8b61-0e2d4c6a8b100",
            400,
        ),
        (
            "a single character, far below the modelled length",
            "4",
            400,
        ),
    ];

    for (case, addressed, expected) in cases {
        // Each row is self-checking: a 404 row must address an id the model admits,
        // and a 400 row one it does not.
        assert_eq!(
            is_modelled_id(addressed),
            expected == 404,
            "case {case:?}: the probe id does not belong to the side of the bound it \
             is listed on"
        );
        let (status, body) = call(&router, "GET", &format!("{PORTALS}/{addressed}"), None).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {body}"
        );
        let (expected_code, expected_message) = if expected == 404 {
            // One wording for every not-found refusal in this service: the resource
            // kind, then the identifier the REQUEST supplied, echoed as sent.
            (
                "ResourceNotFoundException",
                format!("Portal with ID {addressed} not found"),
            )
        } else {
            ("InvalidRequestException", String::new())
        };
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: unexpected error code, got {body}"
        );
        if expected == 404 {
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                expected_message,
                "case {case:?}: the refusal must name the id the request addressed, got {body}"
            );
        }
        // Whatever the refusal, it never leaks the one portal that does exist.
        assert!(
            !body.to_string().contains(id.as_str()),
            "case {case:?}: a refusal must say nothing about another portal, got {body}"
        );
    }

    // Every refusal left the stored portal readable.
    let (status, read) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "the stored portal after the refusals: got {status} {read}"
    );
    assert_eq!(
        read["portalId"].as_str().unwrap_or_default(),
        id.as_str(),
        "the stored portal must still read back as itself, got {read}"
    );
}
