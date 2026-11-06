// SPDX-License-Identifier: CC0-1.0

//! Resolving an application provider from ONE `ListApplicationProviders` call, and
//! creating an application under what that call reported — end to end over the
//! `awsJson1_1` wire.
//!
//! The rule under test: the application-provider catalogue is AWS's own data (the modelled
//! `ApplicationProviderArn` pins the owner segment to the literal `aws` and no operation
//! of this service creates, updates or deletes an entry), so an enumeration of it reports
//! the entries in the CATALOGUE'S OWN order — the read imposes no order of its own — one
//! PAGE at a time: at most the caller's `MaxResults`, or this service's default page size
//! when the caller named none, with a `NextToken` whenever entries remain. A client
//! therefore resolves a provider the first page carries, including the generic
//! customer-managed OAuth 2.0 one `CreateApplication` accepts (the catalogue's first
//! entry), from a single unparameterised call, and the application it then creates
//! round-trips through `DescribeApplication`.
//!
//! `application_providers.rs` owns the catalogue's per-entry projection and its paging over
//! every `MaxResults` the model admits; this suite is the one that spans resolving from a
//! single call: its tables differ along the catalogue ENTRY resolved (both published id
//! forms, entries the recorded cases cite and entries they do not), along the PAGE SIZE a
//! first page is asked for (absent, both published bounds of `range(1..100)`, and sizes in
//! between), and along the CREATE that follows the resolution (which optional members it
//! carries, and its `Status`).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed. The harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::application_providers::expected_arns as catalogue_arns;
use crate::ssoadmin_permission_sets::{call, error_code, router, seeded_instance_arn};

/// The model's `MaxResults` bounds for this operation: `range(min: 1, max: 100)`
/// (`smithy_query(query="operation", operation="ListApplicationProviders")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The page size an enumeration that names no `MaxResults` is served with: this service's
/// own default, the one every paginated read of this crate applies.
const DEFAULT_PAGE_SIZE: i64 = state::MAX_PAGE_SIZE as i64;

/// The provider ARN of the generic customer-managed OAuth 2.0 application provider: the
/// one provider `CreateApplication` accepts ("creates an OAuth 2.0 customer managed
/// application ... does not support creating SAML 2.0 customer managed applications or
/// Amazon Web Services managed applications"), and the entry a client has to find before
/// it can create anything.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// One `ListApplicationProviders` call: `(status, body)`.
async fn list(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "ListApplicationProviders", request).await
}

