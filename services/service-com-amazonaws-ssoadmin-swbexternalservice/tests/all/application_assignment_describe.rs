// SPDX-License-Identifier: CC0-1.0

//! `DescribeApplicationAssignment` end to end over the `awsJson1_1` wire.
//!
//! The rule under test: the report is a function of the STORED grant alone. A tuple some
//! `CreateApplicationAssignment` wrote reads back as exactly that tuple — the three
//! members the model's `DescribeApplicationAssignmentResponse` declares and no others
//! (`smithy_query(query="operation",operation="DescribeApplicationAssignment")`) — and a
//! tuple no grant wrote is refused `ResourceNotFoundException`, whatever else about the
//! call varies: the Region the request is signed for, the application's `Status`, how many
//! other grants the store holds and the order they were written in. The describe is a
//! REPORT, so it writes nothing: the store holds the same rows after it as before.
//!
//! The rows below differ along what that rule generalises over — Region, application
//! status, principal type, the two forms the model's `PrincipalId` pattern admits, and the
//! NUMBER of stored grants — rather than renaming one example. The surrounding slice
//! (minting, repeated grants, the instance cascade) is `application_assignments.rs`'s
//! subject; this suite is about the report itself.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::Value;

// The application builder and the wire helpers of this slice are shared, so this suite
// sends the same requests `application_assignments.rs` does and differs only in what it
// asserts.
use crate::application_assignments::{minted_application, Application};
use crate::ssoadmin_permission_sets::{
    assert_refused, create_application_assignment as grant,
    describe_application_assignment_in_region as describe_in_region, router_over,
    seeded_instance_arn, with_store, TEST_REGION,
};

/// The Regions a describe is signed for. The first is the Region the account's instance is
/// enabled in; the others are in other partitions' geographies, so a report that leaked
/// the request's Region into its lookup would answer differently for them.
///
/// An `ApplicationArn` carries no Region segment (`arn:aws:sso::<account>:application/…`),
/// so the application an ARN names is the same resource whichever Region asks about it.
const DESCRIBE_REGIONS: &[&str] = &[TEST_REGION, "eu-west-1", "ap-southeast-2"];

/// The principals the rule is asserted over: both `PrincipalType` values the model
/// declares (`USER | GROUP`) and both forms its `PrincipalId` pattern admits — a bare GUID
/// and the `[0-9a-f]{10}-` prefixed identity-store form
/// (`^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`).
const PRINCIPALS: &[(&str, &str, &str)] = &[
    (
        "a bare USER GUID",
        "USER",
        "7c2e4b61-5d1a-4f08-9b3c-11aa22bb33cc",
    ),
    (
        "a bare GROUP GUID in upper hex",
        "GROUP",
        "DEADBEEF-1234-4ABC-8DEF-0123456789AB",
    ),
    (
        "a prefixed USER id",
        "USER",
        "1a2b3c4d5e-7c2e4b61-5d1a-4f08-9b3c-11aa22bb33cc",
    ),
    (
        "a prefixed GROUP id",
        "GROUP",
        "0123456789-abcdef01-2345-4678-89ab-cdef01234567",
    ),
];

/// The three members `DescribeApplicationAssignmentResponse` declares. A report that
/// carried anything else would be reporting something the model does not model.
const RESPONSE_MEMBERS: &[&str] = &["ApplicationArn", "PrincipalType", "PrincipalId"];

/// Assert one answer IS the report of `(application_arn, principal_type, principal_id)`:
/// the three modelled members, each the value the grant stored, and nothing else.
fn assert_reports_tuple(
    case: &str,
    status: u16,
    body: &Value,
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) {
    assert_eq!(
        status, 200,
        "case {case:?}: a granted assignment must describe 200, got {status} {body}"
    );
    assert_eq!(
        body["ApplicationArn"].as_str(),
        Some(application_arn),
        "case {case:?}: expected the granted ApplicationArn {application_arn}, got {body}"
    );
    assert_eq!(
        body["PrincipalType"].as_str(),
        Some(principal_type),
        "case {case:?}: expected the granted PrincipalType {principal_type}, got {body}"
    );
    assert_eq!(
        body["PrincipalId"].as_str(),
        Some(principal_id),
        "case {case:?}: expected the granted PrincipalId {principal_id} byte for byte, got {body}"
    );
    let members = body
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the answer must be a JSON object, got {body}"));
    let unexpected: Vec<&String> = members
        .keys()
        .filter(|key| !RESPONSE_MEMBERS.contains(&key.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "case {case:?}: the report carries only {RESPONSE_MEMBERS:?}, got the extra \
         {unexpected:?} in {body}"
    );
}

/// The whole report rule: every granted tuple reads back as itself, from every Region, on
/// an ENABLED and on a DISABLED application, and the describes write nothing.
#[tokio::test]
async fn a_granted_tuple_reports_itself_from_every_region_and_whatever_the_application_status() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;

    // The applications differ in the one member of their own record that a report of an
    // assignment must not depend on: an assignment is granted and reported the same way on
    // a disabled application as on an enabled one.
    let applications = [
        (
            "an application whose create defaulted Status",
            minted_application(
                &router,
                &instance_arn,
                &Application::new("defaulted Status", "default-status-app"),
            )
            .await,
        ),
        (
            "an explicitly ENABLED application",
            minted_application(
                &router,
                &instance_arn,
                &Application::new("explicit ENABLED", "enabled-app").with_status("ENABLED"),
            )
            .await,
        ),
        (
            "a DISABLED application",
            minted_application(
                &router,
                &instance_arn,
                &Application::new("explicit DISABLED", "disabled-app").with_status("DISABLED"),
            )
            .await,
        ),
    ];

    let mut expected_rows = 0usize;
    for (application_case, application_arn) in &applications {
        for (principal_case, principal_type, principal_id) in PRINCIPALS {
            let (status, body) = grant(&router, application_arn, principal_type, principal_id).await;
            assert_eq!(
                status, 200,
                "case {application_case:?} / {principal_case:?}: the grant must answer 200, \
                 got {status} {body}"
            );
            expected_rows += 1;
        }
    }
    assert_eq!(
        with_store(&engine, |store| store.application_assignments.len()),
        expected_rows,
        "every distinct grant of the table must have stored one row"
    );

    for region in DESCRIBE_REGIONS {
        for (application_case, application_arn) in &applications {
            for (principal_case, principal_type, principal_id) in PRINCIPALS {
                let case = format!("{application_case} / {principal_case} described in {region}");
                let (status, body) = describe_in_region(
                    &router,
                    region,
                    application_arn,
                    principal_type,
                    principal_id,
                )
                .await;
                assert_reports_tuple(
                    &case,
                    status,
                    &body,
                    application_arn,
                    principal_type,
                    principal_id,
                );
            }
        }
    }

    // The report wrote nothing: the same rows, and every one of them still its own tuple.
    assert_eq!(
        with_store(&engine, |store| store.application_assignments.len()),
        expected_rows,
        "a describe must store no row and remove none"
    );
    for (application_case, application_arn) in &applications {
        for (principal_case, principal_type, principal_id) in PRINCIPALS {
            let case = format!("{application_case} / {principal_case} after every describe");
            let stored = with_store(&engine, |store| {
                store.application_assignment_position(application_arn, principal_type, principal_id)
            });
            assert!(
                stored.is_some(),
                "case {case:?}: the stored grant must have survived the describes"
            );
        }
    }
}

