// SPDX-License-Identifier: CC0-1.0

//! WHICH SPELLING of a `NextToken` this service reads back as the cursor it carries.
//!
//! RULE under test: a pagination token is this service's own opaque value, and the only
//! thing a caller may do with one is hand it back UNCHANGED, so the only value a paginated
//! read resumes from is, byte for byte, the token that read handed out. Another spelling of
//! the same cursor — the token re-cased, in whole or in part, a value no response of this
//! service ever carried — is a token the service did not mint, so it earns the same modelled
//! 400 `ValidationException` with `Reason` `CANNOT_PARSE` and `Fields` naming `NextToken`
//! that every other unminted value earns, rather than being served a page.
//!
//! This is a rule about the ENCODER's spelling, not about one listing: the rows span every
//! paginated read whose sequence this fixture can cut into pages (whether its cursor is a
//! stored ARN, a composite report key or a fixed-width position, and whether its sequence is
//! the account's own or one parent resource's), the read whose page size is the server's
//! default alone (`ListAccountAssociations`, which models no `MaxResults`), and, per read,
//! the class of respelling: the whole value re-cased, every other digit re-cased, and a
//! single digit at either end of the value re-cased. Each row also pins that the respelling
//! differs from the issued token ONLY in spelling — it decodes to exactly the same bytes —
//! so what is refused is the spelling and nothing else, and that the issued token itself is
//! still served.
//!
//! The classes of value that are not tokens at all (a word, a store key, hex of the wrong
//! length, another listing's token) are `tests/all/pagination_token_opacity.rs`'s subject.
//!
//! Every assertion goes over the service's real `restJson1` wire through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`], on the shared harness of
//! `tests/all/payer_account_gate.rs` — this service's control plane answers the payer
//! account alone.

use serde_json::{json, Value};

use crate::pagination_token_opacity::period;
use crate::payer_account_gate::{error_code, Emulator};

/// How many pricing plans, pricing rules and billing groups the fixture creates: two is
/// enough that a one-row page always leaves a remainder, so every read below mints a token.
const POPULATION: usize = 2;

/// One paginated read of this service.
struct Read {
    /// The modelled operation, which is also the case name.
    operation: &'static str,
    /// The route the model binds it to.
    path: &'static str,
    /// The output member its page travels in.
    member: &'static str,
    /// The request, minus `NextToken`: the members that pick the sequence being read.
    body: Value,
}

/// The linked account id of the `index`th billing group: twelve digits, as `AccountId`'s
/// modelled `^[0-9]{12}$` requires, and distinct per group so no account is claimed twice.
fn linked_account(index: usize) -> String {
    format!("{:012}", 200_000_000_000_u64 + index as u64)
}

/// Send one setup call that must succeed, and return its body.
async fn setup(em: &Emulator, method: &str, path: &str, body: &Value, label: &str) -> Value {
    let (status, response, _) = em.call(method, path, Some(body)).await;
    assert_eq!(
        status, 200,
        "setup {label}: expected HTTP 200 from {path}, got {status} with {response}"
    );
    response
}

/// Send one setup call that must succeed, and return the `Arn` it answered with.
async fn setup_arn(em: &Emulator, method: &str, path: &str, body: &Value, label: &str) -> String {
    let response = setup(em, method, path, body, label).await;
    response
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {label}: expected an Arn member, got {response}"))
        .to_string()
}

