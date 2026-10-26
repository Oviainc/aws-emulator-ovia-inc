// SPDX-License-Identifier: CC0-1.0

//! Reusing one `ClientToken` for a DIFFERENT request, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test: a `ClientToken` identifies the whole REQUEST, so a token already
//! spent replays the first call's answer only for that same request; a request carrying
//! a spent token with any different member is a different request, and is refused with
//! the modelled `ConflictException` (HTTP 409) while nothing is created or written.
//! `tests/all/client_token_idempotency.rs` states the other half of the rule — the
//! IDENTICAL request replays — and is asserted again here, so neither half can be
//! satisfied by giving up the other.
//!
//! The tables span what the rule generalises over: WHICH `ClientToken`-carrying
//! operation is replayed (`CreateBenefitApplication`, `UpdateBenefitApplication`,
//! `AmendBenefitApplication`, `CancelBenefitApplication`, `RecallBenefitApplication`),
//! which member of the request differs, and HOW it differs — a changed scalar, a changed
//! document, a member added, a member dropped, a list grown, a list reordered, and an
//! identifier naming a resource that does not exist.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::IDEMPOTENT_PARAMETER_MISMATCH;

use crate::benefit_application_lifecycle::{error_code, text, Harness, ACCOUNT, CATALOG, REGION};
use crate::resource_tagging::error_message;

/// A benefit id no catalog holds, in the form the modelled `BenefitId` pattern admits
/// (`^(arn:.+|ben-[0-9a-z]{14})$`).
const UNKNOWN_BENEFIT: &str = "ben-00000000000000";

/// Build one request body from `base` with `change` applied to it.
fn with(base: &Value, change: impl FnOnce(&mut Value)) -> Value {
    let mut body = base.clone();
    change(&mut body);
    body
}

/// Drop one member from a request body, which is one way a second request can differ
/// from the first.
fn without(base: &Value, member: &str) -> Value {
    with(base, |body| {
        body.as_object_mut()
            .expect("a request body is a JSON object")
            .remove(member);
    })
}

/// A request body with `token` as its `ClientToken`.
fn under(body: &Value, token: &str) -> Value {
    with(body, |body| body["ClientToken"] = json!(token))
}

