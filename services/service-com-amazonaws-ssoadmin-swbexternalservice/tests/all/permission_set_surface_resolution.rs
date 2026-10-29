// SPDX-License-Identifier: CC0-1.0

//! The ONE resolution the whole permission-set / account-assignment / ABAC surface shares,
//! driven end to end over the `awsJson1_1` wire through an operation of every kind that
//! performs it.
//!
//! The rule under test, which every operation of that surface applies to the ARNs its
//! request names, in this order:
//!
//! 1. the surface is a feature of an ORGANIZATION instance, so on an ACCOUNT instance the
//!    call is refused with the modelled `ValidationException` BEFORE any resource is looked
//!    up — the refusal cannot depend on whether the resource named exists, because on such
//!    an instance none ever can;
//! 2. the instance itself — an `InstanceArn` this account holds no instance for is a
//!    missing resource, the modelled `ResourceNotFoundException`, never the refusal above,
//!    which belongs to a RESOLVED instance;
//! 3. and, where the request names one, the permission set WITHIN that instance — never
//!    created, already deleted, or held under another instance is the same answer either
//!    way, the modelled `ResourceNotFoundException`.
//!
//! Each refusal is asserted on its wire code AND on its exact message, over a table
//! spanning every operation of the surface — the gate-and-instance half (a request that
//! names no permission set, including the ABAC operations) and the permission-set half —
//! and over the input variants `account_instance_restriction.rs` already spans, so the
//! operations are pinned to ONE answer rather than to many that happen to agree today.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints, the
//! `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than bypassed.
//! The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;

use crate::account_instance_restriction::{surface_calls, SurfaceCall, VARIANTS};
use crate::inline_policy::create_permission_set;
use crate::ssoadmin_permission_sets::{
    account_instance_router, call, error_code, organization_engine, router_over, with_store,
    TEST_REGION, UNKNOWN_INSTANCE_ARN,
};

/// The instance id of the SECOND instance the scoping rows need: `ssoins-` + 16 of
/// `[a-zA-Z0-9-.]`, as the model's `InstanceArn` pattern requires.
const OTHER_INSTANCE_ID: &str = "ssoins-6f5e4d3c2b1a0987";

/// Its identity store: `d-` + 10 hex digits, per `InstanceIdentityStoreArn`.
const OTHER_IDENTITY_STORE_ID: &str = "d-6f5e4d3c2b";

/// The `Message` a refusal carries, whichever casing the envelope used for the member.
fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Assert one call was refused with `expected_code` AND with exactly `expected_message`:
/// the message is part of the answer, so a wording that drifted between two copies of the
/// rule is a failure here.
fn assert_refusal(
    case: &str,
    expected_code: &str,
    expected_message: &str,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: the refusal is HTTP 400 on this awsJson1_1 surface, got \
         {status} {body}"
    );
    assert_eq!(
        error_code(body),
        expected_code,
        "case {case:?}: expected {expected_code}, got {body}"
    );
    assert_eq!(
        message(body),
        expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {body}"
    );
}

/// The calls of the surface whose request names a PERMISSION SET — the half that takes the
/// third step of the resolution. The member the request carries is what says so, so the
/// rows are read off the table rather than restated.
fn permission_set_calls(instance_arn: &str) -> Vec<SurfaceCall> {
    VARIANTS
        .iter()
        .flat_map(|variant| surface_calls(instance_arn, variant))
        .filter(|case| case.body.get("PermissionSetArn").is_some())
        .collect()
}

/// The calls of the surface whose request names NO permission set — the half that takes
/// only the gate and the instance lookup (`CreatePermissionSet`, the enumerations, the
/// status reads and all four ABAC operations).
fn instance_only_calls(instance_arn: &str) -> Vec<SurfaceCall> {
    VARIANTS
        .iter()
        .flat_map(|variant| surface_calls(instance_arn, variant))
        .filter(|case| case.body.get("PermissionSetArn").is_none())
        .collect()
}

