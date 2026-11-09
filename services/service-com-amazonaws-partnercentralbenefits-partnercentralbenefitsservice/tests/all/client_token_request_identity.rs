// SPDX-License-Identifier: CC0-1.0

//! WHICH request a `ClientToken` identifies, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test: a `ClientToken` identifies ONE request of its operation for the
//! whole service, and that request's identity is the RECORDS its members name rather than
//! the spellings they were written with. So a retry naming the same records another way an
//! identifier member's modelled pattern admits is that same request and is replayed, and a
//! request repeating a spent token that names a different record — another application,
//! another catalog — is a different request and is refused with the modelled
//! `ConflictException` (HTTP 409), whichever record the token was spent on.
//!
//! `tests/all/client_token_idempotency.rs` and `tests/all/idempotent_parameter_mismatch.rs`
//! state the two halves of the replay rule over the members a request carries; these
//! tables span what this crate's identifier members generalise over: WHICH
//! `ClientToken`-carrying operation is replayed (`CreateBenefitApplication`,
//! `UpdateBenefitApplication`, `AmendBenefitApplication`, `CancelBenefitApplication`,
//! `RecallBenefitApplication`), which identifier member carries the record (the
//! application's `Identifier`, the create's `BenefitIdentifier`, the `Catalog`), which FORM
//! that member used (the bare id and the ARN, in both directions), and whether the record
//! named is the one the token was spent on.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    IDEMPOTENT_PARAMETER_MISMATCH, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::resource_tagging::error_message;

/// One `ClientToken`-carrying write request: the body is built from the application
/// identifier it names, that application's current `Revision`, and the token under test. A
/// write that binds no `Revision` member ignores the revision it is handed.
type BodyOf = fn(&str, &str, &str) -> Value;

/// The write rows every table below runs: (case, operation, submit the application first,
/// the request body). One row per operation this service gives a `ClientToken` and an
/// application `Identifier`.
const WRITE_ROWS: [(&str, &str, bool, BodyOf); 4] = [
    (
        "an update",
        "UpdateBenefitApplication",
        false,
        |identifier, revision, token| {
            json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "Identifier": identifier,
                "Revision": revision,
                "Name": "renamed-by-the-first-call",
            })
        },
    ),
    (
        "an amendment",
        "AmendBenefitApplication",
        false,
        |identifier, revision, token| {
            json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "Identifier": identifier,
                "Revision": revision,
                "AmendmentReason": "a correction",
                "Amendments": [{ "FieldPath": "Name", "NewValue": "amended-by-the-first-call" }],
            })
        },
    ),
    (
        "a cancellation",
        "CancelBenefitApplication",
        false,
        |identifier, _revision, token| {
            json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "Identifier": identifier,
                "Reason": "no longer needed",
            })
        },
    ),
    (
        "a recall",
        "RecallBenefitApplication",
        true,
        |identifier, _revision, token| {
            json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "Identifier": identifier,
                "Reason": "more detail needed",
            })
        },
    ),
];

/// An application id no catalog holds, in the form the modelled
/// `BenefitApplicationIdentifier` pattern admits (`^(arn:.+|benappl-[0-9a-z]{14})$`).
const UNKNOWN_APPLICATION: &str = "benappl-00000000000000";

/// One create request body.
fn create_body(catalog: &str, benefit_identifier: &str, token: &str) -> Value {
    json!({
        "Catalog": catalog,
        "ClientToken": token,
        "BenefitIdentifier": benefit_identifier,
        "Name": "request-identity",
        "Description": "one request, whichever way its members are spelled",
    })
}

/// Create one application of [`CATALOG`] to write to, submitting it first where the row
/// needs a submitted one, and return `(its id, its ARN, its current Revision)`.
async fn seed(harness: &Harness, case: &str, token: &str, submit: bool) -> (String, String, String) {
    let benefit_id = text(case, &harness.active_benefit().await, "Id");
    let created = harness
        .create(case, &create_body(CATALOG, &benefit_id, token))
        .await;
    let id = text(case, &created, "Id");
    let arn = text(case, &created, "Arn");
    if submit {
        harness
            .ok(
                case,
                "SubmitBenefitApplication",
                &json!({ "Catalog": CATALOG, "Identifier": id }),
            )
            .await;
    }
    let revision = text(case, &harness.get(case, &id).await, "Revision");
    (id, arn, revision)
}

