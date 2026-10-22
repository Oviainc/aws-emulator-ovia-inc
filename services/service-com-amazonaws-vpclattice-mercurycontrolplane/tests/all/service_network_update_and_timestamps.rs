// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two wire rules of this service, both driven through the router
//! the way a client drives it and both read back out of a LATER request, so nothing
//! here can pass on a canned body.
//!
//! The harness (`Lattice`, `Answer`, `as_label`, …) is the one the service-slice suite
//! beside this file already builds; nothing is duplicated here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. An update that the service accepts is PERSISTED: `UpdateServiceNetwork` writes
//!    `authType` onto the stored service network and refreshes its `lastUpdatedAt`, so
//!    every later read reports the new value — whichever identifier form either call
//!    used, whatever the resource was created with, and however often it is updated.
//!    What the update does not name (the name, the id, the ARN, `sharingConfig`,
//!    `createdAt`) is left exactly as it was.
//! 2. A timestamp member is serialized in the format its model declares: every
//!    timestamp member of this service targets `Timestamp`, which carries
//!    `@timestampFormat("date-time")`, so each one goes out as an RFC 3339 string and
//!    never as the restJson1 default (a JSON number of epoch seconds). Asserted on
//!    every timestamp member of every read this emulator serves.
//! 3. `sharingConfig` round-trips and is never absent: the service fills the member in
//!    when a create sends no value for it, so the create's own answer and every later
//!    read report the stored setting, and an update of another member leaves it be.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use crate::vpclattice_service_slice::{as_label, Answer, Lattice};
use service_com_amazonaws_vpclattice_mercurycontrolplane::state;

/// `PATCH /servicenetworks/{serviceNetworkIdentifier}`.
async fn update_service_network(
    lattice: &Lattice,
    identifier: &str,
    body: serde_json::Value,
) -> Answer {
    lattice
        .call(
            "PATCH",
            &format!("/servicenetworks/{identifier}"),
            Some(body),
        )
        .await
}

/// The instant a timestamp member reports, refusing anything but the modelled
/// `@timestampFormat("date-time")` wire form.
///
/// A JSON number — the restJson1 default a member with no declared format falls back
/// to — fails here, because it is not even the modelled type: an SDK deserializing a
/// `date-time` member rejects it.
fn date_time_of(case: &str, member: &str, body: &serde_json::Value) -> DateTime {
    let value = &body[member];
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it \
             must be a JSON string, got {value} in {body}"
        )
    });
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {text:?} is not an RFC 3339 date-time: {error}")
    })
}

/// Both instants of a resource, as `(createdAt, lastUpdatedAt)`.
fn created_and_updated(case: &str, body: &serde_json::Value) -> (DateTime, DateTime) {
    (
        date_time_of(case, "createdAt", body),
        date_time_of(case, "lastUpdatedAt", body),
    )
}

/// Total ordering of two instants, so a test can say "not before".
fn at_or_after(later: DateTime, earlier: DateTime) -> bool {
    (later.secs(), later.subsec_nanos()) >= (earlier.secs(), earlier.subsec_nanos())
}

