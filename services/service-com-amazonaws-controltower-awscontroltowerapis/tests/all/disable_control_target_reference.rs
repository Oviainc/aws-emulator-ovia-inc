// SPDX-License-Identifier: CC0-1.0

//! How `DisableControl` reads `targetIdentifier`, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and message bodies — not the handlers' Rust signatures.
//!
//! The rule under test: `targetIdentifier` is "the ARN of the organizational unit"
//! (`smithy_query(query="operation", operation="DisableControl")`, the member's own doc), so
//! a value satisfying the member's `@length` 20..2048 and
//! `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$` that is NOT an AWS Organizations organizational
//! unit ARN names no target an enablement could be on: it is a malformed input, refused with
//! `ValidationException` (400), on EVERY path that reads the member and whichever other
//! members the request carries — the same answer, in the same words, that `EnableControl`
//! gives the identical value. Only a value of that form reaches the store, where a target
//! nothing is enabled on is the modelled `ResourceNotFoundException` (404).
//!
//! The rows differ along the form of the value: another resource type of this very service,
//! the other AWS Organizations resource types, another service's ARN, an organizational unit
//! ARN broken in each segment that makes it one (region, account, organization, unit id) and
//! an ARN at each published length bound. The accepted side varies the same member the other
//! way — an organizational unit ARN in a non-default partition — so the refusal is pinned to
//! what the rule says rather than to one spelling.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogControl, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{
    catalog_arn, control, control_operations, details, enable, ou,
    status_summary_status as status,
};
use crate::front_door_validation::arn_of_length;
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// POST `/disable-control` with exactly the members `body` carries.
async fn disable(router: &MultiServiceRouter, body: serde_json::Value) -> Answer {
    call(router, "/disable-control", body).await
}

/// POST `/enable-control` without asserting the answer: the sibling the rule says
/// `DisableControl` must agree with for one `targetIdentifier` value.
async fn enable_control(
    router: &MultiServiceRouter,
    control_identifier: &str,
    target_identifier: &str,
) -> Answer {
    call(
        router,
        "/enable-control",
        serde_json::json!({
            "controlIdentifier": control_identifier,
            "targetIdentifier": target_identifier,
        }),
    )
    .await
}

/// Every `DISABLE_CONTROL` record the log holds, so a case can assert that a refusal
/// appended none.
async fn teardown_records(router: &MultiServiceRouter) -> Vec<serde_json::Value> {
    control_operations(
        router,
        serde_json::json!({ "controlOperationTypes": ["DISABLE_CONTROL"] }),
    )
    .await
}

/// The catalog controls a caller may enable, in catalog order — the catalog
/// `EnableControl` and `DisableControl` resolve a `controlIdentifier` against, so no row
/// writes a control out by hand.
fn enablable(index: usize) -> &'static CatalogControl {
    let enablable: Vec<&'static CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable && control.parameters.is_empty())
        .collect();
    assert!(
        !enablable.is_empty(),
        "the catalog must publish a control a caller may enable that declares no parameters"
    );
    enablable[index % enablable.len()]
}

/// Assert a refusal's status and error code, and that it minted nothing.
fn assert_refused(case: &str, refused: &Answer, expected_status: u16, expected_code: &str) {
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (expected_status, Some(expected_code)),
        "case {case:?}: expected {expected_status} {expected_code}, got {:?} body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    assert!(
        refused.body.get("operationIdentifier").is_none(),
        "case {case:?}: a refused call must mint no operation, got {:?}",
        refused.body
    );
}

/// The values of `targetIdentifier` that are not organizational unit ARNs: each satisfies
/// the member's `@length` 20..2048 and `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$`, so the front
/// door passes them all and the HANDLER is what refuses them. The Organizations resource
/// forms (`ou`, `root`, `account`, `organization`) are the ones the AWS Organizations ARN
/// documentation publishes, and the broken-unit rows take one segment of the `ou` form away
/// at a time.
fn not_organizational_units() -> Vec<(&'static str, String)> {
    vec![
        (
            "a landing zone of this very service, which is a resource but not a target",
            format!(
                "arn:aws:controltower:us-west-2:{}:landingzone/ABCDEFGHIJKLMNOP",
                engine::emulator_account_id()
            ),
        ),
        (
            "an enabled control's own ARN form, sent as the target it is enabled on",
            format!(
                "arn:aws:controltower:us-west-2:{}:enabledcontrol/\
                 00000000-0000-4000-8000-000000000000",
                engine::emulator_account_id()
            ),
        ),
        (
            "an Organizations ROOT",
            "arn:aws:organizations::123456789012:root/o-exampleorg/r-abcd".to_string(),
        ),
        (
            "an Organizations ACCOUNT",
            "arn:aws:organizations::123456789012:account/o-exampleorg/123456789012".to_string(),
        ),
        (
            "an Organizations organization",
            "arn:aws:organizations::123456789012:organization/o-exampleorg".to_string(),
        ),
        (
            "an S3 bucket, which is another service's resource entirely",
            "arn:aws:s3:::my-example-bucket-name".to_string(),
        ),
        (
            "an organizational unit ARN carrying a region, which an Organizations ARN does not",
            "arn:aws:organizations:us-west-2:123456789012:ou/o-exampleorg/ou-cl22-1a".to_string(),
        ),
        (
            "an organizational unit ARN whose account is not a 12-digit account id",
            "arn:aws:organizations::12345:ou/o-exampleorg/ou-cl22-1a".to_string(),
        ),
        (
            "an organizational unit ARN with no organization in its resource",
            "arn:aws:organizations::123456789012:ou/ou-cl22-1a".to_string(),
        ),
        (
            "an organizational unit ARN whose unit id is not of the published form",
            "arn:aws:organizations::123456789012:ou/o-exampleorg/unit-cl22-1a".to_string(),
        ),
        (
            "an organizational unit ARN whose unit id is uppercase",
            "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-CL22-1A".to_string(),
        ),
        (
            "an ARN-shaped value at the member's length minimum of 20",
            arn_of_length(20),
        ),
        (
            "an ARN-shaped value at the member's length maximum of 2048",
            arn_of_length(2048),
        ),
    ]
}