/// Both halves of the surface are in the table: a count asserted so none of the tables
/// below can pass by driving nothing.
fn assert_both_halves_covered(case: &str, instance_arn: &str) {
    let with_set = permission_set_calls(instance_arn).len();
    let without_set = instance_only_calls(instance_arn).len();
    assert!(
        with_set >= 19 * VARIANTS.len(),
        "case {case:?}: the permission-set half must cover every operation that names one, \
         got {with_set} calls"
    );
    assert!(
        without_set >= 10 * VARIANTS.len(),
        "case {case:?}: the gate-and-instance half must cover every operation that names no \
         permission set, got {without_set} calls"
    );
}

/// STEP 1, over every operation of the surface and every input variant: on an account
/// instance each one answers the SAME `ValidationException` message, whatever resource the
/// request names and whether or not it exists.
#[tokio::test]
async fn the_withdrawn_gate_answers_one_message_across_the_surface() {
    let (router, instance_arn) = account_instance_router().await;

    let mut seen = 0_usize;
    for variant in VARIANTS {
        for case in surface_calls(&instance_arn, variant) {
            let (status, body) = call(&router, case.operation, &case.body).await;
            assert_refusal(
                &case.label,
                "ValidationException",
                state::ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE,
                status,
                &body,
            );
            seen += 1;
        }
    }
    assert!(
        seen >= 29 * VARIANTS.len(),
        "case \"the whole surface\": the table must cover every operation of the surface, \
         drove only {seen} calls"
    );
}

/// STEP 2, over the same table: an `InstanceArn` this account holds no instance for is a
/// missing resource on every operation of the surface, with the crate's single wording for
/// that condition — the gate above is not the answer, because it belongs to a resolved
/// instance.
#[tokio::test]
async fn an_unknown_instance_answers_one_message_across_the_surface() {
    let (engine, _instance_arn) = organization_engine();
    let router = router_over(engine);
    let expected = state::instance_not_found_message(UNKNOWN_INSTANCE_ARN);
    assert_both_halves_covered("an unknown instance", UNKNOWN_INSTANCE_ARN);

    for variant in VARIANTS {
        for case in surface_calls(UNKNOWN_INSTANCE_ARN, variant) {
            let (status, body) = call(&router, case.operation, &case.body).await;
            assert_refusal(
                &case.label,
                "ResourceNotFoundException",
                &expected,
                status,
                &body,
            );
        }
    }
}

/// STEP 3, over every operation whose request names a permission set: a `PermissionSetArn`
/// that resolves to nothing WITHIN the instance earns one message — the same for an ARN
/// nothing was ever minted under and for one held under a DIFFERENT instance of the same
/// account — and the refusal changes nothing a later caller sees.
#[tokio::test]
async fn an_unresolvable_permission_set_answers_one_message_across_the_surface() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));

    // A permission set that DOES exist, under the account's organization instance, plus a
    // SECOND organization instance this account holds that holds none: no operation of this
    // API enables an organization instance, so it is seeded through the same
    // `state::Instance` constructor the handlers use.
    let held_arn = create_permission_set(&router, &instance_arn, "Resolution").await;
    let other_instance_arn = with_store(&engine, |store| {
        let instance = state::Instance::active(
            OTHER_INSTANCE_ID,
            OTHER_IDENTITY_STORE_ID,
            state::InstanceType::Organization,
            Some("SecondOrganization".to_string()),
            aws_smithy_types::DateTime::from(SystemTime::now()),
            TEST_REGION,
        );
        let arn = instance.instance_arn.clone();
        store.instances.insert(arn.clone(), instance);
        arn
    });

    assert_both_halves_covered("an unresolvable permission set", &instance_arn);

    // Row 1: an ARN nothing was ever minted under, on the instance that holds the set
    // above — the variants vary the ARN's id charset, so the answer is about resolution and
    // not about one shape of ARN.
    for case in permission_set_calls(&instance_arn) {
        let named = case.body["PermissionSetArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, body) = call(&router, case.operation, &case.body).await;
        assert_refusal(
            &format!("{} (never minted)", case.label),
            "ResourceNotFoundException",
            &state::permission_set_not_found_message(&named),
            status,
            &body,
        );
    }

    // Row 2: the ARN of the permission set that DOES exist, asked for under the OTHER
    // instance this account holds — a permission set belongs to exactly one instance, so it
    // is as absent there as one that was never created.
    for mut case in permission_set_calls(&other_instance_arn) {
        case.body["PermissionSetArn"] = json!(held_arn);
        let (status, body) = call(&router, case.operation, &case.body).await;
        assert_refusal(
            &format!("{} (held under another instance)", case.label),
            "ResourceNotFoundException",
            &state::permission_set_not_found_message(&held_arn),
            status,
            &body,
        );
    }

    // None of those refusals wrote: the instance still holds exactly the one permission set
    // that was created, and it still describes.
    let (status, listed) = call(
        &router,
        "ListPermissionSets",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"after the refusals\": ListPermissionSets must answer 200, got {status} {listed}"
    );
    assert_eq!(
        listed["PermissionSets"],
        json!([held_arn]),
        "case \"after the refusals\": the refused calls must leave the stored sets alone, \
         got {listed}"
    );
    let (status, described) = call(
        &router,
        "DescribePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": held_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"after the refusals\": DescribePermissionSet must answer 200, got \
         {status} {described}"
    );
}

