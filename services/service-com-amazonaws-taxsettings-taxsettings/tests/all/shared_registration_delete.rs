// SPDX-License-Identifier: CC0-1.0

//! The one single-account delete decision, driven over the restJson1 wire
//! through BOTH operations that take it.
//!
//! RULE: deleting ONE account's tax registration is ONE decision — an account
//! holding no record is refused as a missing resource, a record held in a
//! jurisdiction this API cannot remove one from is refused as a validation
//! failure with its record left readable, and any other record is removed — and
//! `DeleteTaxRegistration` and the per-account loop of
//! `BatchDeleteTaxRegistration` both take that one decision, each only rendering
//! its refusal on its own channel: a modelled error response, or a row of the
//! batch's `errors` list carrying the same code and the same message. So the two
//! paths cannot answer for one record differently, and cannot leave the store in
//! different states.
//!
//! The rows span what the decision generalises over: whether the account holds a
//! record at all, whether the jurisdiction it is held in is carved out of the API
//! delete (every entry of
//! `crate::state::COUNTRIES_WITH_NON_API_REGISTRATION_DELETE`, read from the
//! catalog so a jurisdiction added to it is covered here, and several
//! jurisdictions that are not in it), the letter case the `countryCode` was
//! submitted in, the registration type held under it (including a carved-out
//! jurisdiction's own type held elsewhere, which is removable), and whether the
//! decision has already answered for that account once — each row is deleted
//! twice down each path.
//!
//! What each refusal's own wording and `fieldList` are is a separate rule, with
//! its own suites in `tests/all/non_api_delete_jurisdictions.rs` and
//! `tests/all/missing_resource_errors.rs`; what this suite pins is that one
//! decision answers both paths.
//!
//! The harness (`emulator`, `call`, `Answer`) lives in
//! `tax_registration_lifecycle.rs` and the batch error-row reader in
//! `pending_trn_conflict.rs`; nothing here builds a second copy of either.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    non_api_delete_country, COUNTRIES_WITH_NON_API_REGISTRATION_DELETE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::pending_trn_conflict::error_rows;
use crate::tax_registration_lifecycle::{call, emulator, Answer};

/// The account every row stores its record under and every delete addresses.
/// Which account holds the record is not a dimension of this rule.
const ACCOUNT_ID: &str = "000000000007";

/// The two operations that take the single-account delete decision — the two
/// former copies of it.
const DELETE_PATHS: &[&str] = &["DeleteTaxRegistration", "BatchDeleteTaxRegistration"];

/// What the decision answers for one account.
#[derive(Clone, Copy, Debug)]
enum Decision {
    /// The record is removed.
    Removed,
    /// The account holds no record to remove: the missing-resource class.
    MissingResource,
    /// The record is held in a jurisdiction this API cannot remove one from: the
    /// validation class.
    NotDeletable,
}

impl Decision {
    /// The name of the modelled error `DeleteTaxRegistration` renders this
    /// decision as, which is also the `code` of the batch's per-account row.
    /// Empty when the record is removed, because a removal is refused by
    /// neither path.
    const fn code(self) -> &'static str {
        match self {
            Self::Removed => "",
            Self::MissingResource => "ResourceNotFoundException",
            Self::NotDeletable => "ValidationException",
        }
    }

    /// The HTTP status `DeleteTaxRegistration` answers with. The batch answers
    /// 200 whatever it decides per account: its refusals ride in `errors`.
    const fn single_account_status(self) -> u16 {
        match self {
            Self::Removed => 200,
            Self::MissingResource => 404,
            Self::NotDeletable => 400,
        }
    }

    /// Whether the account still holds its record once the decision has
    /// answered: a refused record is left exactly as it was.
    const fn leaves_record(self) -> bool {
        matches!(self, Self::NotDeletable)
    }

    /// The decision a SECOND delete of the same account meets: a removed record
    /// leaves the account holding nothing, and a refused one leaves everything
    /// as it was, so the refusal repeats.
    const fn again(self) -> Self {
        match self {
            Self::Removed => Self::MissingResource,
            other => other,
        }
    }
}

/// The record an account holds before the delete: the registration type, and the
/// jurisdiction its legal address is submitted with.
struct Holding {
    registration_type: &'static str,
    country_code: String,
}

/// One account state a delete is aimed at.
struct Row {
    case: String,
    /// `None` is an account holding nothing at all.
    holds: Option<Holding>,
}

