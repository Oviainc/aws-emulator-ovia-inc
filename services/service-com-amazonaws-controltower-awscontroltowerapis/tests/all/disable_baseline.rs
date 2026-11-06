// SPDX-License-Identifier: CC0-1.0

//! `DisableBaseline`, end to end over HTTP.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * for ANY `EnabledBaseline` the account holds, whatever members it was enabled with,
//!   `DisableBaseline` starts exactly ONE asynchronous `DISABLE_BASELINE` operation
//!   against that resource and returns its `operationIdentifier`, which resolves through
//!   `GetBaselineOperation` to that call's own record — `IN_PROGRESS` while the teardown
//!   is in flight, `SUCCEEDED` with a stable `endTime` once the clock has made it due;
//! * the operation is `@idempotent`, so repeating the call while that teardown is in
//!   flight reports the SAME identifier rather than starting a second one, and it is
//!   refused with `ConflictException` (409) while any OTHER operation is still changing
//!   the enablement;
//! * an `enabledBaselineIdentifier` names the enablement stored under exactly that ARN:
//!   every other well-formed ARN — including one that merely ENDS in this enablement's
//!   resource id, and one naming a different resource the account really holds — names
//!   no resource, so it is `ResourceNotFoundException` (404);
//! * a refusal writes nothing, and a teardown removes only its OWN resource: every
//!   sibling enablement stays enabled and every other operation record keeps its own
//!   type and status.
//!
//! The contract comes from `smithy_query(query="operation", operation="DisableBaseline")`
//! on this crate's `smithy-model/`: `enabledBaselineIdentifier` is a **required** `Arn`
//! (`@length` 20..2048, `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$`), the output's
//! `operationIdentifier` is 36 characters of the UUID pattern, the operation carries
//! `@idempotent`, and `ConflictException` (409) and `ResourceNotFoundException` (404) are
//! among its modelled errors. The enablement table, and the helpers that drive the three
//! operations, are shared with the `baseline_operations` suite.

use crate::baseline_operations::{disable, enable, enablements, get_operation, member};
use crate::front_door_validation::arn_of_length;
use crate::landing_zone_lifecycle::{
    call, emulator, is_operation_identifier, past_the_settle_span, set_up_emulator,
    simple_manifest,
};

