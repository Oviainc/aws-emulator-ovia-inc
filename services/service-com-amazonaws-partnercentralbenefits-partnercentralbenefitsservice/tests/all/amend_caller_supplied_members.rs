// SPDX-License-Identifier: CC0-1.0

//! Which members `AmendBenefitApplication` may write, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication`.
//!
//! The rule under test: an amendment may write every member of the application a CALLER
//! supplies and `UpdateBenefitApplication` also writes — `Name`, `Description`,
//! `BenefitApplicationDetails`, `PartnerContacts` and `FileDetails` — and only those; a
//! path naming a member the service owns is refused with the modelled
//! `ValidationException`. The two list members hold structures, so the path below one
//! addresses a single element (its 0-based position) and the member of that element to
//! write, and the `NewValue` must satisfy the constraints the model puts on THAT member.
//!
//! The tests are tables. Their rows differ along what the rule generalises over: which
//! member a `FieldPath` names (every member of a `Contact` and both members of a
//! `FileInput`), how the element position is written (a bracket subscript, a `.`-separated
//! segment, JSON-path form), how many amendments one request carries, and each published
//! length bound of the member the path names plus one past it — including this service's
//! own bound where the model constrains the member not at all.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    MAX_DETAILS_VALUE_LEN, STATUS_IN_REVIEW,
};

use crate::amend_benefit_application::{amend_body, amendment, clause, message_of};
use crate::application_status_gates::reach;
use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};

/// The modelled `@pattern` of `ContactEmail`, without the `^`/`$` anchors this service
/// states a pattern requirement without — from
/// `smithy_query(query="shape",shape="Contact")`, member `Email`.
const EMAIL_PATTERN: &str = "[a-z0-9!#$%&'*+/=?^_`{|}~-]+(?:\\.[a-z0-9!#$%&'*+/=?^_`{|}~-]+)*@(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\\.)+[a-z0-9](?:[a-z0-9-]*[a-z0-9])?";

/// The modelled `@pattern` of `FileURI`, likewise unanchored — from
/// `smithy_query(query="shape",shape="FileInput")`, member `FileURI`.
const FILE_URI_PATTERN: &str = "(s3://|https://).*";

/// The modelled `@length` of every `Contact` string member
/// (`ContactEmail`/`ContactFirstName`/`ContactLastName` and `BusinessTitle`: 0..80).
const CONTACT_MAX_LEN: usize = 80;

/// The modelled `@length` maximum of `FileURI` (1..2000).
const FILE_URI_MAX_LEN: usize = 2000;

/// The requirement a `FieldPath` that names no amendable field at all fails, as the
/// handler states it.
const UNAMENDABLE: &str = "Member must name one of the amendable fields (Name, Description, \
                           BenefitApplicationDetails, PartnerContacts, FileDetails), a \
                           `.`-separated path into BenefitApplicationDetails, or a member \
                           of one element of PartnerContacts or FileDetails";

/// The requirement a `FieldPath` fails when it does not address one element of a list
/// member and one member of that element, as the handler states it.
fn element_path_requirement(field: &str) -> String {
    format!(
        "Member must name a member of one element of {field}, as that element's 0-based \
         position followed by the member name"
    )
}

/// The requirement a `FieldPath` fails when it addresses an element the list does not
/// hold.
fn element_count_requirement(field: &str, count: usize) -> String {
    format!("Member must name a 0-based element position within the {count} elements {field} holds")
}

/// The requirement a `FieldPath` fails when it names a member of a list element the
/// service owns rather than one a caller supplies.
fn element_member_requirement(field: &str, members: &[&str]) -> String {
    format!(
        "Member must name one of the amendable members of a {field} element ({})",
        members.join(", ")
    )
}