/// Rule 1, over a table that varies the created `authType` (including the omitted
/// default), the values the updates ask for, how many updates run, and which identifier
/// form the update and the read use. The recorded case — created `AWS_IAM`, updated to
/// `NONE`, read by the bare id — is one row of it.
#[tokio::test]
async fn an_accepted_auth_type_update_is_persisted_and_every_later_read_reports_it() {
    let lattice = Lattice::new();

    // (case, create body, the authTypes each update asks for in turn, update by ARN,
    //  read by ARN)
    let cases: Vec<(&str, serde_json::Value, Vec<&str>, bool, bool)> = vec![
        (
            "created AWS_IAM, switched to NONE, read by the bare id",
            serde_json::json!({
                "name": "auth-iam-to-none",
                "authType": "AWS_IAM",
                "sharingConfig": { "enabled": true },
            }),
            vec!["NONE"],
            false,
            false,
        ),
        (
            "created with the default authType, switched to AWS_IAM, read by the ARN",
            serde_json::json!({ "name": "auth-default-to-iam" }),
            vec!["AWS_IAM"],
            false,
            true,
        ),
        (
            "created NONE, updated to NONE again (no-op update)",
            serde_json::json!({ "name": "auth-none-to-none", "authType": "NONE" }),
            vec!["NONE"],
            true,
            false,
        ),
        (
            "created AWS_IAM, switched twice, both calls identified by the ARN",
            serde_json::json!({
                "name": "auth-round-trip",
                "authType": "AWS_IAM",
                "sharingConfig": { "enabled": false },
            }),
            vec!["NONE", "AWS_IAM", "NONE"],
            true,
            true,
        ),
    ];

    for (case, body, updates, update_by_arn, read_by_arn) in cases {
        let created = lattice.create_service_network(body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        let id = created.str_field("id");
        let arn = created.str_field("arn");
        let name = created.str_field("name");
        let expected_sharing = Some(
            body["sharingConfig"]["enabled"]
                .as_bool()
                .unwrap_or(state::DEFAULT_SHARING_ENABLED),
        );
        let update_identifier = if update_by_arn {
            as_label(&arn)
        } else {
            id.clone()
        };
        let read_identifier = if read_by_arn {
            as_label(&arn)
        } else {
            id.clone()
        };

        let before = lattice.get_service_network(&read_identifier).await;
        let (created_at, mut last_seen) = created_and_updated(case, &before.body);

        for auth_type in updates {
            let updated = update_service_network(
                &lattice,
                &update_identifier,
                serde_json::json!({ "authType": auth_type }),
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?} -> {auth_type}: expected 200, got {} / {}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.str_field("authType"),
                auth_type,
                "case {case:?} -> {auth_type}: the answer must carry the new authType, got {}",
                updated.body
            );
            assert_eq!(
                (updated.str_field("id"), updated.str_field("arn"), updated.str_field("name")),
                (id.clone(), arn.clone(), name.clone()),
                "case {case:?} -> {auth_type}: the answer must name this service network, got {}",
                updated.body
            );

            // The point of the rule: the mutation is readable, not just echoed.
            let read = lattice.get_service_network(&read_identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} -> {auth_type}: read expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                read.str_field("authType"),
                auth_type,
                "case {case:?} -> {auth_type}: the stored authType must be the updated one, got {}",
                read.body
            );
            // Nothing the update did not name may move.
            assert_eq!(
                (read.str_field("id"), read.str_field("arn"), read.str_field("name")),
                (id.clone(), arn.clone(), name.clone()),
                "case {case:?} -> {auth_type}: identity must survive the update, got {}",
                read.body
            );
            assert_eq!(
                read.body["sharingConfig"]["enabled"].as_bool(),
                expected_sharing,
                "case {case:?} -> {auth_type}: expected sharingConfig.enabled \
                 {expected_sharing:?} to survive the update, got {}",
                read.body
            );
            let (read_created_at, read_last_updated_at) = created_and_updated(case, &read.body);
            assert_eq!(
                (read_created_at.secs(), read_created_at.subsec_nanos()),
                (created_at.secs(), created_at.subsec_nanos()),
                "case {case:?} -> {auth_type}: createdAt must not move, got {}",
                read.body
            );
            assert!(
                at_or_after(read_last_updated_at, last_seen),
                "case {case:?} -> {auth_type}: lastUpdatedAt must not go backwards, got {}",
                read.body
            );
            assert!(
                at_or_after(read_last_updated_at, created_at),
                "case {case:?} -> {auth_type}: an updated resource's lastUpdatedAt must be \
                 at or after its createdAt, got {}",
                read.body
            );
            last_seen = read_last_updated_at;
        }
    }
}

