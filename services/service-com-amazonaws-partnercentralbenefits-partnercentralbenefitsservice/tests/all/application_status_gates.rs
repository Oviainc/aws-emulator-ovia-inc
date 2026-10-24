// SPDX-License-Identifier: CC0-1.0

//! The status gate on changing a stored benefit application, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication`, so what these tests pin is the emulator's HTTP behaviour on
//! the persisted record.
//!
//! The rule under test: a request that changes a stored benefit application is accepted
//! only from a `BenefitApplicationStatus` that allows that change, and from any other one
//! it is refused with the modelled `ConflictException` (HTTP 409) with nothing persisted.
//! `UpdateBenefitApplication` rewrites the partner's own DRAFT, so it needs
//! `PENDING_SUBMISSION`; a terminal status (`APPROVED`, `REJECTED`, `CANCELED`) is one AWS
//! has finished with, so it admits no change at all — not an `AmendBenefitApplication`
//! and not a `CancelBenefitApplication`.
//!
//! The tests are tables whose rows differ along what the rule generalises over: which of
//! the six modelled statuses the application is in, which catalog it lives in, which
//! members the change names, and whether the request carries the current revision or a
//! stale one.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::BENEFIT_APPLICATION_STATUS_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    SEEDED_CATALOGS, STATUS_ACTION_REQUIRED, STATUS_APPROVED, STATUS_CANCELED, STATUS_IN_REVIEW,
    STATUS_PENDING_SUBMISSION, STATUS_REJECTED, TERMINAL_STATUSES, UPDATABLE_STATUSES,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};

/// Create one draft application in `catalog` and return its id.
async fn seed(harness: &Harness, case: &str, catalog: &str, token: &str) -> String {
    let benefit_id = text(case, &harness.active_benefit_of(catalog).await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": catalog,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": "status-gate-subject",
                "Description": "the application the status gate is asserted on",
                "BenefitApplicationDetails": { "CampaignName": "seeded" },
            }),
        )
        .await;
    text(case, &created, "Id")
}

/// Put one application into `status`, and assert the read reports it.
///
/// The transition is made through the operation that produces it wherever this emulator
/// has one — `SubmitBenefitApplication` for `IN_REVIEW`, `CancelBenefitApplication` for
/// `CANCELED` — so the record reaches the status the way a client would drive it.
/// `ACTION_REQUIRED`, `APPROVED` and `REJECTED` are set by AWS's own review workflow and
/// no modelled operation reaches them, so those rows are placed in the store
/// ([`Harness::place_status`]) after a real submission.
async fn reach(harness: &Harness, case: &str, catalog: &str, id: &str, status: &str) {
    let submit = json!({ "Catalog": catalog, "Identifier": id });
    match status {
        STATUS_PENDING_SUBMISSION => {}
        STATUS_IN_REVIEW => {
            harness.ok(case, "SubmitBenefitApplication", &submit).await;
        }
        STATUS_CANCELED => {
            harness
                .ok(
                    case,
                    "CancelBenefitApplication",
                    &json!({
                        "Catalog": catalog,
                        "ClientToken": format!("tok-cancel-{id}"),
                        "Identifier": id,
                        "Reason": "withdrawn by the partner",
                    }),
                )
                .await;
        }
        other => {
            harness.ok(case, "SubmitBenefitApplication", &submit).await;
            harness.place_status(case, catalog, id, other);
        }
    }
    let placed = harness.get_in(case, catalog, id).await;
    assert_eq!(
        placed["Status"],
        json!(status),
        "case {case:?}: expected the application to be {status}, got {placed:?}"
    );
}

/// One update request body carrying `change` plus the members every update needs.
fn update_body(catalog: &str, id: &str, revision: &str, token: &str, change: &Value) -> Value {
    let mut body = change.clone();
    body["Catalog"] = json!(catalog);
    body["ClientToken"] = json!(token);
    body["Identifier"] = json!(id);
    body["Revision"] = json!(revision);
    body
}

