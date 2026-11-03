// SPDX-License-Identifier: CC0-1.0

//! How surrounding whitespace in the token a submission names its type by bears
//! on the identity of the exemption record, end to end over the restJson1 wire.
//!
//! The rule under test: `PutTaxExemption` is a `Put` whose record is identified
//! by the type and the jurisdiction the submission names (the request carries no
//! exemption id), and NEITHER member of that identity is distinguished by ASCII
//! case or by surrounding whitespace — a submitted token is placed and matched
//! trimmed and case-folded, the way the store already places a token in the
//! collection `GetTaxExemptionTypes` publishes and stores a jurisdiction. So a
//! submission whose token differs from a held record's only in case or in padding
//! REPLACES that record and takes no second slot of the resident bound, while a
//! token that differs INSIDE, or the same token in another jurisdiction, is
//! another exemption.
//!
//! The first table spans that class along what the rule generalises over: the
//! kind of token (one the published collection names by either of its
//! identifying members, one it does not, one character, a token at the
//! display-name bound, a token carrying non-ASCII letters), the whitespace form
//! it is padded with (none, leading, trailing, both, tab, newline, mixed) and the
//! case it is re-spelled in. The second table is the rule's other side: the
//! differences that DO make a second record. Every row is read back through both
//! exemption reads on a second mount of the service, so what it asserts came from
//! the service's state, and against the resident count the bound is kept on.
//!
//! The harness (`emulator_with_engine`, `mount_on`, `call`) lives in
//! `tax_registration_lifecycle.rs`, and the certificate a submission attaches and
//! the reader of a reported exemption in `tax_exemptions.rs`; nothing here builds
//! a second one.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    find_exemption_type, tax_exemption_type_catalog, ComAmazonawsTaxsettingsTaxsettingsState,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_exemptions::{certificate, reported_exemptions};
use crate::tax_registration_lifecycle::{call, emulator_with_engine, mount_on};

/// The longest `TaxExemptionType$displayName` the model admits: `@length` 0..50
/// (`smithy_query(query="shape", shape="TaxExemptionType")`).
const DISPLAY_NAME_BOUND: usize = 50;

/// The whitespace forms a submission may surround its token with — what
/// `str::trim` strips, which is what the store compares the token without.
const PADDINGS: &[(&str, &str, &str)] = &[
    ("unpadded", "", ""),
    ("a trailing space", "", " "),
    ("a leading space", " ", ""),
    ("spaces on both sides", "  ", "  "),
    ("a leading tab", "\t", ""),
    ("a trailing newline", "", "\n"),
    ("mixed whitespace on both sides", " \t", "\r\n "),
];

/// Submit one exemption for `account` and answer what the call reported.
async fn submit(
    router: &MultiServiceRouter,
    account: &str,
    token: &str,
    authority: &Value,
) -> (u16, String) {
    let answer = call(
        router,
        "PutTaxExemption",
        &json!({
            "accountIds": [account],
            "authority": authority,
            "exemptionType": token,
            "exemptionCertificate": certificate("certificate.pdf"),
        }),
    )
    .await;
    (answer.status, answer.error_code.clone())
}

/// The ways one token may be re-spelled without naming another type: every
/// padding above, and — for a token the store's ASCII case folding covers whole —
/// the same token upper- and lower-cased.
fn restatements(token: &str) -> Vec<(String, String)> {
    let spellings: Vec<(&str, String)> = if token.is_ascii() {
        vec![
            ("as first submitted", token.to_string()),
            ("upper-cased", token.to_uppercase()),
            ("lower-cased", token.to_lowercase()),
        ]
    } else {
        vec![("as first submitted", token.to_string())]
    };
    let mut rows = Vec::new();
    for (padding, before, after) in PADDINGS {
        for (spelling, spelled) in &spellings {
            rows.push((
                format!("{spelling}, {padding}"),
                format!("{before}{spelled}{after}"),
            ));
        }
    }
    rows
}

