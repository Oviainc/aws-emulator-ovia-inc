// SPDX-License-Identifier: CC0-1.0

//! Who uploaded a stored attachment, and when, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication`, so what these tests pin is the emulator's HTTP behaviour on
//! the persisted record.
//!
//! The rule under test: a stored `FileDetail` reports `CreatedBy` as "the identifier of
//! the user who uploaded the file" and `CreatedAt` as "the timestamp when the file was
//! uploaded" (`smithy_query(query="shape",shape="FileDetail")`), so those two members
//! belong to the FILE — not to the request that last wrote the attachment list, nor to the
//! position the file holds in it. A write of the list therefore carries the principal and
//! instant recorded for every file the application already carries forward unchanged, and
//! records the calling principal and the current instant only for a file the application
//! does not already carry.
//!
//! The tables' rows differ along what the rule generalises over: whether the write adds,
//! removes, reorders, replaces or re-describes attachments, how MANY files the application
//! carries before and after, where in the list the new file lands, and how many writes by
//! how many different callers run over the same application.
//!
//! The last test pins the bound that keeps the merge finite, so neither list it walks is
//! caller-unbounded: `FileDetails` carries the modelled `@length(max: 30)` on both write
//! paths (`smithy_query(query="operation",operation="UpdateBenefitApplication")` and
//! `operation="CreateBenefitApplication"`), refused before a handler runs. Its rows are an
//! empty list, one entry, the published maximum and one past it.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::
    VALIDATION_REASON_FIELD;

use crate::benefit_application_lifecycle::{
    error_code, instant, text, Harness, ACCESS_KEY_ID, CATALOG,
};
use crate::service_error_rules::{assert_validation_invariants, clause, message_of};

/// The access key id the UPDATES below are signed with: a caller other than the one that
/// created the application, so a `CreatedBy` the update rewrote is observably different
/// from the one the create recorded.
const UPDATER_KEY: &str = "AKIDUPDATER";

/// How long a test waits between two writes, so the later write's `now` is a different
/// instant: a stored upload time is kept in milliseconds, so a `CreatedAt` the update
/// rewrote is observably later than the one the create recorded.
const BETWEEN_WRITES: Duration = Duration::from_millis(5);

/// One attachment as a request names it: the `FileURI`, and the `BusinessUseCase` beside
/// it where the case supplies one (the member is optional).
type SuppliedFile = (&'static str, Option<&'static str>);

/// The attachment `s3://.../plan.pdf`, with the use case the seed gives it.
const PLAN: SuppliedFile = (
    "s3://partner-bucket/proposals/plan.pdf",
    Some("funding request"),
);

/// A second attachment, carrying no `BusinessUseCase` at all.
const EXTRA: SuppliedFile = ("s3://partner-bucket/proposals/extra.pdf", None);

/// A third attachment, of another file type.
const NOTES: SuppliedFile = (
    "s3://partner-bucket/proposals/notes.docx",
    Some("background"),
);

/// A file no seed below attaches, so a request naming it names a file the application does
/// not already carry.
const FRESH: SuppliedFile = (
    "s3://partner-bucket/proposals/budget.xlsx",
    Some("budget"),
);

/// One row of the first table: the case, the files the create attaches, the files the
/// update names, and for each entry the update stores whether that file was already
/// attached.
type Row = (
    &'static str,
    Vec<SuppliedFile>,
    Vec<SuppliedFile>,
    Vec<bool>,
);

