// SPDX-License-Identifier: CC0-1.0

//! A `clientToken` reused for different content, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: an `@idempotencyToken` names ONE created resource. A
//! repeat carrying that token and the same request content answers with the
//! resource the token first created; the same token carrying DIFFERENT content
//! is refused with the operation's modelled `ConflictException` (HTTP 409) and
//! stores nothing.
//!
//! Two consequences of that rule are pinned alongside it: the agreement a
//! refused reuse NAMED holds nothing afterwards (no record is readable through
//! `GetAgreementCancellationRequest` or `ListAgreementCancellationRequests` for
//! it), and whether the agreement the token already cancelled is still `ACTIVE`
//! does not enter into it — the decision is about the token's content, while a
//! replay of the same content keeps answering with the record the first call
//! created.
//!
//! The two creates whose whole request lives under one token are covered here:
//! `CreateAgreementRequest` and `SendAgreementCancellationRequest`. The rule
//! also holds for `SendAgreementPaymentRequest` and for each entry of
//! `BatchCreateBillingAdjustmentRequest`, which the suites beside this one
//! already pin.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_cancellation_requests::{agreement, list, send, PENDING_APPROVAL};
use super::agreement_lifecycle::{
    agreement_over, call, cancel, create, emulator, term, upfront_term, Answer,
};

/// `TaxConfiguration$taxEstimation`'s two values and its documented default,
/// from `smithy_query(query="shape", shape="TaxConfiguration,TaxEstimation")`:
/// "Values include `ENABLED` and `DISABLED`. Default is `DISABLED`."
const TAX_ESTIMATION_ENABLED: &str = "ENABLED";
/// See [`TAX_ESTIMATION_ENABLED`].
const TAX_ESTIMATION_DEFAULT: &str = "DISABLED";

/// The base body with `change` applied: a member `change` names replaces the
/// base one, and a member it names `null` is left out of the request entirely —
/// which is how a row drops an optional member the first call carried.
fn overlaid(base: &Value, change: &Value) -> Value {
    let mut body = base.clone();
    {
        let members = body
            .as_object_mut()
            .expect("a request body is a JSON object");
        for (member, value) in change.as_object().expect("a change is a JSON object") {
            if value.is_null() {
                members.remove(member);
            } else {
                members.insert(member.clone(), value.clone());
            }
        }
    }
    body
}

/// Assert `answer` is the modelled conflict over `resource`.
fn assert_conflict(case: &str, answer: &Answer, resource_type: &str, resource_id: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceType"),
        resource_type,
        "case {case:?}: the conflict names the resource the token already created, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceId"),
        resource_id,
        "case {case:?}: the conflict names the resource the token already created, got {:?}",
        answer.body
    );
}

/// One row of the `CreateAgreementRequest` table: the members a second create
/// changes under the token the first one used, and whether that makes it the
/// same request.
struct CreateCase {
    case: &'static str,
    /// Each row uses its own token, so the rows are independent of each other.
    token: &'static str,
    /// Overlaid on the base body with [`overlaid`].
    change: Value,
    /// Whether the overlaid body asks for the same content as the base one.
    same: bool,
}

