// SPDX-License-Identifier: CC0-1.0

//! The DUPLICATE GRANT of a direct application assignment, end to end over the
//! `awsJson1_1` wire.
//!
//! THE RULE under test: a grant the model does not mark `@idempotent` refuses, with
//! `ConflictException` whose `Message` names the application and the principal the request
//! sent, a request whose assignment the application already holds (a conflict with the
//! previous successful write), and leaves the stored assignments exactly as they were; the
//! same principal is accepted on an application that does not hold it, and again after a
//! delete has removed it. Which write that is follows from the model:
//! `smithy_query(query="operation", operation="CreateApplicationAssignment")` reports
//! `ConflictException` and NO `idempotent` flag, while the same query for
//! `DeleteApplicationAssignment` reports `"idempotent": true`.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over: the
//! `PrincipalType` (`USER | GROUP`), the FORM the model's `PrincipalId` pattern admits (a
//! bare GUID in lower and in upper hex, and the `[0-9a-f]{10}-` prefixed form at the
//! 47-character length limit), the APPLICATION the grant is made on, and the NUMBER of
//! repeats.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.
//!
//! Every modelled 4xx refusal of this service is answered with HTTP 400 whatever
//! `@httpError` its shape declares, so the assertions below pin the status AND the wire
//! error code, which is what tells one refusal from another.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::application_assignments::{minted_application, Application};
use crate::ssoadmin_permission_sets::{
    assert_refused, call, create_application_assignment as grant, describe_application_assignment,
    router, seeded_instance_arn,
};

/// One row: a principal to grant, in one of the forms the model's `PrincipalId` pattern
/// and `PrincipalType` enum admit.
struct Row {
    label: &'static str,
    principal_type: &'static str,
    principal_id: &'static str,
}

/// The table. The rows span both values of `PrincipalType` and both forms of
/// `PrincipalId` — SOURCE for both:
/// `smithy_query(query="operation", operation="CreateApplicationAssignment")`, whose
/// `PrincipalType` is `GROUP | USER` and whose `PrincipalId` is
/// `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`
/// of length 1..47.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a lowercase-hex USER GUID",
            principal_type: "USER",
            principal_id: "7b9c1d2e-3f40-4a51-8b62-c7d8e9f01234",
        },
        Row {
            label: "an uppercase-hex USER GUID",
            principal_type: "USER",
            principal_id: "1D2E3F40-5A6B-4C7D-8E9F-0A1B2C3D4E5F",
        },
        Row {
            label: "a lowercase-hex GROUP GUID",
            principal_type: "GROUP",
            principal_id: "2c3d4e5f-6071-4829-8a3b-4c5d6e7f8091",
        },
        Row {
            // The prefixed form at the member's 47-character maximum.
            label: "a prefixed GROUP id at the 47-character limit",
            principal_type: "GROUP",
            principal_id: "1f2e3d4c5b-0a1b2c3d-4e5f-4071-8293-a4b5c6d7e8f9",
        },
        Row {
            label: "a prefixed USER id at the 47-character limit",
            principal_type: "USER",
            principal_id: "00112233aa-ffeeddcc-bbaa-4998-8776-554433221100",
        },
    ]
}

impl Row {
    /// The MESSAGE this row's conflict must carry: the fixed text the refusal of a grant
    /// the application already holds is worded with, with the assignment's identity filled
    /// in. Every variable part comes from the request and from the application the test
    /// minted, so no value here is one a single run happened to produce.
    fn conflict_message(&self, application_arn: &str) -> String {
        format!(
            "An assignment of application {application_arn} to {} {} already exists.",
            self.principal_type, self.principal_id
        )
    }

    /// The row's assignment as its identity: what the rest of the tuple is once the
    /// application is fixed.
    fn identity(&self) -> String {
        format!("{}#{}", self.principal_type, self.principal_id)
    }
}

/// Delete one direct application assignment, the way a client does.
async fn delete_assignment(
    router: &MultiServiceRouter,
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteApplicationAssignment",
        &json!({
            "ApplicationArn": application_arn,
            "PrincipalType": principal_type,
            "PrincipalId": principal_id,
        }),
    )
    .await
}

/// The assignments the application holds, read over the wire, sorted so the comparison is
/// about the SET rather than about a page order.
async fn held(router: &MultiServiceRouter, application_arn: &str, case: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListApplicationAssignments",
        &json!({"ApplicationArn": application_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListApplicationAssignments expected 200, got {status} {body}"
    );
    let mut found: Vec<String> = body["ApplicationAssignments"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ApplicationAssignments must report a list, got {body}")
        })
        .iter()
        .map(|item| {
            format!(
                "{}#{}",
                item["PrincipalType"].as_str().unwrap_or_default(),
                item["PrincipalId"].as_str().unwrap_or_default()
            )
        })
        .collect();
    found.sort();
    found
}

/// Mint one application to grant assignments on.
async fn application(router: &MultiServiceRouter, instance_arn: &str, name: &str) -> String {
    minted_application(router, instance_arn, &Application::new(name, name)).await
}

