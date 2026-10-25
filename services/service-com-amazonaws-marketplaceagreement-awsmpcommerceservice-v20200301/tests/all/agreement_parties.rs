// SPDX-License-Identifier: CC0-1.0

//! The two parties of an agreement, end to end over HTTP.
//!
//! The rule under test: an agreement names an AWS account for BOTH of its
//! sides — the party that accepted its terms and the party that proposed them —
//! so every projection of a stored agreement (`DescribeAgreement` and
//! `SearchAgreements`' `AgreementViewSummary`) reports an `accountId` for each,
//! and a query scoped to a `PartyType` selects by the account the agreement
//! records for that side rather than by nothing at all.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_lifecycle::{
    accept, agreement_over, agreement_type_filter, call, cancel, create, emulator, term,
    upfront_term, variable_payment_term, Answer,
};

/// The account id every generated emulator presents, spelled out here rather
/// than imported so a handler naming some other account fails this test.
const EMULATOR_ACCOUNT: &str = "000000000000";

/// The two values of `PartyType`, as `SearchAgreementsInput$filters` documents
/// them: "The party type of the caller. Use `Proposer` or `Acceptor`"
/// (`smithy_query(query="doc", shape="SearchAgreementsInput", member="filters")`).
const PARTY_TYPES: &[&str] = &["Proposer", "Acceptor"];

/// The two party members an agreement reports, as `DescribeAgreementOutput` and
/// `AgreementViewSummary` name them.
const PARTY_MEMBERS: &[&str] = &["acceptor", "proposer"];

/// `Acceptor$accountId` and `Proposer$accountId` both target `AWSAccountId`,
/// which the model constrains to `@length(min: 12, max: 12)` and the pattern
/// `^[0-9]+$` (`smithy_query(query="shape", shape="Acceptor,Proposer")`), so a
/// reported party account has to satisfy both.
fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.chars().all(|digit| digit.is_ascii_digit())
}

async fn describe(router: &MultiServiceRouter, agreement_id: &str) -> Answer {
    call(
        router,
        "DescribeAgreement",
        json!({"agreementId": agreement_id}),
    )
    .await
}

async fn search(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "SearchAgreements", request).await
}

