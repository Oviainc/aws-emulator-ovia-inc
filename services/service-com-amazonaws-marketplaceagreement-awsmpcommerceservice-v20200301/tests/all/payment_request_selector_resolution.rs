// SPDX-License-Identifier: CC0-1.0

//! `ListAgreementPaymentRequests` resolves its selectors BEFORE it selects, over
//! a store that actually holds payment requests.
//!
//! The rule under test: a listing selector whose admissible values this service
//! fixes — `catalog`, which names a catalog AWS Marketplace exposes (it exposes
//! exactly one), and `agreementType`, which names one of the types the model
//! documents — or that must name a resource the account holds (`agreementId`) is
//! resolved against that set before any stored record is read, so a value
//! outside it is refused with HTTP 400 `ValidationException` carrying that
//! member's `INVALID_<MEMBER>` reason and naming the member in `fields[0]`,
//! whatever the store holds. A value INSIDE the set stays a filter: it reports
//! whatever carries it, and an empty page is then a legitimate answer.
//!
//! `selector_resolution.rs` pins the same rule across every operation of this
//! crate that carries one of those members, including on an empty store. This
//! suite is the other half of the same rule for the operation it was reported
//! against: the store here holds payment requests the call WOULD have reported,
//! for both party types, so a refusal cannot be the accident of an empty page —
//! and the refusal is shown to leave the list a later caller sees untouched.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{emulator, Answer};
use crate::payment_requests::{agreement_with_variable_term, items, list, sent};
use crate::selector_resolution::{
    assert_refused, unknown_agreement_ids, AGREEMENT_TYPES, CATALOG, OTHER_AGREEMENT_TYPES,
    OTHER_CATALOGS,
};

/// The `agreementType` this service mints, and the other one the model
/// documents, from `selector_resolution::AGREEMENT_TYPES`: the first is what
/// every agreement here carries, the second a value the member admits that no
/// stored agreement carries, so it selects nothing rather than being refused.
const AGREEMENT_TYPE: &str = AGREEMENT_TYPES[0];
const OTHER_AGREEMENT_TYPE: &str = AGREEMENT_TYPES[1];

/// The status `SendAgreementPaymentRequest` documents a new payment request is
/// "created in", so every request this suite stores carries it: an admitted
/// selector that selects all of them.
const PENDING_APPROVAL: &str = "PENDING_APPROVAL";

/// Both values of `partyType`, from
/// `smithy_query(query="doc", shape="ListAgreementPaymentRequestsInput",
/// member="partyType")`: "Use `Proposer` to list payment requests where you are
/// the seller, or `Acceptor` to list payment requests where you are the buyer."
/// Each selects every payment request this suite stores — the account is the
/// submitter of each and the acceptor of the agreement it was sent under — so
/// each is a party type whose page is non-empty.
const PARTY_TYPES: &[&str] = &["Proposer", "Acceptor"];

/// An `agreementId` no agreement of a store can carry: `AgreementId` admits it
/// (`@length(1, 64)`, `^[A-Za-z0-9_/-]+$`) but nothing mints it, so it is the
/// id used where a row needs ONE unknown id rather than every way of missing —
/// `unknown_agreement_ids` spans those.
const UNKNOWN_AGREEMENT_ID: &str = "agmt-no-such-agreement";

/// What a row expects: a refusal naming a member, or a page holding exactly the
/// payment requests at these indices of what the suite stored.
enum Expected {
    Refused {
        member: &'static str,
        reason: &'static str,
    },
    Listed(Vec<usize>),
}

/// One row: the selectors added to the required `partyType`, and what the
/// operation must answer.
struct Case {
    case: String,
    selector: Value,
    expected: Expected,
}

