// SPDX-License-Identifier: CC0-1.0

//! The entitlements an agreement grants, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises
//! routing, deserialization, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test: accepting an agreement request provisions the
//! entitlements the agreement grants — one per resource its proposal names, and
//! the single one the agreement grants when the proposal names none — each
//! carrying the AWS License Manager licence minted for it once, and
//! `GetAgreementEntitlements` reports that stored list for the agreement its
//! `agreementId` names, with each entitlement's status following the agreement's
//! own status and the clock (`PENDING`/`PROVISIONING_IN_PROGRESS` until
//! provisioning is due, `PROVISIONED`/`AGREEMENT_ACTIVE` after it, and
//! `DEPROVISIONED`/`AGREEMENT_INACTIVE` once the agreement is no longer active),
//! bounded by `maxResults` with a `nextToken` that resumes only that same list.
//! Nothing is echoed from the reporting call's own input, and reporting changes
//! nothing a later caller sees.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, agreement_over, call, cancel, create, emulator, term, variable_payment_term, Answer,
};

/// Longer than the provisioning span the service holds an entitlement `PENDING`
/// for (`state::ENTITLEMENT_PROVISIONING`, 500 ms), so a read taken after this
/// wait is a read past the instant the clock owed the entitlement its licence.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// The account id every generated emulator presents, and the region the test's
/// SigV4 scope signs for — spelled out here rather than imported so an ARN
/// minted in the wrong account or region fails this test.
const EMULATOR_ACCOUNT: &str = "000000000000";
/// See [`EMULATOR_ACCOUNT`].
const SIGNED_REGION: &str = "us-east-1";

/// `AgreementEntitlementStatus` values, from
/// `smithy_query(query="shape", shape="AgreementEntitlementStatus")`:
/// `PROVISIONED | SCHEDULED | PENDING | FAILED | DEPROVISIONED`.
const PENDING: &str = "PENDING";
/// See [`PENDING`].
const PROVISIONED: &str = "PROVISIONED";
/// See [`PENDING`].
const DEPROVISIONED: &str = "DEPROVISIONED";

/// `AgreementEntitlementStatusReasonCode` values, from
/// `smithy_query(query="shape", shape="AgreementEntitlementStatusReasonCode")`.
const PROVISIONING_IN_PROGRESS: &str = "PROVISIONING_IN_PROGRESS";
/// See [`PROVISIONING_IN_PROGRESS`].
const AGREEMENT_ACTIVE: &str = "AGREEMENT_ACTIVE";
/// See [`PROVISIONING_IN_PROGRESS`].
const AGREEMENT_INACTIVE: &str = "AGREEMENT_INACTIVE";

async fn entitlements(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "GetAgreementEntitlements", request).await
}

async fn search(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "SearchAgreements", request).await
}