/// The summaries one search answered.
fn summaries(answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "a search must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The agreement ids one search answered, as a set: the order a search answers
/// in is the sort rule's business, not this rule's.
fn searched_ids(answer: &Answer) -> BTreeSet<String> {
    summaries(answer)
        .iter()
        .map(|summary| {
            summary["agreementId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// What a row does to the agreement it created over its terms.
#[derive(Clone, Copy)]
enum Lineage {
    /// Nothing: the row holds the one brand-new agreement.
    New,
    /// Amends it, so the row holds the source and the amendment.
    Amend,
    /// Replaces it, so the row holds the superseded source and its replacement.
    Replace,
    /// Cancels it, so the row's one agreement is no longer active.
    Cancel,
}

/// One population of stored agreements: the terms the base agreement was
/// accepted over, and what the row then did to it.
struct PartyCase {
    case: &'static str,
    terms: Vec<Value>,
    lineage: Lineage,
}

/// Store the row's agreements, answering every agreement id it left behind.
async fn stored(router: &MultiServiceRouter, case: &PartyCase) -> Vec<String> {
    let base = agreement_over(router, "ap-party", case.terms.clone()).await;
    match case.lineage {
        Lineage::New => vec![base],
        Lineage::Cancel => {
            let cancelled = cancel(router, &base).await;
            assert_eq!(
                cancelled.status, 200,
                "case {:?}: cancelling {base} must succeed, got {} with {:?}",
                case.case, cancelled.status, cancelled.body
            );
            vec![base]
        }
        Lineage::Amend | Lineage::Replace => {
            let mut request = json!({
                "sourceAgreementIdentifier": base,
                "requestedTerms": [term("eula.v2")],
            });
            if matches!(case.lineage, Lineage::Amend) {
                // "`agreementProposalIdentifier` … is required if the intent is
                // not `AMEND`", so an amendment names only the source.
                request["intent"] = json!("AMEND");
            } else {
                request["intent"] = json!("REPLACE");
                request["agreementProposalIdentifier"] = json!("ap-partynext");
            }
            let created = create(router, request).await;
            assert_eq!(
                created.status, 200,
                "case {:?}: the follow-on request must be created, got {} with {:?}",
                case.case, created.status, created.body
            );
            let accepted = accept(
                router,
                json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
            )
            .await;
            assert_eq!(
                accepted.status, 200,
                "case {:?}: accepting the follow-on request must succeed, got {} with {:?}",
                case.case, accepted.status, accepted.body
            );
            vec![base, accepted.str_at("/agreementId")]
        }
    }
}

/// The rows every party case spans: what the agreement's terms are, and how it
/// came to be — a fresh agreement, a link of a chain, or one that has left
/// `ACTIVE`. The parties of an agreement do not depend on any of them.
fn party_cases() -> Vec<PartyCase> {
    vec![
        PartyCase {
            case: "a legal term only, so nothing is committed",
            terms: vec![term("eula.v1")],
            lineage: Lineage::New,
        },
        PartyCase {
            case: "a prepaid purchase of two dimensions",
            terms: vec![upfront_term("cfg", "P12M", &[("Users", 2), ("Admins", 1)])],
            lineage: Lineage::New,
        },
        PartyCase {
            case: "metered pricing and a variable payment term",
            terms: vec![
                term("UsageBasedPricingTerm"),
                variable_payment_term("variable"),
            ],
            lineage: Lineage::New,
        },
        PartyCase {
            case: "an amendment, so a chain of two agreements",
            terms: vec![term("SupportTerm")],
            lineage: Lineage::Amend,
        },
        PartyCase {
            case: "a replacement, which leaves its source superseded",
            terms: vec![term("eula.v1")],
            lineage: Lineage::Replace,
        },
        PartyCase {
            case: "an agreement the caller cancelled",
            terms: vec![term("ValidityTerm")],
            lineage: Lineage::Cancel,
        },
    ]
}

/// Both parties of every stored agreement are reported, by both projections,
/// and each names the account this emulator serves.
///
/// The rows differ along what the rule generalises over: the kind and number of
/// accepted terms, whether the agreement stands alone or is a link of a chain,
/// and whether it is still active — none of which changes who its parties are.
#[tokio::test]
async fn both_parties_of_every_stored_agreement_are_reported() {
    for case in party_cases() {
        let router = emulator();
        let agreements = stored(&router, &case).await;

        let searched = search(&router, json!({"filters": [agreement_type_filter()]})).await;
        let summaries = summaries(&searched);

        for agreement_id in &agreements {
            let described = describe(&router, agreement_id).await;
            assert_eq!(
                described.status, 200,
                "case {:?}: describing {agreement_id} must succeed, got {} with {:?}",
                case.case, described.status, described.body
            );
            for member in PARTY_MEMBERS {
                let account = described.str_at(&format!("/{member}/accountId"));
                assert!(
                    is_account_id(&account),
                    "case {:?}: {member} of {agreement_id} must carry a 12-digit account id, \
                     got {account:?} in {:?}",
                    case.case,
                    described.body
                );
                assert_eq!(
                    account, EMULATOR_ACCOUNT,
                    "case {:?}: {member} of {agreement_id} must be the account this emulator \
                     serves, got {account:?}",
                    case.case
                );
            }

            // The search projection reports the same two parties as the
            // describe: both read the one stored record.
            let summary = summaries
                .iter()
                .find(|summary| summary["agreementId"].as_str() == Some(agreement_id.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the search must report {agreement_id}, got {:?}",
                        case.case, searched.body
                    )
                });
            for member in PARTY_MEMBERS {
                assert_eq!(
                    summary[member], described.body[*member],
                    "case {:?}: the summary's {member} of {agreement_id} must be the described \
                     one, got {:?}",
                    case.case, summary[member]
                );
            }

            // A report changes nothing: the parties a second read answers are
            // the ones the first did.
            let again = describe(&router, agreement_id).await;
            for member in PARTY_MEMBERS {
                assert_eq!(
                    again.body[*member], described.body[*member],
                    "case {:?}: re-reading {agreement_id} must report the same {member}, \
                     got {:?}",
                    case.case, again.body[*member]
                );
            }
        }

        // Each party type selects by the account recorded for that side, so a
        // search scoped to either one selects every agreement the store holds.
        let expected: BTreeSet<String> = agreements.iter().cloned().collect();
        for party_type in PARTY_TYPES {
            let scoped = search(
                &router,
                json!({"filters": [
                    agreement_type_filter(),
                    {"name": "PartyType", "values": [party_type]},
                ]}),
            )
            .await;
            assert_eq!(
                searched_ids(&scoped),
                expected,
                "case {:?}: a search scoped to {party_type:?} must select {expected:?}, \
                 got {:?}",
                case.case,
                scoped.body
            );
        }
    }
}

/// A party-scoped search reports the whole population it selects, whatever its
/// size — including a store that holds nothing, whose answer is the only empty
/// one.
///
/// The rows differ along the count the rule generalises over: no agreement, one,
/// and several.
#[tokio::test]
async fn a_party_scoped_search_reports_the_whole_population() {
    for count in [0_usize, 1, 3] {
        let router = emulator();
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for index in 0..count {
            expected.insert(
                agreement_over(
                    &router,
                    &format!("ap-pop{index}"),
                    vec![term(&format!("eula.v{index}"))],
                )
                .await,
            );
        }

        for party_type in PARTY_TYPES {
            let scoped = search(
                &router,
                json!({"filters": [
                    agreement_type_filter(),
                    {"name": "PartyType", "values": [party_type]},
                ]}),
            )
            .await;
            assert_eq!(
                searched_ids(&scoped),
                expected,
                "case {count} agreement(s), partyType {party_type:?}: expected {expected:?}, \
                 got {:?}",
                scoped.body
            );
            for summary in summaries(&scoped) {
                for member in PARTY_MEMBERS {
                    let account = summary[*member]["accountId"].as_str().unwrap_or_default();
                    assert!(
                        is_account_id(account),
                        "case {count} agreement(s), partyType {party_type:?}: every summary \
                         must report a 12-digit {member} account, got {summary:?}"
                    );
                }
            }
        }
    }
}