/// The `FileURI` of each stored `FileDetail`, which is what a `FileInput` carries.
fn file_uris(body: &Value) -> Vec<Value> {
    body["FileDetails"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|file| file["FileURI"].clone())
        .collect()
}

/// `UpdateBenefitApplication` rewrites a `PENDING_SUBMISSION` application and is refused
/// with a 409 `ConflictException` from every other status, persisting nothing.
///
/// The rows are every modelled `BenefitApplicationStatus`
/// (`BENEFIT_APPLICATION_STATUS_VALUES`, so the set cannot drift from the model) crossed
/// with every seeded catalog (`SEEDED_CATALOGS`), each naming a different subset of the
/// updatable members; every refused row is retried with a stale `Revision` too, so the
/// refusal cannot be coming from the optimistic-concurrency check.
#[tokio::test]
async fn update_is_accepted_only_while_the_application_is_a_draft() {
    let harness = Harness::new();

    // The members `UpdateBenefitApplication` can change, one row each plus one row
    // changing several at once (the operation's input: `Name`, `Description`,
    // `BenefitApplicationDetails`, `PartnerContacts`, `FileDetails`).
    let changes: [(&str, Value); 6] = [
        ("the name", json!({ "Name": "edited-name" })),
        (
            "the description",
            json!({ "Description": "an edited description" }),
        ),
        (
            "the details document",
            json!({ "BenefitApplicationDetails": { "CampaignName": "edited", "RequestedAmount": 7 } }),
        ),
        (
            "the partner contacts",
            json!({ "PartnerContacts": [{ "Email": "edited@example.com", "FirstName": "Ada" }] }),
        ),
        (
            "the file details",
            json!({ "FileDetails": [{ "FileURI": "s3://bucket/edited.csv" }] }),
        ),
        (
            "every updatable member at once",
            json!({
                "Name": "edited-everything",
                "Description": "edited everything",
                "BenefitApplicationDetails": { "CampaignName": "all" },
                "PartnerContacts": [{ "Email": "all@example.com" }],
                "FileDetails": [{ "FileURI": "s3://bucket/all.pdf" }],
            }),
        ),
    ];

    let mut row = 0usize;
    for catalog in SEEDED_CATALOGS {
        for status in BENEFIT_APPLICATION_STATUS_VALUES {
            let (change_case, change) = &changes[row % changes.len()];
            let case = format!("{status} in {catalog}, changing {change_case}");
            let id = seed(&harness, &case, catalog, &format!("tok-gate-{row}")).await;
            row += 1;

            reach(&harness, &case, catalog, &id, status).await;
            let before = harness.get_in(&case, catalog, &id).await;
            let revision = text(&case, &before, "Revision");
            let body = update_body(
                catalog,
                &id,
                &revision,
                &format!("tok-update-{row}"),
                change,
            );

            if UPDATABLE_STATUSES.contains(status) {
                let updated = harness.ok(&case, "UpdateBenefitApplication", &body).await;
                let new_revision = text(&case, &updated, "Revision");
                assert_ne!(
                    new_revision, revision,
                    "case {case:?}: an accepted update must bump the revision, got {updated:?}"
                );

                // Round-trip: the change is persisted and reported by the reader.
                let after = harness.get_in(&case, catalog, &id).await;
                assert_eq!(
                    text(&case, &after, "Revision"),
                    new_revision,
                    "case {case:?}: the read must report the new revision, got {after:?}"
                );
                for (member, sent) in change.as_object().expect("each row is an object") {
                    if member == "FileDetails" {
                        // A stored `FileDetail` adds the members derived from the URI, so
                        // the two are compared by the URI they share.
                        let wanted: Vec<Value> = sent
                            .as_array()
                            .unwrap_or(&Vec::new())
                            .iter()
                            .map(|file| file["FileURI"].clone())
                            .collect();
                        assert_eq!(
                            file_uris(&after),
                            wanted,
                            "case {case:?}: FileDetails must be persisted, got {after:?}"
                        );
                        continue;
                    }
                    assert_eq!(
                        &after[member.as_str()], sent,
                        "case {case:?}: {member} must be persisted, got {after:?}"
                    );
                }
                assert_eq!(
                    after["Status"],
                    json!(status),
                    "case {case:?}: an update must not move the status, got {after:?}"
                );
                continue;
            }

            // Not a draft: the same request is refused, and the whole record — content,
            // status, revision and UpdatedAt — is left exactly as it was. The stale row
            // shows the refusal is the status's, not the revision check's.
            for (attempt, (revision_case, named)) in [
                ("the current revision", revision.clone()),
                ("a stale revision", "1".to_string()),
                ("a revision that is not a number", "not-a-revision".to_string()),
            ]
            .into_iter()
            .enumerate()
            {
                let case = format!("{case}, with {revision_case}");
                let body = update_body(
                    catalog,
                    &id,
                    &named,
                    &format!("tok-update-refused-{row}-{attempt}"),
                    change,
                );
                let (http, out) = harness.call("UpdateBenefitApplication", &body).await;
                assert_eq!(
                    http, 409,
                    "case {case:?}: expected 409 updating a {status} application, got {http} {out:?}"
                );
                assert_eq!(
                    error_code(&out),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {out:?}"
                );
                let after = harness.get_in(&case, catalog, &id).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused update must persist nothing, got {after:?}"
                );
            }
        }
    }
}