/// A reused `clientToken` answers with the first quote only when the request is
/// the same one; a token carrying different content is a conflict and prices
/// nothing.
///
/// The rows differ along what a create can change: the quantity asked for, the
/// contract length, the number of dimensions, the number of terms, which term,
/// which proposal, the intent (with the source agreement that intent needs) and
/// the tax estimation asked for.
#[tokio::test]
async fn a_client_token_reused_for_other_content_is_a_conflict() {
    let router = emulator();
    // The agreement a REPLACE intent acts upon has to exist, so the row that
    // changes the intent has a real one to name.
    let source = agreement_over(&router, "ap-source", vec![term("eula.v1")]).await;

    let cases = vec![
        CreateCase {
            case: "the identical request",
            token: "tok-identical",
            change: json!({}),
            same: true,
        },
        CreateCase {
            // The member's modelled default, so this asks for what the base
            // body (which names no taxConfiguration at all) already asked for.
            case: "the default tax estimation spelled out",
            token: "tok-default-tax",
            change: json!({"taxConfiguration": {"taxEstimation": TAX_ESTIMATION_DEFAULT}}),
            same: true,
        },
        CreateCase {
            case: "a larger quantity",
            token: "tok-quantity",
            change: json!({"requestedTerms": [upfront_term("cfg", "P12M", &[("Users", 30)])]}),
            same: false,
        },
        CreateCase {
            case: "a longer contract length",
            token: "tok-selector",
            change: json!({"requestedTerms": [upfront_term("cfg", "P36M", &[("Users", 3)])]}),
            same: false,
        },
        CreateCase {
            case: "another dimension on the same term",
            token: "tok-dimensions",
            change: json!({
                "requestedTerms": [
                    upfront_term("cfg", "P12M", &[("Users", 3), ("Storage", 100)])
                ]
            }),
            same: false,
        },
        CreateCase {
            case: "an extra term",
            token: "tok-extra-term",
            change: json!({
                "requestedTerms": [
                    upfront_term("cfg", "P12M", &[("Users", 3)]),
                    term("eula.v1"),
                ]
            }),
            same: false,
        },
        CreateCase {
            case: "a different term id",
            token: "tok-term-id",
            change: json!({"requestedTerms": [upfront_term("cfg2", "P12M", &[("Users", 3)])]}),
            same: false,
        },
        CreateCase {
            case: "a different proposal",
            token: "tok-proposal",
            change: json!({"agreementProposalIdentifier": "ap-offer2"}),
            same: false,
        },
        CreateCase {
            case: "a different intent over an existing agreement",
            token: "tok-intent",
            change: json!({"intent": "REPLACE", "sourceAgreementIdentifier": source}),
            same: false,
        },
        CreateCase {
            case: "tax estimation turned on",
            token: "tok-tax",
            change: json!({"taxConfiguration": {"taxEstimation": TAX_ESTIMATION_ENABLED}}),
            same: false,
        },
    ];

    for case in cases {
        let base = json!({
            "intent": "NEW",
            "agreementProposalIdentifier": "ap-offer1",
            "clientToken": case.token,
            "requestedTerms": [upfront_term("cfg", "P12M", &[("Users", 3)])],
        });

        let first = create(&router, base.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first create must succeed, got {} with {:?}",
            case.case, first.status, first.body
        );
        let quote_id = first.str_at("/agreementRequestId");
        let quote = first.body["chargeSummary"].clone();

        let second = create(&router, overlaid(&base, &case.change)).await;
        if case.same {
            assert_eq!(
                second.status, 200,
                "case {:?}: the same content under the same token replays, got {} with {:?}",
                case.case, second.status, second.body
            );
            assert_eq!(
                second.str_at("/agreementRequestId"),
                quote_id,
                "case {:?}: a replay names the first quote, got {:?}",
                case.case,
                second.body
            );
            assert_eq!(
                second.body["chargeSummary"], quote,
                "case {:?}: a replay answers the first quote, got {:?}",
                case.case, second.body
            );
            continue;
        }

        assert_conflict(case.case, &second, "AgreementRequest", &quote_id);

        // The refusal stored nothing and re-priced nothing: the token still
        // names the first quote, unchanged.
        let replay = create(&router, base).await;
        assert_eq!(
            replay.str_at("/agreementRequestId"),
            quote_id,
            "case {:?}: a refused reuse leaves the token naming the first quote, got {:?}",
            case.case,
            replay.body
        );
        assert_eq!(
            replay.body["chargeSummary"], quote,
            "case {:?}: a refused reuse leaves the first quote unchanged, got {:?}",
            case.case, replay.body
        );
    }
}

/// The content a conflict refused is still quotable under a token of its own:
/// the refusal is about the token, not about the request.
#[tokio::test]
async fn the_refused_content_is_quoted_under_its_own_token() {
    let router = emulator();
    let base = json!({
        "intent": "NEW",
        "agreementProposalIdentifier": "ap-offer1",
        "clientToken": "tok-shared",
        "requestedTerms": [upfront_term("cfg", "P12M", &[("Users", 3)])],
    });
    let changed = json!({
        "intent": "NEW",
        "agreementProposalIdentifier": "ap-offer1",
        "clientToken": "tok-shared",
        "requestedTerms": [upfront_term("cfg", "P12M", &[("Users", 30)])],
    });

    let first = create(&router, base).await;
    let refused = create(&router, changed.clone()).await;
    assert_conflict(
        "the same token for 30 units",
        &refused,
        "AgreementRequest",
        &first.str_at("/agreementRequestId"),
    );

    let fresh = create(&router, overlaid(&changed, &json!({"clientToken": "tok-own"}))).await;
    assert_eq!(
        fresh.status, 200,
        "a token of its own quotes the changed terms, got {} with {:?}",
        fresh.status, fresh.body
    );
    assert_ne!(
        fresh.str_at("/agreementRequestId"),
        first.str_at("/agreementRequestId"),
        "a token of its own mints a second quote, got {:?}",
        fresh.body
    );
    assert_ne!(
        fresh.body["chargeSummary"], first.body["chargeSummary"],
        "10 times the quantity is priced differently, got {:?} and {:?}",
        first.body, fresh.body
    );
}

/// One row of the `SendAgreementCancellationRequest` table.
struct SendCase {
    case: &'static str,
    token: &'static str,
    change: Value,
    same: bool,
}