/// Stand up the reads whose sequences this fixture cuts into pages: two pricing plans, two
/// pricing rules (both on the first plan, the first of them on the second plan too), two
/// billing groups each with its own linked account, two custom line items in the first
/// group, a percentage item two resources are associated to, and an item carrying two
/// generations.
async fn fixture(em: &Emulator) -> Vec<Read> {
    let mut plans: Vec<String> = Vec::new();
    for index in 0..POPULATION {
        plans.push(
            setup_arn(
                em,
                "POST",
                "/create-pricing-plan",
                &json!({ "Name": format!("SpellingPlan{index}") }),
                &format!("pricing plan {index}"),
            )
            .await,
        );
    }

    let mut rules: Vec<String> = Vec::new();
    for index in 0..POPULATION {
        rules.push(
            setup_arn(
                em,
                "POST",
                "/create-pricing-rule",
                &json!({
                    "Name": format!("SpellingRule{index}"),
                    "Scope": "GLOBAL",
                    "Type": "DISCOUNT",
                    "ModifierPercentage": 7.5,
                }),
                &format!("pricing rule {index}"),
            )
            .await,
        );
    }
    setup(
        em,
        "PUT",
        "/associate-pricing-rules",
        &json!({ "Arn": plans[0], "PricingRuleArns": rules }),
        "every rule on the first plan",
    )
    .await;
    setup(
        em,
        "PUT",
        "/associate-pricing-rules",
        &json!({ "Arn": plans[1], "PricingRuleArns": [rules[0]] }),
        "the first rule on the second plan",
    )
    .await;

    let mut groups: Vec<String> = Vec::new();
    for index in 0..POPULATION {
        let account = linked_account(index);
        groups.push(
            setup_arn(
                em,
                "POST",
                "/create-billing-group",
                &json!({
                    "Name": format!("SpellingGroup{index}"),
                    "AccountGrouping": { "LinkedAccountIds": [account] },
                    "ComputationPreference": { "PricingPlanArn": plans[0] },
                    "PrimaryAccountId": account,
                }),
                &format!("billing group {index}"),
            )
            .await,
        );
    }

    let mut items: Vec<String> = Vec::new();
    for index in 0..POPULATION {
        items.push(
            setup_arn(
                em,
                "POST",
                "/create-custom-line-item",
                &json!({
                    "Name": format!("SpellingItem{index}"),
                    "Description": "an item of the token-spelling fixture",
                    "BillingGroupArn": groups[0],
                    "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
                }),
                &format!("custom line item {index}"),
            )
            .await,
        );
    }

    // Resources are associated to a PERCENTAGE item only, so the association listing needs
    // one of those, with two resources on it.
    let percentage = setup_arn(
        em,
        "POST",
        "/create-custom-line-item",
        &json!({
            "Name": "SpellingPercentage",
            "Description": "a percentage item of the token-spelling fixture",
            "BillingGroupArn": groups[0],
            "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 10.0 } },
        }),
        "the percentage custom line item",
    )
    .await;
    let associated = vec![groups[0].clone(), items[0].clone()];
    let response = setup(
        em,
        "PUT",
        "/batch-associate-resources-to-custom-line-item",
        &json!({ "TargetArn": percentage, "ResourceArns": associated }),
        "two resources on the percentage item",
    )
    .await;
    assert_eq!(
        response
            .get("SuccessfullyAssociatedResources")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default(),
        associated.len(),
        "setup associations: expected every resource associated, got {response}"
    );

    // A custom line item with TWO generations: created in the previous billing period and
    // written again from the current one, which opens a second generation rather than
    // replacing the first.
    let versioned = setup_arn(
        em,
        "POST",
        "/create-custom-line-item",
        &json!({
            "Name": "SpellingVersioned",
            "Description": "a versioned item of the token-spelling fixture",
            "BillingGroupArn": groups[0],
            "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 2.0 } },
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(-1) },
        }),
        "the versioned custom line item",
    )
    .await;
    setup(
        em,
        "POST",
        "/update-custom-line-item",
        &json!({
            "Arn": versioned,
            "Description": "the second generation of the versioned item",
            "BillingPeriodRange": { "InclusiveStartBillingPeriod": period(0) },
        }),
        "the second generation of the versioned item",
    )
    .await;

    vec![
        Read {
            operation: "ListBillingGroups",
            path: "/list-billing-groups",
            member: "BillingGroups",
            body: json!({}),
        },
        Read {
            operation: "ListPricingPlans",
            path: "/list-pricing-plans",
            member: "PricingPlans",
            body: json!({}),
        },
        Read {
            operation: "ListPricingRules",
            path: "/list-pricing-rules",
            member: "PricingRules",
            body: json!({}),
        },
        Read {
            operation: "ListCustomLineItems",
            path: "/list-custom-line-items",
            member: "CustomLineItems",
            body: json!({}),
        },
        Read {
            operation: "ListBillingGroupCostReports",
            path: "/list-billing-group-cost-reports",
            member: "BillingGroupCostReports",
            body: json!({}),
        },
        Read {
            operation: "ListPricingRulesAssociatedToPricingPlan",
            path: "/list-pricing-rules-associated-to-pricing-plan",
            member: "PricingRuleArns",
            body: json!({ "PricingPlanArn": plans[0] }),
        },
        Read {
            operation: "ListPricingPlansAssociatedWithPricingRule",
            path: "/list-pricing-plans-associated-with-pricing-rule",
            member: "PricingPlanArns",
            body: json!({ "PricingRuleArn": rules[0] }),
        },
        Read {
            operation: "ListResourcesAssociatedToCustomLineItem",
            path: "/list-resources-associated-to-custom-line-item",
            member: "AssociatedResources",
            body: json!({ "Arn": percentage }),
        },
        Read {
            operation: "ListCustomLineItemVersions",
            path: "/list-custom-line-item-versions",
            member: "CustomLineItemVersions",
            body: json!({ "Arn": versioned }),
        },
    ]
}

