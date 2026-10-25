// SPDX-License-Identifier: CC0-1.0

//! `ListApplications` end to end over the `awsJson1_1` wire — the operation the stub
//! issue reported as an unimplemented scaffold.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. the enumeration reports EXACTLY the applications stored under the instance the
//!    request names, one item per stored application, each carrying the member-for-member
//!    view `DescribeApplication` reports for the same record, in one stable
//!    `ApplicationArn` order whatever order the creates arrived in — and an instance that
//!    holds none is an empty list, not a refusal;
//! 2. each member of `ListApplicationsFilter` NARROWS the enumeration to the applications
//!    whose own member equals it (both together are a conjunction), and a value no stored
//!    application carries selects none;
//! 3. it TRACKS the store: an update of an application changes that application's item and
//!    leaves every other one, and an application belongs to its instance, so no listing of
//!    one instance shows another instance's applications and an instance whose deletion
//!    has fallen due holds none;
//! 4. it is a REPORT: repeating it answers identically and changes nothing a later caller
//!    sees;
//! 5. it pages: for every `MaxResults` the model admits, walking the pages yields each
//!    stored application exactly once in that same order, no page carries more than the
//!    page size asked for nor more than the modelled `ApplicationList` holds, and the last
//!    page carries no `NextToken`;
//! 6. an `InstanceArn` this account holds no instance for is refused on the modelled
//!    client-input channel — never an empty list and never the scaffold's HTTP 501;
//! 7. the `NextToken` is this service's own cursor: one it did not issue for THIS
//!    enumeration (including one issued for the same enumeration under another filter) is
//!    refused `ValidationException`, and resuming from one it did issue yields exactly the
//!    stored applications that sort after the key it carries.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, hold_deletion, router_over, seeded_instance_arn, with_store, TEST_REGION,
    UNKNOWN_INSTANCE_ARN,
};

/// The one published provider an application can be CREATED under.
///
/// SOURCE: `state::BuiltInApplicationProvider::accepts_customer_managed_application` over
/// `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue `ListApplicationProviders`
/// publishes over the wire.
const PROVIDER_CUSTOM: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// Two further entries of that published catalogue, used as filter values and as the
/// provider of an application an AWS service created on the customer's behalf. Entries the
/// issue did not cite.
const PROVIDER_ATHENA: &str = "arn:aws:sso::aws:applicationProvider/athena";
const PROVIDER_CATALOG_STATUSPAGE: &str = "arn:aws:sso::aws:applicationProvider/catalog/Statuspage";

/// The account id a filter can name that is NOT the account this emulator serves
/// (`AccountId`: `^\d{12}$`).
const OTHER_ACCOUNT: &str = "210987654321";

/// The id of the SECOND instance the instance-scoping rows are asserted against:
/// `ssoins-` + 16 of `[a-zA-Z0-9-.]`, as the model's `InstanceArn` pattern requires.
const SECOND_INSTANCE_ID: &str = "ssoins-5f4e3d2c1b0a9988";

/// Its identity store: `d-` + 10 hex digits, per `InstanceIdentityStoreArn`.
const SECOND_IDENTITY_STORE_ID: &str = "d-5a4b3c2d1e";

/// Every member the model declares for an `Application` item, and no other.
///
/// SOURCE: `smithy_query(query="shape",shape="Application",
/// model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`.
const MODELLED_ITEM_MEMBERS: [&str; 11] = [
    "ApplicationAccount",
    "ApplicationArn",
    "ApplicationProviderArn",
    "CreatedDate",
    "CreatedFrom",
    "Description",
    "IdentityStoreArn",
    "InstanceArn",
    "Name",
    "PortalOptions",
    "Status",
];

/// The ceiling on one page, from the model: `ApplicationList` is `length(min: 0, max: 50)`
/// while `MaxResults` is `range(min: 1, max: 100)`, so a page carries at most 50 items
/// however many a caller asks for.
///
/// SOURCE: `smithy_query(query="shape",shape="ApplicationList,MaxResults")`.
const MAX_ITEMS_PER_PAGE: usize = 50;