/// The ids of every application one catalog holds, so a refused create can be shown to
/// have created nothing.
///
/// Shared with `tests/all/create_canonical_request_members.rs`, which shows the same of a
/// replayed create.
pub(crate) async fn application_ids(harness: &Harness, case: &str, catalog: &str) -> Vec<String> {
    let listed = harness
        .ok(
            case,
            "ListBenefitApplications",
            &json!({ "Catalog": catalog }),
        )
        .await;
    let mut ids: Vec<String> = listed["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| text(case, summary, "Id"))
        .collect();
    ids.sort();
    ids
}

/// Assert `out` is the refusal a spent `ClientToken` on a different request earns: the
/// modelled `ConflictException`, 409, in AWS's words for this error class.
fn assert_mismatch(case: &str, status: u16, out: &Value) {
    assert_eq!(
        status, 409,
        "case {case:?}: a spent ClientToken naming another record must answer 409, got {status} {out:?}"
    );
    assert_eq!(
        error_code(out),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {out:?}"
    );
    assert_eq!(
        error_message(out),
        IDEMPOTENT_PARAMETER_MISMATCH,
        "case {case:?}: expected the idempotency-mismatch message, got {out:?}"
    );
}

/// A retry that names the same application the OTHER way the modelled
/// `BenefitApplicationIdentifier` admits (`^(arn:.+|benappl-[0-9a-z]{14})$`) is the same
/// request: it answers what the first call reported and applies nothing a second time.
///
/// The rows are every `ClientToken`-carrying write that binds an application `Identifier`,
/// each run in both directions — the first call by id and the retry by ARN, and the other
/// way round — so neither form is privileged.
#[tokio::test]
async fn a_retry_spelling_the_application_the_other_way_is_the_same_request() {
    let harness = Harness::new();
    for (index, (operation_case, operation, submit_first, body_of)) in
        WRITE_ROWS.iter().enumerate()
    {
        for (direction, slug, first_is_arn) in
            [("id then arn", "id-arn", false), ("arn then id", "arn-id", true)]
        {
            let case = format!("{operation_case}, {direction}");
            let (id, arn, revision) =
                seed(&harness, &case, &format!("seed-form-{index}-{slug}"), *submit_first).await;
            let (first_form, retry_form) = if first_is_arn {
                (arn.as_str(), id.as_str())
            } else {
                (id.as_str(), arn.as_str())
            };

            let token = format!("form-{index}-{slug}");
            let first = harness
                .ok(&case, operation, &body_of(first_form, &revision, &token))
                .await;
            let after_first = harness.get(&case, &id).await;

            let (status, out) = harness
                .call(operation, &body_of(retry_form, &revision, &token))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the same request spelled {retry_form:?} must be replayed, got {status} {out:?}"
            );
            assert_eq!(
                out, first,
                "case {case:?}: the retry must answer the first call's output {first:?}, got {out:?}"
            );
            let after_retry = harness.get(&case, &id).await;
            assert_eq!(
                after_retry, after_first,
                "case {case:?}: the retry must change nothing, expected {after_first:?}, got {after_retry:?}"
            );
        }
    }
}

/// The create applies the same rule to its own identifier member: a retry naming the same
/// benefit by its ARN instead of its `ben-...` id is the same create, so it answers the
/// first call's id, ARN and revision and mints no second application.
#[tokio::test]
async fn a_create_retry_spelling_the_benefit_the_other_way_is_the_same_request() {
    let harness = Harness::new();
    let benefit = harness.active_benefit().await;
    let benefit_id = text("seed", &benefit, "Id");
    let benefit_arn = text("seed", &benefit, "Arn");

    for (direction, slug, first_is_arn) in
        [("id then arn", "id-arn", false), ("arn then id", "arn-id", true)]
    {
        let case = format!("a create, {direction}");
        let (first_form, retry_form) = if first_is_arn {
            (benefit_arn.as_str(), benefit_id.as_str())
        } else {
            (benefit_id.as_str(), benefit_arn.as_str())
        };
        let token = format!("create-form-{slug}");

        let first = harness
            .create(&case, &create_body(CATALOG, first_form, &token))
            .await;
        let minted = application_ids(&harness, &case, CATALOG).await;

        let (status, out) = harness
            .call(
                "CreateBenefitApplication",
                &create_body(CATALOG, retry_form, &token),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same create spelled {retry_form:?} must be replayed, got {status} {out:?}"
        );
        assert_eq!(
            out, first,
            "case {case:?}: the retry must answer the first call's output {first:?}, got {out:?}"
        );
        assert_eq!(
            application_ids(&harness, &case, CATALOG).await,
            minted,
            "case {case:?}: a replayed create must mint nothing"
        );
    }
}