/// Rule 1's refusals: an identifier naming no persisted service network is a 404, and a
/// request the modelled `authType` constraint forbids is a 400 — neither writes anything.
#[tokio::test]
async fn an_update_of_an_unknown_network_is_404_and_a_bad_auth_type_is_400() {
    let lattice = Lattice::new();
    let created = lattice
        .create_service_network(serde_json::json!({
            "name": "refusals-net",
            "authType": "AWS_IAM",
        }))
        .await;
    let id = created.str_field("id");
    let arn = created.str_field("arn");

    // (case, identifier, body, expected status)
    let cases: Vec<(&str, String, serde_json::Value, u16)> = vec![
        (
            "an id of the modelled shape that names nothing",
            "sn-00000000000000000".to_string(),
            serde_json::json!({ "authType": "NONE" }),
            404,
        ),
        (
            "an ARN of the modelled shape that names nothing",
            as_label(
                "arn:aws:vpc-lattice:us-west-2:000000000000:servicenetwork/sn-00000000000000000",
            ),
            serde_json::json!({ "authType": "NONE" }),
            404,
        ),
        (
            "authType outside the modelled enum",
            id.clone(),
            serde_json::json!({ "authType": "IAM" }),
            400,
        ),
        (
            "authType in the wrong case",
            id.clone(),
            serde_json::json!({ "authType": "none" }),
            400,
        ),
        (
            "the required authType absent",
            as_label(&arn),
            serde_json::json!({}),
            400,
        ),
    ];

    for (case, identifier, body, expected) in cases {
        let answer = update_service_network(&lattice, &identifier, body).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected {expected}, got {} / {}",
            answer.status, answer.body
        );
        // A refused update changes nothing: the network keeps what it was created with.
        let read = lattice.get_service_network(&id).await;
        assert_eq!(
            read.str_field("authType"),
            "AWS_IAM",
            "case {case:?}: a refused update must not write, got {}",
            read.body
        );
    }
}

/// Rule 2, over every timestamp member of every read this emulator serves: the service
/// network, the service, the service summary a listing carries, the association and the
/// listener. Each is asserted to be the modelled RFC 3339 string, to parse, to report an
/// instant inside this test's own run, and to be stable across two reads.
#[tokio::test]
async fn every_timestamp_member_of_every_read_is_the_modelled_date_time_string() {
    let lattice = Lattice::new();
    let before_any_call = DateTime::from(std::time::SystemTime::now());

    let network = lattice
        .create_service_network(serde_json::json!({ "name": "clock-net" }))
        .await;
    let network_id = network.str_field("id");
    let service = lattice
        .create_service(serde_json::json!({ "name": "clock-svc" }))
        .await;
    let service_id = service.str_field("id");
    let association = lattice.associate(&service_id, &network_id).await;
    let association_id = association.str_field("id");
    let listener = lattice
        .call(
            "POST",
            &format!("/services/{service_id}/listeners"),
            Some(serde_json::json!({
                "name": "clock-listener",
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            })),
        )
        .await;
    assert_eq!(
        listener.status, 201,
        "fixture listener: expected 201, got {} / {}",
        listener.status, listener.body
    );
    let listener_id = listener.str_field("id");

    // (case, path, the timestamp members that read's shape declares)
    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "GetServiceNetwork",
            format!("/servicenetworks/{network_id}"),
            vec!["createdAt", "lastUpdatedAt"],
        ),
        (
            "GetService",
            format!("/services/{service_id}"),
            vec!["createdAt", "lastUpdatedAt"],
        ),
        (
            "GetServiceNetworkServiceAssociation",
            format!("/servicenetworkserviceassociations/{association_id}"),
            vec!["createdAt"],
        ),
        (
            "GetListener",
            format!("/services/{service_id}/listeners/{listener_id}"),
            vec!["createdAt", "lastUpdatedAt"],
        ),
    ];

    for (case, path, members) in cases {
        let read = lattice.call("GET", &path, None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            read.status, read.body
        );
        let again = lattice.call("GET", &path, None).await;
        for member in members {
            let reported = date_time_of(case, member, &read.body);
            assert!(
                at_or_after(reported, before_any_call),
                "case {case:?}: {member} must report an instant from this run, got {}",
                read.body
            );
            let repeated = date_time_of(case, member, &again.body);
            if member == "createdAt" {
                assert_eq!(
                    (repeated.secs(), repeated.subsec_nanos()),
                    (reported.secs(), reported.subsec_nanos()),
                    "case {case:?}: {member} must be stable across reads, got {} then {}",
                    read.body,
                    again.body
                );
            }
        }
    }

    // The same rule inside a list: `ServiceSummary` and
    // `ServiceNetworkServiceAssociationSummary` carry timestamps too.
    for (case, path, members) in [
        (
            "ListServices",
            "/services".to_string(),
            vec!["createdAt", "lastUpdatedAt"],
        ),
        (
            "ListServiceNetworkServiceAssociations",
            format!("/servicenetworkserviceassociations?serviceNetworkIdentifier={network_id}"),
            vec!["createdAt"],
        ),
    ] {
        let listed = lattice.call("GET", &path, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            listed.status, listed.body
        );
        let items = listed.body["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected items, got {}", listed.body))
            .clone();
        assert!(
            !items.is_empty(),
            "case {case:?}: the created resource must be listed, got {}",
            listed.body
        );
        for item in &items {
            for member in &members {
                let reported = date_time_of(case, member, item);
                assert!(
                    at_or_after(reported, before_any_call),
                    "case {case:?}: {member} of a summary must report an instant from this \
                     run, got {item}"
                );
            }
        }
    }
}