/// One `CreateApplication` request: a row of the application table.
#[derive(Debug, Clone)]
struct AppRow {
    label: &'static str,
    name: String,
    description: Option<String>,
    status: Option<&'static str>,
    portal_options: Option<Value>,
}

impl AppRow {
    fn request(&self, instance_arn: &str) -> Value {
        let mut body = json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": PROVIDER_CUSTOM,
            "Name": self.name,
        });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(status) = self.status {
            body["Status"] = json!(status);
        }
        if let Some(portal_options) = &self.portal_options {
            body["PortalOptions"] = portal_options.clone();
        }
        body
    }
}

/// The applications the rules are asserted over.
///
/// They differ along what the listing generalises over: the length of `Name` at both
/// published bounds (`len 1..100`, `^[\S\s]*$`, so spaces and punctuation are admitted),
/// whether `Description` (`len 1..128`, asserted at both bounds) and `Status`
/// (`ENABLED | DISABLED`) are sent, and the shape of `PortalOptions` (absent, `Visibility`
/// only, and each `SignInOrigin`). Every constraint is from
/// `smithy_query(query="operation",operation="CreateApplication")` and
/// `smithy_query(query="shape",shape="PortalOptions,SignInOptions,ApplicationStatus")`.
fn app_rows() -> Vec<AppRow> {
    vec![
        AppRow {
            label: "only the required members",
            // `Name` at its lower length bound.
            name: "a".to_string(),
            description: None,
            status: None,
            portal_options: None,
        },
        AppRow {
            label: "every member set, APPLICATION sign-in origin",
            // `Name` at its upper length bound (100), spanning spaces and punctuation.
            name: format!("Listed app (prod) - {}", "x".repeat(80)),
            // `Description` at its upper length bound (128).
            description: Some("d".repeat(128)),
            status: Some("ENABLED"),
            portal_options: Some(json!({
                "SignInOptions": {
                    "Origin": "APPLICATION",
                    "ApplicationUrl": "https://listed.example.com/sso/start",
                },
                "Visibility": "ENABLED",
            })),
        },
        AppRow {
            label: "DISABLED and hidden from the portal",
            name: "disabled-listed-app".to_string(),
            // `Description` at its lower length bound (1).
            description: Some("d".to_string()),
            status: Some("DISABLED"),
            portal_options: Some(json!({"Visibility": "DISABLED"})),
        },
        AppRow {
            label: "IDENTITY_CENTER sign-in origin",
            name: "identity centre origin".to_string(),
            description: None,
            status: None,
            portal_options: Some(json!({"SignInOptions": {"Origin": "IDENTITY_CENTER"}})),
        },
    ]
}

/// A router over an account whose store holds TWO instances, and both their ARNs: the
/// instance this emulator seeds (the one a client discovers through `ListInstances`) and a
/// second one held under an id of its own.
///
/// An organization instance is enabled through AWS Organizations rather than through this
/// API, so the second one is seeded through the SAME `state::Instance` constructor the
/// handlers use — exactly as the harness's own organization fixture does — and every call
/// a test then makes goes over the wire. Two instances in ONE store is what lets "under
/// the instance the request names" be asserted instead of read off the only instance that
/// exists.
async fn two_instance_router() -> (MultiServiceRouter, String, String) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let first = seeded_instance_arn(&router).await;
    let second = with_store(&engine, |store| {
        let instance = state::Instance::active(
            SECOND_INSTANCE_ID,
            SECOND_IDENTITY_STORE_ID,
            state::InstanceType::Organization,
            Some("Second".to_string()),
            aws_smithy_types::DateTime::from(SystemTime::now()),
            TEST_REGION,
        );
        let arn = instance.instance_arn.clone();
        store.instances.insert(arn.clone(), instance);
        arn
    });
    assert_ne!(
        first, second,
        "the fixture must hold two instances of different ids, got {first}"
    );
    (router, first, second)
}

