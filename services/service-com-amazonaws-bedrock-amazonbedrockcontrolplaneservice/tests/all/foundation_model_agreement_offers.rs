// SPDX-License-Identifier: CC0-1.0

//! `ListFoundationModelAgreementOffers` end to end over HTTP, through the same
//! `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! The offers of a foundation model are the model's own, so the `modelId` has to name a
//! model this account's published catalog carries: one it does not resolves to no model at
//! all and is refused 404 `ResourceNotFoundException`, one of the errors the operation
//! models, rather than answered with an empty list. A model the catalog does carry answers
//! 200 with its `modelId` and its offers — an offer carrying the `offerToken` that feeds
//! `CreateFoundationModelAgreement` and the term structures the agreement is made on — for
//! a model offered under a third-party provider's end-user license agreement, and no offer
//! for a model AWS publishes itself, which an account reaches without accepting another
//! party's terms.
//!
//! # What the tables below generalise over
//!
//! The rows walk the WHOLE published catalog rather than a chosen row of it, so every
//! provider in it is covered, and they differ along the `offerType` the request names
//! (absent, `ALL`, `PUBLIC` — the two values `OfferType` declares) and along the form of
//! the `modelId` (the bare id, and the qualified form `BedrockModelId`'s pattern admits).
//! The refusal rows differ along how an id fails to name a catalogued model.

use serde_json::Value;
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{emulator, label, Emulator, Reply};

/// `GET /list-foundation-model-agreement-offers/{modelId}`, with the `offerType` the row
/// names.
async fn offers(emulator: &Emulator, model_id: &str, offer_type: Option<&str>) -> Reply {
    let path = match offer_type {
        Some(offer_type) => format!(
            "/list-foundation-model-agreement-offers/{}?offerType={offer_type}",
            label(model_id)
        ),
        None => format!(
            "/list-foundation-model-agreement-offers/{}",
            label(model_id)
        ),
    };
    emulator.call("GET", &path, None).await
}

/// The offers a reply reports.
fn reported(reply: &Reply) -> Vec<Value> {
    reply.get("offers").as_array().cloned().unwrap_or_default()
}

/// Every model the account's own `ListFoundationModels` catalog reports.
async fn catalogued(emulator: &Emulator) -> Vec<Value> {
    let listed = emulator.call("GET", "/foundation-models", None).await;
    assert_eq!(
        listed.status, 200,
        "the catalog must be served, got {} {:?}",
        listed.status, listed.body
    );
    let summaries = listed
        .get("modelSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !summaries.is_empty(),
        "the published catalog must report models, got {:?}",
        listed.body
    );
    summaries
}

/// RULE: every catalogued model answers 200 with its own offers — an offer per model
/// offered under a third-party agreement, none for a model AWS publishes itself — and a
/// model that exposes an offer exposes a usable `offerToken` and the terms it is offered
/// on.
#[tokio::test]
async fn every_catalogued_model_reports_its_own_agreement_offers() {
    let emulator = emulator();
    let mut with_offers = 0usize;
    for summary in catalogued(&emulator).await {
        let model_id = summary["modelId"].as_str().expect("a modelId").to_string();
        let provider = summary["providerName"]
            .as_str()
            .expect("a providerName")
            .to_string();
        let case = format!("{model_id} by {provider}");

        let reply = offers(&emulator, &model_id, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: a catalogued model must be answered 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("modelId"),
            model_id,
            "case {case}: the response must name the model it is about, got {:?}",
            reply.get("modelId")
        );

        let listed = reported(&reply);
        // A model AWS publishes itself carries no agreement to accept; every other
        // provider's model is offered under that provider's own terms.
        let aws_published = provider == bedrock::state::AWS_MODEL_PROVIDER;
        assert_eq!(
            listed.is_empty(),
            aws_published,
            "case {case}: a model published by {:?} must {} an offer, got {:?}",
            provider,
            if aws_published { "expose no" } else { "expose" },
            reply.body
        );
        if listed.is_empty() {
            continue;
        }
        with_offers += 1;

        for offer in &listed {
            let token = offer["offerToken"].as_str().unwrap_or_else(|| {
                panic!("case {case}: an offer must carry an offerToken, got {offer:?}")
            });
            assert!(
                !token.is_empty(),
                "case {case}: an offerToken must be usable, got {token:?}"
            );
            let offer_id = offer["offerId"].as_str().unwrap_or_default();
            assert_eq!(
                offer_id,
                bedrock::state::model_offer_id(&model_id),
                "case {case}: the offerId must be the one this model's offer carries, got \
                 {offer_id:?}"
            );
            assert_eq!(
                token,
                bedrock::state::model_offer_token(&model_id, offer_id),
                "case {case}: the offerToken must be the token of this model's offer, got \
                 {token:?}"
            );
            // The terms an offer is made on: the three structures `TermDetails` requires
            // are reported, and the price list, license document and support policy they
            // would name are not invented here.
            for term in ["usageBasedPricingTerm", "legalTerm", "supportTerm"] {
                assert!(
                    offer["termDetails"].get(term).is_some(),
                    "case {case}: termDetails must carry {term}, got {:?}",
                    offer["termDetails"]
                );
            }
            assert_eq!(
                offer["termDetails"]["usageBasedPricingTerm"]
                    .get("rateCard")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(0),
                "case {case}: the usage-based pricing term must report its rate card, got {:?}",
                offer["termDetails"]
            );
        }

        // Reading twice changes nothing: this operation is @readonly, and an offer is
        // published data rather than a resource a read brings into being.
        let again = offers(&emulator, &model_id, None).await;
        assert_eq!(
            reported(&again),
            listed,
            "case {case}: a second listing must report the same offers, got {:?}",
            again.body
        );
    }
    assert!(
        with_offers > 0,
        "the catalog must offer at least one model under an agreement, else no account could \
         ever reach CreateFoundationModelAgreement"
    );
}

