// SPDX-License-Identifier: CC0-1.0

//! `AmendBenefitApplication`, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`), in its entitled and its un-entitled
//! form — so what it pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! The tests are tables. Their rows differ along what each rule generalises over: which
//! field a `FieldPath` names and how deep it reaches, the path's form, how many
//! amendments one request carries, each published length bound of the member a path
//! names and one past it, and which later check a request would otherwise have failed
//! (the gate answers ahead of all of them).

use serde_json::{json, Map, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    not_found_message, APPLICATION_KIND, MAX_AMENDMENTS_PER_REQUEST, MAX_AMENDMENT_REASON_LEN,
    MAX_CLIENT_TOKEN_LEN, MAX_DETAILS_BYTES, MAX_DETAILS_MEMBERS, MAX_DETAILS_VALUE_LEN,
    MAX_FIELD_PATH_LEN,
};

use crate::benefit_application_lifecycle::{error_code, Harness};

/// The production catalog, one of the two the service serves.
const CATALOG: &str = "AWS";

/// The account every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the harness signs for, which the ARNs below must carry.
const REGION: &str = "us-west-2";

/// The sentence the entitlement gate refuses with, written out here rather than read
/// from the service's own constant so the text itself is under test.
const ENTITLEMENT_MESSAGE: &str = "This action requires an active AWS partner benefit. \
Register as an AWS partner if you haven't already, or contact AWS partner central \
support.";

/// The status this service serves its `AccessDeniedException` with (recorded; the model
/// declares the shape `@httpError(403)`) — see `tests/all/entitlement_denial.rs`.
const DENIED_STATUS: u16 = 400;

/// The longest value `Name` accepts, from the modelled `BenefitApplicationName`
/// `@length(min: 1, max: 255)`.
const NAME_MAX_LEN: usize = 255;

/// The longest value `Description` accepts, from the modelled
/// `BenefitApplicationDescription` `@length(min: 1, max: 1000)`.
const DESCRIPTION_MAX_LEN: usize = 1000;

/// An application id the modelled `BenefitApplicationIdentifier` admits and no create
/// ever minted, so every lookup of it finds nothing.
const UNKNOWN_APPLICATION: &str = "benappl-00000000000000";

/// The ARN form of an application id, which `Identifier` also admits.
fn application_arn(id: &str) -> String {
    format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/{CATALOG}/benefit-application/{id}")
}

/// A member of a JSON object as a string, failing the test when it is absent.
fn text(case: &str, body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {body:?}"))
        .to_string()
}