/// The two status sets the gates are defined over are exactly the modelled statuses they
/// claim to be, over every value of `BenefitApplicationStatus`.
///
/// The expected sets are written out here rather than read from the service, so the
/// membership itself is under test: `UPDATABLE_STATUSES` is the one status a partner's
/// draft carries before it is submitted, and `TERMINAL_STATUSES` is every status AWS has
/// finished with — from `smithy_query(query="shape",shape="BenefitApplicationStatus")`:
/// `PENDING_SUBMISSION | IN_REVIEW | ACTION_REQUIRED | APPROVED | REJECTED | CANCELED`.
#[tokio::test]
async fn the_gated_status_sets_are_the_modelled_ones() {
    let updatable: [&str; 1] = [STATUS_PENDING_SUBMISSION];
    let terminal: [&str; 3] = [STATUS_APPROVED, STATUS_REJECTED, STATUS_CANCELED];

    for status in BENEFIT_APPLICATION_STATUS_VALUES {
        assert_eq!(
            UPDATABLE_STATUSES.contains(status),
            updatable.contains(status),
            "case {status:?}: expected updatable == {}, got {}",
            updatable.contains(status),
            UPDATABLE_STATUSES.contains(status)
        );
        assert_eq!(
            TERMINAL_STATUSES.contains(status),
            terminal.contains(status),
            "case {status:?}: expected terminal == {}, got {}",
            terminal.contains(status),
            TERMINAL_STATUSES.contains(status)
        );
    }

    // Neither set may name a status the model does not declare.
    for status in UPDATABLE_STATUSES.iter().chain(TERMINAL_STATUSES) {
        assert!(
            BENEFIT_APPLICATION_STATUS_VALUES.contains(status),
            "case {status:?}: not a modelled BenefitApplicationStatus"
        );
    }

    // The six statuses the model declares are also the statuses the rule is stated over,
    // and `ACTION_REQUIRED` and `IN_REVIEW` are in neither set: a submitted application
    // AWS has not finished with is not updatable, but it is still amendable.
    for status in [STATUS_IN_REVIEW, STATUS_ACTION_REQUIRED] {
        assert!(
            !UPDATABLE_STATUSES.contains(&status) && !TERMINAL_STATUSES.contains(&status),
            "case {status:?}: a submitted, non-terminal status is neither updatable nor terminal"
        );
    }
}