/// The same rule at SCALE: many grants on one application, each still reported as its own
/// row whatever order they are asked about in, and a tuple one hex digit away from a
/// granted one is not that grant.
#[tokio::test]
async fn each_of_many_grants_reports_its_own_row_in_any_order() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = minted_application(
        &router,
        &instance_arn,
        &Application::new("one application, many principals", "many-principals-app"),
    )
    .await;

    // A GUID per row, generated along the model's `PrincipalId` pattern: 8-4-4-4-12 hex
    // with the version and variant nibbles the pattern fixes.
    let principals: Vec<(String, &str, String)> = (0u32..24)
        .map(|index| {
            let principal_type = if index % 2 == 0 { "USER" } else { "GROUP" };
            let principal_id = format!("{index:08x}-0000-4000-8000-{index:012x}");
            (
                format!("{principal_type} #{index}"),
                principal_type,
                principal_id,
            )
        })
        .collect();

    for (case, principal_type, principal_id) in &principals {
        let (status, body) = grant(&router, &application_arn, principal_type, principal_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the grant must answer 200, got {status} {body}"
        );
    }
    assert_eq!(
        with_store(&engine, |store| store.application_assignments.len()),
        principals.len(),
        "each distinct principal must have stored its own row"
    );

    // Asked about in REVERSE grant order: the report resolves the tuple, not the order.
    for (case, principal_type, principal_id) in principals.iter().rev() {
        let (status, body) = describe_in_region(
            &router,
            TEST_REGION,
            &application_arn,
            principal_type,
            principal_id,
        )
        .await;
        assert_reports_tuple(
            case,
            status,
            &body,
            &application_arn,
            principal_type,
            principal_id,
        );
    }

    // A principal id that differs from a granted one in a SINGLE hex digit holds no
    // assignment: the identity is the whole id, and the store holds as many rows it could
    // have matched loosely against. The near miss is DERIVED from a granted id by flipping
    // its last nibble, so exactly one character of it differs.
    let (_, granted_type, granted_id) = &principals[0];
    let mut near_miss = granted_id.clone();
    let last = near_miss
        .pop()
        .expect("a generated principal id is not empty");
    near_miss.push(if last == '0' { '1' } else { '0' });
    assert_eq!(
        near_miss.len(),
        granted_id.len(),
        "the near miss must differ from {granted_id} in one character, not in its length"
    );
    assert_eq!(
        near_miss
            .chars()
            .zip(granted_id.chars())
            .filter(|(near, granted)| near != granted)
            .count(),
        1,
        "the near miss {near_miss} must differ from the granted {granted_id} in exactly \
         one character"
    );
    let (status, body) = describe_in_region(
        &router,
        TEST_REGION,
        &application_arn,
        granted_type,
        &near_miss,
    )
    .await;
    assert_refused(
        "a principal id one hex digit from a granted one",
        "DescribeApplicationAssignment",
        "ResourceNotFoundException",
        status,
        &body,
    );
    // The refusal is built from the request: it names the tuple that resolved to nothing,
    // which is what tells it from a refusal about some other resource of the call.
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default();
    assert!(
        message.contains(&near_miss) && message.contains(&application_arn),
        "the refusal must name the principal and the application it was looked for on, \
         got {message:?}"
    );

    // Nothing the refusal touched: the 24 grants are all still there.
    assert_eq!(
        with_store(&engine, |store| store.application_assignments.len()),
        principals.len(),
        "a refused describe must neither grant nor remove a row"
    );
}