/// One `ListApplications` call.
async fn list_page(
    router: &MultiServiceRouter,
    instance_arn: &str,
    filter: Option<&Value>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({"InstanceArn": instance_arn});
    if let Some(filter) = filter {
        request["Filter"] = filter.clone();
    }
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    call(router, "ListApplications", &request).await
}

/// The items one answer carries, in the order it listed them.
fn listed_items(case: &str, body: &Value) -> Vec<Value> {
    body["Applications"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListApplicationsResponse$Applications is missing from {body}")
        })
        .clone()
}

/// The `ApplicationArn` of each item, in the order they were listed.
fn listed_arns(case: &str, items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item["ApplicationArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an item carries no ApplicationArn: {item}"))
                .to_string()
        })
        .collect()
}

/// Every page of one enumeration, walked the way a client walks it: the items in the order
/// they were listed.
///
/// Asserts RULE 5's page-size and termination invariants on the way: no page carries more
/// than the page size asked for nor more than a modelled `ApplicationList` holds, and only
/// the last page comes back without a `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    instance_arn: &str,
    filter: Option<&Value>,
    max_results: Option<i64>,
    case: &str,
) -> Vec<Value> {
    let limit =
        max_results.map_or(MAX_ITEMS_PER_PAGE, |value| (value as usize).min(MAX_ITEMS_PER_PAGE));
    let mut seen: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (status, body) =
            list_page(router, instance_arn, filter, max_results, next_token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListApplications must answer 200, got {status} {body}"
        );
        let items = listed_items(case, &body);
        assert!(
            items.len() <= limit,
            "case {case:?}: a page must carry at most {limit} items, got {}",
            items.len()
        );
        seen.extend(items.clone());
        pages += 1;
        assert!(
            pages <= state::MAX_RECORDS_PER_STORE,
            "case {case:?}: the enumeration did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(token) => {
                assert_eq!(
                    items.len(),
                    limit,
                    "case {case:?}: a page that carries a NextToken must be full, got {} items",
                    items.len()
                );
                next_token = Some(token.to_string());
            }
            None => break,
        }
    }
    seen
}

/// `DescribeApplication` for `arn`, as a client reads it.
async fn describe(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(router, "DescribeApplication", &json!({"ApplicationArn": arn})).await;
    assert_eq!(
        status, 200,
        "DescribeApplication of {arn} must answer 200, got {status} {body}"
    );
    body
}

/// Create one application from `row` and return its `ApplicationArn`.
async fn create(router: &MultiServiceRouter, instance_arn: &str, row: &AppRow) -> String {
    let (status, body) = call(router, "CreateApplication", &row.request(instance_arn)).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateApplication must answer 200, got {status} {body}",
        row.label
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: the create minted no ARN: {body}", row.label))
        .to_string()
}

/// Create a plainly-named application and return its `ApplicationArn`.
async fn create_named(router: &MultiServiceRouter, instance_arn: &str, name: &str) -> String {
    create(
        router,
        instance_arn,
        &AppRow {
            label: "named",
            name: name.to_string(),
            description: None,
            status: None,
            portal_options: None,
        },
    )
    .await
}