/// A spent `ClientToken` on a request naming ANOTHER application is a different request:
/// it is refused, that application is left exactly as it was, and the very same request
/// under a fresh token is served — so what the refusal answers is the token reuse.
///
/// The two applications are seeded identically, so the identifier is the only member the
/// second request differs in.
#[tokio::test]
async fn a_spent_token_naming_another_application_is_refused() {
    let harness = Harness::new();
    for (index, (operation_case, operation, submit_first, body_of)) in
        WRITE_ROWS.iter().enumerate()
    {
        let case = format!("{operation_case} of another application");
        let (spent_on, _, spent_revision) =
            seed(&harness, &case, &format!("seed-scope-a-{index}"), *submit_first).await;
        let (other, _, other_revision) =
            seed(&harness, &case, &format!("seed-scope-b-{index}"), *submit_first).await;
        assert_eq!(
            other_revision, spent_revision,
            "case {case:?}: the two applications must start at one revision, got {other_revision:?} and {spent_revision:?}"
        );

        let token = format!("scope-{index}");
        harness
            .ok(&case, operation, &body_of(&spent_on, &spent_revision, &token))
            .await;
        let other_before = harness.get(&case, &other).await;

        let (status, out) = harness
            .call(operation, &body_of(&other, &other_revision, &token))
            .await;
        assert_mismatch(&case, status, &out);
        let other_after = harness.get(&case, &other).await;
        assert_eq!(
            other_after, other_before,
            "case {case:?}: a refused write must change nothing, expected {other_before:?}, got {other_after:?}"
        );

        // The same request under a FRESH token is the request it always was.
        let (status, out) = harness
            .call(
                operation,
                &body_of(&other, &other_revision, &format!("fresh-{index}")),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: under a fresh token the same request must be served, got {status} {out:?}"
        );
    }
}

/// The `Catalog` a create names is a member of its request too: a create repeating a spent
/// token in ANOTHER catalog is a different request, refused and minting nothing, whichever
/// benefit it names — while the same create under a fresh token mints its application.
#[tokio::test]
async fn a_spent_create_token_in_another_catalog_is_refused() {
    let harness = Harness::new();
    let other_catalog = SEEDED_CATALOGS
        .iter()
        .copied()
        .find(|catalog| *catalog != CATALOG)
        .expect("the service serves more than one catalog");
    let spent_benefit = text("seed", &harness.active_benefit().await, "Id");
    let other_benefit = text(
        "seed",
        &harness.active_benefit_of(other_catalog).await,
        "Id",
    );
    let token = "create-catalog-scope";

    let first = harness
        .create("a create", &create_body(CATALOG, &spent_benefit, token))
        .await;

    // (case, the benefit the second create names in the other catalog)
    let rows: [(&str, &str); 2] = [
        (
            "the other catalog, naming the first catalog's benefit",
            spent_benefit.as_str(),
        ),
        (
            "the other catalog, naming its own benefit",
            other_benefit.as_str(),
        ),
    ];
    for (case, benefit_identifier) in rows {
        let before = application_ids(&harness, case, other_catalog).await;
        let (status, out) = harness
            .call(
                "CreateBenefitApplication",
                &create_body(other_catalog, benefit_identifier, token),
            )
            .await;
        assert_mismatch(case, status, &out);
        assert_eq!(
            application_ids(&harness, case, other_catalog).await,
            before,
            "case {case:?}: a refused create must mint nothing"
        );
    }

    // The first catalog's create still replays, and the other catalog's create is served
    // under a token of its own.
    let replay = harness
        .create("a create", &create_body(CATALOG, &spent_benefit, token))
        .await;
    assert_eq!(
        replay, first,
        "case \"a create\": the request the token was spent on must still replay {first:?}, got {replay:?}"
    );
    let served = harness
        .create(
            "the other catalog under a fresh token",
            &create_body(other_catalog, &other_benefit, "create-catalog-fresh"),
        )
        .await;
    assert_ne!(
        served["Id"], first["Id"],
        "case \"the other catalog under a fresh token\": a fresh token must mint a new application, got {served:?}"
    );
}

/// An identifier naming no stored application is its own identity, so it too is a member
/// the comparison differs on: a write carrying a SPENT token and such an identifier is the
/// token reuse (409 `ConflictException`), while the same request under a fresh token is the
/// `ResourceNotFoundException` (404) it always was.
///
/// The create pins the same pair for its own identifier member in
/// `tests/all/idempotent_parameter_mismatch.rs` ("a benefit no catalog holds"); these rows
/// are every write that names an application.
#[tokio::test]
async fn a_spent_token_naming_no_stored_application_is_refused_as_the_reuse() {
    let harness = Harness::new();
    for (index, (operation_case, operation, submit_first, body_of)) in
        WRITE_ROWS.iter().enumerate()
    {
        let case = format!("{operation_case} of an application that does not exist");
        let (spent_on, _, revision) =
            seed(&harness, &case, &format!("seed-absent-{index}"), *submit_first).await;

        let token = format!("absent-{index}");
        harness
            .ok(&case, operation, &body_of(&spent_on, &revision, &token))
            .await;

        let (status, out) = harness
            .call(operation, &body_of(UNKNOWN_APPLICATION, &revision, &token))
            .await;
        assert_mismatch(&case, status, &out);

        let (status, out) = harness
            .call(
                operation,
                &body_of(UNKNOWN_APPLICATION, &revision, &format!("absent-fresh-{index}")),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: under a fresh token an application that does not exist must answer 404, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {out:?}"
        );
    }
}
