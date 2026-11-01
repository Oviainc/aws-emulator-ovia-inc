// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `DeleteAcmeEndpoint` takes with the endpoint it removes.
//!
//! RULE: deleting an ACME endpoint removes the record its `AcmeEndpointArn` names and
//! every record that exists ONLY on that endpoint — the ACME accounts registered with it,
//! its external account bindings and its domain validations — whatever status the endpoint
//! carried when the call arrived, so no operation answers for that ARN or for any ARN
//! under it afterwards, while every OTHER endpoint's records are left exactly as they
//! were; and because the call is `@idempotent` and its model declares no
//! missing-resource error
//! (`smithy_query(query="operation", operation="DeleteAcmeEndpoint")` reports
//! `AccessDeniedException`, `ConflictException`, `InternalServerException`,
//! `ThrottlingException` and `ValidationException` only), an ARN of this account that no
//! record answers to is already the state the call asks for and is served.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and what the delete did is read back through
//! DIFFERENT operations — the endpoint's own describe, the three listings of what hangs
//! off an endpoint, and each sub-resource's own describe — so an answer that is not
//! computed from the persisted store cannot pass. The wire harness, the endpoint and
//! validation fixtures and the clock helpers are shared with
//! [`crate::certificate_lifecycle`], [`crate::acme_endpoint_lifecycle`],
//! [`crate::acme_domain_validation_lifecycle`],
//! [`crate::acme_external_account_binding_lifecycle`] and
//! [`crate::acme_directory_endpoint`].
//!
//! The table spans what the rule generalises over: the status the endpoint carries when
//! the delete arrives (`CREATING` and `ACTIVE`, the two `AcmeEndpointStatus` values an
//! endpoint of this emulator stands in) crossed with WHICH kinds of dependent record the
//! endpoint holds and how many (none at all, one of each, several of each, and each kind
//! on its own).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_directory_endpoint::{activate_endpoints_now, hold_endpoints_creating};
use crate::acme_domain_validation_lifecycle::{create_validation, endpoint_body};
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::acme_external_account_binding_lifecycle::{binding_body, binding_cases, create_binding};
use crate::certificate_lifecycle::{call, error_code, error_message, router_with_engine, REGION};

/// Every `AcmeAccountStatus` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeAccountStatus")`), so the
/// seeded accounts of one endpoint do not all carry the same one.
const ACCOUNT_STATUSES: &[&str] = &[
    state::ACME_ACCOUNT_STATUS_VALID,
    state::ACME_ACCOUNT_STATUS_DEACTIVATED,
    state::ACME_ACCOUNT_STATUS_REVOKED,
];

/// Each operation that REPORTS what hangs off one ACME endpoint: its name, the list
/// member it answers in, and the member each item is identified by
/// (`smithy_query(query="operation", operation="ListAcmeAccounts")` and its two
/// siblings).
const LISTINGS: &[(&str, &str, &str)] = &[
    ("ListAcmeAccounts", "AcmeAccounts", "AccountUrl"),
    (
        "ListAcmeExternalAccountBindings",
        "ExternalAccountBindings",
        "AcmeExternalAccountBindingArn",
    ),
    (
        "ListAcmeDomainValidations",
        "AcmeDomainValidations",
        "AcmeDomainValidationArn",
    ),
];

/// One row of the table: what it exercises, whether the endpoints are carried to
/// `ACTIVE` before the delete, and how many records of each dependent kind each endpoint
/// holds.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    active: bool,
    accounts: usize,
    bindings: usize,
    validations: usize,
}