/// The same rule over the other create that carries a whole request under one
/// `clientToken`: a reused token reports the cancellation request it already
/// sent only when the content is the same, and otherwise is a conflict that
/// asks the buyer to review nothing new.
///
/// The rows differ along what a send can change: the reason code, the free-text
/// description, dropping that description, and the agreement the request is
/// about.
#[tokio::test]
async fn a_cancellation_token_reused_for_other_content_is_a_conflict() {
    let router = emulator();
    let first_agreement = agreement(&router, "ap-cancel1").await;
    let other_agreement = agreement(&router, "ap-cancel2").await;

    let cases = vec![
        SendCase {
            case: "the identical request",
            token: "ctok-identical",
            change: json!({}),
            same: true,
        },
        SendCase {
            case: "a different reason code",
            token: "ctok-reason",
            change: json!({"reasonCode": "PRODUCT_DISCONTINUED"}),
            same: false,
        },
        SendCase {
            case: "a different description",
            token: "ctok-description",
            change: json!({"description": "a second account of the same wish"}),
            same: false,
        },
        SendCase {
            case: "the description dropped",
            token: "ctok-no-description",
            change: json!({"description": null}),
            same: false,
        },
        SendCase {
            case: "a different agreement",
            token: "ctok-agreement",
            change: json!({"agreementId": other_agreement}),
            same: false,
        },
    ];

    for case in cases {
        let base = json!({
            "agreementId": &first_agreement,
            "reasonCode": "BUYER_DISSATISFACTION",
            "description": "the buyer asked to cancel",
            "clientToken": case.token,
        });

        let first = send(&router, base.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first send must succeed, got {} with {:?}",
            case.case, first.status, first.body
        );
        let request_id = first.str_at("/agreementCancellationRequestId");

        let second = send(&router, overlaid(&base, &case.change)).await;
        if case.same {
            assert_eq!(
                second.status, 200,
                "case {:?}: the same content under the same token replays, got {} with {:?}",
                case.case, second.status, second.body
            );
            assert_eq!(
                second.str_at("/agreementCancellationRequestId"),
                request_id,
                "case {:?}: a replay names the first cancellation request, got {:?}",
                case.case,
                second.body
            );
            continue;
        }

        assert_conflict(
            case.case,
            &second,
            "AgreementCancellationRequest",
            &request_id,
        );
    }

    // A refused reuse stored nothing: every send that was accepted above is one
    // request, and the refused ones added none.
    let listed = list(&router, json!({"partyType": "Proposer", "maxResults": 50})).await;
    let items = listed.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .len();
    assert_eq!(
        items, 5,
        "one request per accepted send and none for a refused reuse, got {:?}",
        listed.body
    );
}