/// The message an error body carries. The framework's own refusal envelope spells the
/// member `message`; this service's modelled errors spell it `Message` — both are read,
/// so a refusal cannot hide from these tests by travelling through the other channel.
fn message_of(body: &Value) -> String {
    body["message"]
        .as_str()
        .or(body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Call one operation that must answer 200, and return its body.
async fn ok(harness: &Harness, case: &str, operation: &str, body: &Value) -> Value {
    let (status, out) = harness.call(operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {out:?}"
    );
    out
}

/// The id of the first `ACTIVE` benefit of [`CATALOG`], which every create applies for.
async fn active_benefit_id(harness: &Harness) -> String {
    let out = ok(
        harness,
        "seed",
        "ListBenefits",
        &json!({ "Catalog": CATALOG }),
    )
    .await;
    let benefits = out["BenefitSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"seed\": ListBenefits returned no list: {out:?}"))
        .clone();
    text(
        "seed",
        benefits
            .iter()
            .find(|benefit| benefit["Status"] == json!("ACTIVE"))
            .unwrap_or_else(|| panic!("case \"seed\": no ACTIVE benefit in {benefits:?}")),
        "Id",
    )
}

/// Create one application to amend, and return its create response.
async fn seed(harness: &Harness, case: &str, token: &str, extra: Value) -> Value {
    let benefit_id = active_benefit_id(harness).await;
    let mut body = extra;
    body["Catalog"] = json!(CATALOG);
    body["ClientToken"] = json!(token);
    body["BenefitIdentifier"] = json!(benefit_id);
    ok(harness, case, "CreateBenefitApplication", &body).await
}

/// Read one application back through the implemented reader.
async fn get(harness: &Harness, case: &str, identifier: &str) -> Value {
    ok(
        harness,
        case,
        "GetBenefitApplication",
        &json!({ "Catalog": CATALOG, "Identifier": identifier }),
    )
    .await
}

/// One amendment request body.
fn amend_body(identifier: &str, revision: &str, token: &str, amendments: Value) -> Value {
    json!({
        "Catalog": CATALOG,
        "ClientToken": token,
        "Identifier": identifier,
        "Revision": revision,
        "AmendmentReason": "the partner corrected the application",
        "Amendments": amendments,
    })
}

/// One `(FieldPath, NewValue)` pair.
fn amendment(field_path: &str, new_value: &str) -> Value {
    json!({ "FieldPath": field_path, "NewValue": new_value })
}

/// A `FieldPath` into the details document whose whole text is exactly `length` bytes,
/// so each published `MAX_FIELD_PATH_LEN` bound and one past it can be sent.
fn details_path_of_length(length: usize) -> String {
    let prefix = "BenefitApplicationDetails.";
    let filler = length.saturating_sub(prefix.len());
    format!("{prefix}{}", "a".repeat(filler))
}

// ---------------------------------------------------------------------------
// The entitlement gate fronts this operation
// ---------------------------------------------------------------------------

/// From an account with no active AWS partner benefit, an amendment is refused
/// `AccessDeniedException` with [`DENIED_STATUS`] whatever else is wrong with it — ahead
/// of the empty-list, the field-path, the lookup and the revision checks — while a
/// malformed `Identifier` is still the centralized walk's 400, and the same request from
/// an entitled account gets past the gate.
#[tokio::test]
async fn the_entitlement_gate_answers_ahead_of_every_check_this_operation_makes() {
    let harness = Harness::unentitled();

    // Each row would fail a DIFFERENT later check if the gate were not ahead of it.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a well-formed request",
            amend_body(
                UNKNOWN_APPLICATION,
                "1",
                "tok-gate-ok",
                json!([amendment("Name", "amended")]),
            ),
        ),
        (
            "a request naming an application by ARN",
            amend_body(
                &application_arn(UNKNOWN_APPLICATION),
                "1",
                "tok-gate-arn",
                json!([amendment("Name", "amended")]),
            ),
        ),
        (
            "a request with no amendments",
            amend_body(UNKNOWN_APPLICATION, "1", "tok-gate-empty", json!([])),
        ),
        (
            "a request naming an unamendable field",
            amend_body(
                UNKNOWN_APPLICATION,
                "1",
                "tok-gate-field",
                json!([amendment("Status", "APPROVED")]),
            ),
        ),
        (
            "a request carrying a revision no application is at",
            amend_body(
                UNKNOWN_APPLICATION,
                "99999",
                "tok-gate-revision",
                json!([amendment("Name", "amended")]),
            ),
        ),
    ];

    for (case, body) in &rows {
        let (status, out) = harness.call("AmendBenefitApplication", body).await;
        assert_eq!(
            status, DENIED_STATUS,
            "case {case:?}: an un-entitled account must be refused with {DENIED_STATUS}, \
             got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {out:?}"
        );
        assert_eq!(
            message_of(&out),
            ENTITLEMENT_MESSAGE,
            "case {case:?}: expected the entitlement sentence, got {out:?}"
        );
    }

    // The modelled constraints are checked earlier still, by the centralized walk.
    let (status, out) = harness
        .call(
            "AmendBenefitApplication",
            &amend_body(
                "!!not an identifier!!",
                "1",
                "tok-gate-malformed",
                json!([amendment("Name", "amended")]),
            ),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a malformed identifier\": shape validation runs ahead of the gate, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case \"a malformed identifier\": expected ValidationException, got {out:?}"
    );

    // And the denial is the gate's decision about the account, not a constant answer:
    // with an active partner benefit the same request reaches the lookup it was refused
    // ahead of.
    let entitled = Harness::new();
    let (status, out) = entitled
        .call(
            "AmendBenefitApplication",
            &amend_body(
                UNKNOWN_APPLICATION,
                "1",
                "tok-gate-entitled",
                json!([amendment("Name", "amended")]),
            ),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"an entitled account\": expected the lookup's 404, got {status} {out:?}"
    );
    assert_eq!(
        message_of(&out),
        not_found_message(APPLICATION_KIND, UNKNOWN_APPLICATION),
        "case \"an entitled account\": expected the not-found sentence, got {out:?}"
    );
}