/// The entitlements one answer reports, in the order it reported them.
fn reported(answer: &Answer) -> Vec<Value> {
    answer.body["agreementEntitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The entitlements of one agreement, asserting the call succeeded.
async fn entitlements_of(router: &MultiServiceRouter, agreement_id: &str) -> Vec<Value> {
    let answer = entitlements(router, json!({"agreementId": agreement_id})).await;
    assert_eq!(
        answer.status, 200,
        "reading the entitlements of {agreement_id} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    reported(&answer)
}

/// Whether `value` is the ARN of an AWS License Manager licence in this
/// emulator's account and the caller's region, as `AgreementEntitlement`'s
/// `licenseArn` (an `AwsArn`) carries it.
///
/// Spelled out from the ARN grammar here rather than built with the crate's own
/// minter, so a handler that reported a differently shaped ARN cannot make this
/// test pass.
fn is_license_arn(value: &str) -> bool {
    let fields: Vec<&str> = value.split(':').collect();
    fields.len() == 7
        && fields[0] == "arn"
        && fields[1] == "aws"
        && fields[2] == "license-manager"
        && fields[3] == SIGNED_REGION
        && fields[4] == EMULATOR_ACCOUNT
        && fields[5] == "license"
        && fields[6]
            .strip_prefix("l-")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Assert the one entitlement `entitlement` is in the state `status` /
/// `reason_code`, and answer the licence it carries.
fn assert_state(case: &str, entitlement: &Value, status: &str, reason_code: &str) -> String {
    assert_eq!(
        entitlement["status"].as_str(),
        Some(status),
        "case {case:?}: expected status {status:?}, got {entitlement:?}"
    );
    assert_eq!(
        entitlement["statusReasonCode"].as_str(),
        Some(reason_code),
        "case {case:?}: expected statusReasonCode {reason_code:?}, got {entitlement:?}"
    );
    let license = entitlement["licenseArn"].as_str().unwrap_or_default();
    assert!(
        is_license_arn(license),
        "case {case:?}: expected an AWS License Manager licence ARN, got {entitlement:?}"
    );
    license.to_string()
}

/// One row of the terms table: an agreement created over a different set of
/// requested terms, which is what the acceptance derives the agreement from.
struct TermsCase {
    case: &'static str,
    proposal: &'static str,
    terms: Vec<Value>,
}

/// Accepting an agreement request provisions the entitlement the agreement
/// grants: one entitlement, `PENDING` with `PROVISIONING_IN_PROGRESS` while
/// provisioning is owed, carrying its own freshly minted License Manager
/// licence, and `PROVISIONED` with `AGREEMENT_ACTIVE` once the clock has made
/// that transition due — the SAME licence, because the status moves and the
/// entitlement's identity does not.
///
/// The rows differ along what the rule generalises over: how many terms the
/// agreement was accepted over, of which kinds, configured or not. None of them
/// changes the entitlement count, which follows the proposal's resources and not
/// the terms.
#[tokio::test]
async fn an_acceptance_provisions_the_entitlement_the_agreement_grants() {
    let cases = vec![
        TermsCase {
            case: "one legal term",
            proposal: "ap-offer1",
            terms: vec![term("eula.v1")],
        },
        TermsCase {
            case: "two terms of different kinds",
            proposal: "ap-offer2",
            terms: vec![term("LegalTerm"), term("SupportTerm")],
        },
        TermsCase {
            case: "a configured variable payment term",
            proposal: "at-offer3",
            terms: vec![variable_payment_term("vpt-1")],
        },
        TermsCase {
            case: "three terms, one of them configured",
            proposal: "ap-offer4",
            terms: vec![
                term("UsageBasedPricingTerm"),
                term("RenewalTerm"),
                variable_payment_term("vpt-2"),
            ],
        },
    ];

    let router = emulator();
    let mut licenses: BTreeSet<String> = BTreeSet::new();
    let mut provisioning: Vec<(&'static str, String, String)> = Vec::new();

    for case in cases {
        let agreement = agreement_over(&router, case.proposal, case.terms.clone()).await;
        let granted = entitlements_of(&router, &agreement).await;
        assert_eq!(
            granted.len(),
            1,
            "case {:?}: an agreement whose proposal names no resource grants one \
             entitlement, got {granted:?}",
            case.case
        );
        let entitlement = &granted[0];
        let license = assert_state(case.case, entitlement, PENDING, PROVISIONING_IN_PROGRESS);
        assert!(
            licenses.insert(license.clone()),
            "case {:?}: every entitlement carries its own licence, but {license} was \
             already granted",
            case.case
        );

        // The two members this emulator holds no value for stay absent rather
        // than carrying an invented one: the resource lives in the agreement
        // proposal, which is another API's resource, and no operation of this
        // service redeems a registration token.
        for member in ["resource", "type", "registrationToken"] {
            assert!(
                entitlement[member].is_null(),
                "case {:?}: {member} has no value to report, got {entitlement:?}",
                case.case
            );
        }
        assert!(
            granted.len() == 1 && entitlement["licenseArn"].is_string(),
            "case {:?}: the entitlement must carry its licence, got {entitlement:?}",
            case.case
        );
        provisioning.push((case.case, agreement, license));
    }

    // Past the provisioning span the clock owes every one of them its licence.
    sleep(PAST_PROVISIONING);
    for (case, agreement, license) in provisioning {
        let granted = entitlements_of(&router, &agreement).await;
        assert_eq!(
            granted.len(),
            1,
            "case {case:?}: provisioning does not change how many entitlements exist, \
             got {granted:?}"
        );
        let settled = assert_state(case, &granted[0], PROVISIONED, AGREEMENT_ACTIVE);
        assert_eq!(
            settled, license,
            "case {case:?}: the licence is minted once, so it must not change when the \
             status does, got {settled}"
        );

        // A read reports; it changes nothing a later caller sees.
        let again = entitlements_of(&router, &agreement).await;
        assert_eq!(
            again, granted,
            "case {case:?}: two reads of the same settled entitlements must answer the \
             same thing, got {again:?}"
        );
    }
}

/// An entitlement is granted BY an agreement, so its state follows the
/// agreement's own: an agreement that is no longer `ACTIVE` entitles its
/// acceptor to nothing, whichever way it left `ACTIVE`, while one that is still
/// active keeps its entitlement provisioned. The licence itself never changes.
#[tokio::test]
async fn an_entitlement_follows_the_status_of_the_agreement_that_granted_it() {
    let router = emulator();

    // Three agreements that leave `ACTIVE` in the different ways this service
    // moves one, plus the replacement that stays active.
    let untouched = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;
    let cancelled = agreement_over(&router, "ap-offer2", vec![term("eula.v1")]).await;
    let replaced = agreement_over(&router, "ap-offer3", vec![term("eula.v1")]).await;

    let before: Vec<Value> = entitlements_of(&router, &cancelled).await;
    let cancelled_license = before[0]["licenseArn"].as_str().unwrap_or_default().to_string();

    let cancellation = cancel(&router, &cancelled).await;
    assert_eq!(
        cancellation.status, 200,
        "cancelling {cancelled} must succeed, got {} with {:?}",
        cancellation.status, cancellation.body
    );

    let replacement_request = create(
        &router,
        json!({
            "intent": "REPLACE",
            "agreementProposalIdentifier": "ap-offer4",
            "sourceAgreementIdentifier": replaced,
            "requestedTerms": [term("eula.v2")],
        }),
    )
    .await;
    let replacement = accept(
        &router,
        json!({"agreementRequestId": replacement_request.str_at("/agreementRequestId")}),
    )
    .await
    .str_at("/agreementId");
    assert!(
        !replacement.is_empty(),
        "the replacement needs an agreement id, got {:?}",
        replacement_request.body
    );

    // Past provisioning, so an active agreement's entitlement is settled and
    // every row below is decided by the agreement's status alone.
    sleep(PAST_PROVISIONING);

    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an agreement nothing moved",
            &untouched,
            PROVISIONED,
            AGREEMENT_ACTIVE,
        ),
        (
            "a cancelled agreement",
            &cancelled,
            DEPROVISIONED,
            AGREEMENT_INACTIVE,
        ),
        (
            "the agreement a replacement acted upon",
            &replaced,
            DEPROVISIONED,
            AGREEMENT_INACTIVE,
        ),
        (
            "the replacement itself",
            &replacement,
            PROVISIONED,
            AGREEMENT_ACTIVE,
        ),
    ];

    for (case, agreement, status, reason_code) in cases {
        let granted = entitlements_of(&router, agreement).await;
        assert_eq!(
            granted.len(),
            1,
            "case {case:?}: the agreement still grants the entitlement it granted, \
             got {granted:?}"
        );
        let license = assert_state(case, &granted[0], status, reason_code);
        if agreement == cancelled {
            assert_eq!(
                license, cancelled_license,
                "case {case:?}: deprovisioning moves the status, not the licence, got \
                 {license}"
            );
        }
    }
}

/// `maxResults` bounds the page, a list shorter than the page it asked for ends
/// with no `nextToken`, and a token that was not minted for THIS agreement's
/// entitlements is refused with the modelled `INVALID_NEXT_TOKEN` rather than
/// resuming some other list.
#[tokio::test]
async fn a_page_is_bounded_and_a_token_resumes_only_this_list() {
    let router = emulator();
    // Two terms, so `GetAgreementTerms` mints a token of its own for the same
    // agreement — the closest thing to a token that could be confused with
    // this operation's.
    let agreement = agreement_over(
        &router,
        "ap-offer1",
        vec![term("eula.v1"), term("SupportTerm")],
    )
    .await;

    // `MaxResults` is `@range(min: 1, max: 50)`, so the rows are its published
    // bounds and the request that names none.
    for max_results in [None, Some(1), Some(2), Some(50)] {
        let mut request = json!({"agreementId": agreement});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        let answer = entitlements(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults {max_results:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported(&answer).len(),
            1,
            "case maxResults {max_results:?}: the agreement's one entitlement fits in \
             every page it asked for, got {:?}",
            answer.body
        );
        assert!(
            answer.body["nextToken"].is_null(),
            "case maxResults {max_results:?}: a list that ended carries no nextToken, \
             got {:?}",
            answer.body
        );
    }

    // A value past the modelled bounds is refused at the boundary, before the
    // handler runs.
    for max_results in [0, 51] {
        let answer = entitlements(
            &router,
            json!({"agreementId": agreement, "maxResults": max_results}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {max_results}: outside @range(1, 50) must be refused, got \
             {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case maxResults {max_results}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    let terms_page = call(
        &router,
        "GetAgreementTerms",
        json!({"agreementId": agreement, "maxResults": 1}),
    )
    .await;
    let terms_token = terms_page.str_at("/nextToken");
    assert!(
        !terms_token.is_empty(),
        "the terms listing must mint a token to borrow, got {:?}",
        terms_page.body
    );

    let bad_tokens: Vec<(&str, String)> = vec![
        (
            "a token minted for the same agreement's terms",
            terms_token,
        ),
        (
            "a token of this service's shape that it never minted",
            "0".repeat(24),
        ),
        ("a token that is not one at all", "notatoken".to_string()),
    ];
    for (case, token) in bad_tokens {
        let answer = entitlements(
            &router,
            json!({"agreementId": agreement, "nextToken": token}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token naming no page of this list must be refused, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            "INVALID_NEXT_TOKEN",
            "case {case:?}: expected reason INVALID_NEXT_TOKEN, got {:?}",
            answer.body
        );
    }
}

/// The licence an entitlement granted is the SAME wherever the model asks for
/// it: `GetAgreementEntitlements` reports it in full,
/// `AgreementViewSummary$entitlements` carries it, and the `LicenseArn` search
/// filter selects the agreement that granted it — and no other.
#[tokio::test]
async fn the_licence_an_entitlement_granted_is_reported_wherever_the_model_asks_for_it() {
    let router = emulator();
    let first = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;
    let second = agreement_over(&router, "ap-offer2", vec![term("eula.v2")]).await;

    let first_license = entitlements_of(&router, &first).await[0]["licenseArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let second_license = entitlements_of(&router, &second).await[0]["licenseArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        first_license, second_license,
        "two agreements grant two licences, got {first_license} twice"
    );

    let summaries = search(&router, json!({"catalog": "AWSMarketplace"})).await;
    for (agreement, license) in [(&first, &first_license), (&second, &second_license)] {
        let summary = summaries.body["agreementViewSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary["agreementId"].as_str() == Some(agreement.as_str()))
            .unwrap_or_else(|| panic!("{agreement} must be searchable, got {:?}", summaries.body));
        assert_eq!(
            summary["entitlements"]
                .as_array()
                .and_then(|list| list.first())
                .and_then(|entitlement| entitlement["licenseArn"].as_str()),
            Some(license.as_str()),
            "case {agreement:?}: the summary must carry the licence the entitlement \
             granted, got {summary:?}"
        );
    }

    let cases: Vec<(&str, String, Vec<&String>)> = vec![
        (
            "the licence of the first agreement",
            first_license.clone(),
            vec![&first],
        ),
        (
            "the licence of the second agreement",
            second_license.clone(),
            vec![&second],
        ),
        (
            "a licence ARN no entitlement carries",
            format!("arn:aws:license-manager:{SIGNED_REGION}:{EMULATOR_ACCOUNT}:license:l-ffffffffffffffff"),
            vec![],
        ),
    ];
    for (case, license, expected) in cases {
        let answer = search(
            &router,
            json!({
                "catalog": "AWSMarketplace",
                "filters": [{"name": "LicenseArn", "values": [license]}],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let selected: Vec<String> = answer.body["agreementViewSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary["agreementId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.body
        );
    }
}
