// SPDX-License-Identifier: CC0-1.0

//! The order a paginated read of this service resolves a request in, end to end
//! over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the validation seam, the handler, the persisted state and
//! serialization — the same path a client takes. The wire helpers come from the
//! suites beside this one (`agreement_lifecycle`, `billing_adjustments`,
//! `error_bodies`), so there is one harness for the crate.
//!
//! The rule under test: a paginated read whose page belongs to one agreement
//! resolves that agreement BEFORE it interprets the `nextToken` that resumes the
//! page, so a request naming an agreement the store does not hold is answered
//! about `agreementId` — the modelled not-found where the read declares one and
//! the refusal of the member where it does not — whatever `nextToken` the
//! request carries, and never about `nextToken`. A token names a page OF a
//! listing, and there is no listing of an agreement that does not exist.
//!
//! The rows differ along what the rule generalises over: which read (seven of
//! them, spanning both refusal shapes, the listings whose `agreementId` is
//! optional and the reads where it is required), the form of the id that names
//! no agreement (both published bounds of `AgreementId` among them), whether a
//! token is carried at all, where a carried token came from (one this service
//! never minted, one the same read minted over an agreement that exists, one
//! another read minted) and whether a page size accompanies it.
//!
//! The second half of each row's loop pins what the order does NOT change: for
//! an agreement that DOES exist, the same read still serves its page and still
//! refuses a token naming no page of it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    agreement_over, call, emulator, invoiced_agreement, is_next_token, term, Answer,
};
use crate::billing_adjustments::{entry, submit};
use crate::error_bodies::{assert_input_fault, unresolvable_agreement_ids};

/// The `resourceType` a not-found refusal about an agreement names
/// (`ResourceNotFoundException$resourceType`), spelled out here rather than
/// imported so a change of wording in the service fails this test instead of
/// agreeing with it.
const RESOURCE_TYPE_AGREEMENT: &str = "Agreement";

/// The sentence this service states for a reference to an agreement it holds
/// none of, spelled out here for the same reason.
const AGREEMENT_DOES_NOT_EXIST: &str = "Provided agreement does not exist.";

/// A value of the SHAPE this service's page tokens have — a hexadecimal scope
/// digest followed by a hexadecimal offset, within `NextToken`'s
/// `^[a-zA-Z0-9+/=_-]+$` and `@length(max: 8192)` — that belongs to no query
/// this service ever minted a token for, so it reaches a handler rather than
/// being refused at the boundary.
const NEVER_MINTED_TOKEN: &str = "0123456789abcdef00000001";

/// The `ValidationExceptionReason` a refused token carries, which is what a
/// client branches on.
const INVALID_NEXT_TOKEN: &str = "INVALID_NEXT_TOKEN";

/// The `ValidationExceptionReason` for an `agreementId` naming no agreement.
const INVALID_AGREEMENT_ID: &str = "INVALID_AGREEMENT_ID";

/// How one read refuses a reference to an agreement this store does not hold.
#[derive(Clone, Copy, Debug)]
enum Refusal {
    /// The read declares `ResourceNotFoundException`
    /// (`smithy_query(query="operation", operation="<Operation>")`), so the
    /// reference is answered with it.
    NotFound,
    /// The read declares no `ResourceNotFoundException`, so the reference is
    /// refused as a value of the member, naming `agreementId`.
    InvalidMember,
}

/// One paginated read whose page belongs to one agreement.
struct Read {
    operation: &'static str,
    /// The members every call of this read carries besides `agreementId` — the
    /// ones the model makes `@required` — so the only thing a row varies is what
    /// it means to vary.
    members: Value,
    /// An agreement this store holds, for the half of the loop about what the
    /// resolution order leaves untouched.
    present: String,
    refusal: Refusal,
}

impl Read {
    /// This read's request over `agreement`, with the token and page size the
    /// case names.
    fn request(&self, agreement: &str, token: Option<&str>, max_results: Option<i32>) -> Value {
        let mut body = self.members.clone();
        body["agreementId"] = json!(agreement);
        if let Some(token) = token {
            body["nextToken"] = json!(token);
        }
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        body
    }
}

/// The dotted request paths a refusal's `fields` name.
fn field_names(answer: &Answer) -> Vec<String> {
    answer.body["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|field| field["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert one refusal is the modelled not-found for `agreement`, at the status
/// this service answers every lookup that found nothing with.
fn assert_agreement_not_found(case: &str, agreement: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a lookup that found nothing answers HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected a ResourceNotFoundException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceType"),
        RESOURCE_TYPE_AGREEMENT,
        "case {case:?}: expected resourceType {RESOURCE_TYPE_AGREEMENT:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceId"),
        agreement,
        "case {case:?}: expected resourceId {agreement:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/message"),
        AGREEMENT_DOES_NOT_EXIST,
        "case {case:?}: expected the sentence for a missing agreement, got {:?}",
        answer.body
    );
}