// ---------------------------------------------------------------------------
// Applying amendments
// ---------------------------------------------------------------------------

/// An amendment writes the field its `FieldPath` names, leaves every other member
/// alone, and bumps the revision once per request; the next Get reports exactly that.
#[tokio::test]
async fn an_amendment_writes_the_field_its_path_names() {
    let harness = Harness::new();
    let created = seed(
        &harness,
        "seed",
        "tok-apply-seed",
        json!({
            "Name": "demo",
            "Description": "demo app",
            "BenefitApplicationDetails": {
                "CampaignName": "spring",
                "Budget": { "Amount": "100" },
            },
        }),
    )
    .await;
    let id = text("seed", &created, "Id");

    /// One row: the amendments a request carries, and the `(member, expected JSON)`
    /// pairs the Get must then report.
    type Row = (&'static str, Value, Vec<(&'static str, Value)>);
    let rows: Vec<Row> = vec![
        (
            "a top-level string member",
            json!([amendment("Name", "demo-2")]),
            vec![("Name", json!("demo-2"))],
        ),
        (
            "the description",
            json!([amendment("Description", "corrected")]),
            vec![("Description", json!("corrected"))],
        ),
        (
            "the JSON-path form of a field name",
            json!([amendment("$.Name", "demo-3")]),
            vec![("Name", json!("demo-3"))],
        ),
        (
            "an existing member of the details document",
            json!([amendment("BenefitApplicationDetails.CampaignName", "autumn")]),
            vec![(
                "BenefitApplicationDetails",
                json!({
                    "CampaignName": "autumn",
                    "Budget": { "Amount": "100" },
                }),
            )],
        ),
        (
            "a member the details document did not hold",
            json!([amendment("BenefitApplicationDetails.Region", "us-west-2")]),
            vec![(
                "BenefitApplicationDetails",
                json!({
                    "CampaignName": "autumn",
                    "Budget": { "Amount": "100" },
                    "Region": "us-west-2",
                }),
            )],
        ),
        (
            "a nested member of the details document",
            json!([amendment("BenefitApplicationDetails.Budget.Amount", "250")]),
            vec![(
                "BenefitApplicationDetails",
                json!({
                    "CampaignName": "autumn",
                    "Budget": { "Amount": "250" },
                    "Region": "us-west-2",
                }),
            )],
        ),
        (
            "a path that creates the objects it needs",
            json!([amendment(
                "$.BenefitApplicationDetails.Contact.Primary.Email",
                "partner@example.com"
            )]),
            vec![(
                "BenefitApplicationDetails",
                json!({
                    "CampaignName": "autumn",
                    "Budget": { "Amount": "250" },
                    "Region": "us-west-2",
                    "Contact": { "Primary": { "Email": "partner@example.com" } },
                }),
            )],
        ),
        (
            "two fields in one request",
            json!([
                amendment("Name", "demo-4"),
                amendment("BenefitApplicationDetails.CampaignName", "winter"),
            ]),
            vec![
                ("Name", json!("demo-4")),
                (
                    "BenefitApplicationDetails",
                    json!({
                        "CampaignName": "winter",
                        "Budget": { "Amount": "250" },
                        "Region": "us-west-2",
                        "Contact": { "Primary": { "Email": "partner@example.com" } },
                    }),
                ),
            ],
        ),
        (
            "one path twice: the last value wins",
            json!([
                amendment("Description", "first"),
                amendment("Description", "last"),
            ]),
            vec![("Description", json!("last"))],
        ),
        (
            "a value at each published length bound",
            json!([
                amendment("Name", &"n".repeat(NAME_MAX_LEN)),
                amendment("Description", &"d".repeat(DESCRIPTION_MAX_LEN)),
            ]),
            vec![
                ("Name", json!("n".repeat(NAME_MAX_LEN))),
                ("Description", json!("d".repeat(DESCRIPTION_MAX_LEN))),
            ],
        ),
    ];

    let mut before = get(&harness, "seed", &id).await;
    for (index, (case, amendments, expected)) in rows.iter().enumerate() {
        let revision = text(case, &before, "Revision");
        let out = ok(
            &harness,
            case,
            "AmendBenefitApplication",
            &amend_body(
                &id,
                &revision,
                &format!("tok-apply-{index}"),
                amendments.clone(),
            ),
        )
        .await;
        assert_eq!(
            out,
            json!({}),
            "case {case:?}: the model binds no output member on this operation, got {out:?}"
        );

        let after = get(&harness, case, &id).await;
        for (member, value) in expected {
            assert_eq!(
                &after[*member], value,
                "case {case:?}: expected {member} to be {value:?}, got {after:?}"
            );
        }
        let new_revision = text(case, &after, "Revision");
        assert_eq!(
            new_revision
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("case {case:?}: {new_revision:?} is not a revision")),
            revision
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("case {case:?}: {revision:?} is not a revision"))
                + 1,
            "case {case:?}: one amendment request must bump the revision once, got {after:?}"
        );
        for untouched in ["Id", "Arn", "Catalog", "BenefitId", "Status", "CreatedAt"] {
            assert_eq!(
                after[untouched], before[untouched],
                "case {case:?}: an amendment must not change {untouched}, got {after:?}"
            );
        }
        before = after;
    }
}