/// One write of the chain in the second test: the caller signing it, and the files it
/// names.
type Write = (&'static str, Vec<SuppliedFile>);

/// The `FileDetails` list a request carries for `files`, as the modelled `FileInput`
/// members: `FileURI`, and `BusinessUseCase` only where the row names one.
fn file_inputs(files: &[SuppliedFile]) -> Value {
    Value::Array(
        files
            .iter()
            .map(|(uri, use_case)| match use_case {
                Some(use_case) => json!({ "FileURI": uri, "BusinessUseCase": use_case }),
                None => json!({ "FileURI": uri }),
            })
            .collect(),
    )
}

/// The `FileDetails` one read reports, in the order the service reported them.
fn stored_files(case: &str, application: &Value) -> Vec<Value> {
    match &application["FileDetails"] {
        Value::Array(files) => files.clone(),
        Value::Null => Vec::new(),
        other => panic!("case {case:?}: expected a FileDetails list, got {other:?}"),
    }
}

/// Call one operation signed by `access_key_id`, asserting it answered 200.
async fn ok_as(harness: &Harness, case: &str, access_key_id: &str, operation: &str, body: &Value) -> Value {
    let (status, out) = harness.call_as(access_key_id, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {out:?}"
    );
    out
}

/// Create one draft application carrying `files`, signed by `access_key_id`, and return
/// its id.
async fn seed(
    harness: &Harness,
    case: &str,
    access_key_id: &str,
    token: &str,
    files: &[SuppliedFile],
) -> String {
    let benefit_id = text(case, &harness.active_benefit().await, "Id");
    let created = ok_as(
        harness,
        case,
        access_key_id,
        "CreateBenefitApplication",
        &json!({
            "Catalog": CATALOG,
            "ClientToken": token,
            "BenefitIdentifier": benefit_id,
            "Name": "upload-facts-subject",
            "FileDetails": file_inputs(files),
        }),
    )
    .await;
    text(case, &created, "Id")
}

/// The principal this service reports for the caller signing as `access_key_id`, observed
/// through a file that caller attached itself rather than restated here.
async fn principal_of(harness: &Harness, access_key_id: &str, token: &str) -> String {
    let case = format!("the principal of {access_key_id}");
    let id = seed(harness, &case, access_key_id, token, &[PLAN]).await;
    let application = ok_as(
        harness,
        &case,
        access_key_id,
        "GetBenefitApplication",
        &json!({ "Catalog": CATALOG, "Identifier": id }),
    )
    .await;
    let files = stored_files(&case, &application);
    text(&case, &files[0], "CreatedBy")
}

/// An update that changes the attachment list writes the list it names and keeps the
/// recorded uploader and upload time of every file the application already carries,
/// recording the calling principal and the current instant only for a file it did not
/// carry before.
///
/// Each row runs on its own freshly created application, created by one caller and updated
/// by another, so a rewritten `CreatedBy` is visible as well as a rewritten `CreatedAt`.
#[tokio::test]
async fn an_update_keeps_the_upload_facts_of_the_files_already_attached() {
    let harness = Harness::new();
    let creator = principal_of(&harness, ACCESS_KEY_ID, "tok-principal-creator").await;
    let updater = principal_of(&harness, UPDATER_KEY, "tok-principal-updater").await;
    assert_ne!(
        creator, updater,
        "the two callers must report different principals, or the rule is untestable"
    );

    // (the case, the files the create attaches, the files the update names, and for each
    // entry the update must store whether it was already attached)
    let rows: Vec<Row> = vec![
        (
            "a second file added",
            vec![PLAN],
            vec![PLAN, EXTRA],
            vec![true, false],
        ),
        (
            "a file inserted before the two already attached",
            vec![PLAN, EXTRA],
            vec![FRESH, PLAN, EXTRA],
            vec![false, true, true],
        ),
        (
            "one of two files removed",
            vec![PLAN, EXTRA],
            vec![EXTRA],
            vec![true],
        ),
        (
            "every file removed",
            vec![PLAN],
            vec![],
            vec![],
        ),
        (
            "the same two files in the opposite order",
            vec![PLAN, EXTRA],
            vec![EXTRA, PLAN],
            vec![true, true],
        ),
        (
            "three files reordered with one added",
            vec![PLAN, EXTRA, NOTES],
            vec![NOTES, FRESH, PLAN, EXTRA],
            vec![true, false, true, true],
        ),
        (
            "the file already attached under another use case",
            vec![PLAN],
            vec![(PLAN.0, Some("credit request"))],
            vec![true],
        ),
        (
            "the file already attached with its use case dropped",
            vec![PLAN],
            vec![(PLAN.0, None)],
            vec![true],
        ),
        (
            "the attached file replaced by another",
            vec![PLAN],
            vec![FRESH],
            vec![false],
        ),
        (
            "the attached file named twice",
            vec![PLAN],
            vec![PLAN, PLAN],
            vec![true, false],
        ),
    ];

    for (row, (case, seeded, supplied, already_attached)) in rows.iter().enumerate() {
        let id = seed(
            &harness,
            case,
            ACCESS_KEY_ID,
            &format!("tok-seed-{row}"),
            seeded,
        )
        .await;
        let before = harness.get(case, &id).await;
        let attached = stored_files(case, &before);
        let uploaded_at: Vec<i64> = attached
            .iter()
            .map(|file| instant(case, file, "CreatedAt"))
            .collect();
        let last_upload = uploaded_at.iter().copied().max().unwrap_or_default();

        sleep(BETWEEN_WRITES);
        ok_as(
            &harness,
            case,
            UPDATER_KEY,
            "UpdateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": format!("tok-update-{row}"),
                "Identifier": id,
                "Revision": text(case, &before, "Revision"),
                "FileDetails": file_inputs(supplied),
            }),
        )
        .await;

        let after = harness.get(case, &id).await;
        let stored = stored_files(case, &after);
        assert_eq!(
            stored.len(),
            supplied.len(),
            "case {case:?}: expected the {} files the update named, got {stored:?}",
            supplied.len()
        );

        for (index, ((uri, use_case), carried)) in supplied.iter().zip(already_attached).enumerate() {
            let file = &stored[index];
            assert_eq!(
                file["FileURI"],
                json!(uri),
                "case {case:?}: expected {uri:?} at position {index}, got {file:?}"
            );
            match use_case {
                Some(use_case) => assert_eq!(
                    file["BusinessUseCase"],
                    json!(use_case),
                    "case {case:?}: expected the use case {use_case:?} at position {index}, \
                     got {file:?}"
                ),
                None => assert!(
                    file.get("BusinessUseCase").is_none_or(Value::is_null),
                    "case {case:?}: expected no use case at position {index}, got {file:?}"
                ),
            }

            let created_by = text(case, file, "CreatedBy");
            let created_at = instant(case, file, "CreatedAt");
            if *carried {
                // The file was already attached, so its upload facts are the ones the
                // create recorded — the same principal, and the very instant the read
                // before the update reported for that file.
                let was = attached
                    .iter()
                    .position(|stored| stored["FileURI"] == json!(uri))
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: {uri:?} must have been attached, got {attached:?}")
                    });
                assert_eq!(
                    created_by, creator,
                    "case {case:?}: position {index} must keep the uploading principal \
                     {creator:?}, got {created_by:?} in {file:?}"
                );
                assert_eq!(
                    created_at, uploaded_at[was],
                    "case {case:?}: position {index} must keep the upload instant {}, got \
                     {created_at} in {file:?}",
                    uploaded_at[was]
                );
            } else {
                // The file was not attached before, so this update uploaded it: the
                // calling principal, at an instant later than every earlier upload.
                assert_eq!(
                    created_by, updater,
                    "case {case:?}: position {index} must record the uploading principal \
                     {updater:?}, got {created_by:?} in {file:?}"
                );
                assert!(
                    created_at > last_upload,
                    "case {case:?}: position {index} must record an upload instant later \
                     than {last_upload}, got {created_at} in {file:?}"
                );
            }
        }
    }
}