/// `read`'s request with the extra members `extra` carries written onto it.
fn request(body: &Value, extra: Value) -> Value {
    let mut request = body.clone();
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    request
}

/// The `NextToken` a page of `read` hands back, asked for with the page-size members
/// `extra` carries — `MaxResults: 1` for a read that models that member, and nothing at all
/// for a read whose page size is the server's.
async fn minted_token(em: &Emulator, read: &Read, extra: &Value) -> String {
    let body = request(&read.body, extra.clone());
    let (status, response, _) = em.call("POST", read.path, Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {}: expected a first page, got {status} with {response}",
        read.operation
    );
    response
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {}: expected a NextToken for a one-row page of a longer sequence, got \
                 {response}",
                read.operation
            )
        })
        .to_string()
}

/// How many respellings of an issued token [`respellings`] produces: the whole value, every
/// other digit, and one digit at each end.
const RESPELLING_CLASSES: usize = 4;

/// The respellings of the issued token `issued` this suite drives: values that decode to
/// exactly the bytes `issued` decodes to — a decoder reads `A-F` and `a-f` alike — and that
/// the minter, which writes lower-case hex only, never wrote.
///
/// The classes span how much of a token a client could re-case: all of it (a value read out
/// of an upper-casing log or form), every other digit, and a single digit at either end. A
/// class whose digit the token does not carry is left out, and
/// [`RESPELLING_CLASSES`] is what holds a row set this short to being the whole table.
fn respellings(issued: &str) -> Vec<(&'static str, String)> {
    let recased = |positions: &dyn Fn(usize) -> bool| -> String {
        issued
            .chars()
            .enumerate()
            .map(|(index, digit)| {
                if positions(index) {
                    digit.to_ascii_uppercase()
                } else {
                    digit
                }
            })
            .collect()
    };
    let letters: Vec<usize> = issued
        .char_indices()
        .filter(|(_, digit)| digit.is_ascii_alphabetic())
        .map(|(index, _)| index)
        .collect();
    let mut rows = vec![
        ("every hex digit upper-cased", recased(&|_| true)),
        (
            "every other hex digit upper-cased",
            recased(&|index| index % 2 == 0),
        ),
    ];
    for (class, position) in [
        ("the first hex letter upper-cased", letters.first()),
        ("the last hex letter upper-cased", letters.last()),
    ] {
        if let Some(&position) = position {
            rows.push((class, recased(&move |index| index == position)));
        }
    }
    rows
}

/// Assert `read`, asked to resume from `body`'s `NextToken`, is refused as a token this
/// service did not mint: the modelled 400 `ValidationException` with `Reason`
/// `CANNOT_PARSE`, `Fields` naming `NextToken` — and no page at all.
async fn assert_refused(em: &Emulator, read: &Read, case: &str, body: &Value) {
    let (status, response, error_type) = em.call("POST", read.path, Some(body)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a token this service did not mint, got {status} \
         with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {response}"
    );
    assert_eq!(
        error_type
            .as_deref()
            .map(|value| value.rsplit('#').next().unwrap_or_default().to_string()),
        Some("ValidationException".to_string()),
        "case {case:?}: expected the error shape in x-amzn-errortype, got {error_type:?}"
    );
    assert_eq!(
        response.get("Reason").and_then(Value::as_str),
        Some("CANNOT_PARSE"),
        "case {case:?}: expected Reason CANNOT_PARSE, got {response}"
    );
    let fields: Vec<String> = response
        .get("Fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|field| field.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    assert_eq!(
        fields,
        vec!["NextToken".to_string()],
        "case {case:?}: expected Fields to name NextToken, got {response}"
    );
    assert!(
        response.get(read.member).is_none(),
        "case {case:?}: the refusal must carry no {} page, got {response}",
        read.member
    );
    assert!(
        response.get("NextToken").is_none(),
        "case {case:?}: the refusal must carry no NextToken, got {response}"
    );
}