/// The `paymentRequestId`s one page reported, in its order.
fn listed_ids(answer: &Answer) -> Vec<String> {
    items(answer)
        .iter()
        .map(|item| {
            item["paymentRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// List with `party_type` and `selector`.
async fn listed(router: &MultiServiceRouter, party_type: &str, selector: &Value) -> Answer {
    let mut body = json!({"partyType": party_type});
    for (member, value) in selector.as_object().expect("a selector is an object") {
        body[member] = value.clone();
    }
    list(router, body).await
}

/// Check one row against what the operation answered.
fn assert_answered(case: &str, expected: &Expected, stored: &[String], answer: &Answer) {
    match expected {
        Expected::Refused { member, reason } => {
            // The refusal envelope itself — the status, the error code, the
            // reason, the member named in `fields[0]` and the absent page — is
            // the one `selector_resolution` asserts for every operation of this
            // crate, so this suite checks it through that same helper rather
            // than a second copy that could drift from it.
            assert_refused(case, member, reason, answer);
            assert!(
                answer.body.get("nextToken").is_none(),
                "case {case:?}: a refused listing hands out no page token, got {:?}",
                answer.body
            );
        }
        Expected::Listed(indices) => {
            let expected_ids: Vec<String> = indices
                .iter()
                .map(|index| stored[*index].clone())
                .collect();
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                listed_ids(answer),
                expected_ids,
                "case {case:?}: expected {expected_ids:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// Every refused and every admitted selector value, over a store that holds
/// three payment requests under two agreements, for both party types.
#[tokio::test]
async fn a_selector_is_resolved_before_any_stored_payment_request_is_selected() {
    let router = emulator();
    let alpha = agreement_with_variable_term(&router, "ap-prsalpha", "vpt-alpha").await;
    let beta = agreement_with_variable_term(&router, "ap-prsbeta", "vpt-beta").await;
    let stored = vec![
        sent(&router, &alpha, "vpt-alpha", "Alpha first", "10").await,
        sent(&router, &alpha, "vpt-alpha", "Alpha second", "20.5").await,
        sent(&router, &beta, "vpt-beta", "Beta only", "0.125").await,
    ];

    let mut cases: Vec<Case> = vec![
        Case {
            case: "no selector reports every stored request".to_string(),
            selector: json!({}),
            expected: Expected::Listed(vec![0, 1, 2]),
        },
        Case {
            case: "the published catalog reports every stored request".to_string(),
            selector: json!({"catalog": CATALOG}),
            expected: Expected::Listed(vec![0, 1, 2]),
        },
        Case {
            case: "the agreement type these agreements carry reports all of them".to_string(),
            selector: json!({"agreementType": AGREEMENT_TYPE}),
            expected: Expected::Listed(vec![0, 1, 2]),
        },
        Case {
            case: "the other documented agreement type is admitted and selects nothing"
                .to_string(),
            selector: json!({"agreementType": OTHER_AGREEMENT_TYPE}),
            expected: Expected::Listed(vec![]),
        },
        Case {
            case: "a stored agreement reports the requests sent under it".to_string(),
            selector: json!({"agreementId": alpha.clone()}),
            expected: Expected::Listed(vec![0, 1]),
        },
        Case {
            case: "another stored agreement reports only its own".to_string(),
            selector: json!({"agreementId": beta.clone()}),
            expected: Expected::Listed(vec![2]),
        },
        Case {
            case: "every admitted selector at once reports what carries them all".to_string(),
            selector: json!({
                "agreementId": alpha.clone(),
                "agreementType": AGREEMENT_TYPE,
                "catalog": CATALOG,
                "status": PENDING_APPROVAL,
            }),
            expected: Expected::Listed(vec![0, 1]),
        },
    ];

    for catalog in OTHER_CATALOGS {
        cases.push(Case {
            case: format!("the catalog {catalog:?} no marketplace exposes"),
            selector: json!({"catalog": catalog}),
            expected: Expected::Refused {
                member: "catalog",
                reason: "INVALID_CATALOG",
            },
        });
    }
    for agreement_type in OTHER_AGREEMENT_TYPES {
        cases.push(Case {
            case: format!("the agreement type {agreement_type:?} outside the documented pair"),
            selector: json!({"agreementType": agreement_type}),
            expected: Expected::Refused {
                member: "agreementType",
                reason: "INVALID_AGREEMENT_TYPE",
            },
        });
    }
    for agreement_id in unknown_agreement_ids(&alpha) {
        cases.push(Case {
            case: format!("the agreement id {agreement_id:?} naming no agreement"),
            selector: json!({"agreementId": agreement_id}),
            expected: Expected::Refused {
                member: "agreementId",
                reason: "INVALID_AGREEMENT_ID",
            },
        });
    }
    // A refused value is refused even where every OTHER selector of the call
    // would have reported rows: the refusal is not an empty page in disguise.
    cases.push(Case {
        case: "an unknown agreement beside selectors that would have reported rows".to_string(),
        selector: json!({
            "agreementId": UNKNOWN_AGREEMENT_ID,
            "agreementType": AGREEMENT_TYPE,
            "catalog": CATALOG,
            "status": PENDING_APPROVAL,
        }),
        expected: Expected::Refused {
            member: "agreementId",
            reason: "INVALID_AGREEMENT_ID",
        },
    });
    cases.push(Case {
        case: "an unpublished catalog beside a stored agreement".to_string(),
        selector: json!({"agreementId": alpha.clone(), "catalog": OTHER_CATALOGS[2]}),
        expected: Expected::Refused {
            member: "catalog",
            reason: "INVALID_CATALOG",
        },
    });
    cases.push(Case {
        case: "an undocumented agreement type beside a stored agreement".to_string(),
        selector: json!({"agreementId": beta.clone(), "agreementType": OTHER_AGREEMENT_TYPES[4]}),
        expected: Expected::Refused {
            member: "agreementType",
            reason: "INVALID_AGREEMENT_TYPE",
        },
    });

    for party_type in PARTY_TYPES {
        for Case {
            case,
            selector,
            expected,
        } in &cases
        {
            let case = format!("partyType {party_type:?}: {case}");
            let answer = listed(&router, party_type, selector).await;
            assert_answered(&case, expected, &stored, &answer);
        }
    }

    // A refusal reports nothing and changes nothing: the unselected list a later
    // caller reads is the one it was before any of the rows above ran.
    for party_type in PARTY_TYPES {
        let answer = listed(&router, party_type, &json!({})).await;
        assert_eq!(
            listed_ids(&answer),
            stored,
            "partyType {party_type:?}: a refused call leaves the list unchanged, got {:?}",
            answer.body
        );
    }
}

/// The refusal does not depend on the party type the call names, nor on a page
/// size or a token it carries: the selector is resolved before either is read.
#[tokio::test]
async fn a_refused_selector_is_refused_whatever_else_the_call_carries() {
    let router = emulator();
    let agreement = agreement_with_variable_term(&router, "ap-prspaged", "vpt-paged").await;
    for index in 0..3 {
        sent(
            &router,
            &agreement,
            "vpt-paged",
            &format!("Payment number {index}"),
            &format!("{}.5", index + 1),
        )
        .await;
    }

    // A token minted by an accepted call: `maxResults` 1 over three stored
    // requests leaves more to report.
    let first = listed(&router, "Acceptor", &json!({"maxResults": 1})).await;
    let token = first.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a bounded page leaves a token: {:?}", first.body))
        .to_string();

    // `maxResults` spans the published bounds of `MaxResults` (`@range(min: 1,
    // max: 50)`), and the token is the one the list itself minted.
    let extras = [
        json!({}),
        json!({"maxResults": 1}),
        json!({"maxResults": 50}),
        json!({"nextToken": token}),
    ];
    let refused = [
        (
            json!({"catalog": OTHER_CATALOGS[0]}),
            "catalog",
            "INVALID_CATALOG",
        ),
        (
            json!({"agreementType": OTHER_AGREEMENT_TYPES[0]}),
            "agreementType",
            "INVALID_AGREEMENT_TYPE",
        ),
        (
            json!({"agreementId": UNKNOWN_AGREEMENT_ID}),
            "agreementId",
            "INVALID_AGREEMENT_ID",
        ),
    ];

    for party_type in PARTY_TYPES {
        for (selector, member, reason) in &refused {
            for extra in &extras {
                let mut combined = selector.clone();
                for (name, value) in extra.as_object().expect("an extra is an object") {
                    combined[name] = value.clone();
                }
                let case = format!("partyType {party_type:?}: {selector} beside {extra}");
                let answer = listed(&router, party_type, &combined).await;
                assert_answered(&case, &Expected::Refused { member, reason }, &[], &answer);
            }
        }
    }
}