/// THE RULE: the assignment the application already holds is refused `ConflictException`,
/// however often the request is repeated, and the stored assignments keep exactly the one
/// row the first grant made.
#[tokio::test]
async fn a_repeated_grant_of_an_assignment_the_application_holds_is_refused() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    for (index, row) in rows().into_iter().enumerate() {
        let application_arn =
            application(&router, &instance_arn, &format!("conflict-app-{index}")).await;

        let (status, body) = grant(
            &router,
            &application_arn,
            row.principal_type,
            row.principal_id,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the first grant expected 200, got {status} {body}",
            row.label
        );

        for attempt in 2..=4 {
            let (status, body) = grant(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_refused(
                row.label,
                &format!("grant #{attempt}"),
                "ConflictException",
                status,
                &body,
            );
            let found = held(&router, &application_arn, row.label).await;
            assert_eq!(
                found,
                vec![row.identity()],
                "case {:?}: after grant #{attempt} the application must still hold exactly \
                 {:?}, got {found:?}",
                row.label,
                row.identity()
            );
            // The refused repeat left the grant readable: the access the first grant made
            // is neither removed nor duplicated.
            let (status, body) = describe_application_assignment(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: after grant #{attempt} the assignment must still describe, got \
                 {status} {body}",
                row.label
            );
        }
    }
}

/// The conflict is about the assignment THIS application holds: the same principal is
/// accepted on an application that does not hold it, so the refusal never leaks across
/// applications.
#[tokio::test]
async fn the_same_principal_is_accepted_on_an_application_that_does_not_hold_it() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    for (index, row) in rows().into_iter().enumerate() {
        let holder = application(&router, &instance_arn, &format!("holder-app-{index}")).await;
        let other = application(&router, &instance_arn, &format!("other-app-{index}")).await;

        for (case, application_arn) in [("the holder", &holder), ("another application", &other)] {
            let (status, body) = grant(
                &router,
                application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the grant on {case} expected 200, got {status} {body}",
                row.label
            );
            let found = held(&router, application_arn, row.label).await;
            assert_eq!(
                found,
                vec![row.identity()],
                "case {:?}: {case} must hold {:?}, got {found:?}",
                row.label,
                row.identity()
            );
        }

        // Each application now holds the assignment, so the repeat is refused on BOTH.
        for (case, application_arn) in [("the holder", &holder), ("another application", &other)] {
            let (status, body) = grant(
                &router,
                application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_refused(
                row.label,
                &format!("the repeat on {case}"),
                "ConflictException",
                status,
                &body,
            );
        }
    }
}

/// The refusal's MESSAGE, which is as much a part of the answer as the status and the wire
/// code: the conflict names the application the request sent it for and the principal it
/// named, so a client reading it can tell WHICH grant already exists. Asserted on TWO
/// applications per row, so the message is shown to follow the request rather than being
/// one fixed text.
#[tokio::test]
async fn the_conflict_message_names_the_application_and_the_principal() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    for (index, row) in rows().into_iter().enumerate() {
        let mut messages: Vec<String> = Vec::new();
        for (which, label) in [(0, "the first application"), (1, "a second one")] {
            let application_arn =
                application(&router, &instance_arn, &format!("msg-app-{index}x{which}")).await;

            let (status, body) = grant(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the grant on {label} expected 200, got {status} {body}",
                row.label
            );

            let (status, body) = grant(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_refused(
                row.label,
                &format!("the repeat on {label}"),
                "ConflictException",
                status,
                &body,
            );
            // The model names the member `ConflictException$Message`, so that is where the
            // detail text is read from.
            let message = body["Message"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                message,
                row.conflict_message(&application_arn),
                "case {:?}: the conflict on {label} must name that application and the \
                 principal, got {message:?}",
                row.label
            );
            messages.push(message);
        }
        assert_ne!(
            messages[0], messages[1],
            "case {:?}: the message follows the application the request named, got \
             {messages:?}",
            row.label
        );
    }
}

/// A delete removes the assignment, so the write the conflict refers to is gone: the same
/// grant is accepted again, and refused again once it has been made.
#[tokio::test]
async fn an_assignment_a_delete_removed_can_be_granted_again() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    for (index, row) in rows().into_iter().enumerate() {
        let application_arn =
            application(&router, &instance_arn, &format!("cycle-app-{index}")).await;

        // Two full grant / refused-repeat / delete cycles, so the rule holds whatever the
        // application's history is.
        for cycle in 1..=2 {
            let (status, body) = grant(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the grant of cycle {cycle} expected 200, got {status} {body}",
                row.label
            );

            let (status, body) = grant(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_refused(
                row.label,
                &format!("the repeat of cycle {cycle}"),
                "ConflictException",
                status,
                &body,
            );

            let (status, body) = delete_assignment(
                &router,
                &application_arn,
                row.principal_type,
                row.principal_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the delete of cycle {cycle} expected 200, got {status} {body}",
                row.label
            );
            let found = held(&router, &application_arn, row.label).await;
            assert!(
                found.is_empty(),
                "case {:?}: after the delete of cycle {cycle} the application must hold \
                 nothing, got {found:?}",
                row.label
            );
        }
    }
}