/// An amendment of an application named by its ARN reaches the same record as one named
/// by its bare id.
#[tokio::test]
async fn an_amendment_resolves_the_identifier_by_id_and_by_arn() {
    let harness = Harness::new();
    let created = seed(&harness, "seed", "tok-arn-seed", json!({ "Name": "demo" })).await;
    let id = text("seed", &created, "Id");
    let arn = text("seed", &created, "Arn");

    for (index, (case, identifier)) in [("the bare id", &id), ("the full ARN", &arn)]
        .iter()
        .enumerate()
    {
        let revision = text(case, &get(&harness, case, &id).await, "Revision");
        ok(
            &harness,
            case,
            "AmendBenefitApplication",
            &amend_body(
                identifier,
                &revision,
                &format!("tok-arn-{index}"),
                json!([amendment("Name", &format!("amended-{index}"))]),
            ),
        )
        .await;
        let after = get(&harness, case, &id).await;
        assert_eq!(
            after["Name"],
            json!(format!("amended-{index}")),
            "case {case:?}: the amendment must reach the one stored record, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A `FieldPath` naming no amendable field, or a `NewValue` outside the length the
/// member it names is modelled with, is refused 400 `ValidationException` — and the
/// stored application is left exactly as it was, however many amendments of the same
/// request were applicable.
#[tokio::test]
async fn amend_refuses_an_unamendable_path_or_a_value_past_a_bound() {
    let harness = Harness::new();
    let created = seed(
        &harness,
        "seed",
        "tok-refuse-seed",
        json!({
            "Name": "demo",
            "Description": "demo app",
            "BenefitApplicationDetails": { "CampaignName": "spring" },
        }),
    )
    .await;
    let id = text("seed", &created, "Id");
    let revision = text("seed", &created, "Revision");
    let before = get(&harness, "seed", &id).await;

    // `expected` is the clause the refusal must state, where the rule fixes it: the
    // envelope and the length wording are the ones this service answers every refused
    // request with (`service::constraint_envelope`).
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "no amendment at all",
            json!([]),
            Some(clause(
                "amendments",
                "Member must name at least one field to amend",
            )),
        ),
        ("a service-owned field", json!([amendment("Status", "APPROVED")]), None),
        (
            "the application's identity",
            json!([amendment("Id", UNKNOWN_APPLICATION)]),
            None,
        ),
        ("a member of a string field", json!([amendment("Name.First", "demo")]), None),
        ("an empty path", json!([amendment("", "demo")]), None),
        (
            "a path with an empty segment",
            json!([amendment("BenefitApplicationDetails..CampaignName", "x")]),
            None,
        ),
        (
            "a path through a value that is not an object",
            json!([amendment(
                "BenefitApplicationDetails.CampaignName.Deeper",
                "x"
            )]),
            None,
        ),
        (
            "an empty Name",
            json!([amendment("Name", "")]),
            Some(clause(
                "amendments.1.member.newValue",
                "Member must have length greater than or equal to 1",
            )),
        ),
        (
            "a Name one character past its bound",
            json!([amendment("Name", &"n".repeat(NAME_MAX_LEN + 1))]),
            Some(clause(
                "amendments.1.member.newValue",
                &format!("Member must have length less than or equal to {NAME_MAX_LEN}"),
            )),
        ),
        (
            "a Description one character past its bound",
            json!([amendment(
                "Description",
                &"d".repeat(DESCRIPTION_MAX_LEN + 1)
            )]),
            Some(clause(
                "amendments.1.member.newValue",
                &format!("Member must have length less than or equal to {DESCRIPTION_MAX_LEN}"),
            )),
        ),
        (
            "one applicable amendment and one that is not",
            json!([
                amendment("Name", "would-apply"),
                amendment("Programs", "would-not"),
            ]),
            // The refusal names the SECOND element of the list, 1-based.
            Some(clause(
                "amendments.2.member.fieldPath",
                "Member must name one of the amendable fields (Name, Description, \
                 BenefitApplicationDetails), or a `.`-separated path into \
                 BenefitApplicationDetails",
            )),
        ),
        (
            "a details value one byte past its bound",
            json!([amendment(
                "BenefitApplicationDetails.Campaign",
                &"v".repeat(MAX_DETAILS_VALUE_LEN + 1)
            )]),
            Some(clause(
                "amendments.1.member.newValue",
                &format!("Member must have length less than or equal to {MAX_DETAILS_VALUE_LEN}"),
            )),
        ),
        (
            "a field path one byte past its bound",
            json!([amendment(
                &details_path_of_length(MAX_FIELD_PATH_LEN + 1),
                "x"
            )]),
            None,
        ),
        (
            "one amendment past the most a request may carry",
            Value::Array(
                (0..=MAX_AMENDMENTS_PER_REQUEST)
                    .map(|index| amendment(&format!("BenefitApplicationDetails.f{index}"), "x"))
                    .collect(),
            ),
            Some(clause(
                "amendments",
                &format!("Member must have length less than or equal to {MAX_AMENDMENTS_PER_REQUEST}"),
            )),
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
        match expected {
            Some(expected) => assert_eq!(
                &message, expected,
                "case {case:?}: expected the refusal to state {expected:?}, got {out:?}"
            ),
            // Every refusal states the member path it is about, whatever its clause.
            None => assert!(
                message.contains("amendments.1.member.fieldPath"),
                "case {case:?}: a refusal must name the member that failed, got {message:?}"
            ),
        }
        // Nothing the request asked for was applied, and no value was echoed back.
        assert!(
            !message.contains("would-apply") && !message.contains("APPROVED"),
            "case {case:?}: a refusal must not echo the rejected value, got {message:?}"
        );
        let after = get(&harness, case, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused amendment must change nothing, got {after:?}"
        );
    }
}

/// The validation envelope over one clause, as this service answers a refused request:
/// `service::constraint_envelope` of a single violation.
fn clause(path: &str, requirement: &str) -> String {
    format!("1 validation error detected: Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// AT each published bound of this operation's own guards — the longest value an
/// amendment may write into the details document, the longest `FieldPath`, and the most
/// amendments one request may carry — the amendment is APPLIED (one past each is refused
/// by the table above).
#[tokio::test]
async fn amend_accepts_a_request_at_each_published_bound() {
    let harness = Harness::new();
    let created = seed(&harness, "seed", "tok-bounds-seed", json!({ "Name": "demo" })).await;
    let id = text("seed", &created, "Id");

    let longest_path = details_path_of_length(MAX_FIELD_PATH_LEN);
    let longest_value = "v".repeat(MAX_DETAILS_VALUE_LEN);
    let rows: Vec<(&str, Value)> = vec![
        (
            "a details value of exactly the longest length",
            json!([amendment(
                "BenefitApplicationDetails.Campaign",
                &longest_value
            )]),
        ),
        (
            "a field path of exactly the longest length",
            json!([amendment(&longest_path, "x")]),
        ),
        (
            "exactly the most amendments one request may carry",
            Value::Array(
                (0..MAX_AMENDMENTS_PER_REQUEST)
                    .map(|index| amendment(&format!("BenefitApplicationDetails.f{index}"), "x"))
                    .collect(),
            ),
        ),
    ];

    for (index, (case, amendments)) in rows.iter().enumerate() {
        let revision = text(case, &get(&harness, case, &id).await, "Revision");
        ok(
            &harness,
            case,
            "AmendBenefitApplication",
            &amend_body(
                &id,
                &revision,
                &format!("tok-bounds-{index}"),
                amendments.clone(),
            ),
        )
        .await;
    }

    // Every one of them is in the document the Get reports.
    let after = get(&harness, "at the bounds", &id).await;
    let details = &after["BenefitApplicationDetails"];
    assert_eq!(
        details["Campaign"],
        json!(longest_value),
        "case \"at the bounds\": the longest value must have been stored, got {after:?}"
    );
    let longest_member = longest_path
        .rsplit_once('.')
        .expect("the path has a member segment")
        .1;
    assert_eq!(
        details[longest_member],
        json!("x"),
        "case \"at the bounds\": the longest path must have been written, got {after:?}"
    );
    for index in 0..MAX_AMENDMENTS_PER_REQUEST {
        assert_eq!(
            details[format!("f{index}")],
            json!("x"),
            "case \"at the bounds\": amendment {index} of a full request must have applied, got {after:?}"
        );
    }
}

/// The two caller-chosen strings an amendment PERSISTS carry the lengths the model
/// gives them: at each bound the request applies, one past it is refused 400 and stores
/// nothing.
#[tokio::test]
async fn amend_bounds_the_strings_it_persists() {
    let harness = Harness::new();
    let created = seed(&harness, "seed", "tok-strings-seed", json!({ "Name": "demo" })).await;
    let id = text("seed", &created, "Id");

    // AT the bounds: the longest reason, and a token of the longest length.
    let longest_token = "t".repeat(MAX_CLIENT_TOKEN_LEN);
    let accepted: Vec<(&str, String, String)> = vec![
        (
            "a reason of exactly the longest length",
            "r".repeat(MAX_AMENDMENT_REASON_LEN),
            "tok-strings-reason".to_string(),
        ),
        (
            "a client token of exactly the longest length",
            "short reason".to_string(),
            longest_token,
        ),
    ];
    for (case, reason, token) in &accepted {
        let revision = text(case, &get(&harness, case, &id).await, "Revision");
        let mut body = amend_body(
            &id,
            &revision,
            token,
            json!([amendment("Name", "amended-at-the-bound")]),
        );
        body["AmendmentReason"] = json!(reason);
        ok(&harness, case, "AmendBenefitApplication", &body).await;
    }
    let before = get(&harness, "at the bounds", &id).await;

    // ONE PAST them: refused, and nothing stored.
    let refused: Vec<(&str, String, String)> = vec![
        (
            "a reason one character past its bound",
            "r".repeat(MAX_AMENDMENT_REASON_LEN + 1),
            "tok-strings-long-reason".to_string(),
        ),
        (
            "a client token one character past its bound",
            "short reason".to_string(),
            "t".repeat(MAX_CLIENT_TOKEN_LEN + 1),
        ),
    ];
    for (case, reason, token) in &refused {
        let revision = text(case, &before, "Revision");
        let mut body = amend_body(
            &id,
            &revision,
            token,
            json!([amendment("Name", "should-not-apply")]),
        );
        body["AmendmentReason"] = json!(reason);
        let (status, out) = harness.call("AmendBenefitApplication", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
        let after = get(&harness, case, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused amendment must change nothing, got {after:?}"
        );
    }
}

/// A document already holding the most members this service stores accepts an amendment
/// that CHANGES a member and refuses one that would ADD one — the bound that keeps
/// repeated amendments from growing one record without limit.
#[tokio::test]
async fn amend_refuses_a_member_past_the_document_member_bound() {
    let harness = Harness::new();
    let mut details = Map::new();
    for index in 0..MAX_DETAILS_MEMBERS {
        details.insert(format!("m{index}"), json!("x"));
    }
    let created = seed(
        &harness,
        "seed",
        "tok-members-seed",
        json!({ "BenefitApplicationDetails": Value::Object(details) }),
    )
    .await;
    let id = text("seed", &created, "Id");

    // Changing a member it already holds keeps the count, so it applies.
    let revision = text("seed", &created, "Revision");
    ok(
        &harness,
        "changing an existing member",
        "AmendBenefitApplication",
        &amend_body(
            &id,
            &revision,
            "tok-members-change",
            json!([amendment("BenefitApplicationDetails.m0", "y")]),
        ),
    )
    .await;
    let after = get(&harness, "changing an existing member", &id).await;
    assert_eq!(
        after["BenefitApplicationDetails"]["m0"],
        json!("y"),
        "case \"changing an existing member\": the amendment must have applied, got {after:?}"
    );

    // Adding one more would take the document past the bound, so it is refused.
    let (status, out) = harness
        .call(
            "AmendBenefitApplication",
            &amend_body(
                &id,
                &text("after the change", &after, "Revision"),
                "tok-members-add",
                json!([amendment("BenefitApplicationDetails.extra", "y")]),
            ),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"adding one member too many\": expected 400, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case \"adding one member too many\": expected ValidationException, got {out:?}"
    );
    let unchanged = get(&harness, "adding one member too many", &id).await;
    assert_eq!(
        unchanged, after,
        "case \"adding one member too many\": a refused amendment must change nothing, got {unchanged:?}"
    );
}

/// The same for the document's total text: an amendment that would take it past the
/// bound is refused, while one that keeps the size is applied.
#[tokio::test]
async fn amend_refuses_text_past_the_document_byte_bound() {
    let harness = Harness::new();
    let value = "v".repeat(MAX_DETAILS_VALUE_LEN);
    let mut details = Map::new();
    let mut bytes = 0usize;
    // Fill the document as close to the bound as whole members reach.
    for index in 0..MAX_DETAILS_MEMBERS {
        let name = format!("m{index}");
        let next = bytes + name.len() + value.len();
        if next > MAX_DETAILS_BYTES {
            break;
        }
        details.insert(name, json!(value));
        bytes = next;
    }
    assert!(
        bytes + MAX_DETAILS_VALUE_LEN > MAX_DETAILS_BYTES,
        "case \"a full document\": the fixture must sit within one member of the bound, got {bytes}"
    );
    let created = seed(
        &harness,
        "seed",
        "tok-bytes-seed",
        json!({ "BenefitApplicationDetails": Value::Object(details) }),
    )
    .await;
    let id = text("seed", &created, "Id");
    let revision = text("seed", &created, "Revision");

    // Rewriting a member with a value of the same length keeps the size, so it applies.
    ok(
        &harness,
        "a value of the same length",
        "AmendBenefitApplication",
        &amend_body(
            &id,
            &revision,
            "tok-bytes-same",
            json!([amendment("BenefitApplicationDetails.m0", &value)]),
        ),
    )
    .await;
    let after = get(&harness, "a value of the same length", &id).await;

    // One more member of the same size would pass the bound.
    let (status, out) = harness
        .call(
            "AmendBenefitApplication",
            &amend_body(
                &id,
                &text("after the rewrite", &after, "Revision"),
                "tok-bytes-more",
                json!([amendment("BenefitApplicationDetails.extra", &value)]),
            ),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"one member too many bytes\": expected 400, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case \"one member too many bytes\": expected ValidationException, got {out:?}"
    );
    let unchanged = get(&harness, "one member too many bytes", &id).await;
    assert_eq!(
        unchanged, after,
        "case \"one member too many bytes\": a refused amendment must change nothing, got {unchanged:?}"
    );
}

/// An amendment that does not name the application's current revision is a 409 and
/// changes nothing; a retry of one already applied answers success and changes nothing
/// either, even though the revision it carries is now stale.
#[tokio::test]
async fn amend_checks_the_revision_and_is_idempotent_in_its_client_token() {
    let harness = Harness::new();
    let created = seed(
        &harness,
        "seed",
        "tok-concurrency-seed",
        json!({ "Name": "demo" }),
    )
    .await;
    let id = text("seed", &created, "Id");
    let first_revision = text("seed", &created, "Revision");

    let request = amend_body(
        &id,
        &first_revision,
        "tok-concurrency",
        json!([amendment("Name", "amended-once")]),
    );
    ok(&harness, "first amendment", "AmendBenefitApplication", &request).await;
    let applied = get(&harness, "first amendment", &id).await;

    // The retry carries the revision the first call consumed, and still succeeds: the
    // ClientToken is how the caller says "this is the same request".
    ok(
        &harness,
        "retried amendment",
        "AmendBenefitApplication",
        &request,
    )
    .await;
    let retried = get(&harness, "retried amendment", &id).await;
    assert_eq!(
        retried, applied,
        "case \"retried amendment\": a retry must apply nothing a second time, got {retried:?}"
    );

    // A DIFFERENT token carrying a stale revision is a conflict.
    for stale in [
        first_revision.clone(),
        "0".to_string(),
        "99999".to_string(),
        "not-a-revision".to_string(),
    ] {
        let case = format!("revision {stale:?}");
        let (status, out) = harness
            .call(
                "AmendBenefitApplication",
                &amend_body(
                    &id,
                    &stale,
                    &format!("tok-stale-{stale}"),
                    json!([amendment("Name", "should-not-apply")]),
                ),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {out:?}"
        );
        let after = get(&harness, &case, &id).await;
        assert_eq!(
            after, applied,
            "case {case:?}: a refused amendment must change nothing, got {after:?}"
        );
    }

    // And the current revision, with a fresh token, applies.
    let current = text("second amendment", &applied, "Revision");
    ok(
        &harness,
        "second amendment",
        "AmendBenefitApplication",
        &amend_body(
            &id,
            &current,
            "tok-concurrency-2",
            json!([amendment("Name", "amended-twice")]),
        ),
    )
    .await;
    let after = get(&harness, "second amendment", &id).await;
    assert_eq!(
        after["Name"],
        json!("amended-twice"),
        "case \"second amendment\": the current revision must be accepted, got {after:?}"
    );
}

/// An amendment naming an application that does not exist is a 404 whose message names
/// the resource and the identifier the request named it with.
#[tokio::test]
async fn amend_reports_an_application_that_does_not_exist() {
    let harness = Harness::new();
    seed(&harness, "seed", "tok-missing-seed", json!({ "Name": "demo" })).await;

    let rows: Vec<(&str, String)> = vec![
        (
            "a well-formed id naming nothing",
            UNKNOWN_APPLICATION.to_string(),
        ),
        (
            "an ARN naming nothing",
            application_arn("benappl-00000000000001"),
        ),
    ];

    for (index, (case, identifier)) in rows.iter().enumerate() {
        let (status, out) = harness
            .call(
                "AmendBenefitApplication",
                &amend_body(
                    identifier,
                    "1",
                    &format!("tok-missing-{index}"),
                    json!([amendment("Name", "should-not-apply")]),
                ),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {out:?}"
        );
        assert_eq!(
            message_of(&out),
            not_found_message(APPLICATION_KIND, identifier),
            "case {case:?}: expected the not-found sentence, got {out:?}"
        );
    }
}