/// Assert `out` is the refusal a reused `ClientToken` carrying a different request earns:
/// the modelled `ConflictException`, 409, in AWS's words for this error class.
fn assert_mismatch(case: &str, status: u16, out: &Value) {
    assert_eq!(
        status, 409,
        "case {case:?}: reusing a spent ClientToken for a different request must answer 409, got {status} {out:?}"
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

/// Every application id the catalog holds, so a refused create can be shown to have
/// created nothing.
async fn stored_ids(harness: &Harness, case: &str) -> Vec<String> {
    let listed = harness
        .ok(
            case,
            "ListBenefitApplications",
            &json!({ "Catalog": CATALOG }),
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

/// A create reusing a spent `ClientToken` with any different member is refused and
/// creates nothing — while the same request under a fresh token is accepted, so what is
/// refused is the token reuse and not the member that changed.
#[tokio::test]
async fn a_create_reusing_a_spent_token_for_a_different_request_is_refused() {
    let harness = Harness::new();
    let active: Vec<String> = harness
        .benefits(CATALOG)
        .await
        .iter()
        .filter(|benefit| benefit["Status"] == json!("ACTIVE"))
        .map(|benefit| text("seed", benefit, "Id"))
        .collect();
    assert!(
        active.len() >= 2,
        "the seeded catalog must offer at least two ACTIVE benefits, got {active:?}"
    );
    let (first_benefit, other_benefit) = (&active[0], &active[1]);
    let one_resource = format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/benefits-one");
    let another_resource = format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/benefits-two");

    // The create every row replays the token of: a request carrying every member the
    // operation binds, so a row can differ in any one of them. The details document
    // names no field the seeded `BenefitRequestSchema`s declare, so it satisfies every
    // benefit's schema.
    let base = json!({
        "Catalog": CATALOG,
        "BenefitIdentifier": first_benefit,
        "Name": "northstar",
        "Description": "the first request",
        "FulfillmentTypes": ["CASH"],
        "BenefitApplicationDetails": { "Notes": "spring", "Round": 1 },
        "Tags": [{ "Key": "team", "Value": "alliances" }],
        "AssociatedResources": [one_resource, another_resource],
        "PartnerContacts": [{ "Email": "ada@example.com", "FirstName": "Ada" }],
        "FileDetails": [{ "FileURI": "s3://b/first.csv" }],
    });

    // (case, the second request, what that request answers under a FRESH token)
    let rows: Vec<(&str, Value, u16)> = vec![
        (
            "another benefit of the same catalog",
            with(&base, |body| body["BenefitIdentifier"] = json!(other_benefit)),
            200,
        ),
        (
            "a benefit no catalog holds",
            with(&base, |body| body["BenefitIdentifier"] = json!(UNKNOWN_BENEFIT)),
            404,
        ),
        (
            "a different Name",
            with(&base, |body| body["Name"] = json!("northstar-ii")),
            200,
        ),
        (
            "a different Description",
            with(&base, |body| body["Description"] = json!("the second request")),
            200,
        ),
        ("no Description at all", without(&base, "Description"), 200),
        (
            "another FulfillmentType",
            with(&base, |body| body["FulfillmentTypes"] = json!(["CREDITS"])),
            200,
        ),
        (
            "one more FulfillmentType",
            with(&base, |body| body["FulfillmentTypes"] = json!(["CASH", "CREDITS"])),
            200,
        ),
        (
            "a different value in BenefitApplicationDetails",
            with(&base, |body| {
                body["BenefitApplicationDetails"] = json!({ "Notes": "summer", "Round": 1 })
            }),
            200,
        ),
        (
            "a different TYPE in BenefitApplicationDetails",
            with(&base, |body| {
                body["BenefitApplicationDetails"] = json!({ "Notes": "spring", "Round": "1" })
            }),
            200,
        ),
        (
            "one more member in BenefitApplicationDetails",
            with(&base, |body| {
                body["BenefitApplicationDetails"] =
                    json!({ "Notes": "spring", "Round": 1, "Owner": "ada" })
            }),
            200,
        ),
        (
            "a different tag value",
            with(&base, |body| {
                body["Tags"] = json!([{ "Key": "team", "Value": "marketing" }])
            }),
            200,
        ),
        (
            "one more tag",
            with(&base, |body| {
                body["Tags"] = json!([
                    { "Key": "team", "Value": "alliances" },
                    { "Key": "stage", "Value": "draft" },
                ])
            }),
            200,
        ),
        ("no Tags at all", without(&base, "Tags"), 200),
        (
            "fewer associated resources",
            with(&base, |body| {
                body["AssociatedResources"] = json!([one_resource])
            }),
            200,
        ),
        (
            "the same associated resources in another order",
            with(&base, |body| {
                body["AssociatedResources"] = json!([another_resource, one_resource])
            }),
            200,
        ),
        (
            "a different partner contact",
            with(&base, |body| {
                body["PartnerContacts"] = json!([{ "Email": "grace@example.com", "FirstName": "Grace" }])
            }),
            200,
        ),
        (
            "one more member on the partner contact",
            with(&base, |body| {
                body["PartnerContacts"] = json!([{
                    "Email": "ada@example.com",
                    "FirstName": "Ada",
                    "LastName": "Lovelace",
                }])
            }),
            200,
        ),
        (
            "a different file",
            with(&base, |body| {
                body["FileDetails"] = json!([{ "FileURI": "s3://b/second.csv" }])
            }),
            200,
        ),
        ("no files at all", without(&base, "FileDetails"), 200),
    ];

    for (index, (case, second, fresh_status)) in rows.iter().enumerate() {
        let token = format!("mismatch-{index}");
        let first = harness.create(case, &under(&base, &token)).await;
        let id = text(case, &first, "Id");
        let after_first = harness.get(case, &id).await;

        // The IDENTICAL request still replays the first answer: the mismatch rule does
        // not cost the retry the token exists for.
        let replay = harness.create(case, &under(&base, &token)).await;
        assert_eq!(
            replay, first,
            "case {case:?}: the identical request must replay the first answer {first:?}, got {replay:?}"
        );

        let before_ids = stored_ids(&harness, case).await;
        let (status, out) = harness
            .call("CreateBenefitApplication", &under(second, &token))
            .await;
        assert_mismatch(case, status, &out);
        assert_eq!(
            stored_ids(&harness, case).await,
            before_ids,
            "case {case:?}: a refused create must create nothing"
        );
        let after_refusal = harness.get(case, &id).await;
        assert_eq!(
            after_refusal, after_first,
            "case {case:?}: a refused create must leave the first application alone, expected {after_first:?}, got {after_refusal:?}"
        );

        // The very same request under a FRESH token: what the token reuse refused is the
        // reuse, so a request this service accepts is accepted here, and one naming a
        // benefit that does not exist is the 404 it always was.
        let fresh = under(second, &format!("fresh-{index}"));
        let (status, out) = harness.call("CreateBenefitApplication", &fresh).await;
        assert_eq!(
            status, *fresh_status,
            "case {case:?}: under a fresh token the same request must answer {fresh_status}, got {status} {out:?}"
        );
        if *fresh_status == 200 {
            assert_ne!(
                out["Id"], first["Id"],
                "case {case:?}: a fresh token must mint a new application, got {out:?}"
            );
        } else {
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {out:?}"
            );
        }
    }
}

/// One `ClientToken`-carrying write request: the body is built from the application it
/// names, that application's current `Revision`, and the token under test.
type BodyOf = fn(&str, &str, &str) -> Value;

/// Every other operation that carries a `ClientToken` refuses a spent token on a
/// different request too, and writes nothing — the same rule the create applies, wherever
/// a token is spent.
#[tokio::test]
async fn every_token_carrying_write_refuses_a_spent_token_for_a_different_request() {
    // (case, operation, submit the application first, the first request, the second)
    let rows: [(&str, &str, bool, BodyOf, BodyOf); 7] = [
        (
            "an update naming a different Name",
            "UpdateBenefitApplication",
            false,
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "Name": "updated-once",
                })
            },
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "Name": "updated-twice",
                })
            },
        ),
        (
            "an update carrying a member the first did not",
            "UpdateBenefitApplication",
            false,
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "Name": "updated-once",
                })
            },
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "Name": "updated-once",
                    "Description": "and a description",
                })
            },
        ),
        (
            "an update naming a different document",
            "UpdateBenefitApplication",
            false,
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "BenefitApplicationDetails": { "Notes": "spring" },
                })
            },
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "BenefitApplicationDetails": { "Notes": "autumn" },
                })
            },
        ),
        (
            "an amendment naming a different value",
            "AmendBenefitApplication",
            false,
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "AmendmentReason": "a correction",
                    "Amendments": [{ "FieldPath": "Name", "NewValue": "amended-once" }],
                })
            },
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "AmendmentReason": "a correction",
                    "Amendments": [{ "FieldPath": "Name", "NewValue": "amended-twice" }],
                })
            },
        ),
        (
            "an amendment naming a different reason",
            "AmendBenefitApplication",
            false,
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "AmendmentReason": "the partner corrected the figures",
                    "Amendments": [{ "FieldPath": "Description", "NewValue": "amended" }],
                })
            },
            |id, revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Revision": revision,
                    "AmendmentReason": "AWS asked for a correction",
                    "Amendments": [{ "FieldPath": "Description", "NewValue": "amended" }],
                })
            },
        ),
        (
            "a cancellation naming a different reason",
            "CancelBenefitApplication",
            false,
            |id, _revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Reason": "no longer needed",
                })
            },
            |id, _revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Reason": "the customer withdrew",
                })
            },
        ),
        (
            "a recall naming a different reason",
            "RecallBenefitApplication",
            true,
            |id, _revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Reason": "more detail needed",
                })
            },
            |id, _revision, token| {
                json!({
                    "Catalog": CATALOG,
                    "ClientToken": token,
                    "Identifier": id,
                    "Reason": "the figures were wrong",
                })
            },
        ),
    ];

    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    for (index, (case, operation, submit_first, first_of, second_of)) in rows.iter().enumerate() {
        let created = harness
            .create(
                case,
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("seed-{index}"),
                    "BenefitIdentifier": benefit_id,
                    "Name": "demo",
                    "Description": "demo app",
                }),
            )
            .await;
        let id = text(case, &created, "Id");
        if *submit_first {
            harness
                .ok(
                    case,
                    "SubmitBenefitApplication",
                    &json!({ "Catalog": CATALOG, "Identifier": id }),
                )
                .await;
        }

        let revision = text(case, &harness.get(case, &id).await, "Revision");
        let token = format!("spent-{index}");
        let first_body = first_of(&id, &revision, &token);
        let first = harness.ok(case, operation, &first_body).await;
        let after_first = harness.get(case, &id).await;

        let (status, out) = harness
            .call(operation, &second_of(&id, &revision, &token))
            .await;
        assert_mismatch(case, status, &out);
        let after_refusal = harness.get(case, &id).await;
        assert_eq!(
            after_refusal, after_first,
            "case {case:?}: a refused write must change nothing, expected {after_first:?}, got {after_refusal:?}"
        );

        // The identical request still replays what the first call reported.
        let replay = harness.ok(case, operation, &first_body).await;
        assert_eq!(
            replay, first,
            "case {case:?}: the identical request must replay the first answer {first:?}, got {replay:?}"
        );
        let after_replay = harness.get(case, &id).await;
        assert_eq!(
            after_replay, after_first,
            "case {case:?}: the replay must change nothing, expected {after_first:?}, got {after_replay:?}"
        );
    }
}