/// RULES 1 and 4 — the enumeration reports exactly the stored applications, in
/// `ApplicationArn` order, each agreeing with its describe, for every SUBSET of the
/// application table and whatever order the creates arrived in; and repeating it changes
/// nothing.
#[tokio::test]
async fn list_reports_exactly_the_applications_stored_under_the_instance() {
    let rows = app_rows();
    // The cases span the whole class the rule generalises over: every subset of the
    // application table (so every count from none to all, and every combination of the
    // member shapes), each created in a ROTATED order so the answer's order is the
    // service's and not the order the caller happened to create in.
    for mask in 0..(1u32 << rows.len()) {
        let (router, instance_arn, _second) = two_instance_router().await;
        let mut selected: Vec<&AppRow> = rows
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, row)| row)
            .collect();
        let case = format!(
            "applications {:?}",
            selected.iter().map(|row| row.label).collect::<Vec<&str>>()
        );

        // An instance nobody created an application under lists NOTHING, and that is a 200
        // with an empty list rather than a refusal.
        let empty = walk(&router, &instance_arn, None, None, &case).await;
        assert!(
            empty.is_empty(),
            "case {case:?}: an instance with no application must list none, got {empty:?}"
        );

        let rotation = mask as usize % selected.len().max(1);
        selected.rotate_right(rotation);
        let mut created: Vec<String> = Vec::new();
        for row in &selected {
            created.push(create(&router, &instance_arn, row).await);
        }

        let items = walk(&router, &instance_arn, None, None, &case).await;
        let mut expected = created.clone();
        expected.sort();
        assert_eq!(
            listed_arns(&case, &items),
            expected,
            "case {case:?}: the list must report exactly the stored applications in ARN \
             order, got {items:?}"
        );

        // Every item is the member-for-member view the describe of the same application
        // reports — nothing recomputed on the way out and nothing the create was given
        // beyond the modelled members.
        for item in &items {
            let arn = item["ApplicationArn"].as_str().unwrap_or_default().to_string();
            let described = describe(&router, &arn).await;
            assert_eq!(
                item, &described,
                "case {case:?}: the listed item and the describe of {arn} must agree, got {item}"
            );
            let members: Vec<&String> = item
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: an item must be a structure: {item}"))
                .keys()
                .collect();
            for member in &members {
                assert!(
                    MODELLED_ITEM_MEMBERS.contains(&member.as_str()),
                    "case {case:?}: {member:?} is not a member the model declares for \
                     Application, got {item}"
                );
            }
        }

        // RULE 4: the list is a report, so a second and a third call answer identically.
        let again = walk(&router, &instance_arn, None, None, &case).await;
        assert_eq!(
            again, items,
            "case {case:?}: repeating the list must answer identically, got {again:?}"
        );
        let third = walk(&router, &instance_arn, None, None, &case).await;
        assert_eq!(
            third, items,
            "case {case:?}: a list must change nothing a later caller sees, got {third:?}"
        );
    }
}

/// One row of the filter table: the `Filter` the request carries (when it carries one) and
/// the two members the row's expectation is computed from.
#[derive(Debug, Clone)]
struct FilterCase {
    label: &'static str,
    filter: Option<Value>,
    account: Option<String>,
    provider: Option<&'static str>,
}