/// The `description` the record a submission of `token` is held under reports:
/// the published entry's own where the token names one, else the token as the
/// submission spelled it.
fn expected_description(token: &str) -> String {
    find_exemption_type(token)
        .map(|(_, description, _)| (*description).to_string())
        .unwrap_or_else(|| token.to_string())
}

/// How many exemption records the store holds across every account — the count
/// the resident bound is kept on, so a replacement that took a second slot shows
/// here even where one account's reads do not.
fn resident_records(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsTaxsettingsTaxsettingsState>()
        .lock()
        .stored_exemption_count()
}

/// The exemptions both reads report for one account, asserted to agree: the
/// per-account read and the listing project the same records.
async fn held_by_both_reads(
    engine: &Arc<engine::InMemoryEngine>,
    account: &str,
    case: &str,
) -> Vec<Value> {
    // A SECOND mount of the service on the same engine, built after the writes:
    // anything it reports came from the service's state.
    let reader = mount_on(engine);
    let fetched = call(
        &reader,
        "BatchGetTaxExemptions",
        &json!({ "accountIds": [account] }),
    )
    .await;
    assert_eq!(
        (fetched.status, fetched.error_code.as_str()),
        (200, ""),
        "case {case:?}: expected the records to be readable, got {fetched:?}"
    );
    let listed = call(&reader, "ListTaxExemptions", &json!({})).await;
    assert_eq!(
        (listed.status, listed.error_code.as_str()),
        (200, ""),
        "case {case:?}: expected the records to be listable, got {listed:?}"
    );
    let fetched = reported_exemptions(&fetched, account);
    let listed = reported_exemptions(&listed, account);
    assert_eq!(
        fetched, listed,
        "case {case:?}: both reads report the same records, got {fetched:?} and {listed:?}"
    );
    fetched
}

/// The tokens the identity table submits first: a case name and the
/// `exemptionType` the first submission names.
///
/// The two rows that name a published entry take their token FROM the collection
/// the service publishes, so the table covers whatever that collection holds
/// rather than restating its wording.
fn first_tokens() -> Vec<(String, String)> {
    let catalog = tax_exemption_type_catalog();
    let published = catalog
        .first()
        .cloned()
        .expect("the service publishes at least one exemption type");
    let description = published
        .description
        .clone()
        .expect("a published entry carries a description");
    let display_name = published
        .display_name
        .clone()
        .expect("a published entry carries a display name");

    vec![
        (
            "a published entry named by its description".to_string(),
            description,
        ),
        (
            "a published entry named by its display name".to_string(),
            display_name,
        ),
        (
            "a token the collection does not name".to_string(),
            "Exemption certificate on file".to_string(),
        ),
        ("one character".to_string(), "x".to_string()),
        (
            format!("a token at the display-name bound of {DISPLAY_NAME_BOUND} characters"),
            "T".repeat(DISPLAY_NAME_BOUND),
        ),
        (
            "a token carrying non-ASCII letters".to_string(),
            "Befreiung für Wiederverkäufer".to_string(),
        ),
    ]
}

