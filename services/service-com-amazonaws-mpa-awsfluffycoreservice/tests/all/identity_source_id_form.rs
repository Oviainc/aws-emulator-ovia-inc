// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the identity-source id inside an `IdentitySourceArn`, over BOTH
//! operations that take one in their URI label: `GetIdentitySource` and
//! `DeleteIdentitySource`.
//!
//! The rule under test: an identity-source id is MINTED BY THIS SERVICE (no request names
//! one), so an operation addressed by an identity source's ARN accepts only an id in the
//! form this service mints ([`state::is_identity_source_id`]) and refuses every other
//! value — including an otherwise well-formed identity-source ARN of the caller's own
//! account — as the value that addresses no identity source: `ValidationException` (400)
//! with the sentence that NAMES the rejected member
//! ([`state::INVALID_IDENTITY_SOURCE_ARN_MESSAGE`]), on both operations identically,
//! before the account boundary and before any existence lookup. An ARN whose id IS in
//! that form addresses an identity source, so it reaches each operation's own answer for
//! one the store holds nothing under: the read's `ResourceNotFoundException` (404) and the
//! `@idempotent` delete's success that writes nothing.
//!
//! The tables span the class the rule generalises over: the FORM of the id (a UUID, a
//! word, a dotted or underscored name, another service's id form, the prefix alone, the
//! prefix with a second separator, the prefix in the wrong case), its length up to the
//! member's published bound, the partition and region the ARN names, and the account it
//! names. Where a row names a published bound or grammar, a comment names the modelled
//! member it comes from.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the
//! `GET /identity-sources/{IdentitySourceArn}` and
//! `DELETE /identity-sources/{IdentitySourceArn}` routes, the URI label, the error
//! envelope and the HTTP status are all exercised.

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};
use crate::identity_source_deletion::{create_source, delete_source, listed_status, stored};

/// `@length(max: 1000)` on `GetIdentitySource$IdentitySourceArn` and
/// `DeleteIdentitySource$IdentitySourceArn` — the one bound the model places on the
/// member.
const MAX_IDENTITY_SOURCE_ARN: usize = 1000;

/// The two operations addressed by an `IdentitySourceArn`, each as the request it sends.
#[derive(Clone, Copy, Debug)]
enum Op {
    Get,
    Delete,
}

impl Op {
    const ALL: [Self; 2] = [Self::Get, Self::Delete];

    async fn call(self, harness: &Harness, value: &str) -> (u16, Value) {
        match self {
            Self::Get => {
                harness
                    .call(
                        "GET",
                        &format!("/identity-sources/{}", encode_label(value)),
                        None,
                    )
                    .await
            }
            Self::Delete => delete_source(harness, value).await,
        }
    }
}

/// An identity-source ARN of this service carrying `id`, in the grammar every regional,
/// account-owned ARN of this service takes — the one `ApprovalTeamArn`'s own pattern
/// spells (`^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$`),
/// with the resource name the `IdentitySource` resource's `arn` template carries.
fn source_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:mpa:{region}:{account}:identity-source/{id}")
}

/// Ids that are NOT ids this service mints, so an otherwise well-formed identity-source
/// ARN carrying one addresses no identity source. Rows differ along the form of the id.
fn ids_outside_the_minted_form() -> Vec<(&'static str, String)> {
    vec![
        // The id form the recorded client supplies when it has no minted id to hand.
        ("a uuid", "0f5e0119-ab78-418e-b630-3b421ab2fd6f".to_string()),
        (
            "a uuid in upper case",
            "0F5E0119-AB78-418E-B630-3B421AB2FD6F".to_string(),
        ),
        (
            "a uuid with its hyphens removed",
            "0f5e0119ab78418eb6303b421ab2fd6f".to_string(),
        ),
        ("a word", "nonexistent".to_string()),
        ("a dotted name", "identity.source.1".to_string()),
        ("an underscored name", "ids_000000000001".to_string()),
        (
            // `IdcInstanceArn`'s pattern `^arn:.+:sso:::instance/(?:sso)?ins-…$`: the IAM
            // Identity Center instance's id is not the identity source's.
            "an IAM Identity Center instance id",
            "ssoins-1234567890abcdef".to_string(),
        ),
        ("the prefix alone", "ids-".to_string()),
        ("the prefix with a second separator", "ids-000-000".to_string()),
        ("the prefix in upper case", "IDS-000000000001".to_string()),
        ("the prefix at the end", "000000000001-ids".to_string()),
        ("a hyphen-only id", "-".to_string()),
        (
            // `[a-zA-Z0-9._-]+` is the resource-id class this service's ARN patterns
            // declare, and every character of it is still not this service's id form.
            "an id spanning the resource-id character class",
            "Ids.000_000-9".to_string(),
        ),
    ]
}