/// RULE 2 — each filter member narrows the enumeration to the applications whose own
/// member equals it, and both together are a conjunction.
#[tokio::test]
async fn the_filter_narrows_the_enumeration_to_the_applications_it_names() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;

    // Four applications, created over the wire under the one provider this API creates
    // under.
    let mut stored: BTreeMap<String, (String, String)> = BTreeMap::new();
    for name in ["filter-one", "filter-two", "filter-three", "filter-four"] {
        let arn = create_named(&router, &instance_arn, name).await;
        stored.insert(arn, (String::new(), PROVIDER_CUSTOM.to_string()));
    }
    let arns: Vec<String> = stored.keys().cloned().collect();
    let own_account = arns[0].split(':').nth(4).unwrap_or_default().to_string();
    for value in stored.values_mut() {
        value.0 = own_account.clone();
    }

    // An application of an AWS managed or pre-integrated provider is created by the owning
    // Amazon Web Services service or in the console, and an application of ANOTHER account
    // is created by that account, so neither can be reached through this API. Both are
    // seeded on the stored records through the same store the handlers read, so the filter
    // has something to discriminate BETWEEN; every call below goes over the wire.
    with_store(&engine, |store| {
        let record = store
            .applications
            .get_mut(&arns[1])
            .expect("the second application is stored");
        record.application_provider_arn = PROVIDER_ATHENA.to_string();
        let record = store
            .applications
            .get_mut(&arns[2])
            .expect("the third application is stored");
        record.application_account = OTHER_ACCOUNT.to_string();
        let record = store
            .applications
            .get_mut(&arns[3])
            .expect("the fourth application is stored");
        record.application_provider_arn = PROVIDER_ATHENA.to_string();
        record.application_account = OTHER_ACCOUNT.to_string();
    });
    stored.get_mut(&arns[1]).expect("row").1 = PROVIDER_ATHENA.to_string();
    stored.get_mut(&arns[2]).expect("row").0 = OTHER_ACCOUNT.to_string();
    stored.get_mut(&arns[3]).expect("row").0 = OTHER_ACCOUNT.to_string();
    stored.get_mut(&arns[3]).expect("row").1 = PROVIDER_ATHENA.to_string();

    // The rows span the filter: absent, each member alone (naming a value some stored
    // application carries and a value none carries), and both members together — matching
    // the same application, matching different ones (so the conjunction is empty) and one
    // of each.
    let cases: Vec<FilterCase> = vec![
        FilterCase {
            label: "no filter at all",
            filter: None,
            account: None,
            provider: None,
        },
        FilterCase {
            label: "an empty filter structure",
            filter: Some(json!({})),
            account: None,
            provider: None,
        },
        FilterCase {
            label: "the account this emulator serves",
            filter: Some(json!({"ApplicationAccount": &own_account})),
            account: Some(own_account.clone()),
            provider: None,
        },
        FilterCase {
            label: "another account",
            filter: Some(json!({"ApplicationAccount": OTHER_ACCOUNT})),
            account: Some(OTHER_ACCOUNT.to_string()),
            provider: None,
        },
        FilterCase {
            label: "an account no stored application lives in",
            filter: Some(json!({"ApplicationAccount": "999999999999"})),
            account: Some("999999999999".to_string()),
            provider: None,
        },
        FilterCase {
            label: "the custom provider",
            filter: Some(json!({"ApplicationProvider": PROVIDER_CUSTOM})),
            account: None,
            provider: Some(PROVIDER_CUSTOM),
        },
        FilterCase {
            label: "an AWS managed provider",
            filter: Some(json!({"ApplicationProvider": PROVIDER_ATHENA})),
            account: None,
            provider: Some(PROVIDER_ATHENA),
        },
        FilterCase {
            label: "a catalogue provider no stored application runs under",
            filter: Some(json!({"ApplicationProvider": PROVIDER_CATALOG_STATUSPAGE})),
            account: None,
            provider: Some(PROVIDER_CATALOG_STATUSPAGE),
        },
        FilterCase {
            label: "both members, naming one stored application",
            filter: Some(json!({
                "ApplicationAccount": &own_account,
                "ApplicationProvider": PROVIDER_CUSTOM,
            })),
            account: Some(own_account.clone()),
            provider: Some(PROVIDER_CUSTOM),
        },
        FilterCase {
            label: "both members, naming another stored application",
            filter: Some(json!({
                "ApplicationAccount": OTHER_ACCOUNT,
                "ApplicationProvider": PROVIDER_ATHENA,
            })),
            account: Some(OTHER_ACCOUNT.to_string()),
            provider: Some(PROVIDER_ATHENA),
        },
        FilterCase {
            label: "both members, whose conjunction nothing satisfies",
            filter: Some(json!({
                "ApplicationAccount": "999999999999",
                "ApplicationProvider": PROVIDER_CUSTOM,
            })),
            account: Some("999999999999".to_string()),
            provider: Some(PROVIDER_CUSTOM),
        },
    ];

    for row in &cases {
        let case = row.label;
        let account = row.account.as_deref();
        let provider = row.provider;
        let expected: Vec<String> = stored
            .iter()
            .filter(|(_, (stored_account, stored_provider))| {
                account.is_none_or(|value| stored_account.as_str() == value)
                    && provider.is_none_or(|value| stored_provider.as_str() == value)
            })
            .map(|(arn, _)| arn.clone())
            .collect();
        let items = walk(&router, &instance_arn, row.filter.as_ref(), None, case).await;
        assert_eq!(
            listed_arns(case, &items),
            expected,
            "case {case:?}: the filter must select exactly the applications it names, got \
             {items:?}"
        );
        // Every item really carries the members the filter asked for.
        for item in &items {
            if let Some(account) = account {
                assert_eq!(
                    item["ApplicationAccount"].as_str(),
                    Some(account),
                    "case {case:?}: a filtered item must carry the account asked for, got {item}"
                );
            }
            if let Some(provider) = provider {
                assert_eq!(
                    item["ApplicationProviderArn"].as_str(),
                    Some(provider),
                    "case {case:?}: a filtered item must carry the provider asked for, got {item}"
                );
            }
        }
    }
}

