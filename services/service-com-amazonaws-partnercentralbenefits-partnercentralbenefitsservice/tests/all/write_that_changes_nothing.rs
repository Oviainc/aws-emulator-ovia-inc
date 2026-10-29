// SPDX-License-Identifier: CC0-1.0

//! What a write that changes nothing does to a stored benefit application, end to end
//! over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication`, so what these tests pin is the emulator's HTTP behaviour on
//! the persisted record.
//!
//! The rule under test: `Revision` is the application's optimistic-concurrency token (the
//! model documents the input member as "the current revision number of the benefit
//! application to ensure optimistic concurrency control"), so a write moves it — and the
//! `UpdatedAt` beside it, and the idempotency window those two describe — only when the
//! write actually CHANGED the stored record. A request that applies nothing, because it
//! names no member to change or names only values the record already holds, is accepted
//! and answers with the revision the record already carries, leaving every reported
//! member and `UpdatedAt` exactly as they were.
//!
//! The tests are tables whose rows differ along what the rule generalises over: which
//! members the request names, whether each names the stored value or a new one, the
//! COUNT of the entries in a list member and of the members of an entry, and which of the
//! two writes that apply caller-supplied members carries the request
//! (`UpdateBenefitApplication`, `AmendBenefitApplication`).

use serde_json::{json, Value};

use crate::benefit_application_lifecycle::{error_code, instant, text, Harness, CATALOG};

/// The `Name` every application below is created with, so a request naming it is naming
/// the value the record already holds.
const SEED_NAME: &str = "change-nothing-subject";

/// The `Description` every application below is created with.
const SEED_DESCRIPTION: &str = "the application the no-op write rule is asserted on";

/// The `CampaignName` the seeded details document carries, which an amendment can name
/// again.
const SEED_CAMPAIGN: &str = "spring";

/// The stored `BenefitApplicationDetails`: the two fields the first `ACTIVE` benefit of
/// `CATALOG` declares in its `BenefitRequestSchema` (`CampaignName` string,
/// `RequestedAmount` number), so the document the create stores satisfies that schema.
fn seed_details() -> Value {
    json!({ "CampaignName": SEED_CAMPAIGN, "RequestedAmount": 2500 })
}

/// The stored `PartnerContacts`: one contact, which is as many as the modelled `Contacts`
/// list admits (`@length(max: 1)`).
fn seed_contacts() -> Value {
    json!([{ "Email": "ada@example.com", "FirstName": "Ada", "LastName": "Lovelace" }])
}

/// The stored `FileDetails`, as the `FileInput` members a request carries.
fn seed_files() -> Value {
    json!([{
        "FileURI": "s3://partner-bucket/proposals/plan.pdf",
        "BusinessUseCase": "funding request",
    }])
}

/// Create one draft application carrying every member an update can change, and return
/// its id.
async fn seed(harness: &Harness, case: &str, token: &str) -> String {
    let benefit_id = text(case, &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": SEED_NAME,
                "Description": SEED_DESCRIPTION,
                "BenefitApplicationDetails": seed_details(),
                "PartnerContacts": seed_contacts(),
                "FileDetails": seed_files(),
            }),
        )
        .await;
    text(case, &created, "Id")
}

/// One update request body carrying `change` plus the members every update requires.
fn update_body(id: &str, revision: &str, token: &str, change: &Value) -> Value {
    let mut body = change.clone();
    body["Catalog"] = json!(CATALOG);
    body["ClientToken"] = json!(token);
    body["Identifier"] = json!(id);
    body["Revision"] = json!(revision);
    body
}

/// A read application's `Revision` as the number it renders, so two tokens compare by
/// value and the step between them is observable.
fn revision_of(case: &str, body: &Value) -> u64 {
    let rendered = text(case, body, "Revision");
    rendered.parse().unwrap_or_else(|error| {
        panic!("case {case:?}: Revision {rendered:?} is not a number: {error}")
    })
}