/// The table: the status the delete arrives on, crossed with which dependent kinds the
/// endpoint holds and how many of each.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an ACTIVE endpoint holding several of every dependent kind",
            active: true,
            accounts: 3,
            bindings: 2,
            validations: 2,
        },
        Row {
            case: "an endpoint still CREATING, holding one of each kind",
            active: false,
            accounts: 1,
            bindings: 1,
            validations: 1,
        },
        Row {
            case: "an ACTIVE endpoint holding registered accounts only",
            active: true,
            accounts: 2,
            bindings: 0,
            validations: 0,
        },
        Row {
            case: "an endpoint still CREATING, holding external account bindings only",
            active: false,
            accounts: 0,
            bindings: 2,
            validations: 0,
        },
        Row {
            case: "an ACTIVE endpoint holding domain validations only",
            active: true,
            accounts: 0,
            bindings: 0,
            validations: 3,
        },
        Row {
            case: "an endpoint still CREATING, holding nothing at all",
            active: false,
            accounts: 0,
            bindings: 0,
            validations: 0,
        },
    ]
}

/// The records created under one endpoint, by the ARN or URL each of them is addressed
/// by — in the same order as [`LISTINGS`].
#[derive(Debug, Default)]
struct Dependents {
    account_urls: Vec<String>,
    binding_arns: Vec<String>,
    validation_arns: Vec<String>,
}

impl Dependents {
    /// The values the listing answering in `list_member` must report, sorted.
    fn expected(&self, list_member: &str) -> Vec<String> {
        let mut values = match list_member {
            "AcmeAccounts" => self.account_urls.clone(),
            "ExternalAccountBindings" => self.binding_arns.clone(),
            "AcmeDomainValidations" => self.validation_arns.clone(),
            other => panic!("{other} is not a listing this suite knows"),
        };
        values.sort();
        values
    }
}

/// The endpoint id of an ACME endpoint ARN: the part after `acme-endpoint/`.
fn endpoint_id(arn: &str) -> &str {
    arn.rsplit_once("acme-endpoint/")
        .unwrap_or_else(|| panic!("{arn} is not an ACME endpoint ARN"))
        .1
}

/// Register `count` ACME accounts with `endpoint_arn` in the store, as an ACME client's
/// own `newAccount` request to the endpoint's directory URL would.
///
/// Seeded rather than created through an operation because nothing in this service's API
/// registers one ("ACME accounts are created when clients use external account binding
/// credentials to register", `ListAcmeAccounts` doc), and the account URL is rendered by
/// the emulator's own [`state::acme_account_url`], so this cannot invent a URL form the
/// handler would not report. Answers the account URLs written.
fn register_accounts(
    engine: &Arc<engine::InMemoryEngine>,
    endpoint_arn: &str,
    label: &str,
    count: usize,
    binding_arn: Option<&str>,
) -> Vec<String> {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let created_at = store
        .acme_endpoints
        .get(endpoint_arn)
        .unwrap_or_else(|| panic!("{endpoint_arn} must be a stored ACME endpoint"))
        .created_at;
    let mut urls = Vec::new();
    for nth in 0..count {
        let seconds = u64::try_from(nth).expect("the table is small");
        let account_url = state::acme_account_url(
            REGION,
            endpoint_id(endpoint_arn),
            &format!("{label}-account-{nth}"),
        );
        let record = state::AcmeAccountRecord {
            acme_endpoint_arn: endpoint_arn.to_string(),
            account_url: account_url.clone(),
            // A JWK thumbprint is base64url text (RFC 7638 §3); what matters here is
            // that each account carries its own, so two cannot be confused.
            public_key_thumbprint: format!("thumbprint-{label}-{nth}"),
            // The account key that thumbprint is the digest of; nothing here verifies a
            // signature under it, so what matters is that each account carries its own.
            public_key_jwk: json!({
                "crv": "P-256",
                "kty": "EC",
                "x": format!("x-{label}-{nth}"),
                "y": format!("y-{label}-{nth}"),
            })
            .to_string(),
            status: ACCOUNT_STATUSES[nth % ACCOUNT_STATUSES.len()].to_string(),
            acme_external_account_binding_arn: binding_arn.map(str::to_string),
            contacts: vec![format!("mailto:{label}-{nth}@example.com")],
            created_at: created_at + Duration::from_secs(seconds + 1),
        };
        store
            .acme_accounts
            .insert((endpoint_arn.to_string(), account_url.clone()), record);
        urls.push(account_url);
    }
    urls
}