impl Row {
    /// What the decision answers for this row's account, derived from the rule
    /// rather than from either handler: nothing held is the missing-resource
    /// class, a record held in a carved-out jurisdiction is the validation
    /// class, and anything else is removed.
    fn decision(&self) -> Decision {
        match &self.holds {
            None => Decision::MissingResource,
            Some(holding) => {
                if non_api_delete_country(&holding.country_code).is_some() {
                    Decision::NotDeletable
                } else {
                    Decision::Removed
                }
            }
        }
    }

    /// Store this row's record, if it holds one.
    async fn store(&self, router: &MultiServiceRouter) {
        let Some(holding) = &self.holds else {
            return;
        };
        let written = call(
            router,
            "PutTaxRegistration",
            &json!({
                "accountId": ACCOUNT_ID,
                "taxRegistrationEntry": entry(holding),
            }),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {:?}: storing the record must succeed, got {written:?}",
            self.case
        );
    }
}

/// A `TaxRegistrationEntry` for `holding`. Only the members every entry needs
/// are filled in, plus `stateOrRegion`, which one jurisdiction in these rows
/// documents as required and every other admits as optional
/// (`crate::state::COUNTRY_RULES`, whose per-country requirements
/// `tests/all/country_requirements.rs` covers).
fn entry(holding: &Holding) -> Value {
    json!({
        "registrationId": format!("{}-0000000001", holding.registration_type),
        "registrationType": holding.registration_type,
        "legalName": "Example Holdings",
        "legalAddress": {
            "city": "Example City",
            "stateOrRegion": "Example Region",
            "postalCode": "01001-000",
            "countryCode": holding.country_code,
        },
    })
}

/// The account states the decision distinguishes.
///
/// The carved-out rows are read from
/// `crate::state::COUNTRIES_WITH_NON_API_REGISTRATION_DELETE` — the catalog the
/// rule is keyed on — so each entry of it is driven down both paths, in the
/// letter case it publishes and in lower case. The rows that are NOT carved out
/// name jurisdictions absent from that catalog, one of them holding a type the
/// carved-out jurisdiction uses, because the key is the country the record is
/// held in rather than the type it is registered under.
fn rows() -> Vec<Row> {
    let mut rows = vec![Row {
        case: "an account holding no record".to_string(),
        holds: None,
    }];
    for (country_code, country) in COUNTRIES_WITH_NON_API_REGISTRATION_DELETE {
        rows.push(Row {
            case: format!("a CNPJ record held in {country}"),
            holds: Some(Holding {
                registration_type: "CNPJ",
                country_code: (*country_code).to_string(),
            }),
        });
        rows.push(Row {
            case: format!("a TIN record held in {country}, submitted in lower case"),
            holds: Some(Holding {
                registration_type: "TIN",
                country_code: country_code.to_lowercase(),
            }),
        });
    }
    for (registration_type, country_code, case) in [
        ("VAT", "DE", "a VAT record held in another jurisdiction"),
        (
            "NIP",
            "pl",
            "a NIP record in another jurisdiction, submitted in lower case",
        ),
        (
            "CNPJ",
            "PT",
            "a carved-out jurisdiction's own type held in another jurisdiction",
        ),
    ] {
        assert!(
            non_api_delete_country(country_code).is_none(),
            "case {case:?}: this row's jurisdiction must not be a carved-out one"
        );
        rows.push(Row {
            case: case.to_string(),
            holds: Some(Holding {
                registration_type,
                country_code: country_code.to_string(),
            }),
        });
    }
    rows
}

/// What one delete answered, in the terms both paths share: the code the refusal
/// carries (empty when the record was removed), its message, and the
/// registration type the account still reports afterwards.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    code: String,
    message: String,
    still_held: Option<String>,
}

/// The `registrationType` a read of [`ACCOUNT_ID`] reports, or `None` when the
/// account reports no registration.
async fn still_held(router: &MultiServiceRouter, case: &str) -> Option<String> {
    let read = call(
        router,
        "GetTaxRegistration",
        &json!({ "accountId": ACCOUNT_ID }),
    )
    .await;
    let missing = (read.status, read.error_code.as_str()) == (404, "ResourceNotFoundException");
    assert!(
        read.status == 200 || missing,
        "case {case:?}: a read either reports the record or answers 404 for a missing one, got \
         {read:?}"
    );
    read.get("taxRegistration")
        .get("registrationType")
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string)
}