/// RULE: `offerType` narrows the listing to the offers of that type. Every offer this
/// emulator publishes is public, so `ALL` and `PUBLIC` admit the same offers and a request
/// naming neither gets all of them.
#[tokio::test]
async fn the_offer_type_narrows_the_listing_to_offers_of_that_type() {
    let emulator = emulator();
    // A third-party model: the one that has an offer to narrow at all. Its id is a
    // published row of this service's own catalog.
    const MODEL_ID: &str = "anthropic.claude-3-haiku-20240307-v1:0";
    let all = reported(&offers(&emulator, MODEL_ID, None).await);
    assert!(
        !all.is_empty(),
        "the fixture model must expose an offer, got {all:?}"
    );
    for offer_type in [
        bedrock::state::OFFER_TYPE_ALL,
        bedrock::state::OFFER_TYPE_PUBLIC,
    ] {
        let reply = offers(&emulator, MODEL_ID, Some(offer_type)).await;
        assert_eq!(
            reply.status, 200,
            "case {offer_type}: the listing must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reported(&reply),
            all,
            "case {offer_type}: every offer here is public, so this type admits them all, got \
             {:?}",
            reply.body
        );
    }

    // A value `OfferType` does not declare is refused by the member's own constraint,
    // ahead of the handler.
    let unknown = offers(&emulator, MODEL_ID, Some("PRIVATE")).await;
    assert_eq!(
        unknown.status, 400,
        "case an undeclared offerType: must be refused 400, got {} {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error.as_deref(),
        Some("ValidationException"),
        "case an undeclared offerType: a constraint violation is a ValidationException, got {:?}",
        unknown.error
    );
}

/// RULE: a `modelId` that names no model in the catalog is refused 404
/// `ResourceNotFoundException` — never an empty success — and the qualified form of a
/// catalogued id names that same published model.
#[tokio::test]
async fn an_id_that_names_no_catalogued_model_is_refused() {
    let emulator = emulator();

    // The qualified form `BedrockModelId`'s pattern admits — the published id with the
    // `:`-separated qualifiers AWS appends to it — names the same model, so it answers the
    // offers of that model.
    let qualified = offers(&emulator, "anthropic.claude-3-haiku-20240307-v1:0:200k", None).await;
    assert_eq!(
        qualified.status, 200,
        "case a qualified model id: must name the same published model, got {} {:?}",
        qualified.status, qualified.body
    );
    assert!(
        !reported(&qualified).is_empty(),
        "case a qualified model id: must report that model's offers, got {:?}",
        qualified.body
    );

    // One axis per row: a provider the catalog has no model of, a model id of a provider it
    // does carry, and a well-formed id whose model was never published.
    let rows: [(&str, &str); 3] = [
        ("a provider the catalog carries no model of", "acme.model-v1"),
        (
            "a model the catalog does not carry from a provider it does",
            "anthropic.claude-9-opus-20990101-v9:0",
        ),
        ("a model id with a deployment suffix", "meta.llama3-1-8b-instruct-v1:0/abcdef123456"),
    ];
    for (case, model_id) in rows {
        let reply = offers(&emulator, model_id, None).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: must be refused 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: a model the catalog does not carry is a ResourceNotFoundException, \
             got {:?}",
            reply.error
        );
        assert_eq!(
            reply.body.get("message").and_then(Value::as_str),
            Some(bedrock::state::foundation_model_not_found(model_id).as_str()),
            "case {case:?}: the refusal must say the identifier resolved to no model, got {:?}",
            reply.body
        );
    }
}