/// The token `operation` mints over `query`, asserted to be one.
async fn minted(router: &MultiServiceRouter, operation: &str, query: Value) -> String {
    let mut body = query;
    body["maxResults"] = json!(1);
    let answer = call(router, operation, body.clone()).await;
    assert_eq!(
        answer.status, 200,
        "minting a token with {operation} {body} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let token = answer.str_at("/nextToken");
    assert!(
        is_next_token(&token),
        "{operation} {body} must leave a token over the records it holds, got {:?}",
        answer.body
    );
    token
}

/// A read scoped to one agreement answers about that agreement, not about the
/// token, when the agreement does not exist.
#[tokio::test]
async fn a_paginated_read_resolves_its_agreement_before_its_page_token() {
    let router = emulator();

    // An invoiced agreement carrying two billing adjustment requests, and an
    // agreement over two terms — so two different reads of this emulator can
    // mint a real token below.
    let (invoiced, invoice) = invoiced_agreement(&router, "ap-orderinvoiced").await;
    let submitted = submit(
        &router,
        vec![
            entry(&invoiced, &invoice, "1", "tok-order-one"),
            entry(&invoiced, &invoice, "2", "tok-order-two"),
        ],
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "submitting two billing adjustment requests must succeed, got {} with {:?}",
        submitted.status, submitted.body
    );
    let termed = agreement_over(
        &router,
        "ap-ordertermed",
        vec![term("eula.v1"), term("SupportTerm")],
    )
    .await;

    let terms_token = minted(&router, "GetAgreementTerms", json!({"agreementId": termed})).await;
    let adjustments_token = minted(
        &router,
        "ListBillingAdjustmentRequests",
        json!({"agreementId": invoiced}),
    )
    .await;

    let reads = vec![
        Read {
            operation: "ListAgreementInvoiceLineItems",
            members: json!({"groupBy": "INVOICE_ID"}),
            present: invoiced.clone(),
            refusal: Refusal::NotFound,
        },
        Read {
            operation: "GetAgreementTerms",
            members: json!({}),
            present: termed.clone(),
            refusal: Refusal::NotFound,
        },
        Read {
            operation: "GetAgreementEntitlements",
            members: json!({}),
            present: termed.clone(),
            refusal: Refusal::NotFound,
        },
        Read {
            operation: "ListBillingAdjustmentRequests",
            members: json!({}),
            present: invoiced.clone(),
            refusal: Refusal::InvalidMember,
        },
        Read {
            operation: "ListAgreementCharges",
            members: json!({}),
            present: termed.clone(),
            refusal: Refusal::InvalidMember,
        },
        Read {
            operation: "ListAgreementPaymentRequests",
            members: json!({"partyType": "Acceptor"}),
            present: termed.clone(),
            refusal: Refusal::InvalidMember,
        },
        Read {
            operation: "ListAgreementCancellationRequests",
            members: json!({"partyType": "Acceptor"}),
            present: termed.clone(),
            refusal: Refusal::InvalidMember,
        },
    ];

    // What the request carries beside the agreement that does not exist: no
    // token, a token of the right shape this service never minted, a token one
    // read minted over an agreement that exists, and a token another read
    // minted together with a page size.
    let tokens: Vec<(&str, Option<&str>, Option<i32>)> = vec![
        ("no token at all", None, None),
        (
            "a well-formed token this service never minted",
            Some(NEVER_MINTED_TOKEN),
            None,
        ),
        (
            "a token GetAgreementTerms minted over an agreement that exists",
            Some(terms_token.as_str()),
            None,
        ),
        (
            "a token ListBillingAdjustmentRequests minted, beside a page size",
            Some(adjustments_token.as_str()),
            Some(1),
        ),
    ];

    for read in &reads {
        for (id_case, absent) in unresolvable_agreement_ids() {
            for (token_case, token, max_results) in &tokens {
                let case = format!("{} over {id_case} with {token_case}", read.operation);
                let answer = call(
                    &router,
                    read.operation,
                    read.request(&absent, *token, *max_results),
                )
                .await;

                match read.refusal {
                    Refusal::NotFound => assert_agreement_not_found(&case, &absent, &answer),
                    Refusal::InvalidMember => {
                        assert_input_fault(&case, INVALID_AGREEMENT_ID, "agreementId", &answer)
                    }
                }
                // Whichever shape the read refuses in, the refusal is about the
                // member at fault: the token is not it.
                assert_ne!(
                    answer.reason(),
                    INVALID_NEXT_TOKEN,
                    "case {case:?}: the member at fault is agreementId, not nextToken, got {:?}",
                    answer.body
                );
                assert!(
                    !field_names(&answer).iter().any(|name| name == "nextToken"),
                    "case {case:?}: no fields entry may name nextToken, got {:?}",
                    answer.body
                );
            }
        }

        // The order of the two resolutions is all that changed: for an
        // agreement that exists, the read still serves its page and still
        // refuses a token that names no page of it.
        let case = format!("{} over an agreement that exists", read.operation);
        let served = call(&router, read.operation, read.request(&read.present, None, None)).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: the read must still serve its page, got {} with {:?}",
            served.status, served.body
        );
        let refused = call(
            &router,
            read.operation,
            read.request(&read.present, Some(NEVER_MINTED_TOKEN), None),
        )
        .await;
        assert_input_fault(
            &format!("{case}, with a token naming no page of it"),
            INVALID_NEXT_TOKEN,
            "nextToken",
            &refused,
        );
    }
}
