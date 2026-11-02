// SPDX-License-Identifier: CC0-1.0

//! Which refusal `UpdateLandingZone` answers with when the request is itself unacceptable
//! AND an asynchronous operation is still changing the landing zone, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies — not the handlers'
//! Rust signatures.
//!
//! The rule under test, the one the sibling update handlers apply and
//! `crate::update_refusal_order` asserts for an enablement: an update settles WHAT the
//! request asks for — whether the configuration it names is one the resource can be moved
//! to at all — before it looks at whether an operation is still changing that resource, so
//! a request the service would never accept is refused with the modelled
//! `ValidationException` (400) in the same words whether or not the clock has made that
//! operation due, and the modelled `ConflictException` (409) answers only a request the
//! service would otherwise accept. Both statuses are the modelled ones, from
//! `smithy_query(query="operation", operation="UpdateLandingZone")`.
//!
//! For a landing zone the never-acceptable request is a move BACKWARDS: an update deploys
//! the requested version over the version the landing zone has, so a numerically older one
//! is a configuration it cannot reach. The version it is ordered against is the one the
//! landing zone is on its way to while a deployment is in flight, so a version already
//! accepted cannot be walked backwards by a call that arrives before it is due.
//!
//! The table's rows differ along WHICH call put the operation in flight (the create itself,
//! an update, a reset, a decommissioning) and along the size of the move backwards (one
//! minor step, back past the deployed version, the whole published line). Every version a
//! row uses is drawn from `state::CATALOG_LANDING_ZONE_VERSIONS` — the versions the service
//! offers, so no row can be refused for naming a version the catalog does not publish —
//! and no refusal message is spelled out here: the message a refusal carries with an
//! operation in flight is compared with the message the SAME request gets once the clock
//! has made everything due.

use service_com_amazonaws_controltower_awscontroltowerapis::state::CATALOG_LANDING_ZONE_VERSIONS;
use smithy_server_runtime::MultiServiceRouter;

use crate::landing_zone_lifecycle::{
    call, emulator, manifest_governing, past_the_settle_span, Answer,
};

/// Which asynchronous call leaves an operation in flight against the landing zone while a
/// row's refused update is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InFlight {
    /// The `CREATE` the row's own landing zone started, not yet due.
    Create,
    /// An `UPDATE` moving the landing zone forward, not yet due.
    Update,
    /// A `RESET` re-deploying the configuration already in place.
    Reset,
    /// A `DELETE` decommissioning the landing zone.
    Delete,
}

/// One row: a landing zone in a given state, with a given operation in flight, and an
/// update the service can never accept against it.
struct Row {
    case: &'static str,
    /// The version the landing zone is created at.
    created: &'static str,
    /// What is still in flight when the refused update is sent.
    in_flight: InFlight,
    /// For [`InFlight::Update`], the version that update is on its way to.
    in_flight_to: Option<&'static str>,
    /// The version the refused update asks for — older than the version the landing zone
    /// is on or on its way to, so it is never acceptable.
    requested: &'static str,
}

impl Row {
    /// The version a further update is ordered against: the one an in-flight deployment is
    /// on its way to while there is one, otherwise the deployed one.
    fn configured(&self) -> &'static str {
        self.in_flight_to.unwrap_or(self.created)
    }
}

/// POST `/update-landingzone` asking for `version`.
async fn update(router: &MultiServiceRouter, arn: &str, version: &str) -> Answer {
    call(
        router,
        "/update-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn, "version": version }),
    )
    .await
}

/// POST `/get-landingzone` for `identifier`: the whole answer, so a row can assert a
/// refusal as well as a reported landing zone.
async fn get_landing_zone(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": identifier }),
    )
    .await
}

/// A member of the reported `LandingZoneDetail`, as a `&str`.
fn detail_member<'a>(answer: &'a Answer, name: &str) -> Option<&'a str> {
    answer
        .get("landingZone")
        .get(name)
        .and_then(serde_json::Value::as_str)
}