/// A chain of updates, each by a different caller, never rewrites the upload facts of a
/// file that stays attached: the entry for the file the create attached reports the same
/// `CreatedBy` and `CreatedAt` after every one of them, while the list changes around it
/// and the revision advances with each write.
#[tokio::test]
async fn repeated_updates_leave_an_attached_files_upload_facts_alone() {
    let harness = Harness::new();
    let case = "a file carried through a chain of updates";
    let id = seed(&harness, case, ACCESS_KEY_ID, "tok-chain-seed", &[PLAN]).await;
    let first = harness.get(case, &id).await;
    let attached = stored_files(case, &first);
    let uploaded_by = text(case, &attached[0], "CreatedBy");
    let uploaded_at = instant(case, &attached[0], "CreatedAt");

    // (the caller signing the update, the files it names)
    let writes: [Write; 4] = [
        ("AKIDWRITERONE", vec![PLAN, EXTRA]),
        ("AKIDWRITERTWO", vec![EXTRA, PLAN, NOTES]),
        ("AKIDWRITERTHREE", vec![NOTES, PLAN]),
        ("AKIDWRITERFOUR", vec![(PLAN.0, Some("renewed request"))]),
    ];

    for (write, (caller, files)) in writes.iter().enumerate() {
        let before = harness.get(case, &id).await;
        let revision = text(case, &before, "Revision");
        sleep(BETWEEN_WRITES);
        ok_as(
            &harness,
            case,
            caller,
            "UpdateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": format!("tok-chain-{write}"),
                "Identifier": id,
                "Revision": revision,
                "FileDetails": file_inputs(files),
            }),
        )
        .await;

        let after = harness.get(case, &id).await;
        assert_ne!(
            text(case, &after, "Revision"),
            revision,
            "case {case:?}: write {write} changed the list, so the revision must advance, \
             got {after:?}"
        );
        let stored = stored_files(case, &after);
        let plan = stored
            .iter()
            .find(|file| file["FileURI"] == json!(PLAN.0))
            .unwrap_or_else(|| {
                panic!("case {case:?}: write {write} must keep {:?}, got {stored:?}", PLAN.0)
            });
        assert_eq!(
            text(case, plan, "CreatedBy"),
            uploaded_by,
            "case {case:?}: write {write} must keep the uploading principal \
             {uploaded_by:?}, got {plan:?}"
        );
        assert_eq!(
            instant(case, plan, "CreatedAt"),
            uploaded_at,
            "case {case:?}: write {write} must keep the upload instant {uploaded_at}, got \
             {plan:?}"
        );
    }
}