/// An email address of exactly `length` characters that the modelled pattern admits, so
/// the published `@length` bound and one past it can both be sent.
fn email_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// An `s3://` URI of exactly `length` characters, for the same reason.
fn file_uri_of_length(length: usize) -> String {
    let prefix = "s3://bucket/";
    format!("{prefix}{}", "u".repeat(length - prefix.len()))
}

/// The value a `.`-separated path names in a read application: a numeric segment indexes a
/// list, so a probe path reads like the `FieldPath` that wrote it.
fn at<'a>(body: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(body, |value, step| {
        match step.parse::<usize>() {
            Ok(index) => &value[index],
            Err(_) => &value[step],
        }
    })
}

/// Create one application carrying a partner contact and two attachments, submit it, and
/// return its id — so every amendment below is made on a SUBMITTED application, the state
/// in which `UpdateBenefitApplication` is refused on status.
async fn submitted(harness: &Harness, case: &str, token: &str) -> String {
    let benefit_id = text(case, &harness.active_benefit_of(CATALOG).await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": "contacts-and-attachments",
                "Description": "the application whose contacts and files are amended",
                "BenefitApplicationDetails": { "CampaignName": "seeded" },
                "PartnerContacts": [{ "Email": "ada@example.com", "FirstName": "Ada" }],
                "FileDetails": [
                    { "FileURI": "s3://bucket/first.csv", "BusinessUseCase": "the first attachment" },
                    { "FileURI": "https://example.com/second.pdf" },
                ],
            }),
        )
        .await;
    let id = text(case, &created, "Id");
    reach(harness, case, CATALOG, &id, STATUS_IN_REVIEW).await;
    id
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