/// Assert every respelling of the token `read` issued is refused, that each one differs from
/// that token in SPELLING alone, and that the token as issued is still served.
async fn assert_only_the_issued_spelling_resumes(
    em: &Emulator,
    read: &Read,
    issued: &str,
    extra: Value,
) {
    let rows = respellings(issued);
    assert_eq!(
        rows.len(),
        RESPELLING_CLASSES,
        "case {}: expected every respelling class to apply to the issued token {issued:?}, got \
         {} of them",
        read.operation,
        rows.len()
    );

    for (class, respelt) in rows {
        let case = format!("{} given its own token with {class}", read.operation);
        assert_ne!(
            respelt, issued,
            "case {case:?}: a respelling must differ from the token that was issued"
        );
        assert_eq!(
            ::hex::decode(&respelt).ok(),
            ::hex::decode(issued).ok(),
            "case {case:?}: a respelling must carry the same bytes as the issued token, so the \
             spelling is the only difference, got {respelt:?}"
        );
        let body = request(&read.body, request(&extra, json!({ "NextToken": respelt })));
        assert_refused(em, read, &case, &body).await;
    }

    // The token AS ISSUED still resumes: the rule narrows which spelling is read, it does
    // not stop a caller handing a token back unchanged.
    let body = request(&read.body, request(&extra, json!({ "NextToken": issued })));
    let (status, response, _) = em.call("POST", read.path, Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {}: expected the token as issued to resume the listing, got {status} with \
         {response}",
        read.operation
    );
    assert!(
        response.get(read.member).and_then(Value::as_array).is_some(),
        "case {}: expected a resumed page of {}, got {response}",
        read.operation,
        read.member
    );
}

/// THE RULE: every paginated read that hands out a `NextToken` resumes from that token in
/// the ONE spelling it was handed out in, and refuses every other spelling of it.
#[tokio::test]
async fn a_respelt_next_token_is_refused_by_every_paginated_read() {
    let em = Emulator::payer();
    let reads = fixture(&em).await;

    let page_size = json!({ "MaxResults": 1 });
    for read in &reads {
        let issued = minted_token(&em, read, &page_size).await;
        assert!(
            issued
                .chars()
                .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit)),
            "case {}: expected a token of lower-case hex digits, got {issued:?}",
            read.operation
        );
        assert_only_the_issued_spelling_resumes(&em, read, &issued, page_size.clone()).await;
    }
}

/// THE RULE on the read whose page size is the SERVER's alone: `ListAccountAssociations`
/// models no `MaxResults`, so the only token it hands out is the one its default-sized page
/// mints — and that token, too, is read back only as it was spelled.
#[tokio::test]
async fn a_respelt_next_token_is_refused_by_the_account_listing() {
    /// The default page size the server picks for a read that cannot be asked for one, so
    /// one more account than this makes the listing hand out a token.
    const DEFAULT_PAGE_SIZE: usize = 100;
    /// How many account ids one `AccountGrouping`/`AccountIds` list may carry, so the
    /// fixture's accounts are claimed in chunks.
    const ACCOUNTS_PER_CALL: usize = 30;

    let em = Emulator::payer();
    let plan = setup_arn(
        &em,
        "POST",
        "/create-pricing-plan",
        &json!({ "Name": "SpellingAccountsPlan" }),
        "the pricing plan",
    )
    .await;
    let accounts: Vec<String> = (0..=DEFAULT_PAGE_SIZE).map(linked_account).collect();
    let group = setup_arn(
        &em,
        "POST",
        "/create-billing-group",
        &json!({
            "Name": "SpellingAccountsGroup",
            "AccountGrouping": { "LinkedAccountIds": &accounts[..ACCOUNTS_PER_CALL] },
            "ComputationPreference": { "PricingPlanArn": plan },
            "PrimaryAccountId": accounts[0].as_str(),
        }),
        "the billing group",
    )
    .await;
    for chunk in accounts[ACCOUNTS_PER_CALL..].chunks(ACCOUNTS_PER_CALL) {
        setup(
            &em,
            "POST",
            "/associate-accounts",
            &json!({ "Arn": group, "AccountIds": chunk }),
            "a chunk of linked accounts",
        )
        .await;
    }

    let read = Read {
        operation: "ListAccountAssociations",
        path: "/list-account-associations",
        member: "LinkedAccounts",
        body: json!({}),
    };
    let issued = minted_token(&em, &read, &json!({})).await;
    assert_only_the_issued_spelling_resumes(&em, &read, &issued, json!({})).await;
}
