// SPDX-License-Identifier: CC0-1.0

//! Where the constraints an amendment's `NewValue` must satisfy come from, end to end over
//! the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test: `Amendment$NewValue` is an unconstrained `String`, so the value an
//! amendment writes is judged against the MODELLED constraints of the member the
//! `FieldPath` names — the same constraints the centralized request walk enforces on the
//! value `UpdateBenefitApplication` supplies for that same member. So for one value and one
//! member the two write paths agree: either both serve the write, or both refuse it with the
//! same requirement, the one the model carries.
//!
//! That is asserted WITHOUT restating any bound here: each row sends its value through
//! `UpdateBenefitApplication` (where the centralized walk reads the constraint off the
//! model) and then through `AmendBenefitApplication`, and the update's own answer is what
//! the amendment's answer is checked against. A bound that moved in the model moves both
//! sides of every row at once.
//!
//! The rows span what the rule generalises over: which member the path names (`Name` and
//! `Description` on the application itself, every string member of a `PartnerContacts`
//! element, both members of a `FileDetails` element, including the one the model constrains
//! not at all), which constraint class the value fails (`@length` at either end, `@pattern`),
//! how far from the bound it is (inside it, far past it), and the character width of the
//! value (ASCII and multi-byte, since a `@length` counts characters).

use serde_json::{json, Map, Value};

use crate::amend_benefit_application::{amend_body, amendment};
use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::service_error_rules::{assert_validation_invariants, clause, message_of, one};

/// The member path a refused amendment names: the `NewValue` of the first amendment the
/// request carried.
const NEW_VALUE_MEMBER: &str = "amendments.1.member.newValue";

/// The envelope a single violation renders into, which every row's refusal must be — a row
/// whose value failed TWO constraints at once would make "the requirement the model states"
/// ambiguous, so the rows are chosen to fail one and the assertion says so.
const ONE_VIOLATION: &str = "1 validation error detected: ";

/// The attachment URI the seeded record carries, and the one every row that writes another
/// member of that attachment keeps: `FileInput$FileURI` is `@required`, so an update naming
/// one attachment must carry its URI (`smithy_query(query="shape",shape="FileInput")`).
const SEED_FILE_URI: &str = "s3://bucket/seeded.csv";

/// Which member of the application one row writes.
enum Member {
    /// A string member of the application itself, as the model spells it on the wire.
    Application(&'static str),
    /// A member of one `PartnerContacts` element (a modelled `Contact`).
    Contact(&'static str),
    /// A member of one `FileDetails` element (a modelled `FileInput`).
    File(&'static str),
}

impl Member {
    /// The `FieldPath` an amendment names this member by — for a list member, the 0-based
    /// position of the element the seeded record holds, then the member to write.
    fn field_path(&self) -> String {
        match self {
            Member::Application(member) => (*member).to_string(),
            Member::Contact(member) => format!("PartnerContacts.0.{member}"),
            Member::File(member) => format!("FileDetails.0.{member}"),
        }
    }

    /// The input members `UpdateBenefitApplication` carries the same value on, so the
    /// centralized walk judges the identical value against the identical modelled member.
    fn update_members(&self, value: &str) -> Value {
        match self {
            Member::Application(member) => object(member, json!(value)),
            Member::Contact(member) => object("PartnerContacts", json!([object(member, json!(value))])),
            Member::File(member) => {
                let mut file = Map::new();
                file.insert("FileURI".to_string(), json!(SEED_FILE_URI));
                file.insert((*member).to_string(), json!(value));
                object("FileDetails", json!([Value::Object(file)]))
            }
        }
    }

    /// The value a read of the application reports for this member.
    fn stored<'b>(&self, body: &'b Value) -> &'b Value {
        match self {
            Member::Application(member) => &body[*member],
            Member::Contact(member) => &body["PartnerContacts"][0][*member],
            Member::File(member) => &body["FileDetails"][0][*member],
        }
    }
}

/// One row: the member written, the value written into it, and what makes the row its own
/// case in the class.
struct Row {
    case: &'static str,
    member: Member,
    value: String,
}

/// A one-member JSON object, for a member named by a value rather than by a literal.
fn object(member: &str, value: Value) -> Value {
    let mut members = Map::new();
    members.insert(member.to_string(), value);
    Value::Object(members)
}

/// Copy every member of `extra` into `body`, both objects.
fn merge(body: &mut Value, extra: Value) {
    let Value::Object(extra) = extra else {
        unreachable!("every row's update members are a JSON object")
    };
    for (member, value) in extra {
        body[member] = value;
    }
}

/// The requirement a refusal states: the clause's text after the member path.
fn requirement_of(case: &str, message: &str) -> String {
    message
        .split_once("failed to satisfy constraint: ")
        .unwrap_or_else(|| panic!("case {case:?}: expected a constraint clause, got {message:?}"))
        .1
        .to_string()
}

/// An email address of exactly `length` characters the modelled `ContactEmail` `@pattern`
/// admits, so a row can miss that member's `@length` and nothing else.
fn email_of_length(length: usize) -> String {
    let domain = "@example.com";
    format!("{}{domain}", "a".repeat(length - domain.len()))
}

/// An `s3://` URI of exactly `length` characters, which the modelled `FileURI` `@pattern`
/// admits, for the same reason.
fn file_uri_of_length(length: usize) -> String {
    let prefix = "s3://bucket/";
    format!("{prefix}{}", "u".repeat(length - prefix.len()))
}

/// One draft application carrying a partner contact and an attachment, so every row has an
/// element 0 of both list members to write.
async fn seed(harness: &Harness, case: &str, token: &str) -> String {
    let benefit_id = text(case, &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": "model-judged",
                "Description": "the application whose amendments are judged by the model",
                "PartnerContacts": [{ "Email": "ada@example.com", "FirstName": "Ada" }],
                "FileDetails": [{ "FileURI": SEED_FILE_URI, "BusinessUseCase": "seeded" }],
            }),
        )
        .await;
    text(case, &created, "Id")
}