/// Every member of a partner contact and of an attachment that a caller supplies can be
/// amended on a submitted application, whichever form the path is written in; the next read
/// reports exactly that, the service-owned members that FOLLOW an amended value are derived
/// again, and the revision moves once per request.
#[tokio::test]
async fn every_caller_supplied_member_of_a_contact_or_an_attachment_can_be_amended() {
    let harness = Harness::new();
    let id = submitted(&harness, "seed", "tok-supplied-seed").await;

    /// One row: the amendments a request carries, and the `(path, expected JSON)` pairs the
    /// read must then report.
    type Row = (&'static str, Value, Vec<(&'static str, Value)>);
    let rows: Vec<Row> = vec![
        (
            "a contact email, the position subscripted",
            json!([amendment("PartnerContacts[0].Email", "grace@example.com")]),
            vec![("PartnerContacts.0.Email", json!("grace@example.com"))],
        ),
        (
            "a contact first name, the position a segment of its own",
            json!([amendment("PartnerContacts.0.FirstName", "Grace")]),
            vec![("PartnerContacts.0.FirstName", json!("Grace"))],
        ),
        (
            "a contact last name, in JSON-path form",
            json!([amendment("$.PartnerContacts[0].LastName", "Hopper")]),
            vec![("PartnerContacts.0.LastName", json!("Hopper"))],
        ),
        (
            "a contact business title",
            json!([amendment("PartnerContacts[0].BusinessTitle", "Rear Admiral")]),
            vec![("PartnerContacts.0.BusinessTitle", json!("Rear Admiral"))],
        ),
        (
            "a contact phone",
            json!([amendment("PartnerContacts[0].Phone", "+1-555-0100")]),
            vec![("PartnerContacts.0.Phone", json!("+1-555-0100"))],
        ),
        (
            "a phone at this service's bound on a member the model leaves unconstrained",
            json!([amendment(
                "PartnerContacts[0].Phone",
                &"5".repeat(MAX_DETAILS_VALUE_LEN)
            )]),
            vec![(
                "PartnerContacts.0.Phone",
                json!("5".repeat(MAX_DETAILS_VALUE_LEN)),
            )],
        ),
        (
            "an email at the published length bound",
            json!([amendment(
                "PartnerContacts[0].Email",
                &email_of_length(CONTACT_MAX_LEN)
            )]),
            vec![(
                "PartnerContacts.0.Email",
                json!(email_of_length(CONTACT_MAX_LEN)),
            )],
        ),
        (
            "an attachment URI, which the members derived from it follow",
            json!([amendment("FileDetails[0].FileURI", "s3://bucket/corrected.pdf")]),
            vec![
                ("FileDetails.0.FileURI", json!("s3://bucket/corrected.pdf")),
                ("FileDetails.0.FileName", json!("corrected.pdf")),
                ("FileDetails.0.FileType", json!("application/pdf")),
                // The attachment's own use case is untouched by a write to its URI.
                (
                    "FileDetails.0.BusinessUseCase",
                    json!("the first attachment"),
                ),
            ],
        ),
        (
            "an attachment business use case, on the second attachment",
            json!([amendment(
                "FileDetails.1.BusinessUseCase",
                "the corrected second attachment"
            )]),
            vec![(
                "FileDetails.1.BusinessUseCase",
                json!("the corrected second attachment"),
            )],
        ),
        (
            "an https attachment URI",
            json!([amendment(
                "FileDetails[1].FileURI",
                "https://example.com/third.png"
            )]),
            vec![
                (
                    "FileDetails.1.FileURI",
                    json!("https://example.com/third.png"),
                ),
                ("FileDetails.1.FileName", json!("third.png")),
                ("FileDetails.1.FileType", json!("image/png")),
            ],
        ),
        (
            "an attachment URI at the published length bound",
            json!([amendment(
                "FileDetails[0].FileURI",
                &file_uri_of_length(FILE_URI_MAX_LEN)
            )]),
            vec![
                (
                    "FileDetails.0.FileURI",
                    json!(file_uri_of_length(FILE_URI_MAX_LEN)),
                ),
                // The last path segment is longer than the modelled `FileName`, so the
                // service reports no file name for it rather than one the model refuses.
                ("FileDetails.0.FileName", Value::Null),
            ],
        ),
        (
            "a contact member and an attachment member in one request",
            json!([
                amendment("PartnerContacts[0].Email", "ada@example.org"),
                amendment("FileDetails[0].FileURI", "s3://bucket/both.csv"),
            ]),
            vec![
                ("PartnerContacts.0.Email", json!("ada@example.org")),
                ("FileDetails.0.FileURI", json!("s3://bucket/both.csv")),
                ("FileDetails.0.FileName", json!("both.csv")),
                ("FileDetails.0.FileType", json!("text/csv")),
            ],
        ),
        (
            // A subscript is a list index on the FIELD NAME only: a key of the details
            // document is a name the caller chose, so it is addressed exactly as written.
            "a details key that contains a bracket",
            json!([amendment("BenefitApplicationDetails.a[b]", "verbatim")]),
            vec![("BenefitApplicationDetails.a[b]", json!("verbatim"))],
        ),
    ];

    let mut before = harness.get("seed", &id).await;
    for (index, (case, amendments, expected)) in rows.iter().enumerate() {
        let revision = text(case, &before, "Revision");
        harness
            .ok(
                case,
                "AmendBenefitApplication",
                &amend_body(
                    &id,
                    &revision,
                    &format!("tok-supplied-{index}"),
                    amendments.clone(),
                ),
            )
            .await;

        let after = harness.get(case, &id).await;
        for (path, value) in expected {
            assert_eq!(
                at(&after, path),
                value,
                "case {case:?}: expected {path} to be {value:?}, got {after:?}"
            );
        }
        assert_ne!(
            text(case, &after, "Revision"),
            revision,
            "case {case:?}: an amendment that changed the record must move the revision, \
             got {after:?}"
        );
        assert_eq!(
            after["Status"],
            json!(STATUS_IN_REVIEW),
            "case {case:?}: an amendment must not move the status, got {after:?}"
        );
        for untouched in ["Id", "Arn", "Catalog", "BenefitId", "CreatedAt"] {
            assert_eq!(
                after[untouched], before[untouched],
                "case {case:?}: an amendment must not change {untouched}, got {after:?}"
            );
        }
        // Who attached each file and when is the service's own record of the attachment,
        // so amending the file's caller-supplied content leaves both as they were.
        for file in ["0", "1"] {
            for recorded in ["CreatedBy", "CreatedAt"] {
                let path = format!("FileDetails.{file}.{recorded}");
                assert_eq!(
                    at(&after, &path),
                    at(&before, &path),
                    "case {case:?}: an amendment must not change {path}, got {after:?}"
                );
            }
        }
        before = after;
    }
}