/// Ids that ARE ids in the form this service mints, under which this store holds nothing.
/// Rows differ along the length and the character class of the id's body.
fn ids_in_the_minted_form() -> Vec<(&'static str, String)> {
    vec![
        (
            "the minted form with a counter this store never reached",
            format!("{}000000009999", state::IDENTITY_SOURCE_ID_PREFIX),
        ),
        (
            "the shortest body the form admits",
            format!("{}1", state::IDENTITY_SOURCE_ID_PREFIX),
        ),
        (
            "a body of letters and digits",
            format!("{}a1b2c3d4e5f6", state::IDENTITY_SOURCE_ID_PREFIX),
        ),
        (
            "a body in mixed case",
            format!("{}AbC123", state::IDENTITY_SOURCE_ID_PREFIX),
        ),
    ]
}

/// An ARN of the caller's own account carrying `id`.
fn own_arn(id: &str) -> String {
    source_arn("aws", REGION, engine::emulator_account_id(), id)
}

/// Assert one refusal: the status, the code, the member-named sentence, and that the
/// sentence quotes nothing out of the request.
fn assert_refused(case: &str, op: Op, value: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?} on {op:?}: expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?} on {op:?}: expected error code ValidationException, got {body:?}"
    );
    let message = text(body, "Message");
    assert_eq!(
        message,
        state::INVALID_IDENTITY_SOURCE_ARN_MESSAGE,
        "case {case:?} on {op:?}: expected the sentence naming the member, got {body:?}"
    );
    assert!(
        !message.contains(value),
        "case {case:?} on {op:?}: the refusal echoed the rejected value: {message:?}"
    );
}

/// An identity-source ARN whose resource id is not an id this service mints addresses no
/// identity source: both operations refuse it with the member-named `ValidationException`
/// (400), and nothing in the store moves.
#[tokio::test]
async fn an_id_outside_the_minted_form_is_refused_by_every_operation() {
    let harness = Harness::new();
    let bystander = create_source(&harness, 1, 0).await;

    let mut rows: Vec<(String, String)> = ids_outside_the_minted_form()
        .into_iter()
        .map(|(case, id)| (case.to_string(), own_arn(&id)))
        .collect();
    // `@length(max: 1000)`: the longest value a conforming request can carry, padded in an
    // id that is still outside the minted form.
    rows.push(("an id at the member's maximum length".to_string(), {
        let base = own_arn("uuid-");
        format!("{base}{}", "f".repeat(MAX_IDENTITY_SOURCE_ARN - base.len()))
    }));

    for (case, value) in rows {
        for op in Op::ALL {
            let (status, body) = op.call(&harness, &value).await;
            assert_refused(&case, op, &value, status, &body);
            assert_eq!(
                stored(&harness, &value),
                None,
                "case {case:?} on {op:?}: the refused call recorded an identity source"
            );
        }
        assert!(
            listed_status(&harness, &bystander).await.is_some(),
            "case {case:?}: a refused call removed the caller's own identity source"
        );
    }

    let store = state::service_state(&*harness.engine).expect("the service store");
    let remaining = store.lock().identity_sources.len();
    assert_eq!(
        remaining, 1,
        "the store holds {remaining} source(s), expected only the bystander"
    );
}

/// The id form is screened BEFORE the account boundary and before any lookup: an ARN whose
/// id is outside the form earns the 400 even when it names another account, while an ARN
/// whose id IS in the form and names another account earns the boundary's 403.
#[tokio::test]
async fn the_id_form_is_screened_before_the_account_boundary() {
    let harness = Harness::new();
    let foreign = "999999999999";

    for (case, id) in ids_outside_the_minted_form() {
        let value = source_arn("aws", REGION, foreign, &id);
        for op in Op::ALL {
            let (status, body) = op.call(&harness, &value).await;
            assert_refused(&format!("{case} in another account"), op, &value, status, &body);
        }
    }

    for (case, id) in ids_in_the_minted_form() {
        let value = source_arn("aws", REGION, foreign, &id);
        for op in Op::ALL {
            let (status, body) = op.call(&harness, &value).await;
            assert_eq!(
                status, 403,
                "case {case:?} on {op:?}: expected HTTP 403, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?} on {op:?}: expected error code AccessDeniedException, got \
                 {body:?}"
            );
        }
    }
}

