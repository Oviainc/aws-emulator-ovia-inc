// SPDX-License-Identifier: CC0-1.0

//! `DeleteInstance` and `DeleteTrustedTokenIssuer` end to end over the `awsJson1_1` wire.
//!
//! The rules under test:
//!
//! 1. a DELETE removes the resource it names, so every later read answers as though it had
//!    never been created: `DescribeTrustedTokenIssuer` of a deleted issuer is the modelled
//!    `ResourceNotFoundException` naming the ARN asked for, and a second delete of it is
//!    the same refusal — never a 200 and never a 501. Only the named resource goes.
//! 2. an instance's removal is a transition the CLOCK owes it: the instance reports the
//!    `DELETE_IN_PROGRESS` the model's `InstanceStatus` enum declares (and
//!    `DescribeInstance` documents — "DELETE_IN_PROGRESS - The instance is being deleted.
//!    Returns AccessDeniedException after the delete operation completes") until the
//!    removal falls due one never-zero span later, and once it is due the instance is
//!    gone, together with everything held inside it, which frees the account's
//!    one-instance quota so `CreateInstance` succeeds again.
//! 3. an `InstanceArn` the account holds no instance for is refused with the modelled
//!    `AccessDeniedException`, the only not-found channel `DeleteInstance` declares.
//!
//! Each rule is asserted over a TABLE: rule 1 over several issuers (so a delete is shown
//! to remove the ONE the request names), rule 2 on both sides of the due instant and over
//! the resources an instance holds, and rule 3 over the ARN spellings the model's
//! `InstanceArn` pattern admits.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns. Where a test has to be on the far side of a due
//! instant it moves the STORED instant rather than sleeping — the transition itself is
//! still applied by the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, router_over, seeded_instance_arn, with_store, UNKNOWN_INSTANCE_ARN,
};

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")` → `["OIDC_JWT"]`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")` → member
/// `JwksRetrievalOption` → `["OPEN_ID_DISCOVERY"]`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// A router over a fresh engine, and that engine.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// The `CreateTrustedTokenIssuer` request body for an issuer named `name`.
///
/// Every member is inside the constraints
/// `smithy_query(query="operation",operation="CreateTrustedTokenIssuer")` and
/// `smithy_query(query="shape",shape="OidcJwtConfiguration")` declare.
fn issuer_request(instance_arn: &str, name: &str) -> Value {
    json!({
        "InstanceArn": instance_arn,
        "Name": name,
        "TrustedTokenIssuerType": ISSUER_TYPE,
        "TrustedTokenIssuerConfiguration": {
            "OidcJwtConfiguration": {
                "IssuerUrl": format!("https://{name}.example.com/oidc"),
                "ClaimAttributePath": "email",
                "IdentityStoreAttributePath": "emails.primary.value",
                "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
            }
        },
    })
}

/// Create one trusted token issuer under `instance_arn` and return its ARN.
async fn create_issuer(router: &MultiServiceRouter, instance_arn: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateTrustedTokenIssuer",
        &issuer_request(instance_arn, name),
    )
    .await;
    assert_eq!(
        status, 200,
        "issuer {name}: CreateTrustedTokenIssuer expected 200, got {status} {body}"
    );
    body["TrustedTokenIssuerArn"]
        .as_str()
        .unwrap_or_else(|| panic!("issuer {name}: no TrustedTokenIssuerArn in {body}"))
        .to_string()
}

/// `DescribeTrustedTokenIssuer` for `arn`.
async fn describe_issuer(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "DescribeTrustedTokenIssuer",
        &json!({"TrustedTokenIssuerArn": arn}),
    )
    .await
}

/// The `InstanceArn`s `ListInstances` reports.
async fn listed_arns(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {body}");
    body["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("no Instances in {body}"))
        .iter()
        .map(|instance| {
            instance["InstanceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Move the stored deletion instant of every instance to `due_at`, so both sides of it can
/// be observed without the test sleeping.
fn hold_deletion(engine: &Arc<engine::InMemoryEngine>, due_at: SystemTime) {
    with_store(engine, |store| {
        for instance in store.instances.values_mut() {
            if instance.deleted_due.is_some() {
                instance.deleted_due = Some(due_at);
            }
        }
    });
}

/// RULE 1 — a delete removes exactly the issuer it names, and the ARN stops resolving:
/// the describe and a second delete both answer the modelled `ResourceNotFoundException`,
/// naming the ARN asked for.
#[tokio::test]
async fn deleting_a_trusted_token_issuer_makes_its_arn_stop_resolving() {
    let (router, _engine) = router_and_engine();
    let instance_arn = seeded_instance_arn(&router).await;

    // The rows differ along the position of the deleted issuer in a set of several: the
    // first created, one in the middle, and the last.
    let names = ["issuer-one", "issuer-two", "issuer-three", "issuer-four"];
    let mut arns = Vec::new();
    for name in names {
        arns.push(create_issuer(&router, &instance_arn, name).await);
    }

    for (position, arn) in [(0usize, &arns[0]), (2, &arns[2]), (3, &arns[3])] {
        let case = format!("the issuer created {}", position + 1);
        let (status, before) = describe_issuer(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the issuer must resolve before the delete, got {status} {before}"
        );

        let (status, deleted) = call(
            &router,
            "DeleteTrustedTokenIssuer",
            &json!({"TrustedTokenIssuerArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteTrustedTokenIssuer expected 200, got {status} {deleted}"
        );

        // Gone: the describe and a repeat of the delete answer the same refusal, and both
        // name the ARN asked for.
        for operation in ["DescribeTrustedTokenIssuer", "DeleteTrustedTokenIssuer"] {
            let (status, body) = call(
                &router,
                operation,
                &json!({"TrustedTokenIssuerArn": arn}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} of a deleted issuer expected 400, got {status} \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: {operation} expected ResourceNotFoundException, got {body}"
            );
            let message = body["Message"]
                .as_str()
                .or_else(|| body["message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains(arn.as_str()),
                "case {case:?}: {operation} must name the ARN asked for, got {message:?}"
            );
        }
    }

    // Only the named issuers went: the ones no delete named still resolve.
    let (status, survivor) = describe_issuer(&router, &arns[1]).await;
    assert_eq!(
        status, 200,
        "an issuer no delete named must still resolve, got {status} {survivor}"
    );
    assert_eq!(
        survivor["Name"].as_str(),
        Some(names[1]),
        "the surviving issuer must still report its own record, got {survivor}"
    );
}

/// RULE 2 — the instance moves through `DELETE_IN_PROGRESS` and then stops existing,
/// taking what it held with it and freeing the account's one-instance quota.
#[tokio::test]
async fn deleting_the_instance_runs_the_clock_and_frees_the_quota() {
    let (router, engine) = router_and_engine();
    let instance_arn = seeded_instance_arn(&router).await;
    let issuer_arn = create_issuer(&router, &instance_arn, "held-by-the-instance").await;

    assert!(
        state::INSTANCE_DELETION_SPAN > Duration::ZERO,
        "the deletion span must never be zero, so a caller that reads straight after the \
         delete still observes DELETE_IN_PROGRESS"
    );

    let opened_at = SystemTime::now();
    let (status, deleted) = call(
        &router,
        "DeleteInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    let closed_at = SystemTime::now();
    assert_eq!(
        status, 200,
        "DeleteInstance expected 200, got {status} {deleted}"
    );

    // The delete owes the removal a due INSTANT one span out, not a removal that had
    // already happened before the caller could observe the status.
    let due_at = with_store(&engine, |store| {
        store
            .instance(&instance_arn)
            .expect("the instance is still held while it is being deleted")
            .deleted_due
    })
    .expect("the delete owed a removal instant");
    assert!(
        due_at >= opened_at + state::INSTANCE_DELETION_SPAN
            && due_at <= closed_at + state::INSTANCE_DELETION_SPAN,
        "the removal must fall due one span after the call, got {due_at:?}"
    );

    // Before that instant the instance is still there, in the transient status.
    hold_deletion(&engine, closed_at + Duration::from_secs(3_600));
    let (status, pending) = call(
        &router,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "an instance being deleted still resolves, got {status} {pending}"
    );
    assert_eq!(
        pending["Status"].as_str(),
        Some("DELETE_IN_PROGRESS"),
        "an instance being deleted reports DELETE_IN_PROGRESS, got {:?}",
        pending["Status"]
    );
    assert_eq!(
        listed_arns(&router).await,
        vec![instance_arn.clone()],
        "an instance being deleted is still listed"
    );
    // The account still holds it, so the quota is still crossed.
    let (status, refused) = call(&router, "CreateInstance", &json!({})).await;
    assert_eq!(
        status, 400,
        "while the account still holds an instance a create is refused, got {status} \
         {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "expected the quota refusal, got {refused}"
    );

    // Once the removal is due the instance is gone — and so is what it held.
    hold_deletion(&engine, closed_at - Duration::from_secs(1));
    let (status, gone) = call(
        &router,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 403,
        "a removed instance is refused AccessDeniedException, got {status} {gone}"
    );
    assert_eq!(
        error_code(&gone),
        "AccessDeniedException",
        "expected AccessDeniedException, got {gone}"
    );
    assert!(
        listed_arns(&router).await.is_empty(),
        "a removed instance is no longer listed"
    );
    let (status, orphan) = describe_issuer(&router, &issuer_arn).await;
    assert_eq!(
        status, 400,
        "a trusted token issuer of a removed instance no longer resolves, got {status} \
         {orphan}"
    );
    assert_eq!(
        error_code(&orphan),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {orphan}"
    );

    // The quota is free again, and the account's instance is the created one — no ambient
    // instance comes back.
    let (status, created) = call(&router, "CreateInstance", &json!({"Name": "AfterDelete"})).await;
    assert_eq!(
        status, 200,
        "with no instance held, CreateInstance expected 200, got {status} {created}"
    );
    let created_arn = created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no InstanceArn in {created}"))
        .to_string();
    assert_eq!(
        listed_arns(&router).await,
        vec![created_arn],
        "the account's instance is the one the create minted"
    );
}

/// RULE 3 — an `InstanceArn` this account holds no instance for is refused
/// `AccessDeniedException`, whichever spelling the model's pattern admits.
#[tokio::test]
async fn deleting_an_instance_the_account_does_not_hold_is_refused() {
    let (router, _engine) = router_and_engine();

    // The rows span the `InstanceArn` pattern
    // `^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`: the
    // `ssoins-` and bare `ins-` prefixes, the full `[a-zA-Z0-9-.]` id class, and the
    // partition suffixes it allows.
    let rows = [
        ("a pattern-valid ssoins- ARN nothing created", UNKNOWN_INSTANCE_ARN),
        ("the bare ins- spelling", "arn:aws:sso:::instance/ins-abcdefghij123456"),
        (
            "an id using the full [a-zA-Z0-9-.] class",
            "arn:aws:sso:::instance/ssoins-Ab.9-cDeF0123456",
        ),
        (
            "a GovCloud partition ARN",
            "arn:aws-us-gov:sso:::instance/ssoins-1111111111111111",
        ),
        (
            "a China partition ARN",
            "arn:aws-cn:sso:::instance/ssoins-2222222222222222",
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = call(&router, "DeleteInstance", &json!({"InstanceArn": arn})).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for {arn}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException for {arn}, got {body}"
        );
    }

    // And the account's own instance is untouched by every one of those refusals.
    let held = listed_arns(&router).await;
    assert_eq!(
        held.len(),
        1,
        "a refused delete must leave the account's instance in place, got {held:?}"
    );
}