/// RULE: a `targetIdentifier` that is not an organizational unit ARN is refused with 400
/// `ValidationException` — in the words `EnableControl` refuses the identical value with —
/// whichever members of `DisableControl`'s input accompany it, and the refusal tears
/// nothing down.
#[tokio::test]
async fn a_target_that_is_not_an_organizational_unit_is_refused() {
    let router = set_up_emulator().await;
    let entry = enablable(0);
    let target = ou("g001");
    let (arn, _) = enable(
        &router,
        serde_json::json!({
            "controlIdentifier": control(entry.name),
            "targetIdentifier": target,
        }),
    )
    .await;

    // Past the settle span, so no refusal below can be the in-flight enable's 409.
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, bad_target) in not_organizational_units() {
        // What the sibling that CREATES an enablement answers for this value, so the
        // comparison below is against the rule's own wording rather than a string this
        // suite spells out.
        let enabling = enable_control(&router, &control(entry.name), &bad_target).await;
        assert_refused(
            &format!("{case}: enable-control"),
            &enabling,
            400,
            "ValidationException",
        );
        let expected_message = enabling.string("message");
        assert!(
            expected_message.contains("targetIdentifier"),
            "case {case:?}: the refusal must name the member it is about, got \
             {expected_message:?}"
        );

        // Every form in which `DisableControl` reads this member: the control/target pair in
        // both published ARN forms of the control, and all three members together.
        let forms: Vec<(&str, serde_json::Value)> = vec![
            (
                "the control ARN pair",
                serde_json::json!({
                    "controlIdentifier": control(entry.name),
                    "targetIdentifier": bad_target,
                }),
            ),
            (
                "the catalog ARN pair",
                serde_json::json!({
                    "controlIdentifier": catalog_arn(entry.catalog_id),
                    "targetIdentifier": bad_target,
                }),
            ),
            (
                "all three members, the enablement's own ARN included",
                serde_json::json!({
                    "enabledControlIdentifier": arn,
                    "controlIdentifier": control(entry.name),
                    "targetIdentifier": bad_target,
                }),
            ),
        ];

        for (form, body) in forms {
            let refused = disable(&router, body.clone()).await;
            let addressed = format!("{case}: disable-control by {form}");
            assert_refused(&addressed, &refused, 400, "ValidationException");
            assert_eq!(
                refused.string("message"),
                expected_message,
                "case {addressed:?} naming {body}: the same value must be refused in the \
                 same words enable-control uses, got {:?}",
                refused.string("message")
            );
        }
    }

    // No refusal tore anything down, and none appended a teardown record.
    assert!(
        teardown_records(&router).await.is_empty(),
        "a refused teardown must append no DISABLE_CONTROL record, got {:?}",
        teardown_records(&router).await
    );
    let reported = details(&router, &arn).await;
    assert_eq!(
        status(&reported),
        Some("SUCCEEDED"),
        "an enablement no accepted call touched must still be SUCCEEDED, got {reported:?}"
    );
}

/// RULE (the other side of it): a `targetIdentifier` that IS an organizational unit ARN
/// reaches the store, so the pair path answers 404 `ResourceNotFoundException` for a target
/// nothing is enabled on and 200 for the enablement it names — in any partition.
#[tokio::test]
async fn an_organizational_unit_target_reaches_the_store() {
    let router = set_up_emulator().await;

    // Two enablements of the same control shape on organizational units that differ in the
    // partition their ARN names: the rule is about the ARN being an organizational unit's,
    // not about one spelling of one.
    let targets = [
        ("the default partition", ou("g101")),
        (
            "a non-default partition",
            "arn:aws-us-gov:organizations::123456789012:ou/o-emulatororg/ou-abcd-g102"
                .to_string(),
        ),
    ];

    let mut enabled: Vec<(&'static CatalogControl, String)> = Vec::new();
    for (index, (_, target)) in targets.iter().enumerate() {
        let entry = enablable(index);
        let (arn, _) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": control(entry.name),
                "targetIdentifier": target,
            }),
        )
        .await;
        enabled.push((entry, arn));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    // A well-formed organizational unit nothing is enabled on is the MISSING resource, not a
    // malformed input: the form of the value is what the 400 is about.
    let absent = disable(
        &router,
        serde_json::json!({
            "controlIdentifier": control(enablable(0).name),
            "targetIdentifier": ou("g199"),
        }),
    )
    .await;
    assert_refused(
        "a well-formed organizational unit nothing is enabled on",
        &absent,
        404,
        "ResourceNotFoundException",
    );

    for ((case, target), (entry, arn)) in targets.iter().zip(&enabled) {
        let disabled = disable(
            &router,
            serde_json::json!({
                "controlIdentifier": control(entry.name),
                "targetIdentifier": target,
            }),
        )
        .await;
        assert_eq!(
            disabled.status, 200,
            "case {case:?}: tearing {arn:?} down by its control/target pair expected 200, \
             got {} body {:?}",
            disabled.status, disabled.body
        );
    }
}