/// One endpoint holding the dependent records `row` calls for, beside what was created
/// under it. `label` keeps one row's two endpoints' fixtures apart.
async fn given_endpoint_with(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    label: &str,
    row: &Row,
) -> (String, Dependents) {
    let endpoint_arn = create_endpoint(router, case, &endpoint_body()).await;
    let mut dependents = Dependents::default();

    for (nth, binding_case) in binding_cases().into_iter().take(row.bindings).enumerate() {
        let created = create_binding(
            router,
            &format!("{case}: {label} binding #{nth}"),
            &binding_body(&binding_case, &endpoint_arn),
        )
        .await;
        dependents.binding_arns.push(
            created["AcmeExternalAccountBindingArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the create reported no binding ARN in {created}")
                })
                .to_string(),
        );
    }
    for nth in 0..row.validations {
        dependents.validation_arns.push(
            create_validation(
                router,
                &format!("{case}: {label} validation #{nth}"),
                &json!({
                    "AcmeEndpointArn": endpoint_arn,
                    "DomainName": format!("{label}-{nth}.example.com"),
                    "PrevalidationOptions": { "DnsPrevalidation": {} },
                }),
            )
            .await,
        );
    }
    dependents.account_urls = register_accounts(
        engine,
        &endpoint_arn,
        label,
        row.accounts,
        dependents.binding_arns.first().map(String::as_str),
    );

    (endpoint_arn, dependents)
}