/// RULE: every enablement tears down through its own `DISABLE_BASELINE` operation —
/// refused while the enablement is still being applied, idempotent while the teardown is
/// in flight, terminal once the clock makes it due, and leaving the pair enablable again
/// as a NEW resource. Asserted over the whole enablement table, so the rule holds
/// whatever version, parameter list and tag map the enablement carries.
#[tokio::test]
async fn every_enablement_tears_down_through_its_own_idempotent_operation() {
    let router = set_up_emulator().await;
    let rows = enablements();

    // case, the minted EnabledBaseline ARN, the ENABLE_BASELINE operation's identifier.
    let mut enabled: Vec<(&'static str, String, String)> = Vec::new();
    for row in &rows {
        let answer = enable(&router, row).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: enable expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
        let case = row.case;
        let arn = answer.string("arn");
        let enable_operation = answer.string("operationIdentifier");

        // The enable "starts an asynchronous operation" (model docs), so the enablement
        // is still changing and a teardown would leave it half applied. Probed
        // IMMEDIATELY after this row's own enable, so the assertion does not depend on
        // how long the rest of the table takes.
        let too_soon = disable(&router, &arn).await;
        assert_eq!(
            (too_soon.status, too_soon.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: a teardown while the enable is in flight expected 409 \
             ConflictException, got {:?} body {:?}",
            (too_soon.status, too_soon.error_code.as_deref()),
            too_soon.body
        );
        assert!(
            too_soon.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused teardown must mint no operation, got {:?}",
            too_soon.body
        );
        // The refusal left the enable's own record alone.
        let (_, operation) = get_operation(&router, &enable_operation).await;
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (Some("ENABLE_BASELINE"), Some("IN_PROGRESS")),
            "case {case:?}: a refused teardown must not disturb the enable's record, got \
             {operation:?}"
        );

        enabled.push((case, arn, enable_operation));
    }

    // Nothing sleeps server-side: the clock makes the enables due, and the wait is the
    // test's.
    tokio::time::sleep(past_the_settle_span()).await;

    // case, ARN, the DISABLE_BASELINE operation's identifier.
    let mut teardowns: Vec<(&'static str, String, String)> = Vec::new();
    for (case, arn, enable_operation) in &enabled {
        let disabled = disable(&router, arn).await;
        assert_eq!(
            disabled.status, 200,
            "case {case:?}: a teardown once the enable is due expected 200, got {} body \
             {:?}",
            disabled.status, disabled.body
        );
        let identifier = disabled.string("operationIdentifier");
        assert!(
            is_operation_identifier(&identifier),
            "case {case:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {identifier:?}"
        );
        assert_ne!(
            &identifier, enable_operation,
            "case {case:?}: the teardown must mint its own identifier, got the enable's \
             {enable_operation:?}"
        );

        // `@idempotent`: repeating the call while this teardown is in flight is the same
        // work, so it reports the same operation rather than starting a second one.
        let repeated = disable(&router, arn).await;
        assert_eq!(
            (repeated.status, repeated.string("operationIdentifier")),
            (200, identifier.clone()),
            "case {case:?}: a repeated teardown expected 200 and {identifier:?}, got {} \
             body {:?}",
            repeated.status,
            repeated.body
        );

        let (answer, operation) = get_operation(&router, &identifier).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected {identifier:?} to resolve, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            (
                member(&operation, "operationIdentifier"),
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (
                Some(identifier.as_str()),
                Some("DISABLE_BASELINE"),
                Some("IN_PROGRESS")
            ),
            "case {case:?}: expected the teardown's own record IN_PROGRESS, got \
             {operation:?}"
        );
        assert!(
            operation.get("startTime").is_some() && operation.get("endTime").is_none(),
            "case {case:?}: expected a startTime and no endTime while the teardown is in \
             flight, got {operation:?}"
        );
        assert!(
            operation.get("statusMessage").is_none(),
            "case {case:?}: a statusMessage is reported in case of failure only, got \
             {operation:?}"
        );

        teardowns.push((case, arn.clone(), identifier));
    }

    // One operation per call: no identifier is handed out twice.
    let mut minted: Vec<(&'static str, &'static str, &String)> = Vec::new();
    let all = enabled
        .iter()
        .map(|(case, _, identifier)| (*case, "enable", identifier))
        .chain(
            teardowns
                .iter()
                .map(|(case, _, identifier)| (*case, "disable", identifier)),
        );
    for (case, kind, identifier) in all {
        for (other_case, other_kind, other) in &minted {
            assert_ne!(
                identifier, *other,
                "case {case:?} ({kind}) and case {other_case:?} ({other_kind}): two \
                 calls must mint different operationIdentifiers, got {identifier:?} twice"
            );
        }
        minted.push((case, kind, identifier));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (case, arn, identifier) in &teardowns {
        let (answer, operation) = get_operation(&router, identifier).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the teardown's record must outlive the resource, got {} body \
             {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (Some("DISABLE_BASELINE"), Some("SUCCEEDED")),
            "case {case:?}: expected DISABLE_BASELINE SUCCEEDED once due, got {operation:?}"
        );
        let end_time = operation.get("endTime").cloned();
        assert!(
            end_time.is_some(),
            "case {case:?}: expected an endTime once the teardown is due, got {operation:?}"
        );
        let (_, reread) = get_operation(&router, identifier).await;
        assert_eq!(
            reread.get("endTime").cloned(),
            end_time,
            "case {case:?}: re-reading a terminal teardown must report the same endTime, \
             got {reread:?}"
        );

        // The resource it tore down is gone, so its ARN names nothing.
        let gone = disable(&router, arn).await;
        assert_eq!(
            (gone.status, gone.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a teardown of an ARN already torn down expected 404 \
             ResourceNotFoundException, got {:?} body {:?}",
            (gone.status, gone.error_code.as_deref()),
            gone.body
        );
    }

    // Each pair enables again, as a NEW resource.
    for (row, (case, arn, _)) in rows.iter().zip(&teardowns) {
        let reenabled = enable(&router, row).await;
        assert_eq!(
            reenabled.status, 200,
            "case {case:?}: enabling the pair again once its teardown is due expected \
             200, got {} body {:?}",
            reenabled.status, reenabled.body
        );
        assert_ne!(
            &reenabled.string("arn"),
            arn,
            "case {case:?}: a re-enabled pair must mint a new ARN, got the torn-down \
             {arn:?}"
        );
    }
}

/// RULE: an `enabledBaselineIdentifier` names the enablement stored under exactly that
/// ARN. Every other well-formed ARN names no resource — so it is 404
/// `ResourceNotFoundException` and tears nothing down — including one that merely ends in
/// this enablement's resource id and one naming a different resource the account holds.
#[tokio::test]
async fn only_the_enablements_own_arn_names_it() {
    let router = emulator();

    // A resource of ANOTHER kind that the account really holds, for the cross-store rows.
    let created = call(
        &router,
        "/create-landingzone",
        serde_json::json!({
            "version": "3.3",
            "manifest": simple_manifest()
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "create-landingzone: expected 200, got {} body {:?}",
        created.status, created.body
    );
    let landing_zone_arn = created.string("arn");

    let row = &enablements()[0];
    let answer = enable(&router, row).await;
    assert_eq!(
        answer.status, 200,
        "enable: expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    let arn = answer.string("arn");
    let resource_id = arn
        .rsplit('/')
        .next()
        .expect("the minted ARN carries a resource id")
        .to_string();

    // Past the settle span, so a refusal below cannot be the in-flight enable's 409.
    tokio::time::sleep(past_the_settle_span()).await;

    // Every row satisfies `Arn`'s @length 20..2048 and
    // @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$, including both published length bounds, so
    // the front door passes them all and the HANDLER is what refuses them.
    let cases: Vec<(&str, String)> = vec![
        ("an ARN at exactly Arn's length min of 20", arn_of_length(20)),
        (
            "an ARN at exactly Arn's length max of 2048",
            arn_of_length(2048),
        ),
        (
            "an enabledbaseline ARN no enablement was minted under",
            "arn:aws:controltower:us-west-2:123456789012:enabledbaseline/\
             00000000-0000-4000-8000-000000000000"
                .to_string(),
        ),
        (
            "another service's ARN ending in this enablement's resource id",
            format!("arn:aws:iam::123456789012:role/{resource_id}"),
        ),
        (
            "this enablement's resource id under a different resource type and region",
            format!("arn:aws:controltower:eu-west-1::baseline/{resource_id}"),
        ),
        (
            "the ARN of a landing zone the account does hold",
            landing_zone_arn.clone(),
        ),
        (
            "the baseline's own ARN rather than its enablement's",
            row.baseline_identifier.to_string(),
        ),
        (
            "the target's ARN rather than the enablement's",
            row.target_identifier.to_string(),
        ),
    ];

    for (case, identifier) in cases {
        let refused = disable(&router, &identifier).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?} ({identifier:?}): expected 404 ResourceNotFoundException, got \
             {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused teardown must mint no operation, got {:?}",
            refused.body
        );
    }

    // None of those refusals tore the enablement down: its OWN ARN still names it.
    let disabled = disable(&router, &arn).await;
    assert_eq!(
        disabled.status, 200,
        "the enablement's own ARN after every refusal: expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    let (_, operation) = get_operation(&router, &disabled.string("operationIdentifier")).await;
    assert_eq!(
        member(&operation, "operationType"),
        Some("DISABLE_BASELINE"),
        "the enablement's own ARN after every refusal: expected a DISABLE_BASELINE \
         record, got {operation:?}"
    );
}

/// RULE (the blast radius of a teardown): a `DisableBaseline` removes only the enablement
/// its identifier names. Every sibling stays enabled — so re-enabling its pair is still a
/// conflict — and every other operation record keeps its own type and status.
#[tokio::test]
async fn a_teardown_removes_only_its_own_enablement() {
    let router = set_up_emulator().await;
    let rows = enablements();
    assert!(
        rows.len() >= 3,
        "this case needs at least three enablements to tell siblings apart, got {}",
        rows.len()
    );

    let mut enabled: Vec<(&'static str, String, String)> = Vec::new();
    for row in &rows {
        let answer = enable(&router, row).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: enable expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
        enabled.push((
            row.case,
            answer.string("arn"),
            answer.string("operationIdentifier"),
        ));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    // Tear down a MIDDLE row, so neither the first nor the last enablement is the one
    // being removed.
    let target_index = rows.len() / 2;
    let (torn_case, torn_arn, _) = enabled[target_index].clone();
    let disabled = disable(&router, &torn_arn).await;
    assert_eq!(
        disabled.status, 200,
        "case {torn_case:?}: teardown expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    let torn_operation = disabled.string("operationIdentifier");

    tokio::time::sleep(past_the_settle_span()).await;

    let gone = disable(&router, &torn_arn).await;
    assert_eq!(
        (gone.status, gone.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {torn_case:?}: the torn-down ARN expected 404 ResourceNotFoundException, \
         got {:?} body {:?}",
        (gone.status, gone.error_code.as_deref()),
        gone.body
    );

    for (index, (case, arn, enable_operation)) in enabled.iter().enumerate() {
        if index == target_index {
            continue;
        }

        // The sibling's enable record is untouched by another enablement's teardown.
        let (_, operation) = get_operation(&router, enable_operation).await;
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (Some("ENABLE_BASELINE"), Some("SUCCEEDED")),
            "case {case:?}: a sibling's enable record must keep its own type and status, \
             got {operation:?}"
        );

        // The sibling is still enabled — and deployed, the settle span having passed — so
        // its pair cannot be enabled a second time...
        let again = enable(&router, &rows[index]).await;
        assert_eq!(
            (again.status, again.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: re-enabling a sibling's deployed pair expected 400 \
             ValidationException, got {:?} body {:?}",
            (again.status, again.error_code.as_deref()),
            again.body
        );

        // ...and its own ARN still names it, so it can be torn down in turn.
        let sibling_teardown = disable(&router, arn).await;
        assert_eq!(
            sibling_teardown.status, 200,
            "case {case:?}: a sibling must still be disablable after another's teardown, \
             got {} body {:?}",
            sibling_teardown.status, sibling_teardown.body
        );
        assert_ne!(
            sibling_teardown.string("operationIdentifier"),
            torn_operation,
            "case {case:?}: a sibling's teardown must mint its own identifier, got \
             {torn_operation:?}"
        );
    }

    // The torn-down pair enables again, as a new resource.
    let reenabled = enable(&router, &rows[target_index]).await;
    assert_eq!(
        reenabled.status, 200,
        "case {torn_case:?}: enabling the torn-down pair again expected 200, got {} body \
         {:?}",
        reenabled.status, reenabled.body
    );
    assert_ne!(
        reenabled.string("arn"),
        torn_arn,
        "case {torn_case:?}: a re-enabled pair must mint a new ARN, got the torn-down \
         {torn_arn:?}"
    );
}