/// For one value and one member, an amendment and the update agree: the modelled constraint
/// is what judges the value on either path, so a value the update's centralized walk refuses
/// is refused by the amendment with the SAME requirement, and a value that walk admits is
/// written by the amendment and reported by the next read.
#[tokio::test]
async fn an_amendment_is_judged_by_the_same_modelled_constraints_the_update_is() {
    let harness = Harness::new();

    let rows = vec![
        Row {
            case: "an empty Name",
            member: Member::Application("Name"),
            value: String::new(),
        },
        Row {
            case: "a Name far past its modelled maximum",
            member: Member::Application("Name"),
            value: "n".repeat(4096),
        },
        Row {
            case: "a Name the model admits",
            member: Member::Application("Name"),
            value: "n".repeat(64),
        },
        Row {
            case: "an empty Description",
            member: Member::Application("Description"),
            value: String::new(),
        },
        Row {
            case: "a multi-byte Description far past its modelled maximum",
            member: Member::Application("Description"),
            value: "é".repeat(4096),
        },
        Row {
            case: "a multi-byte Description the model admits",
            member: Member::Application("Description"),
            value: "é".repeat(500),
        },
        Row {
            case: "a contact email past its modelled maximum",
            member: Member::Contact("Email"),
            value: email_of_length(120),
        },
        Row {
            case: "a contact email outside its modelled pattern",
            member: Member::Contact("Email"),
            value: "not-an-email".to_string(),
        },
        Row {
            case: "a contact email the model admits",
            member: Member::Contact("Email"),
            value: "grace@example.com".to_string(),
        },
        Row {
            case: "a contact first name past its modelled maximum",
            member: Member::Contact("FirstName"),
            value: "f".repeat(120),
        },
        Row {
            case: "a multi-byte contact last name the model admits",
            member: Member::Contact("LastName"),
            value: "Hoppé".to_string(),
        },
        Row {
            case: "a contact business title past its modelled maximum",
            member: Member::Contact("BusinessTitle"),
            value: "t".repeat(120),
        },
        Row {
            case: "a contact phone, a member the model constrains not at all",
            member: Member::Contact("Phone"),
            value: "+1-555-0100".to_string(),
        },
        Row {
            case: "an attachment URI outside its modelled pattern",
            member: Member::File("FileURI"),
            value: "ftp://bucket/rejected.csv".to_string(),
        },
        Row {
            case: "an attachment URI past its modelled maximum",
            member: Member::File("FileURI"),
            value: file_uri_of_length(2500),
        },
        Row {
            case: "an attachment URI the model admits",
            member: Member::File("FileURI"),
            value: "s3://bucket/corrected.csv".to_string(),
        },
        Row {
            case: "an attachment business use case, a member the model constrains not at all",
            member: Member::File("BusinessUseCase"),
            value: "the corrected attachment".to_string(),
        },
    ];

    // The table spans both former enforcement sites — the application's own string members
    // and the members of a list element — on each side of the rule, so it cannot shrink to
    // one of them.
    for (what, covered) in [
        (
            "an application member",
            rows.iter()
                .any(|row| matches!(row.member, Member::Application(_))),
        ),
        (
            "a list element's member",
            rows.iter()
                .any(|row| !matches!(row.member, Member::Application(_))),
        ),
    ] {
        assert!(
            covered,
            "case \"table\": expected a row writing {what}, got {:?}",
            rows.iter().map(|row| row.case).collect::<Vec<_>>()
        );
    }

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let id = seed(&harness, case, &format!("tok-model-judged-seed-{index}")).await;

        // What the model says about this value on this member: the centralized walk reads
        // the constraint off the shape, so the update's answer IS the model's answer.
        let revision = text(case, &harness.get(case, &id).await, "Revision");
        let mut update = json!({
            "Catalog": CATALOG,
            "ClientToken": format!("tok-model-judged-update-{index}"),
            "Identifier": id,
            "Revision": revision,
        });
        merge(&mut update, row.member.update_members(&row.value));
        let (update_status, update_out) = harness.call("UpdateBenefitApplication", &update).await;

        // The same value, written into the same member by an amendment.
        let revision = text(case, &harness.get(case, &id).await, "Revision");
        let (amend_status, amend_out) = harness
            .call(
                "AmendBenefitApplication",
                &amend_body(
                    &id,
                    &revision,
                    &format!("tok-model-judged-amend-{index}"),
                    json!([amendment(&row.member.field_path(), &row.value)]),
                ),
            )
            .await;

        if update_status == 200 {
            assert_eq!(
                amend_status, 200,
                "case {case:?}: a value the update stores must be amendable too, \
                 got {amend_status} {amend_out:?}"
            );
            let stored = harness.get(case, &id).await;
            let reported = row.member.stored(&stored);
            assert_eq!(
                reported,
                &json!(row.value),
                "case {case:?}: expected the read to report the amended value, got {reported:?}"
            );
            continue;
        }

        assert_eq!(
            update_status, 400,
            "case {case:?}: expected the update to be served or refused 400, \
             got {update_status} {update_out:?}"
        );
        assert_eq!(
            error_code(&update_out),
            "ValidationException",
            "case {case:?}: expected the update refused ValidationException, got {update_out:?}"
        );
        let update_message = message_of(&update_out)
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {update_out:?}"));
        assert!(
            update_message.starts_with(ONE_VIOLATION),
            "case {case:?}: expected the value to fail ONE constraint, got {update_message:?}"
        );
        let requirement = requirement_of(case, &update_message);

        assert_eq!(
            amend_status, 400,
            "case {case:?}: a value the model refuses on the update must be refused on the \
             amendment, got {amend_status} {amend_out:?}"
        );
        assert_eq!(
            error_code(&amend_out),
            "ValidationException",
            "case {case:?}: expected the amendment refused ValidationException, got {amend_out:?}"
        );
        let amend_message = message_of(&amend_out)
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {amend_out:?}"));
        assert_validation_invariants(case, &amend_message);
        let want = one(&clause(NEW_VALUE_MEMBER, &requirement));
        assert_eq!(
            amend_message, want,
            "case {case:?}: expected the amendment refused with the requirement the model \
             states ({want:?}), got {amend_message:?}"
        );

        // A refused amendment changed nothing: the member still holds what the record held.
        let stored = harness.get(case, &id).await;
        let reported = row.member.stored(&stored);
        assert_ne!(
            reported,
            &json!(row.value),
            "case {case:?}: a refused amendment must leave the record as it was, \
             got {reported:?}"
        );
    }
}