/// An amendment and a cancel are accepted while AWS has not finished with the
/// application, and refused with a 409 `ConflictException` once it is terminal — with
/// nothing persisted.
///
/// The rows are every modelled `BenefitApplicationStatus`: the amendment names a field of
/// the application, and the cancel follows it on the same record, so each row pins both
/// operations against the status it was placed in.
#[tokio::test]
async fn an_amendment_and_a_cancel_are_refused_once_the_application_is_terminal() {
    let harness = Harness::new();

    // The amendable fields, one per row, so the refusal is not specific to one path
    // (`FieldPath`: `Name`, `Description`, or a path into `BenefitApplicationDetails`).
    let paths: [&str; 3] = [
        "Name",
        "Description",
        "BenefitApplicationDetails.CampaignName",
    ];

    for (row, status) in BENEFIT_APPLICATION_STATUS_VALUES.iter().enumerate() {
        let case = format!("{status}, amending {}", paths[row % paths.len()]);
        let field_path = paths[row % paths.len()];
        let id = seed(&harness, &case, CATALOG, &format!("tok-amend-gate-{row}")).await;
        reach(&harness, &case, CATALOG, &id, status).await;

        let before = harness.get_in(&case, CATALOG, &id).await;
        let amend = json!({
            "Catalog": CATALOG,
            "ClientToken": format!("tok-amend-{row}"),
            "Identifier": id,
            "Revision": text(&case, &before, "Revision"),
            "AmendmentReason": "the partner corrected the application",
            "Amendments": [{ "FieldPath": field_path, "NewValue": "amended-value" }],
        });

        if TERMINAL_STATUSES.contains(status) {
            let (http, out) = harness.call("AmendBenefitApplication", &amend).await;
            assert_eq!(
                http, 409,
                "case {case:?}: expected 409 amending a {status} application, got {http} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {out:?}"
            );
            let after = harness.get_in(&case, CATALOG, &id).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused amendment must persist nothing, got {after:?}"
            );

            // A cancel of a terminal application is refused the same way, and the record
            // keeps the status it already had.
            let (http, out) = harness
                .call(
                    "CancelBenefitApplication",
                    &json!({
                        "Catalog": CATALOG,
                        "ClientToken": format!("tok-cancel-terminal-{row}"),
                        "Identifier": id,
                        "Reason": "cancel it anyway",
                    }),
                )
                .await;
            assert_eq!(
                http, 409,
                "case {case:?}: expected 409 cancelling a {status} application, got {http} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {out:?}"
            );
            let after = harness.get_in(&case, CATALOG, &id).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused cancel must persist nothing, got {after:?}"
            );
            continue;
        }

        // Not terminal: the amendment is what this operation is for, so it applies and
        // the reader reports it, without moving the status.
        harness.ok(&case, "AmendBenefitApplication", &amend).await;
        let after = harness.get_in(&case, CATALOG, &id).await;
        let amended = match field_path {
            "Name" => after["Name"].clone(),
            "Description" => after["Description"].clone(),
            _ => after["BenefitApplicationDetails"]["CampaignName"].clone(),
        };
        assert_eq!(
            amended,
            json!("amended-value"),
            "case {case:?}: the amendment must be persisted, got {after:?}"
        );
        assert_ne!(
            text(&case, &after, "Revision"),
            text(&case, &before, "Revision"),
            "case {case:?}: an accepted amendment must bump the revision, got {after:?}"
        );
        assert_eq!(
            after["Status"],
            json!(status),
            "case {case:?}: an amendment must not move the status, got {after:?}"
        );

        // And a cancel from a non-terminal status still ends the application.
        harness
            .ok(
                &case,
                "CancelBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-cancel-live-{row}"),
                    "Identifier": id,
                    "Reason": "withdrawn",
                }),
            )
            .await;
        let cancelled = harness.get_in(&case, CATALOG, &id).await;
        assert_eq!(
            cancelled["Status"],
            json!(STATUS_CANCELED),
            "case {case:?}: a non-terminal application must still be cancellable, got {cancelled:?}"
        );
    }
}