/// `DeleteAcmeEndpoint` for `arn`, asserted to be served with the empty body its
/// modelled output carries.
async fn delete_endpoint(router: &MultiServiceRouter, case: &str, arn: &str) {
    let (status, answer) = call(
        router,
        "DeleteAcmeEndpoint",
        &json!({ "AcmeEndpointArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteAcmeEndpoint({arn}) must be served, got {status} {answer}"
    );
    assert!(
        answer.is_null() || answer.as_object().is_some_and(|members| members.is_empty()),
        "case {case:?}: DeleteAcmeEndpoint's output carries no members, got {answer}"
    );
}

/// The identifiers the listing `(operation, list_member, key_member)` reports for
/// `endpoint_arn`, sorted — asserted to be served.
async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    listing: &(&str, &str, &str),
    endpoint_arn: &str,
) -> Vec<String> {
    let (operation, list_member, key_member) = *listing;
    let (status, response) = call(
        router,
        operation,
        &json!({ "AcmeEndpointArn": endpoint_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation}({endpoint_arn}) must succeed, got {status} {response}"
    );
    let mut values: Vec<String> = response[list_member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {list_member} list in {response}"))
        .iter()
        .map(|item| {
            item[key_member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an item carried no {key_member}: {item}"))
                .to_string()
        })
        .collect();
    values.sort();
    values
}

/// Assert `operation`, addressing `arn` through `arn_member`, reports it missing.
async fn assert_missing(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    arn_member: &str,
    arn: &str,
) -> Value {
    let mut body = json!({});
    body[arn_member] = json!(arn);
    let (status, response) = call(router, operation, &body).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: {operation} must report {arn} missing, got {status} {response}"
    );
    response
}

/// Assert every record created under `endpoint_arn` is still reported, through each
/// listing of what hangs off an endpoint and through the endpoint's own describe.
async fn assert_intact(
    router: &MultiServiceRouter,
    case: &str,
    endpoint_arn: &str,
    dependents: &Dependents,
) {
    let (status, response) = call(
        router,
        "DescribeAcmeEndpoint",
        &json!({ "AcmeEndpointArn": endpoint_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {endpoint_arn} must still be describable, got {status} {response}"
    );
    for listing in LISTINGS {
        let reported = listed(router, case, listing, endpoint_arn).await;
        let expected = dependents.expected(listing.1);
        assert_eq!(
            reported, expected,
            "case {case:?}: {} must still report {expected:?} for {endpoint_arn}, got {reported:?}",
            listing.0
        );
    }
}

/// RULE: the delete removes the endpoint and every record that exists only on it — its
/// accounts, its external account bindings and its domain validations — whatever status
/// it carried, leaves another endpoint's identical records alone, and repeating it is
/// served because the store is already in the state it asks for.
#[tokio::test]
async fn deleting_an_endpoint_removes_every_record_that_exists_only_on_it() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = router_with_engine();
        let (kept, kept_records) = given_endpoint_with(&router, &engine, case, "kept", &row).await;
        let (doomed, doomed_records) =
            given_endpoint_with(&router, &engine, case, "doomed", &row).await;

        // Pin the status the delete arrives on: either the activation both endpoints are
        // owed is made due, or it is held out of this test's way, so the row's
        // precondition cannot race the wall clock.
        let expected_status = if row.active {
            activate_endpoints_now(&engine);
            state::ACME_ENDPOINT_STATUS_ACTIVE
        } else {
            hold_endpoints_creating(&engine);
            state::ACME_ENDPOINT_STATUS_CREATING
        };
        let (status, response) = call(
            &router,
            "DescribeAcmeEndpoint",
            &json!({ "AcmeEndpointArn": doomed }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the endpoint under test must be describable first, got {status} \
             {response}"
        );
        assert_eq!(
            response["AcmeEndpoint"]["Status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected the delete to arrive on {expected_status}, got {response}"
        );
        assert_intact(&router, case, &doomed, &doomed_records).await;
        assert_intact(&router, case, &kept, &kept_records).await;

        delete_endpoint(&router, case, &doomed).await;

        // The endpoint itself is gone, in this service's own missing-resource wording.
        let response = assert_missing(
            &router,
            case,
            "DescribeAcmeEndpoint",
            "AcmeEndpointArn",
            &doomed,
        )
        .await;
        assert_eq!(
            error_message(&response),
            state::acme_endpoint_not_found_message(&doomed),
            "case {case:?}: expected this service's not-found sentence, got {response}"
        );

        // And so is everything that existed only on it: no listing of what hangs off that
        // endpoint answers for the ARN any more, and each sub-resource's own describe
        // reports it missing.
        for (operation, _list_member, _key_member) in LISTINGS {
            assert_missing(&router, case, operation, "AcmeEndpointArn", &doomed).await;
        }
        for arn in &doomed_records.binding_arns {
            assert_missing(
                &router,
                case,
                "DescribeAcmeExternalAccountBinding",
                "AcmeExternalAccountBindingArn",
                arn,
            )
            .await;
        }
        for arn in &doomed_records.validation_arns {
            assert_missing(
                &router,
                case,
                "DescribeAcmeDomainValidation",
                "AcmeDomainValidationArn",
                arn,
            )
            .await;
        }
        // The endpoint's accounts are reachable through no other operation once its own
        // listing refuses, so they are read straight out of the store the handler writes.
        {
            let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
            let store = store.lock();
            let left: Vec<&str> = store
                .acme_accounts_of(&doomed)
                .map(|account| account.account_url.as_str())
                .collect();
            assert!(
                left.is_empty(),
                "case {case:?}: the endpoint's accounts must go with it, got {left:?}"
            );
        }

        // Nothing of the OTHER endpoint went with it, and repeating the delete asks for
        // the state the store is already in, so it is served and still removes nothing
        // else.
        assert_intact(&router, case, &kept, &kept_records).await;
        delete_endpoint(&router, &format!("{case}, repeated"), &doomed).await;
        assert_intact(&router, case, &kept, &kept_records).await;
    }
}

/// RULE: an `AcmeEndpointArn` of this account that no record answers to is already the
/// state the delete asks for, so the call is served rather than refused — and it removes
/// nothing else.
///
/// The rows differ along what the rule generalises over: the shape of the id the ARN
/// carries (one never minted, the shortest the ARN pattern's `[a-zA-Z0-9-]+` admits, one
/// of the maximum length that pattern and the member's 1..200 length admit) and an
/// endpoint that really existed and has been deleted.
#[tokio::test]
async fn deleting_an_endpoint_no_record_answers_to_is_served() {
    let row = Row {
        case: "the endpoint standing beside the missing ones",
        active: true,
        accounts: 2,
        bindings: 1,
        validations: 1,
    };
    let (router, engine) = router_with_engine();
    let (kept, kept_records) = given_endpoint_with(&router, &engine, row.case, "kept", &row).await;
    let (gone, _gone_records) =
        given_endpoint_with(&router, &engine, row.case, "gone", &row).await;
    activate_endpoints_now(&engine);
    delete_endpoint(&router, "the endpoint deleted first", &gone).await;

    // The longest id this account's ARN pattern and the member's modelled length (1..200)
    // leave room for, so the table reaches that bound as well as the short end.
    let prefix = state::acme_endpoint_arn(REGION, "");
    let longest = "b".repeat(200 - prefix.len());

    for (case, arn) in [
        (
            "an endpoint id this account never minted",
            state::acme_endpoint_arn(REGION, "00000000-0000-4000-8000-000000000000"),
        ),
        (
            "the shortest id the ARN pattern admits",
            state::acme_endpoint_arn(REGION, "a"),
        ),
        (
            "the longest id the ARN's modelled length admits",
            state::acme_endpoint_arn(REGION, &longest),
        ),
        ("an endpoint that was already deleted", gone.clone()),
    ] {
        delete_endpoint(&router, case, &arn).await;
        assert_missing(
            &router,
            case,
            "DescribeAcmeEndpoint",
            "AcmeEndpointArn",
            &arn,
        )
        .await;
        // Each served no-op left the endpoint that does exist, and everything under it,
        // exactly as it was.
        assert_intact(&router, case, &kept, &kept_records).await;
    }
}

/// RULE: the delete removes what resolves to the endpoint as well — a `CreateAcmeEndpoint`
/// idempotency token whose endpoint has been deleted stands for nothing, so re-sending it
/// mints a NEW endpoint instead of answering an ARN with no record behind it, while the
/// same token re-sent before the delete resolves to the endpoint it made.
#[tokio::test]
async fn an_idempotency_token_whose_endpoint_was_deleted_mints_a_new_one() {
    let (router, _engine) = router_with_engine();
    let mut body = endpoint_body();
    body["IdempotencyToken"] = json!("delete-then-create-again");

    let first = create_endpoint(&router, "the first create", &body).await;
    let replayed = create_endpoint(&router, "the token re-sent", &body).await;
    assert_eq!(
        replayed, first,
        "case \"the token re-sent\": a live token must resolve to the endpoint it made, got \
         {replayed} for {first}"
    );

    delete_endpoint(&router, "the token's endpoint", &first).await;

    let second = create_endpoint(&router, "the token after the delete", &body).await;
    assert_ne!(
        second, first,
        "case \"the token after the delete\": a token whose endpoint is gone must mint a new \
         endpoint, got the deleted {first} again"
    );
    let (status, response) = call(
        &router,
        "DescribeAcmeEndpoint",
        &json!({ "AcmeEndpointArn": second }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the token after the delete\": the ARN it answered must have a record behind it, \
         got {status} {response}"
    );
    assert_eq!(
        response["AcmeEndpoint"]["AcmeEndpointArn"].as_str(),
        Some(second.as_str()),
        "case \"the token after the delete\": expected the record of {second}, got {response}"
    );
    assert_missing(
        &router,
        "the deleted endpoint",
        "DescribeAcmeEndpoint",
        "AcmeEndpointArn",
        &first,
    )
    .await;
}