/// Create the account's one landing zone at `version`, asserting it succeeded, and answer
/// its ARN. Nothing waits for the clock here: a row that wants the `CREATE` still in flight
/// sends its update straight away.
async fn create_landing_zone(router: &MultiServiceRouter, case: &str, version: &str) -> String {
    let created = call(
        router,
        "/create-landingzone",
        serde_json::json!({
            "version": version,
            "manifest": manifest_governing(&["us-west-2"]),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the create to return 200, got {} body {:?}",
        created.status, created.body
    );
    created.string("arn")
}

/// Put the row's operation in flight and answer the ARN of the landing zone it is against.
async fn landing_zone_with_an_operation_in_flight(router: &MultiServiceRouter, row: &Row) -> String {
    let case = row.case;
    let arn = create_landing_zone(router, case, row.created).await;
    if row.in_flight == InFlight::Create {
        return arn;
    }

    // The CREATE has to be due before another call against the landing zone is accepted.
    tokio::time::sleep(past_the_settle_span()).await;
    let started = match row.in_flight {
        InFlight::Create => unreachable!("handled above"),
        InFlight::Update => {
            let version = row
                .in_flight_to
                .expect("an update row names the version its update is on its way to");
            update(router, &arn, version).await
        }
        InFlight::Reset => {
            call(
                router,
                "/reset-landingzone",
                serde_json::json!({ "landingZoneIdentifier": arn }),
            )
            .await
        }
        InFlight::Delete => {
            call(
                router,
                "/delete-landingzone",
                serde_json::json!({ "landingZoneIdentifier": arn }),
            )
            .await
        }
    };
    assert_eq!(
        started.status, 200,
        "case {case:?}: the call that puts an operation in flight expected 200, got {} body \
         {:?}",
        started.status, started.body
    );
    arn
}

/// The table. Built from the catalog, so the rows follow the published version line
/// wherever it grows rather than naming one recorded pair.
fn rows() -> Vec<Row> {
    let catalog = CATALOG_LANDING_ZONE_VERSIONS;
    assert!(
        catalog.len() >= 4,
        "the rows below need at least four published versions, got {catalog:?}"
    );
    let oldest = catalog[0];
    let newest = catalog[catalog.len() - 1];
    let second_newest = catalog[catalog.len() - 2];
    let middle = catalog[catalog.len() / 2];
    let below_middle = catalog[catalog.len() / 2 - 1];

    vec![
        Row {
            case: "the create still settling, one minor step back",
            created: middle,
            in_flight: InFlight::Create,
            in_flight_to: None,
            requested: below_middle,
        },
        Row {
            case: "the create still settling, the whole published line back",
            created: newest,
            in_flight: InFlight::Create,
            in_flight_to: None,
            requested: oldest,
        },
        Row {
            case: "an update on its way forward, a step back from where it is going",
            created: oldest,
            in_flight: InFlight::Update,
            in_flight_to: Some(newest),
            requested: second_newest,
        },
        Row {
            case: "an update on its way forward, back past the deployed version",
            created: middle,
            in_flight: InFlight::Update,
            in_flight_to: Some(newest),
            requested: oldest,
        },
        Row {
            case: "a reset re-deploying the configuration in place",
            created: middle,
            in_flight: InFlight::Reset,
            in_flight_to: None,
            requested: oldest,
        },
        Row {
            case: "a decommissioning still running",
            created: newest,
            in_flight: InFlight::Delete,
            in_flight_to: None,
            requested: oldest,
        },
    ]
}

/// RULE: an `UpdateLandingZone` asking for a version older than the one the landing zone is
/// on or on its way to is the modelled `ValidationException` (400) — in the same words —
/// whether an operation is still changing that landing zone or nothing is, while the
/// modelled `ConflictException` (409) answers the same call asking for a version the
/// landing zone can reach; neither refusal writes anything.
#[tokio::test]
async fn an_update_to_an_older_version_is_a_bad_request_while_an_operation_is_in_flight() {
    for row in rows() {
        let case = row.case;
        // A router per row: the account holds one landing zone, and a row may remove it.
        let router = emulator();
        let arn = landing_zone_with_an_operation_in_flight(&router, &row).await;

        // The rows really do overlap an operation: a landing zone whose transition the
        // clock still owes reports itself as being changed.
        let reported = get_landing_zone(&router, &arn).await;
        assert_eq!(
            (reported.status, detail_member(&reported, "status")),
            (200, Some("PROCESSING")),
            "case {case:?}: expected the landing zone to read PROCESSING while the operation \
             is in flight, got {:?} body {:?}",
            reported.status,
            reported.body
        );

        // WHAT the request asks for decides: a move backwards is the modelled 400, not the
        // conflict the landing zone's own state would otherwise answer.
        let refused = update(&router, &arn, row.requested).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?} ({} -> {}): expected 400 ValidationException while the operation \
             is in flight, got {:?} body {:?}",
            row.configured(),
            row.requested,
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        let in_flight_message = refused.string("message");
        assert!(
            in_flight_message.contains(row.requested) && in_flight_message.contains(row.configured()),
            "case {case:?}: expected the refusal to name the requested version {:?} and the \
             version the landing zone is on its way to {:?}, got {in_flight_message:?}",
            row.requested,
            row.configured()
        );

        // Not vacuous: the same call asking for a version the landing zone CAN reach — the
        // one it is on or on its way to, re-applied — is what the conflict answers.
        let conflicted = update(&router, &arn, row.configured()).await;
        assert_eq!(
            (conflicted.status, conflicted.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected 409 ConflictException for a request the service would \
             otherwise accept, got {:?} body {:?}",
            (conflicted.status, conflicted.error_code.as_deref()),
            conflicted.body
        );

        tokio::time::sleep(past_the_settle_span()).await;

        if row.in_flight == InFlight::Delete {
            // The decommissioning completed, so the landing zone the refusals were sent to
            // is gone — which is also what says neither refusal wrote anything: the
            // decommissioning in flight ran to completion.
            let gone = get_landing_zone(&router, &arn).await;
            assert_eq!(
                (gone.status, gone.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected the decommissioning to have removed the landing \
                 zone, got {:?} body {:?}",
                (gone.status, gone.error_code.as_deref()),
                gone.body
            );
            continue;
        }

        // Neither refusal wrote anything: the configuration the in-flight operation was
        // deploying is the one deployed, and the landing zone is settled again.
        let settled = get_landing_zone(&router, &arn).await;
        assert_eq!(
            (
                settled.status,
                detail_member(&settled, "version"),
                detail_member(&settled, "status"),
            ),
            (200, Some(row.configured()), Some("ACTIVE")),
            "case {case:?}: expected the refusals to leave {:?} deployed and ACTIVE, got {:?} \
             body {:?}",
            row.configured(),
            settled.status,
            settled.body
        );

        // The SAME request, with nothing in flight any more: the same modelled error in the
        // same words, so the answer does not depend on whether the clock has made the
        // deployment due.
        let again = update(&router, &arn, row.requested).await;
        assert_eq!(
            (
                again.status,
                again.error_code.as_deref(),
                again.string("message"),
            ),
            (400, Some("ValidationException"), in_flight_message.clone()),
            "case {case:?}: expected the same request to be refused in the same words once \
             nothing is in flight, got {:?} body {:?}",
            (again.status, again.error_code.as_deref()),
            again.body
        );
    }
}