/// The other half of the rule: a request the resolution DOES resolve is SERVED, by an
/// operation of each kind that performs it — so the refusals above are about the ARNs asked
/// for and about nothing else.
#[tokio::test]
async fn the_resolution_serves_a_request_it_resolves() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(engine);
    let held_arn = create_permission_set(&router, &instance_arn, "Served").await;

    // One row per kind of request the resolution serves: an instance-only read, an
    // instance-only enumeration, a permission-set read, a permission-set write, a
    // permission-set enumeration, and both halves of the ABAC pair.
    let served: Vec<(&str, Value)> = vec![
        ("ListPermissionSets", json!({"InstanceArn": instance_arn})),
        (
            "ListAccountAssignmentCreationStatus",
            json!({"InstanceArn": instance_arn}),
        ),
        (
            "DescribePermissionSet",
            json!({"InstanceArn": instance_arn, "PermissionSetArn": held_arn}),
        ),
        (
            "PutInlinePolicyToPermissionSet",
            json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": held_arn,
                "InlinePolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
        ),
        (
            "ListManagedPoliciesInPermissionSet",
            json!({"InstanceArn": instance_arn, "PermissionSetArn": held_arn}),
        ),
        (
            "ListAccountAssignments",
            json!({
                "InstanceArn": instance_arn,
                "AccountId": "123456789012",
                "PermissionSetArn": held_arn,
            }),
        ),
        (
            "CreateInstanceAccessControlAttributeConfiguration",
            json!({
                "InstanceArn": instance_arn,
                "InstanceAccessControlAttributeConfiguration": {
                    "AccessControlAttributes": [{
                        "Key": "department",
                        "Value": {"Source": ["${path:enterprise.department}"]},
                    }],
                },
            }),
        ),
        (
            "DescribeInstanceAccessControlAttributeConfiguration",
            json!({"InstanceArn": instance_arn}),
        ),
    ];
    for (operation, body) in served {
        let (status, answered) = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: a resolved request is served, got {status} {answered}"
        );
    }

    // The inline policy the write above stored is read back through its own getter: the
    // resolution handed the write the stored record, not a copy of it.
    let (status, policy) = call(
        &router,
        "GetInlinePolicyForPermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": held_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GetInlinePolicyForPermissionSet\": expected 200, got {status} {policy}"
    );
    assert_eq!(
        policy["InlinePolicy"].as_str(),
        Some("{\"Version\":\"2012-10-17\",\"Statement\":[]}"),
        "case \"GetInlinePolicyForPermissionSet\": the write must be readable back, got \
         {policy}"
    );
}