/// An update applies the members whose value is not the one the record already holds, and
/// moves the revision and `UpdatedAt` only when it applied something: a request naming no
/// member, or naming only stored values, answers the revision the record already carries
/// and leaves the whole record — every reported member, the revision and `UpdatedAt` —
/// exactly as it was.
///
/// Each row runs on its own freshly created application, so the rows are independent.
#[tokio::test]
async fn an_update_moves_the_revision_only_when_it_changed_the_record() {
    let harness = Harness::new();

    // (the case, the members the update names, whether that changes the stored record)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("no optional member at all", json!({}), false),
        ("the name it already holds", json!({ "Name": SEED_NAME }), false),
        (
            "the description it already holds",
            json!({ "Description": SEED_DESCRIPTION }),
            false,
        ),
        (
            "the details document it already holds",
            json!({ "BenefitApplicationDetails": seed_details() }),
            false,
        ),
        (
            "the contacts it already holds",
            json!({ "PartnerContacts": seed_contacts() }),
            false,
        ),
        (
            "the files it already holds",
            json!({ "FileDetails": seed_files() }),
            false,
        ),
        (
            "every member it already holds at once",
            json!({
                "Name": SEED_NAME,
                "Description": SEED_DESCRIPTION,
                "BenefitApplicationDetails": seed_details(),
                "PartnerContacts": seed_contacts(),
                "FileDetails": seed_files(),
            }),
            false,
        ),
        ("a new name", json!({ "Name": "renamed" }), true),
        (
            "a new description",
            json!({ "Description": "a reworked description" }),
            true,
        ),
        (
            "one field of the details document changed",
            json!({ "BenefitApplicationDetails": { "CampaignName": "summer", "RequestedAmount": 2500 } }),
            true,
        ),
        (
            "the stored contact under another email",
            json!({ "PartnerContacts": [
                { "Email": "grace@example.com", "FirstName": "Ada", "LastName": "Lovelace" },
            ] }),
            true,
        ),
        (
            "the stored contact with one member fewer",
            json!({ "PartnerContacts": [
                { "Email": "ada@example.com", "FirstName": "Ada" },
            ] }),
            true,
        ),
        (
            "no contact at all",
            json!({ "PartnerContacts": [] }),
            true,
        ),
        (
            "the stored file under another use case",
            json!({ "FileDetails": [{
                "FileURI": "s3://partner-bucket/proposals/plan.pdf",
                "BusinessUseCase": "credit request",
            }] }),
            true,
        ),
        (
            "the name it already holds beside a new description",
            json!({ "Name": SEED_NAME, "Description": "only the description moves" }),
            true,
        ),
    ];

    for (row, (case, change, changes_the_record)) in rows.iter().enumerate() {
        let id = seed(&harness, case, &format!("tok-seed-{row}")).await;
        let before = harness.get(case, &id).await;
        let revision = revision_of(case, &before);

        let body = update_body(
            &id,
            &revision.to_string(),
            &format!("tok-update-{row}"),
            change,
        );
        let out = harness.ok(case, "UpdateBenefitApplication", &body).await;
        let reported = revision_of(case, &out);
        let after = harness.get(case, &id).await;

        assert_eq!(
            out["Id"],
            json!(id),
            "case {case:?}: the update must report the application it answered for, got {out:?}"
        );
        assert_eq!(
            text(case, &after, "Revision"),
            text(case, &out, "Revision"),
            "case {case:?}: the read must report the revision the update answered, got {after:?}"
        );

        if !changes_the_record {
            assert_eq!(
                reported, revision,
                "case {case:?}: a write that changed nothing must report revision \
                 {revision}, got {reported} in {out:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a write that changed nothing must leave the record exactly \
                 as it was, got {after:?}"
            );
            continue;
        }

        assert_eq!(
            reported,
            revision + 1,
            "case {case:?}: a write that changed the record must advance the revision to \
             {}, got {reported} in {out:?}",
            revision + 1
        );
        assert!(
            instant(case, &after, "UpdatedAt") > instant(case, &before, "UpdatedAt"),
            "case {case:?}: a write that changed the record must advance UpdatedAt, got \
             {after:?}"
        );
        for (member, sent) in change.as_object().expect("each row names its members") {
            if member == "FileDetails" {
                // A stored `FileDetail` adds the members derived from the URI, so the two
                // are compared by the caller's own values.
                let caller_values = |files: &Value| -> Vec<Value> {
                    files
                        .as_array()
                        .map(|files| {
                            files
                                .iter()
                                .map(|file| json!([file["FileURI"], file["BusinessUseCase"]]))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                assert_eq!(
                    caller_values(&after["FileDetails"]),
                    caller_values(sent),
                    "case {case:?}: the files the request named must be stored, got {after:?}"
                );
                continue;
            }
            if member == "PartnerContacts" {
                // A list member nothing was stored for is omitted from the read, so the
                // two are compared as the lists they report.
                let entries = |contacts: &Value| -> Vec<Value> {
                    contacts.as_array().cloned().unwrap_or_default()
                };
                assert_eq!(
                    entries(&after["PartnerContacts"]),
                    entries(sent),
                    "case {case:?}: the contacts the request named must be stored, got {after:?}"
                );
                continue;
            }
            assert_eq!(
                &after[member.as_str()], sent,
                "case {case:?}: {member} must be stored, got {after:?}"
            );
        }
    }
}

/// An amendment moves the revision and `UpdatedAt` only when it changed the record: an
/// amendment naming the value the field already holds is accepted and leaves the record
/// as it was, while one naming a new value advances the revision by one step.
///
/// The rows are the three fields a `FieldPath` may name — `Name`, `Description` and a
/// path into `BenefitApplicationDetails`
/// (`crate::ops::amend_benefit_application::AMENDABLE_FIELDS`) — each named twice: once
/// with the stored value, once with a new one.
#[tokio::test]
async fn an_amendment_moves_the_revision_only_when_it_changed_the_record() {
    let harness = Harness::new();

    // (the case, the `FieldPath`, the `NewValue`, whether that changes the record)
    let rows: [(&str, &str, &str, bool); 6] = [
        ("the name it already holds", "Name", SEED_NAME, false),
        (
            "the description it already holds",
            "Description",
            SEED_DESCRIPTION,
            false,
        ),
        (
            "the details field it already holds",
            "BenefitApplicationDetails.CampaignName",
            SEED_CAMPAIGN,
            false,
        ),
        ("a new name", "Name", "amended-name", true),
        (
            "a new description",
            "Description",
            "an amended description",
            true,
        ),
        (
            "a new details field value",
            "BenefitApplicationDetails.CampaignName",
            "autumn",
            true,
        ),
    ];

    for (row, (case, field_path, new_value, changes_the_record)) in rows.iter().enumerate() {
        let id = seed(&harness, case, &format!("tok-amend-seed-{row}")).await;
        let before = harness.get(case, &id).await;
        let revision = revision_of(case, &before);

        harness
            .ok(
                case,
                "AmendBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-amend-{row}"),
                    "Identifier": id,
                    "Revision": revision.to_string(),
                    "AmendmentReason": "the partner reviewed the application",
                    "Amendments": [{ "FieldPath": field_path, "NewValue": new_value }],
                }),
            )
            .await;

        let after = harness.get(case, &id).await;
        if !changes_the_record {
            assert_eq!(
                after, before,
                "case {case:?}: an amendment that changed nothing must leave the record \
                 exactly as it was, got {after:?}"
            );
            continue;
        }

        let amended = revision_of(case, &after);
        assert_eq!(
            amended,
            revision + 1,
            "case {case:?}: an amendment that changed the record must advance the revision \
             to {}, got {amended} in {after:?}",
            revision + 1
        );
        assert!(
            instant(case, &after, "UpdatedAt") > instant(case, &before, "UpdatedAt"),
            "case {case:?}: an amendment that changed the record must advance UpdatedAt, \
             got {after:?}"
        );
    }
}