/// The message a refusal carries, empty when the answer carries none.
fn message_of(answer: &Answer) -> String {
    answer
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Delete [`ACCOUNT_ID`] down `path`, as that operation renders the decision.
async fn delete_through(router: &MultiServiceRouter, path: &str, case: &str) -> Outcome {
    if path == "BatchDeleteTaxRegistration" {
        let answer = call(router, path, &json!({ "accountIds": [ACCOUNT_ID] })).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: a batch delete answers 200 whatever it decides per account, got \
             {answer:?}"
        );
        let rows = error_rows(&answer);
        assert!(
            rows.len() <= 1,
            "case {case:?}: a batch naming one account reports at most one row, got {rows:?}"
        );
        let (code, message) = match rows.first() {
            Some((account_id, code, message)) => {
                assert_eq!(
                    account_id, ACCOUNT_ID,
                    "case {case:?}: a row must name the account it refused, got {account_id:?}"
                );
                (code.clone(), message.clone())
            }
            None => (String::new(), String::new()),
        };
        return Outcome {
            code,
            message,
            still_held: still_held(router, case).await,
        };
    }

    let answer = call(router, path, &json!({ "accountId": ACCOUNT_ID })).await;
    let message = if answer.error_code.is_empty() {
        String::new()
    } else {
        message_of(&answer)
    };
    Outcome {
        code: answer.error_code.clone(),
        message,
        still_held: still_held(router, case).await,
    }
}

/// RULE: both delete paths take the one single-account delete decision — the same
/// code, the same message and the same post-state for one record, whichever path
/// asked, and the same again once that decision has already answered for the
/// account once.
#[tokio::test]
async fn both_delete_paths_take_the_one_delete_decision() {
    for row in rows() {
        let decisions = [row.decision(), row.decision().again()];
        let mut per_path: Vec<Vec<Outcome>> = Vec::new();
        for path in DELETE_PATHS {
            let router = emulator();
            row.store(&router).await;
            let mut outcomes = Vec::new();
            for (attempt, decision) in decisions.iter().copied().enumerate() {
                let case = format!("{} deleted through {path} (delete {attempt})", row.case);
                let outcome = delete_through(&router, path, &case).await;
                assert_eq!(
                    outcome.code,
                    decision.code(),
                    "case {case:?}: expected the {decision:?} code {:?}, got {outcome:?}",
                    decision.code()
                );
                let expected_held = decision
                    .leaves_record()
                    .then(|| {
                        row.holds
                            .as_ref()
                            .map(|holding| holding.registration_type.to_string())
                    })
                    .flatten();
                assert_eq!(
                    outcome.still_held, expected_held,
                    "case {case:?}: a refused record is left as it was and a removed one is gone, \
                     so the account must report {expected_held:?}, got {:?}",
                    outcome.still_held
                );
                if let Some(country) = row
                    .holds
                    .as_ref()
                    .and_then(|holding| non_api_delete_country(&holding.country_code))
                {
                    assert!(
                        outcome.message.contains(country),
                        "case {case:?}: the refusal must state the jurisdiction {country}, got \
                         {:?}",
                        outcome.message
                    );
                }
                outcomes.push(outcome);
            }
            per_path.push(outcomes);
        }
        assert_eq!(
            per_path[0], per_path[1],
            "case {:?}: both delete paths must answer for one record identically, got {per_path:?}",
            row.case
        );
    }
}

/// RULE (single-account path): the decision's refusals carry the status class
/// each one's modelled error declares — the missing resource 404 and the
/// validation 400 — while a removal answers 200 with an empty body.
#[tokio::test]
async fn the_single_account_path_renders_each_decision_on_its_own_status() {
    for row in rows() {
        let decision = row.decision();
        let case = format!("{} deleted through DeleteTaxRegistration", row.case);
        let router = emulator();
        row.store(&router).await;

        let answer = call(
            &router,
            "DeleteTaxRegistration",
            &json!({ "accountId": ACCOUNT_ID }),
        )
        .await;
        assert_eq!(
            answer.status,
            decision.single_account_status(),
            "case {case:?}: expected HTTP {} for {decision:?}, got {answer:?}",
            decision.single_account_status()
        );
        assert_eq!(
            answer.error_code,
            decision.code(),
            "case {case:?}: expected the error code {:?}, got {:?}",
            decision.code(),
            answer.error_code
        );
        if matches!(decision, Decision::Removed) {
            assert_eq!(
                answer.body,
                json!({}),
                "case {case:?}: a removal carries no response members, got {:?}",
                answer.body
            );
        } else {
            assert!(
                !message_of(&answer).is_empty(),
                "case {case:?}: a refusal must state why, got {:?}",
                answer.body
            );
        }
    }
}