/// An amendment naming the value a caller-supplied member already holds is accepted and
/// leaves the record — and so the revision — exactly as it was, the rule every write of
/// this service applies.
#[tokio::test]
async fn an_amendment_writing_the_stored_value_moves_nothing() {
    let harness = Harness::new();
    let id = submitted(&harness, "seed", "tok-noop-seed").await;
    let before = harness.get("seed", &id).await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "the contact email it already holds",
            json!([amendment("PartnerContacts[0].Email", "ada@example.com")]),
        ),
        (
            "the contact first name it already holds",
            json!([amendment("PartnerContacts.0.FirstName", "Ada")]),
        ),
        (
            "the attachment URI it already holds",
            json!([amendment("FileDetails[0].FileURI", "s3://bucket/first.csv")]),
        ),
        (
            "the attachment use case it already holds",
            json!([amendment(
                "FileDetails[0].BusinessUseCase",
                "the first attachment"
            )]),
        ),
    ];

    for (index, (case, amendments)) in rows.iter().enumerate() {
        let revision = text(case, &before, "Revision");
        harness
            .ok(
                case,
                "AmendBenefitApplication",
                &amend_body(
                    &id,
                    &revision,
                    &format!("tok-noop-{index}"),
                    amendments.clone(),
                ),
            )
            .await;
        let after = harness.get(case, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: an amendment writing the stored value must change nothing, \
             got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A `FieldPath` naming a member of a contact or an attachment that the SERVICE owns, a
/// path that does not address one element of a list member, or a `NewValue` the model does
/// not let that member hold, is refused 400 `ValidationException` — and the stored
/// application is left exactly as it was.
#[tokio::test]
async fn amend_refuses_a_service_owned_member_a_bad_element_path_or_a_value_the_model_refuses() {
    let harness = Harness::new();
    let id = submitted(&harness, "seed", "tok-refuse-seed").await;
    let before = harness.get("seed", &id).await;
    let revision = text("seed", &before, "Revision");

    // The members of a stored attachment a caller supplies, and the members of a contact,
    // as the refusals name them.
    let file_members = ["FileURI", "BusinessUseCase"];
    let contact_members = ["Email", "FirstName", "LastName", "BusinessTitle", "Phone"];

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "the file name derived from the URI",
            json!([amendment("FileDetails[0].FileName", "renamed.csv")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_member_requirement("FileDetails", &file_members),
            ),
        ),
        (
            "the file type derived from the URI",
            json!([amendment("FileDetails[0].FileType", "text/csv")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_member_requirement("FileDetails", &file_members),
            ),
        ),
        (
            "who attached the file",
            json!([amendment("FileDetails.0.CreatedBy", "someone-else")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_member_requirement("FileDetails", &file_members),
            ),
        ),
        (
            "when the file was attached",
            json!([amendment("FileDetails[0].CreatedAt", "2020-01-01T00:00:00Z")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_member_requirement("FileDetails", &file_members),
            ),
        ),
        (
            "the status of the file",
            json!([amendment("FileDetails[0].FileStatus", "APPROVED")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_member_requirement("FileDetails", &file_members),
            ),
        ),
        (
            "a member no contact has",
            json!([amendment("PartnerContacts[0].Nickname", "Amazing Grace")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_member_requirement("PartnerContacts", &contact_members),
            ),
        ),
        (
            "the contacts list itself, with no element named",
            json!([amendment("PartnerContacts", "grace@example.com")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_path_requirement("PartnerContacts"),
            ),
        ),
        (
            "the files list itself, with no element named",
            json!([amendment("FileDetails", "s3://bucket/loose.csv")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_path_requirement("FileDetails"),
            ),
        ),
        (
            "a position that is not a number",
            json!([amendment("PartnerContacts[first].Email", "grace@example.com")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_path_requirement("PartnerContacts"),
            ),
        ),
        (
            "a position written with a leading zero",
            json!([amendment("PartnerContacts[00].Email", "grace@example.com")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_path_requirement("PartnerContacts"),
            ),
        ),
        (
            "a signed position",
            json!([amendment("FileDetails[+1].FileURI", "s3://bucket/signed.csv")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_path_requirement("FileDetails"),
            ),
        ),
        (
            "an unclosed subscript",
            json!([amendment("PartnerContacts[0.Email", "grace@example.com")]),
            clause("amendments.1.member.fieldPath", UNAMENDABLE),
        ),
        (
            // A subscript is an element position, so it is read on a LIST member's name
            // and nowhere else: on any other field name the brackets are part of the
            // name, which names no amendable field.
            "a subscript on a field that is not a list",
            json!([amendment("BenefitApplicationDetails[0]", "subscripted")]),
            clause("amendments.1.member.fieldPath", UNAMENDABLE),
        ),
        (
            "a subscript on a string field",
            json!([amendment("Name[0]", "subscripted")]),
            clause("amendments.1.member.fieldPath", UNAMENDABLE),
        ),
        (
            "a path below a member of a contact",
            json!([amendment(
                "PartnerContacts[0].Email.Domain",
                "example.com"
            )]),
            clause(
                "amendments.1.member.fieldPath",
                &element_path_requirement("PartnerContacts"),
            ),
        ),
        (
            "an element the contacts do not hold",
            json!([amendment("PartnerContacts[1].Email", "grace@example.com")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_count_requirement("PartnerContacts", 1),
            ),
        ),
        (
            "an element the files do not hold",
            json!([amendment("FileDetails[7].FileURI", "s3://bucket/eighth.csv")]),
            clause(
                "amendments.1.member.fieldPath",
                &element_count_requirement("FileDetails", 2),
            ),
        ),
        (
            "an email the modelled pattern refuses",
            json!([amendment("PartnerContacts[0].Email", "not-an-email")]),
            clause(
                "amendments.1.member.newValue",
                &format!("Member must satisfy regular expression pattern: {EMAIL_PATTERN}"),
            ),
        ),
        (
            "an email one character past the published bound",
            json!([amendment(
                "PartnerContacts[0].Email",
                &email_of_length(CONTACT_MAX_LEN + 1)
            )]),
            clause(
                "amendments.1.member.newValue",
                &format!("Member must have length less than or equal to {CONTACT_MAX_LEN}"),
            ),
        ),
        (
            "an attachment URI the modelled pattern refuses",
            json!([amendment("FileDetails[0].FileURI", "ftp://bucket/first.csv")]),
            clause(
                "amendments.1.member.newValue",
                &format!("Member must satisfy regular expression pattern: {FILE_URI_PATTERN}"),
            ),
        ),
        (
            "an attachment URI one character past the published bound",
            json!([amendment(
                "FileDetails[0].FileURI",
                &file_uri_of_length(FILE_URI_MAX_LEN + 1)
            )]),
            clause(
                "amendments.1.member.newValue",
                &format!("Member must have length less than or equal to {FILE_URI_MAX_LEN}"),
            ),
        ),
        (
            "a phone one character past this service's bound",
            json!([amendment(
                "PartnerContacts[0].Phone",
                &"5".repeat(MAX_DETAILS_VALUE_LEN + 1)
            )]),
            clause(
                "amendments.1.member.newValue",
                &format!("Member must have length less than or equal to {MAX_DETAILS_VALUE_LEN}"),
            ),
        ),
        (
            "a business use case one character past this service's bound",
            json!([amendment(
                "FileDetails[0].BusinessUseCase",
                &"u".repeat(MAX_DETAILS_VALUE_LEN + 1)
            )]),
            clause(
                "amendments.1.member.newValue",
                &format!("Member must have length less than or equal to {MAX_DETAILS_VALUE_LEN}"),
            ),
        ),
        (
            "one applicable amendment and one that is not",
            json!([
                amendment("PartnerContacts[0].FirstName", "would-apply"),
                amendment("FileDetails[0].CreatedBy", "would-not"),
            ]),
            // The refusal names the SECOND element of the list, 1-based.
            clause(
                "amendments.2.member.fieldPath",
                &element_member_requirement("FileDetails", &file_members),
            ),
        ),
    ];

    for (index, (case, amendments, expected)) in rows.iter().enumerate() {
        let (status, out) = harness
            .call(
                "AmendBenefitApplication",
                &amend_body(
                    &id,
                    &revision,
                    &format!("tok-refuse-{index}"),
                    amendments.clone(),
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
        let message = message_of(&out);
        assert_eq!(
            &message, expected,
            "case {case:?}: expected the refusal to state {expected:?}, got {out:?}"
        );
        // A refusal states the requirement, never the value it rejected.
        assert!(
            !message.contains("would-apply") && !message.contains("not-an-email"),
            "case {case:?}: a refusal must not echo the rejected value, got {message:?}"
        );
        let after = harness.get(case, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused amendment must change nothing, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The hole the rule closes
// ---------------------------------------------------------------------------

/// Once the application is submitted `UpdateBenefitApplication` is refused on status, and
/// the amendment is what changes the members that update would have written — so a
/// submitted application's contacts and attachments are reachable through this operation.
#[tokio::test]
async fn a_submitted_applications_contacts_and_attachments_are_amendable() {
    let harness = Harness::new();
    let id = submitted(&harness, "seed", "tok-hole-seed").await;
    let before = harness.get("seed", &id).await;
    let revision = text("seed", &before, "Revision");

    // The update is refused: it rewrites the partner's own draft, and this one is in
    // review.
    let (status, out) = harness
        .call(
            "UpdateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-hole-update",
                "Identifier": id,
                "Revision": revision,
                "PartnerContacts": [{ "Email": "grace@example.com" }],
                "FileDetails": [{ "FileURI": "s3://bucket/updated.csv" }],
            }),
        )
        .await;
    assert_eq!(
        status, 409,
        "case \"the update of a submitted application\": expected 409, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ConflictException",
        "case \"the update of a submitted application\": expected ConflictException, got {out:?}"
    );

    // The amendment reaches both of those members on the same record.
    harness
        .ok(
            "the amendment of a submitted application",
            "AmendBenefitApplication",
            &amend_body(
                &id,
                &revision,
                "tok-hole-amend",
                json!([
                    amendment("PartnerContacts[0].Email", "grace@example.com"),
                    amendment("FileDetails[0].FileURI", "s3://bucket/updated.csv"),
                ]),
            ),
        )
        .await;

    let after = harness.get("the amendment of a submitted application", &id).await;
    for (path, expected) in [
        ("PartnerContacts.0.Email", json!("grace@example.com")),
        ("FileDetails.0.FileURI", json!("s3://bucket/updated.csv")),
    ] {
        assert_eq!(
            at(&after, path),
            &expected,
            "case \"the amendment of a submitted application\": expected {path} to be \
             {expected:?}, got {after:?}"
        );
    }
    assert_eq!(
        after["Status"],
        json!(STATUS_IN_REVIEW),
        "case \"the amendment of a submitted application\": the status must not move, \
         got {after:?}"
    );
}