/// A write that changed nothing leaves the idempotency window a preceding
/// `RecallBenefitApplication` opened OPEN — replaying that recall is still recognised as
/// a retry — while a write that DID change the record closes it, because the record the
/// recall left has moved on.
///
/// The rows are an update naming no member, one naming only the stored name, and one
/// naming a new name.
#[tokio::test]
async fn a_write_that_changes_nothing_keeps_the_recall_window_open() {
    let harness = Harness::new();

    // (the case, the members the update names, whether that changes the stored record)
    let rows: [(&str, Value, bool); 3] = [
        ("an update naming no member", json!({}), false),
        (
            "an update naming the name it already holds",
            json!({ "Name": SEED_NAME }),
            false,
        ),
        (
            "an update naming a new name",
            json!({ "Name": "reworked" }),
            true,
        ),
    ];

    for (row, (case, change, changes_the_record)) in rows.iter().enumerate() {
        let id = seed(&harness, case, &format!("tok-recall-seed-{row}")).await;
        harness
            .ok(
                case,
                "SubmitBenefitApplication",
                &json!({ "Catalog": CATALOG, "Identifier": id }),
            )
            .await;

        let recall = json!({
            "Catalog": CATALOG,
            "ClientToken": format!("tok-recall-{row}"),
            "Identifier": id,
            "Reason": "the partner is reworking the figures",
        });
        harness.ok(case, "RecallBenefitApplication", &recall).await;
        let recalled = harness.get(case, &id).await;

        let body = update_body(
            &id,
            &text(case, &recalled, "Revision"),
            &format!("tok-recall-update-{row}"),
            change,
        );
        harness.ok(case, "UpdateBenefitApplication", &body).await;
        let updated = harness.get(case, &id).await;

        let (status, out) = harness.call("RecallBenefitApplication", &recall).await;
        if *changes_the_record {
            assert_eq!(
                status, 409,
                "case {case:?}: once the record moved on the replay is a new recall of a \
                 draft, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {out:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: a write that changed nothing must leave the recall's \
             idempotency window open, got {status} {out:?}"
        );
        let replayed = harness.get(case, &id).await;
        assert_eq!(
            replayed, updated,
            "case {case:?}: the replayed recall must change nothing, got {replayed:?}"
        );
    }
}

/// A `ClientToken` spent on a write that changed nothing is still SPENT: retrying that
/// same request answers what it answered, and the same token on a request naming
/// different members is a different request and is refused.
#[tokio::test]
async fn a_token_spent_on_a_write_that_changed_nothing_is_still_spent() {
    let harness = Harness::new();
    let case = "a no-op update replayed under its own token";
    let id = seed(&harness, case, "tok-replay-seed").await;
    let before = harness.get(case, &id).await;
    let revision = text(case, &before, "Revision");

    let body = update_body(&id, &revision, "tok-replay", &json!({}));
    let first = harness.ok(case, "UpdateBenefitApplication", &body).await;
    assert_eq!(
        text(case, &first, "Revision"),
        revision,
        "case {case:?}: a write that changed nothing must report revision {revision:?}, \
         got {first:?}"
    );

    let replay = harness.ok(case, "UpdateBenefitApplication", &body).await;
    assert_eq!(
        replay, first,
        "case {case:?}: the retry must answer the first response {first:?}, got {replay:?}"
    );
    assert_eq!(
        harness.get(case, &id).await,
        before,
        "case {case:?}: neither call may change the record"
    );

    let case = "another request under the same token";
    let different = update_body(&id, &revision, "tok-replay", &json!({ "Name": "renamed" }));
    let (status, out) = harness.call("UpdateBenefitApplication", &different).await;
    assert_eq!(
        status, 409,
        "case {case:?}: a token spent on another request must be refused, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {out:?}"
    );
    assert_eq!(
        harness.get(case, &id).await,
        before,
        "case {case:?}: a refused update must change nothing"
    );
}