/// RULE: padding and ASCII case in the token a submission names its type by make
/// no second exemption — a re-spelled submission of a held record's type, in that
/// record's jurisdiction, replaces it in place and takes no further slot of the
/// resident bound.
#[tokio::test]
async fn padding_and_case_in_the_type_make_no_second_exemption() {
    let account = "000000000031";
    let jurisdiction = json!({ "country": "US", "state": "OH" });
    for (token_case, token) in first_tokens() {
        for (spelling, resubmitted) in restatements(&token) {
            let case = format!("{token_case}, resubmitted {spelling}");
            let (router, engine) = emulator_with_engine();

            let first = submit(&router, account, &token, &jurisdiction).await;
            assert_eq!(
                (first.0, first.1.as_str()),
                (200, ""),
                "case {case:?}: expected the first submission to be applied, got {first:?}"
            );
            let second = submit(&router, account, &resubmitted, &jurisdiction).await;
            assert_eq!(
                (second.0, second.1.as_str()),
                (200, ""),
                "case {case:?}: expected the resubmission to be applied, got {second:?}"
            );

            let held = held_by_both_reads(&engine, account, &case).await;
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected the resubmission of {resubmitted:?} to replace the one \
                 record of {token:?}, got {held:?}"
            );
            assert_eq!(
                resident_records(&engine),
                1,
                "case {case:?}: a replacement takes no second slot of the resident bound, got {}",
                resident_records(&engine)
            );

            let reported = held[0]
                .get("taxExemptionType")
                .and_then(|held_type| held_type.get("description"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a description, got {held:?}"))
                .to_string();
            let expected = expected_description(&resubmitted);
            assert!(
                reported.trim().eq_ignore_ascii_case(expected.trim()),
                "case {case:?}: expected the record held under {expected:?}, got {reported:?}"
            );
            assert_eq!(
                held[0].get("authority").cloned().unwrap_or(Value::Null),
                jurisdiction,
                "case {case:?}: expected the submitted jurisdiction, got {held:?}"
            );
        }
    }
}

/// One row of the table of differences that DO make a second exemption: the two
/// submissions, each a token and the jurisdiction it is submitted in, and how
/// many records the account then holds.
struct Distinct {
    case: &'static str,
    first_token: String,
    first_authority: Value,
    second_token: String,
    second_authority: Value,
    expected: usize,
}

/// RULE's other side: a difference the trimmed, case-folded comparison keeps —
/// whitespace INSIDE the token, another token, or another jurisdiction — is
/// another exemption, recorded beside the first.
#[tokio::test]
async fn a_difference_the_identity_keeps_adds_a_record() {
    let account = "000000000032";
    let token = "Exemption certificate on file";
    let country_only = json!({ "country": "US" });
    let with_state = json!({ "country": "US", "state": "OH" });
    let rows = vec![
        Distinct {
            case: "an extra space inside the token",
            first_token: token.to_string(),
            first_authority: country_only.clone(),
            second_token: token.replacen("on file", "on  file", 1),
            second_authority: country_only.clone(),
            expected: 2,
        },
        Distinct {
            case: "the token's inner spaces removed",
            first_token: token.to_string(),
            first_authority: country_only.clone(),
            second_token: token.replace(' ', ""),
            second_authority: country_only.clone(),
            expected: 2,
        },
        Distinct {
            case: "another token entirely",
            first_token: token.to_string(),
            first_authority: country_only.clone(),
            second_token: "Exemption certificate pending".to_string(),
            second_authority: country_only.clone(),
            expected: 2,
        },
        Distinct {
            case: "the padded token in another jurisdiction",
            first_token: token.to_string(),
            first_authority: country_only.clone(),
            second_token: format!("  {token}  "),
            second_authority: with_state.clone(),
            expected: 2,
        },
        // The control: the same token, padded, in the SAME jurisdiction — the
        // one difference the identity does not keep.
        Distinct {
            case: "the padded token in the same jurisdiction",
            first_token: token.to_string(),
            first_authority: with_state.clone(),
            second_token: format!("  {token}  "),
            second_authority: with_state.clone(),
            expected: 1,
        },
    ];

    for row in rows {
        let case = row.case;
        let expected = row.expected;
        let (router, engine) = emulator_with_engine();
        for (token, authority) in [
            (&row.first_token, &row.first_authority),
            (&row.second_token, &row.second_authority),
        ] {
            let answer = submit(&router, account, token, authority).await;
            assert_eq!(
                (answer.0, answer.1.as_str()),
                (200, ""),
                "case {case:?}: expected the submission of {token:?} to be applied, got {answer:?}"
            );
        }
        let held = held_by_both_reads(&engine, account, case).await;
        assert_eq!(
            held.len(),
            expected,
            "case {case:?}: expected {expected} record(s) for {:?} and {:?}, got {held:?}",
            row.first_token,
            row.second_token
        );
        assert_eq!(
            resident_records(&engine),
            expected,
            "case {case:?}: the resident count is the records held, got {}",
            resident_records(&engine)
        );
    }
}
