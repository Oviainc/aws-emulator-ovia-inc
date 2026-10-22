// SPDX-License-Identifier: CC0-1.0

//! Two rules about what this service accepts and how it reports what it does
//! not hold, driven end-to-end over the wire (`awsJson1_0`) through the same
//! multi-service router the emulator boots.
//!
//! 1. **An enum member is judged against the value set the SERVICE accepts.**
//!    `UpdateEnrollmentStatus.status` admits the wire set
//!    (`cost_optimization_hub::state::ENROLLMENT_STATUS_WIRE_VALUES`), which
//!    carries a terminal state the published `EnrollmentStatus` enum does not
//!    list (`smithy_query(query="shape", shape="EnrollmentStatus")` gives
//!    `[Active, Inactive]`); every value of the wire set is accepted and stored,
//!    and the refusal of a value outside it advertises that same set.
//! 2. **A resource the caller holds none of is identified by `resourceId`, not
//!    by prose.** `GetRecommendation`'s 404 `ResourceNotFoundException` echoes
//!    the requested id in the required `resourceId` member — "the identifier of
//!    the resource that was not found"
//!    (`smithy_query(query="shape", shape="ResourceNotFoundException")`) — and
//!    carries no message.
//!
//! Each rule is asserted over a table spanning the class it generalises over —
//! every value of the accepted set and several spellings outside it; the form of
//! the id and whether the account holds a catalog at all — rather than the one
//! request the issue recorded.

use serde_json::json;
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{call, enrolled_router, error_code, fields, ok, router};

/// The `EnrollmentStatus` values the service accepts on the wire — read from the
/// service rather than copied here, so the table cannot drift from the set the
/// handler judges against.
use cost_optimization_hub::state::ENROLLMENT_STATUS_WIRE_VALUES;

/// Rule 1 — every status the service's wire set holds is accepted and stored,
/// and a value outside it is refused with a field entry advertising that set.
#[tokio::test]
async fn an_enrollment_status_is_judged_against_the_services_wire_set() {
    let router = router();

    // Each accepted value, including the terminal state the published enum does
    // not list: the write goes through and the answer is the value read back out
    // of the store.
    for status in ENROLLMENT_STATUS_WIRE_VALUES {
        let answered = ok(
            &router,
            "UpdateEnrollmentStatus",
            &json!({"status": status}),
        )
        .await;
        assert_eq!(
            answered["status"].as_str(),
            Some(*status),
            "case {status:?}: expected the stored status back, got {answered}"
        );
        // And a separate read sees what the write stored.
        let listed = ok(
            &router,
            "ListEnrollmentStatuses",
            &json!({"includeOrganizationInfo": false}),
        )
        .await;
        assert_eq!(
            listed["items"][0]["status"].as_str(),
            Some(*status),
            "case {status:?}: expected the stored status in the enrollment list, got {listed}"
        );
    }

    // Values outside the set: another spelling of an accepted one, a value of a
    // DIFFERENT enum of this service, the empty string, one with trailing space,
    // and one that merely contains an accepted value.
    let expected_clause = format!(
        "Member must satisfy enum value set: [{}]",
        ENROLLMENT_STATUS_WIRE_VALUES.join(", ")
    );
    for refused in ["Suspended", "active", "", "All", "Active ", "Failed,Active"] {
        let (status, payload) = call(
            &router,
            "UpdateEnrollmentStatus",
            &json!({"status": refused}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {refused:?}: expected HTTP 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "ValidationException",
            "case {refused:?}: expected a ValidationException, got {code:?} in {payload}"
        );
        assert_eq!(
            fields(&payload),
            vec![("status".to_string(), expected_clause.clone())],
            "case {refused:?}: expected the wire value set in fields, got {payload}"
        );
    }
}

/// Rule 2 — the 404 for an id this account holds no recommendation for carries
/// the requested id in `resourceId` and no message.
///
/// The rows span the form of the id (a uuid of the catalog's shape, one of
/// another shape, a whitespace id, a very long one) and whether the account
/// holds a catalog at all: an account that never enrolled holds none, so every
/// id is unknown to it and gets the same envelope.
#[tokio::test]
async fn a_missing_recommendation_is_identified_by_resource_id() {
    let long_id = "a".repeat(512);
    let unknown: [&str; 6] = [
        "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c9999",
        "dc9df624-5ed8-442d-948d-4cdce4656ce4",
        "does-not-exist",
        " ",
        "0001",
        long_id.as_str(),
    ];

    // (case, the router) — an account whose catalog is readable, and one that
    // never enrolled and so holds no recommendation at all.
    let enrolled = enrolled_router().await;
    let never_enrolled = router();
    let worlds: [(&str, &MultiServiceRouter); 2] = [
        ("enrolled", &enrolled),
        ("never enrolled", &never_enrolled),
    ];

    for (world, router) in worlds {
        for id in unknown {
            let (status, payload) = call(
                router,
                "GetRecommendation",
                &json!({"recommendationId": id}),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {world} {id:?}: expected HTTP 404, got {status} with {payload}"
            );
            let code = error_code(&payload);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {world} {id:?}: expected ResourceNotFoundException, got {code:?} in {payload}"
            );
            assert_eq!(
                payload["resourceId"].as_str(),
                Some(id),
                "case {world} {id:?}: expected the requested id in resourceId, got {payload}"
            );
            assert_eq!(
                payload["message"].as_str(),
                Some(""),
                "case {world} {id:?}: expected no error message, got {payload}"
            );
        }
    }

    // The same reader still finds a recommendation the enrolled account holds,
    // so the 404 is about the id and not about the operation.
    let listed = ok(
        &enrolled,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true}),
    )
    .await;
    let known = listed["items"][0]["recommendationId"]
        .as_str()
        .expect("the enrolled account holds a catalog")
        .to_string();
    let found = ok(
        &enrolled,
        "GetRecommendation",
        &json!({"recommendationId": known}),
    )
    .await;
    assert_eq!(
        found["recommendationId"].as_str(),
        Some(known.as_str()),
        "case known id: expected the recommendation back, got {found}"
    );
}