/// The `ApplicationProviderArn` of every entry of one page, in the order reported.
fn page_arns(body: &Value) -> Vec<String> {
    body["ApplicationProviders"]
        .as_array()
        .unwrap_or_else(|| panic!("no ApplicationProviders in {body}"))
        .iter()
        .map(|entry| {
            entry["ApplicationProviderArn"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry without an ApplicationProviderArn: {entry}"))
                .to_string()
        })
        .collect()
}

/// RULE — ONE call that names no `MaxResults` carries the catalogue's FIRST PAGE, in the
/// catalogue's own order, with a `NextToken` whenever entries remain beyond it.
///
/// The table is every entry of that page: both published id forms (an AWS managed
/// provider under its service slug and a pre-integrated application under
/// `catalog/<Name>`), the entry the recorded cases resolve (`custom`) and the entries they
/// never name.
#[tokio::test]
async fn one_unparameterised_call_reports_the_catalogues_first_page() {
    let router = router();
    let (status, body) = list(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "ListApplicationProviders expected 200, got {status} {body}"
    );

    let catalogue = catalogue_arns();
    let expected: Vec<String> = catalogue
        .iter()
        .take(DEFAULT_PAGE_SIZE as usize)
        .cloned()
        .collect();
    let reported = page_arns(&body);
    assert_eq!(
        reported, expected,
        "one call with no MaxResults must report the catalogue's first {DEFAULT_PAGE_SIZE} \
         entries in its own order, got {} of {} entries",
        reported.len(),
        catalogue.len()
    );
    if catalogue.len() > expected.len() {
        assert!(
            body["NextToken"].as_str().is_some_and(|token| !token.is_empty()),
            "entries remain beyond the first page, so it must carry a NextToken, got {body}"
        );
    } else {
        assert_eq!(
            body["NextToken"],
            Value::Null,
            "a page that holds the whole catalogue leaves nothing to resume, got {body}"
        );
    }

    // Every entry of that page is reported once — the generic customer-managed OAuth 2.0
    // provider among them, which is what a resolve-then-create client needs.
    for arn in &expected {
        assert!(
            reported.contains(arn),
            "case {arn:?}: the first unparameterised page must carry this catalogue entry"
        );
    }
    assert!(
        reported.iter().any(|arn| arn == CUSTOM_PROVIDER_ARN),
        "the first unparameterised page must carry {CUSTOM_PROVIDER_ARN}, the only \
         provider CreateApplication accepts, got {} entries",
        reported.len()
    );
}

/// RULE — a cursor of THIS enumeration whose entry the catalogue does not hold cannot be
/// resumed, so it is refused with the modelled `ValidationException` rather than answered
/// with an empty page that would end a caller's enumeration early.
///
/// The rows are cursors this service's own encoder issued for this very enumeration — so
/// only the KEY is wrong — spanning the ways a key can name no entry: a provider id of
/// neither published form, one of the `catalog/<Name>` form, and the empty key.
#[tokio::test]
async fn a_cursor_naming_an_entry_the_catalogue_does_not_hold_is_refused() {
    let router = router();

    for (case, key) in [
        (
            "a provider id of no published form",
            "arn:aws:sso::aws:applicationProvider/not-a-provider",
        ),
        (
            "a catalogue application nothing publishes",
            "arn:aws:sso::aws:applicationProvider/catalog/NotAnApplication",
        ),
        ("the empty key", ""),
    ] {
        let token = state::encode_page_token(state::CURSOR_APPLICATION_PROVIDERS, key);
        let (status, body) = list(&router, &json!({"NextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for the cursor {key:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException for the cursor {key:?}, got {body}"
        );
    }

    // A cursor this enumeration DID issue still resumes: the refusal is about the key the
    // catalogue does not hold, and nothing else.
    let (status, first) = list(&router, &json!({"MaxResults": 1})).await;
    assert_eq!(status, 200, "the first page expected 200, got {status} {first}");
    let issued = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of one provider must issue a cursor: {first}"))
        .to_string();
    let (status, second) = list(&router, &json!({"NextToken": issued})).await;
    assert_eq!(
        status, 200,
        "an issued cursor must resume, got {status} {second}"
    );
    assert_eq!(
        page_arns(&second).first(),
        catalogue_arns().get(1),
        "the resumed page must start at the entry after the first, got {second}"
    );
}

/// RULE — the order is the CATALOGUE's, not an order this read invents: for any page size
/// the model admits, the first page is the catalogue's first entries, in that order.
///
/// The rows differ along the page size: absent, both published bounds of `range(1..100)`,
/// and sizes in between.
#[tokio::test]
async fn the_first_page_is_the_catalogues_first_entries_for_every_page_size() {
    let router = router();
    let catalogue = catalogue_arns();

    for size in [
        None,
        Some(MAX_RESULTS_MIN),
        Some(3),
        Some(37),
        Some(MAX_RESULTS_MAX),
    ] {
        let case = format!("MaxResults = {size:?}");
        let mut request = json!({});
        if let Some(size) = size {
            request["MaxResults"] = json!(size);
        }
        let (status, body) = list(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let reported = page_arns(&body);
        let expected: Vec<String> = catalogue
            .iter()
            .take(size.unwrap_or(DEFAULT_PAGE_SIZE) as usize)
            .cloned()
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: the first page must be the catalogue's first entries, in the \
             catalogue's order"
        );
    }
}

/// RULE — a client resolves the provider from that one call and the create it then issues
/// is served: the application is minted under the resolved provider and reads back
/// through `DescribeApplication`.
///
/// The rows differ along the create that follows the resolution: which optional members it
/// carries (`Description`, `Tags`, `ClientToken`) and its `Status`
/// (`smithy_query(query="operation", operation="CreateApplication")`).
#[tokio::test]
async fn a_provider_resolved_from_one_call_backs_a_create_that_round_trips() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // Resolve the provider exactly as a client does: one unparameterised list call, then
    // the entry whose ARN is the generic customer-managed OAuth 2.0 provider.
    let (status, listed) = list(&router, &json!({})).await;
    assert_eq!(
        status, 200,
        "ListApplicationProviders expected 200, got {status} {listed}"
    );
    let provider_arn = page_arns(&listed)
        .into_iter()
        .find(|arn| arn == CUSTOM_PROVIDER_ARN)
        .unwrap_or_else(|| {
            panic!("one list call must resolve {CUSTOM_PROVIDER_ARN}, got {listed}")
        });

    let rows: Vec<(&str, Value)> = vec![
        ("the required members only", json!({"Name": "resolved-min"})),
        (
            "a Description as well",
            json!({"Name": "resolved-described", "Description": "resolved from one page"}),
        ),
        (
            "Tags and a ClientToken",
            json!({
                "Name": "resolved-tagged",
                "Tags": [{"Key": "Owner", "Value": "platform"}],
                "ClientToken": "resolved-token-1",
            }),
        ),
        (
            "an explicit DISABLED Status",
            json!({"Name": "resolved-disabled", "Status": "DISABLED"}),
        ),
    ];

    for (case, members) in rows {
        let mut request = json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": provider_arn,
        });
        for (member, value) in members.as_object().expect("the row's members") {
            request[member] = value.clone();
        }

        let (status, created) = call(&router, "CreateApplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication under the resolved provider expected 200, \
             got {status} {created}"
        );
        let application_arn = created["ApplicationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create minted no ARN: {created}"))
            .to_string();

        let (status, described) = call(
            &router,
            "DescribeApplication",
            &json!({"ApplicationArn": application_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeApplication expected 200, got {status} {described}"
        );
        assert_eq!(
            described["ApplicationProviderArn"].as_str(),
            Some(provider_arn.as_str()),
            "case {case:?}: the stored application must carry the resolved provider, got \
             {described}"
        );
        assert_eq!(
            described["Name"].as_str(),
            members["Name"].as_str(),
            "case {case:?}: the stored application must carry the Name the create sent, \
             got {described}"
        );
    }
}