/// An id IN the minted form addresses an identity source, so each operation gives its own
/// answer for one this store holds nothing under: the read is `ResourceNotFoundException`
/// (404) and the `@idempotent` delete succeeds and writes nothing. Rows span the partition
/// and the region as well as the id, since the ARN carries where the source lives.
#[tokio::test]
async fn an_id_in_the_minted_form_reaches_each_operations_own_answer() {
    let harness = Harness::new();
    let bystander = create_source(&harness, 1, 0).await;
    let account = engine::emulator_account_id();

    let mut rows: Vec<(String, String)> = ids_in_the_minted_form()
        .into_iter()
        .map(|(case, id)| (case.to_string(), own_arn(&id)))
        .collect();
    // `aws(-[^:]+)?` admits every suffixed partition, and `[a-z0-9-]{1,20}` every region.
    rows.push((
        "the minted form in the China partition".to_string(),
        source_arn(
            "aws-cn",
            "cn-north-1",
            account,
            &format!("{}000000000001", state::IDENTITY_SOURCE_ID_PREFIX),
        ),
    ));
    rows.push((
        "the minted form in a region nothing was created in".to_string(),
        source_arn(
            "aws",
            "eu-west-1",
            account,
            &format!("{}000000000001", state::IDENTITY_SOURCE_ID_PREFIX),
        ),
    ));
    // `@length(max: 1000)`: an ARN padded in its id, still in the minted form.
    rows.push(("the minted form at the member's maximum length".to_string(), {
        let base = own_arn(state::IDENTITY_SOURCE_ID_PREFIX);
        format!("{base}{}", "0".repeat(MAX_IDENTITY_SOURCE_ARN - base.len()))
    }));

    for (case, value) in rows {
        let (status, read) = Op::Get.call(&harness, &value).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from the read, got {status} with {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case:?}: expected error code ResourceNotFoundException, got {read:?}"
        );
        assert_eq!(
            text(&read, "Message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected this service's not-found message, got {read:?}"
        );

        let (status, deleted) = Op::Delete.call(&harness, &value).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {status} with {deleted:?}"
        );
        assert_eq!(
            stored(&harness, &value),
            None,
            "case {case:?}: the no-op delete recorded an identity source"
        );
        assert!(
            listed_status(&harness, &bystander).await.is_some(),
            "case {case:?}: the call removed the caller's own identity source"
        );
    }
}

/// The screen refuses only ids this service could not have minted: the ARN a create minted
/// passes it, reads back through `GetIdentitySource` and still deletes, the delete reported
/// on the record the read reports.
#[tokio::test]
async fn an_arn_this_service_minted_still_reads_back_and_still_deletes() {
    let harness = Harness::new();
    let arn = create_source(&harness, 1, 0).await;
    let id = arn
        .rsplit('/')
        .next()
        .expect("the resource id of a minted ARN")
        .to_string();
    assert!(
        state::is_identity_source_id(&id),
        "the minted id {id:?} is outside the form this service accepts"
    );

    let (status, before) = Op::Get.call(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "reading the minted source returned {status} with {before:?}"
    );
    assert_eq!(
        text(&before, "IdentitySourceArn"),
        arn,
        "the read reported another source: {before:?}"
    );

    let (status, deleted) = Op::Delete.call(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "deleting the minted source returned {status} with {deleted:?}"
    );
    // The removal is CLOCK-OWED, so straight after the call the record is reported in the
    // non-terminal status the model declares for it.
    let (status, after) = Op::Get.call(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "re-reading the deleted source returned {status} with {after:?}"
    );
    assert_eq!(
        text(&after, "Status"),
        state::IdentitySourceStatus::Deleting.as_wire(),
        "expected the source reported DELETING, got {after:?}"
    );
}

/// The member-named sentence belongs to the member that ADDRESSES an identity source: the
/// `ResourceArn` family, whose refusal this service words with one service-wide sentence,
/// keeps that sentence for the same value.
#[tokio::test]
async fn the_resource_arn_family_keeps_the_service_wide_sentence() {
    let harness = Harness::new();

    for (case, id) in ids_outside_the_minted_form() {
        let value = own_arn(&id);
        // `GET /tags/{ResourceArn}` — `ListTagsForResource`, keyed by `ResourceArn`
        // rather than by `IdentitySourceArn`.
        let (status, body) = harness
            .call("GET", &format!("/tags/{}", encode_label(&value)), None)
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a well-formed MPA ARN is the tag family's existence answer, \
             got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected error code ResourceNotFoundException, got {body:?}"
        );
    }

    // And a value that is no ARN at all still earns that family's own service-wide
    // malformed-ARN sentence.
    let (status, body) = harness
        .call("GET", &format!("/tags/{}", encode_label("not-an-arn")), None)
        .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} with {body:?}");
    assert_eq!(
        text(&body, "Message"),
        state::INVALID_ARN_MESSAGE,
        "expected the service-wide malformed-ARN sentence, got {body:?}"
    );
}