/// RULE 3 — the enumeration tracks the store: an update changes exactly its own
/// application's item, an application belongs to its instance, and an instance whose
/// deletion has fallen due holds none.
#[tokio::test]
async fn the_list_tracks_the_store_and_its_own_instance() {
    let (router, first, second) = two_instance_router().await;

    let mut under_first: Vec<String> = Vec::new();
    for row in app_rows() {
        under_first.push(create(&router, &first, &row).await);
    }
    under_first.sort();
    let under_second = vec![create_named(&router, &second, "second-instance-app").await];

    // Each instance lists its OWN applications and none of the other's.
    for (case, instance_arn, expected) in [
        ("the seeded instance", &first, &under_first),
        ("the second instance", &second, &under_second),
    ] {
        let items = walk(&router, instance_arn, None, None, case).await;
        assert_eq!(
            listed_arns(case, &items),
            *expected,
            "case {case:?}: the list must carry exactly this instance's applications, got \
             {items:?}"
        );
        for item in &items {
            assert_eq!(
                item["InstanceArn"].as_str(),
                Some(instance_arn.as_str()),
                "case {case:?}: every listed item must belong to the instance asked for, got \
                 {item}"
            );
        }
    }

    // An UPDATE of one application changes that item and leaves every other one. The rows
    // differ along the members an update can change.
    let updates: Vec<(&str, Value)> = vec![
        ("the status alone", json!({"Status": "DISABLED"})),
        (
            "the name and description",
            json!({"Name": "renamed listed app", "Description": "updated description"}),
        ),
        (
            "the portal visibility",
            json!({"PortalOptions": {"Visibility": "DISABLED"}}),
        ),
    ];
    for (case, change) in updates {
        let subject = under_first[1].clone();
        let mut request = change.clone();
        request["ApplicationArn"] = json!(subject);
        let (status, body) = call(&router, "UpdateApplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateApplication must answer 200, got {status} {body}"
        );

        let items = walk(&router, &first, None, None, case).await;
        assert_eq!(
            listed_arns(case, &items),
            under_first,
            "case {case:?}: an update must not change which applications are listed, got \
             {items:?}"
        );
        for item in &items {
            let arn = item["ApplicationArn"].as_str().unwrap_or_default().to_string();
            let described = describe(&router, &arn).await;
            assert_eq!(
                item, &described,
                "case {case:?}: the listed item and the describe of {arn} must agree after the \
                 update, got {item}"
            );
        }
        // The other instance's listing is untouched by the update.
        let neighbour = walk(&router, &second, None, None, case).await;
        assert_eq!(
            listed_arns(case, &neighbour),
            under_second,
            "case {case:?}: the other instance's list must be untouched, got {neighbour:?}"
        );
    }

    // An application belongs to its instance: once the removal a `DeleteInstance` owes
    // falls due, the instance is gone and the applications held inside it with it, so the
    // enumeration is refused rather than answered with an empty page.
    let doomed_engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let doomed_router = router_over(Arc::clone(&doomed_engine));
    let doomed_instance = seeded_instance_arn(&doomed_router).await;
    let doomed_app = create_named(&doomed_router, &doomed_instance, "doomed-with-its-instance").await;
    let items = walk(&doomed_router, &doomed_instance, None, None, "before the delete").await;
    assert_eq!(
        listed_arns("before the delete", &items),
        vec![doomed_app],
        "case \"before the delete\": the list must carry the stored application, got {items:?}"
    );
    let (status, body) = call(
        &doomed_router,
        "DeleteInstance",
        &json!({"InstanceArn": doomed_instance}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteInstance must answer 200, got {status} {body}"
    );
    hold_deletion(&doomed_engine, SystemTime::now() - Duration::from_secs(60));
    let (status, body) = list_page(&doomed_router, &doomed_instance, None, None, None).await;
    assert_eq!(
        status, 400,
        "case \"after the deletion fell due\": the list must be refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"after the deletion fell due\": expected ValidationException, got {body}"
    );
}

/// RULE 5 — the enumeration pages: for every `MaxResults` the model admits, the pages
/// together carry each stored application exactly once, and a page never exceeds the
/// modelled `ApplicationList`.
#[tokio::test]
async fn every_page_size_the_model_admits_walks_the_whole_store_once() {
    let (router, instance_arn, _second) = two_instance_router().await;

    // One more application than a page can carry, so the modelled ceiling on
    // `ApplicationList` is crossed and not only the caller's `MaxResults`.
    let mut stored: Vec<String> = Vec::new();
    for index in 0..(MAX_ITEMS_PER_PAGE + 1) {
        stored.push(create_named(&router, &instance_arn, &format!("page-app-{index:03}")).await);
    }
    stored.sort();

    // The rows span `MaxResults` (`range(min: 1, max: 100)`): absent, both published
    // bounds, the modelled page ceiling and either side of it, and a value between.
    for max_results in [None, Some(1), Some(2), Some(7), Some(49), Some(50), Some(51), Some(100)] {
        let case = format!("MaxResults {max_results:?}");
        let items = walk(&router, &instance_arn, None, max_results, &case).await;
        assert_eq!(
            listed_arns(&case, &items),
            stored,
            "case {case:?}: every page size must walk the whole store once in ARN order, got \
             {} items",
            items.len()
        );
    }

    // The FIRST page of a `MaxResults` above the modelled ceiling carries the ceiling, and
    // the enumeration continues through its `NextToken`.
    let (status, body) = list_page(&router, &instance_arn, None, Some(100), None).await;
    assert_eq!(
        status, 200,
        "case \"MaxResults 100\": the first page must answer 200, got {status} {body}"
    );
    let items = listed_items("MaxResults 100", &body);
    assert_eq!(
        items.len(),
        MAX_ITEMS_PER_PAGE,
        "case \"MaxResults 100\": the first page must carry the modelled ceiling of \
         {MAX_ITEMS_PER_PAGE} items, got {}",
        items.len()
    );
    assert!(
        body["NextToken"].as_str().is_some(),
        "case \"MaxResults 100\": a truncated page must carry a NextToken, got {body}"
    );

    // A `MaxResults` outside the modelled range is refused by the boundary validator, on
    // each side of it.
    for max_results in [0, 101] {
        let case = format!("MaxResults {max_results}");
        let (status, body) = list_page(&router, &instance_arn, None, Some(max_results), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside range 1..100 must be refused 400, got {status} \
             {body}"
        );
    }
}

/// RULE 6 — an `InstanceArn` this account holds no instance for is refused on the modelled
/// client-input channel, never answered with an empty list and never with the scaffold's
/// HTTP 501.
#[tokio::test]
async fn an_instance_arn_this_account_holds_no_instance_for_is_refused() {
    let (router, instance_arn, _second) = two_instance_router().await;
    // One stored application exists, so the refusal is about the instance asked for and
    // not about an empty store.
    let stored = create_named(&router, &instance_arn, "stored-app").await;

    // The rows span the parts of the ARN that identify the instance — the instance id, the
    // `sso` prefix form the pattern admits and the partition — none of which this account
    // holds an instance under.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid instance id that was never enabled",
            UNKNOWN_INSTANCE_ARN.to_string(),
        ),
        (
            "the ins- spelling of an id nothing holds",
            "arn:aws:sso:::instance/ins-1234567890abcdef".to_string(),
        ),
        (
            "the stored instance in another partition",
            instance_arn.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
    ];
    for (case, arn) in cases {
        assert_ne!(
            arn, instance_arn,
            "case {case:?}: the row must name a DIFFERENT instance than the stored one"
        );
        let (status, body) = list_page(&router, &arn, None, None, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a list under an instance nothing holds must be refused 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // None of those refusals touched the stored application.
    let items = walk(&router, &instance_arn, None, None, "after the refusals").await;
    assert_eq!(
        listed_arns("after the refusals", &items),
        vec![stored],
        "a refused list must change nothing, got {items:?}"
    );
}

/// RULE 7 — the `NextToken` is this service's own cursor: a token it did not issue for this
/// enumeration is refused, and its own cursor resumes after the key it carries.
#[tokio::test]
async fn list_refuses_a_next_token_it_did_not_issue_and_resumes_from_its_own() {
    let (router, instance_arn, second) = two_instance_router().await;
    let mut stored: Vec<String> = Vec::new();
    for index in 0..4 {
        stored.push(create_named(&router, &instance_arn, &format!("cursor-app-{index}")).await);
    }
    stored.sort();
    let under_second = create_named(&router, &second, "cursor-other-instance").await;

    // `Token` is modelled `^[-a-zA-Z0-9+=/_]*$`, so every row below reaches the handler.
    // The rows differ along what a token this enumeration never issued can look like: not
    // hex at all, hex of a plain word, hex of a stored key with no cursor tag, hex of a
    // cursor of another kind, a cursor of the SAME enumeration under another instance, and
    // one under another filter of the same instance.
    let hex_of_a_word = hex::encode("aa");
    let hex_of_a_key = hex::encode(&stored[0]);
    let hex_of_another_kind = hex::encode(format!("ssoadmin:v1:bananas:{}", stored[0]));
    let foreign_instance_cursor = state::encode_page_token(
        &state::cursor_applications(&second, None, None),
        &under_second,
    );
    let foreign_filter_cursor = state::encode_page_token(
        &state::cursor_applications(&instance_arn, Some(OTHER_ACCOUNT), None),
        &stored[0],
    );
    for token in [
        "notatoken",
        "zz",
        "0",
        hex_of_a_word.as_str(),
        hex_of_a_key.as_str(),
        hex_of_another_kind.as_str(),
        foreign_instance_cursor.as_str(),
        foreign_filter_cursor.as_str(),
    ] {
        let (status, body) = list_page(&router, &instance_arn, None, None, Some(token)).await;
        assert_eq!(
            status, 400,
            "case {token:?}: a token this enumeration did not issue must be refused 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {body}"
        );
    }

    // Its OWN cursor resumes the enumeration: for every stored application, a cursor at
    // that key answers exactly the stored applications whose ARN sorts after it — the
    // invariant that makes a walk of the pages see each application once.
    let scope = state::cursor_applications(&instance_arn, None, None);
    for key in &stored {
        let token = state::encode_page_token(&scope, key);
        let (status, body) = list_page(&router, &instance_arn, None, None, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {key:?}: resuming from this enumeration's own cursor must answer 200, got \
             {status} {body}"
        );
        let expected: Vec<String> = stored.iter().filter(|arn| *arn > key).cloned().collect();
        let items = listed_items(key, &body);
        assert_eq!(
            listed_arns(key, &items),
            expected,
            "case {key:?}: a resumed page must carry exactly the applications after the cursor, \
             got {items:?}"
        );
    }
}