/// Rule 3, over every form a create can take for `sharingConfig`: enabled, disabled,
/// a `sharingConfig` without `enabled`, and the member omitted altogether. The setting
/// is reported on the create's own answer and on every later read — the service's own
/// default where the request named none — and an `authType` update leaves it alone.
#[tokio::test]
async fn sharing_config_round_trips_through_the_create_and_every_read() {
    let lattice = Lattice::new();

    // (case, create body, expected sharingConfig.enabled)
    let cases: Vec<(&str, serde_json::Value, Option<bool>)> = vec![
        (
            "sharing enabled",
            serde_json::json!({ "name": "share-on", "sharingConfig": { "enabled": true } }),
            Some(true),
        ),
        (
            "sharing disabled",
            serde_json::json!({ "name": "share-off", "sharingConfig": { "enabled": false } }),
            Some(false),
        ),
        (
            "sharingConfig omitted",
            serde_json::json!({ "name": "share-unset" }),
            Some(state::DEFAULT_SHARING_ENABLED),
        ),
        (
            "sharingConfig present but empty",
            serde_json::json!({ "name": "share-empty", "sharingConfig": {} }),
            Some(state::DEFAULT_SHARING_ENABLED),
        ),
        (
            "sharing enabled beside the other optional members",
            serde_json::json!({
                "name": "share-on-with-tags",
                "authType": "AWS_IAM",
                "sharingConfig": { "enabled": true },
                "tags": { "team": "platform" },
            }),
            Some(true),
        ),
    ];

    for (case, body, expected) in cases {
        let created = lattice.create_service_network(body).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        assert_eq!(
            created.body["sharingConfig"]["enabled"].as_bool(),
            expected,
            "case {case:?}: the create must report sharingConfig.enabled {expected:?}, got {}",
            created.body
        );
        let id = created.str_field("id");
        let arn = created.str_field("arn");

        for (form, identifier) in [("bare id", id.clone()), ("ARN", as_label(&arn))] {
            let read = lattice.get_service_network(&identifier).await;
            assert_eq!(
                read.body["sharingConfig"]["enabled"].as_bool(),
                expected,
                "case {case:?} by {form}: expected sharingConfig.enabled {expected:?}, got {}",
                read.body
            );
        }

        // An update of another member must not disturb it.
        let updated =
            update_service_network(&lattice, &id, serde_json::json!({ "authType": "NONE" })).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: the update expected 200, got {} / {}",
            updated.status, updated.body
        );
        let read = lattice.get_service_network(&id).await;
        assert_eq!(
            read.body["sharingConfig"]["enabled"].as_bool(),
            expected,
            "case {case:?} after an authType update: expected sharingConfig.enabled \
             {expected:?}, got {}",
            read.body
        );
    }
}