/// `count` attachments, each naming a `FileURI` of its own, as a request carries them.
fn numbered_files(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "FileURI": format!("s3://partner-bucket/proposals/file-{index}.pdf"),
                    "BusinessUseCase": "evidence",
                })
            })
            .collect(),
    )
}

/// The most attachments a request may name, which is the modelled `FileDetails`
/// `@length(max: 30)` carried by both `CreateBenefitApplication` and
/// `UpdateBenefitApplication`
/// (`smithy_query(query="operation",operation="UpdateBenefitApplication")`).
const MAX_SUPPLIED_FILES: usize = 30;

/// How many attachments one application may hold is the attachment list's own modelled
/// maximum: a list up to [`MAX_SUPPLIED_FILES`] entries is stored and reported back, and
/// one past it is refused with the modelled `ValidationException` (HTTP 400) naming the
/// list member, leaving the record exactly as it was. Both paths that supply the list —
/// the create and the update — answer the one bound, so neither the supplied list nor the
/// stored one the merge walks can be longer than it.
#[tokio::test]
async fn the_attachment_list_carries_its_modelled_element_bound() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    let too_many = clause(
        "fileDetails",
        &format!("Member must have length less than or equal to {MAX_SUPPLIED_FILES}"),
    );

    // (the case, how many attachments the request names, whether the list is inside the
    // bound) — an empty list, one entry, the published maximum and one past it.
    let rows: [(&str, usize, bool); 4] = [
        ("no attachment at all", 0, true),
        ("one attachment", 1, true),
        ("as many as the bound admits", MAX_SUPPLIED_FILES, true),
        (
            "one attachment past the bound",
            MAX_SUPPLIED_FILES + 1,
            false,
        ),
    ];

    for (row, (case, count, inside)) in rows.iter().enumerate() {
        let files = numbered_files(*count);

        // 1. the create, which supplies the whole list
        let (status, out) = harness
            .call(
                "CreateBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-count-create-{row}"),
                    "BenefitIdentifier": benefit_id,
                    "Name": "attachment-count-subject",
                    "FileDetails": files,
                }),
            )
            .await;
        if !*inside {
            assert_eq!(
                status, 400,
                "case {case:?}: a list past the bound must be refused, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {out:?}"
            );
            let message = message_of(&out)
                .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {out:?}"));
            assert_validation_invariants(case, &message);
            assert!(
                message.contains(&too_many),
                "case {case:?}: expected the clause {too_many:?}, got {message:?}"
            );
            assert_eq!(
                out["Reason"],
                json!(VALIDATION_REASON_FIELD),
                "case {case:?}: expected the field-validation reason, got {out:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a list inside the bound must be created, got {status} {out:?}"
            );
            let stored = harness.get(case, &text(case, &out, "Id")).await;
            assert_eq!(
                stored_files(case, &stored).len(),
                *count,
                "case {case:?}: expected the {count} attachments the create named, got {stored:?}"
            );
        }

        // 2. the update, which supplies the same list for a record that already holds one
        let id = seed(
            &harness,
            case,
            ACCESS_KEY_ID,
            &format!("tok-count-seed-{row}"),
            &[PLAN],
        )
        .await;
        let before = harness.get(case, &id).await;
        let (status, out) = harness
            .call(
                "UpdateBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-count-update-{row}"),
                    "Identifier": id,
                    "Revision": text(case, &before, "Revision"),
                    "FileDetails": numbered_files(*count),
                }),
            )
            .await;
        let after = harness.get(case, &id).await;
        if !*inside {
            assert_eq!(
                status, 400,
                "case {case:?}: a list past the bound must be refused by the update too, \
                 got {status} {out:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must change nothing, got {after:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a list inside the bound must be updated, got {status} {out:?}"
            );
            assert_eq!(
                stored_files(case, &after).len(),
                *count,
                "case {case:?}: expected the {count} attachments the update named, got {after:?}"
            );
        }
    }
}