/// The `agreementCancellationRequestId` of every item a listing answered, in the
/// order it answered them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing answers an items array, got {:?}", answer.body))
        .iter()
        .map(|item| {
            item["agreementCancellationRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

async fn get_cancellation(router: &MultiServiceRouter, agreement_id: &str, id: &str) -> Answer {
    call(
        router,
        "GetAgreementCancellationRequest",
        json!({"agreementId": agreement_id, "agreementCancellationRequestId": id}),
    )
    .await
}

/// The reason code the first send of [`a_refused_cancellation_reuse_stores_nothing_for_the_agreement_it_named`]
/// carries — one value of `AgreementCancellationRequestReasonCode`, the whole
/// catalog of which `tests/all/agreement_cancellation_requests.rs` holds.
const FIRST_REASON_CODE: &str = "BUYER_DISSATISFACTION";

/// One row of the refused-reuse table: what the second send changed under the
/// token the first one used, which agreement it named, and whether the agreement
/// the token already cancelled has left `ACTIVE` by the time the reuse arrives.
struct ReuseCase {
    case: &'static str,
    /// Overlaid on the base send with [`overlaid`], beside whatever
    /// `names_other_agreement` points the send at.
    change: Value,
    /// Whether the reuse names the second agreement rather than the one the
    /// token already sent a cancellation request for.
    names_other_agreement: bool,
    /// Whether `CancelAgreement` ran over the first agreement before the reuse,
    /// so the agreement the token's record belongs to is no longer `ACTIVE`.
    first_agreement_cancelled: bool,
}

/// What a refused reuse leaves behind: nothing for the agreement it NAMED, and
/// the first request untouched — and the refusal does not depend on the status
/// of the agreement the token already cancelled, because it is decided from the
/// token's content.
///
/// The rows differ along what the reuse changed (the reason code, the free-text
/// description, dropping that description, the agreement named, and two members
/// at once) crossed with whether the first agreement has since left `ACTIVE`.
#[tokio::test]
async fn a_refused_cancellation_reuse_stores_nothing_for_the_agreement_it_named() {
    let cases = vec![
        ReuseCase {
            case: "a different reason code",
            change: json!({"reasonCode": "UNINTENDED_RENEWAL"}),
            names_other_agreement: false,
            first_agreement_cancelled: false,
        },
        ReuseCase {
            case: "a different description",
            change: json!({"description": "a second account of the same wish"}),
            names_other_agreement: false,
            first_agreement_cancelled: false,
        },
        ReuseCase {
            case: "the description dropped",
            change: json!({"description": null}),
            names_other_agreement: false,
            first_agreement_cancelled: false,
        },
        ReuseCase {
            case: "another agreement",
            change: json!({}),
            names_other_agreement: true,
            first_agreement_cancelled: false,
        },
        ReuseCase {
            case: "another agreement and a different reason code",
            change: json!({"reasonCode": "ALTERNATIVE_PROCUREMENT_CHANNEL"}),
            names_other_agreement: true,
            first_agreement_cancelled: false,
        },
        ReuseCase {
            case: "a different reason code, the first agreement no longer active",
            change: json!({"reasonCode": "REPLACING_AGREEMENT"}),
            names_other_agreement: false,
            first_agreement_cancelled: true,
        },
        ReuseCase {
            case: "another agreement, the first agreement no longer active",
            change: json!({}),
            names_other_agreement: true,
            first_agreement_cancelled: true,
        },
    ];

    for case in cases {
        // A router of its own per row, so what each row reads back is what that
        // row's own calls stored.
        let router = emulator();
        let first_agreement = agreement(&router, "ap-reuse1").await;
        let other_agreement = agreement(&router, "ap-reuse2").await;

        let base = json!({
            "agreementId": &first_agreement,
            "reasonCode": FIRST_REASON_CODE,
            "description": "the buyer asked to cancel",
            "clientToken": "ctok-reuse",
        });
        let first = send(&router, base.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first send must succeed, got {} with {:?}",
            case.case, first.status, first.body
        );
        let request_id = first.str_at("/agreementCancellationRequestId");
        let created_at = first.body["createdAt"].as_f64();

        if case.first_agreement_cancelled {
            let cancelled = cancel(&router, &first_agreement).await;
            assert_eq!(
                cancelled.status, 200,
                "case {:?}: cancelling the first agreement must succeed, got {} with {:?}",
                case.case, cancelled.status, cancelled.body
            );
        }

        let mut change = case.change.clone();
        if case.names_other_agreement {
            change["agreementId"] = json!(&other_agreement);
        }
        let reuse = overlaid(&base, &change);
        let named = reuse["agreementId"]
            .as_str()
            .expect("every row's reuse names an agreement")
            .to_string();

        let second = send(&router, reuse).await;
        assert_conflict(
            case.case,
            &second,
            "AgreementCancellationRequest",
            &request_id,
        );

        // Nothing was stored for the agreement the reuse NAMED: the only record
        // anywhere is the one the first send created, under the agreement that
        // send named.
        let listed = list(
            &router,
            json!({"partyType": "Proposer", "agreementId": &named, "maxResults": 50}),
        )
        .await;
        let expected: Vec<String> = if named == first_agreement {
            vec![request_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {:?}: the agreement the reuse named holds only what was stored for it, \
             got {:?}",
            case.case,
            listed.body
        );

        // And reading the first request back under the agreement the reuse
        // named only answers where that agreement really owns it.
        let read_back = get_cancellation(&router, &named, &request_id).await;
        if named == first_agreement {
            assert_eq!(
                read_back.status, 200,
                "case {:?}: the first request still reads back, got {} with {:?}",
                case.case, read_back.status, read_back.body
            );
            assert_eq!(
                read_back.str_at("/status"),
                PENDING_APPROVAL,
                "case {:?}: the refused reuse left the first request pending, got {:?}",
                case.case,
                read_back.body
            );
        } else {
            assert_eq!(
                read_back.error_code(),
                "ResourceNotFoundException",
                "case {:?}: the other agreement owns no cancellation request, got {} with {:?}",
                case.case,
                read_back.status,
                read_back.body
            );
        }

        // The token still names the first request, with the content and the
        // creation time it was created with: a replay of the same request is
        // answered from that record whatever the agreement's status is now.
        let replay = send(&router, base).await;
        assert_eq!(
            replay.status, 200,
            "case {:?}: the same content under the same token replays, got {} with {:?}",
            case.case, replay.status, replay.body
        );
        assert_eq!(
            replay.str_at("/agreementCancellationRequestId"),
            request_id,
            "case {:?}: a replay names the first cancellation request, got {:?}",
            case.case,
            replay.body
        );
        assert_eq!(
            replay.str_at("/reasonCode"),
            FIRST_REASON_CODE,
            "case {:?}: a replay reports the stored reason code, got {:?}",
            case.case,
            replay.body
        );
        assert_eq!(
            replay.body["createdAt"].as_f64(),
            created_at,
            "case {:?}: a replay reports the first request's creation time, got {:?}",
            case.case,
            replay.body
        );
    }
}
