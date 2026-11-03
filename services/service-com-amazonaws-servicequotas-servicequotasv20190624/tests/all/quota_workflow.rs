// SPDX-License-Identifier: CC0-1.0

//! The Service Quotas read-the-quotas + request-an-increase workflow, end to end
//! over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine — so what these tests exercise is
//! the HTTP behaviour and the persisted state behind it, not a handler called
//! directly.
//!
//! The suites assert RULES over a table of inputs, not one recorded case:
//!
//! * a listing returns the same items whatever page size it is walked with, and the
//!   last page carries no `NextToken`;
//! * the quota a single read returns is the entry the listing returned for the same
//!   pair, field for field, for EVERY pair in the seeded catalog;
//! * an approved increase changes the applied value and never the AWS default;
//! * an invalid request is refused with the error the model declares for it.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The routing header from this crate's `OPERATIONS.md`: `POST /` with
/// `X-Amz-Target: ServiceQuotasV20190624.<OperationName>`.
const TARGET_PREFIX: &str = "ServiceQuotasV20190624.";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The history listing, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open), so a request
/// made before this wait has settled afterwards.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// A caller pointed at one regional endpoint of one emulator.
///
/// `pub(crate)` so the other suites in this target share ONE harness instead of each
/// growing its own copy.
pub(crate) struct Client {
    router: MultiServiceRouter,
    region: String,
}

/// The ONE shared engine, exactly as the router builds it.
pub(crate) fn emulator() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// A caller for `region`, serving out of `engine`.
pub(crate) fn client(engine: &Arc<engine::InMemoryEngine>, region: &str) -> Client {
    let mut router = MultiServiceRouter::new();
    // These tests sign nothing, so they must not be refused for it.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_servicequotas_servicequotasv20190624::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    Client {
        router,
        region: region.to_string(),
    }
}

impl Client {
    /// Dispatch one awsJson1_1 call and return its status and parsed body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("servicequotas.{}.amazonaws.com", self.region))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            // The credential scope is what routes the request to this service and
            // names the region it is addressed to.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTCLIENT/20260101/{}/servicequotas/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=notchecked",
                    self.region
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Dispatch a call that must succeed, and return its body.
    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, &body).await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Dispatch a call that must be refused, and return its status and error code.
    pub(crate) async fn refused(&self, operation: &str, body: Value) -> (u16, String) {
        let (status, parsed) = self.call(operation, &body).await;
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .to_string();
        (status, code)
    }
}

/// Walk a paginated listing to the end and return every item it yielded.
///
/// Asserts the page-size contract on the way: no page exceeds `MaxResults`, and the
/// walk stops because a page came back without a `NextToken`.
pub(crate) async fn walk_pages(
    client: &Client,
    operation: &str,
    base: &Value,
    items: &str,
    page_size: Option<i64>,
) -> Vec<Value> {
    walk_pages_counted(client, operation, base, items, page_size)
        .await
        .0
}

/// Walk a paginated listing to the end, returning its items AND how many pages it
/// took.
///
/// Asserts the whole page contract: no page exceeds `MaxResults`, a page that is
/// followed by another is exactly `MaxResults` long (a short non-final page would
/// mean the cursor skipped items), and the walk ends on a page with no `NextToken`.
pub(crate) async fn walk_pages_counted(
    client: &Client,
    operation: &str,
    base: &Value,
    items: &str,
    page_size: Option<i64>,
) -> (Vec<Value>, usize) {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;

    // Bounded so a listing that never reports its last page fails instead of hanging.
    // The ceiling clears the longest listing (the service catalog, a few hundred
    // entries) walked ONE item at a time, with headroom.
    for _ in 0..2_048 {
        let mut body = base.clone();
        if let Some(size) = page_size {
            body["MaxResults"] = json!(size);
        }
        if let Some(resume) = &token {
            body["NextToken"] = json!(resume);
        }
        let response = client.ok(operation, body).await;
        let page = response
            .get(items)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        pages += 1;
        let next = response
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {operation} {base} MaxResults={size}: a page carried {} items",
                page.len()
            );
            assert!(
                next.is_none() || page.len() as i64 == size,
                "case {operation} {base} MaxResults={size}: a page followed by another carried \
                 only {} items",
                page.len()
            );
        }
        collected.extend(page);
        match next {
            Some(resume) => token = Some(resume),
            None => return (collected, pages),
        }
    }
    panic!("case {operation} {base}: the listing never reported a last page");
}

/// A `NextToken`-shaped base64 string carrying `payload`.
pub(crate) fn base64_of(payload: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(payload)
}

/// Every `(ServiceCode, ServiceName)` pair `ListServices` reports.
///
/// `pub(crate)` so the catalog suites share ONE walk of the inventory.
pub(crate) async fn service_inventory(client: &Client) -> Vec<Value> {
    walk_pages(client, "ListServices", &json!({}), "Services", None).await
}

/// Every AWS default quota of `service_code`.
pub(crate) async fn default_quotas(client: &Client, service_code: &str) -> Vec<Value> {
    walk_pages(
        client,
        "ListAWSDefaultServiceQuotas",
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await
}

/// The whole catalog, as `ServiceCode -> its default quotas`.
pub(crate) async fn whole_default_catalog(
    client: &Client,
) -> std::collections::BTreeMap<String, Vec<Value>> {
    let mut catalog = std::collections::BTreeMap::new();
    for service in service_inventory(client).await {
        let code = service["ServiceCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case ListServices: a service without a code: {service}"))
            .to_string();
        let quotas = default_quotas(client, &code).await;
        catalog.insert(code, quotas);
    }
    catalog
}

/// Every listing yields ONE ordered set of items, whatever page size it is walked
/// with, and a page size is honoured.
#[tokio::test]
async fn a_listing_pages_the_same_items_whatever_the_page_size() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // One row per listing: (operation, request, the output member that carries the
    // items). The services are the seeded catalog (`src/state.rs::seed_services`),
    // so the rows differ in listing length as well as in operation.
    let listings = [
        ("ListServices", json!({}), "Services"),
        ("ListServiceQuotas", json!({"ServiceCode": "vpc"}), "Quotas"),
        ("ListServiceQuotas", json!({"ServiceCode": "ec2"}), "Quotas"),
        ("ListServiceQuotas", json!({"ServiceCode": "s3"}), "Quotas"),
        (
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": "vpc"}),
            "Quotas",
        ),
        (
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": "dynamodb"}),
            "Quotas",
        ),
    ];

    for (operation, base, items) in &listings {
        let whole = walk_pages(&client, operation, base, items, None).await;
        assert!(
            !whole.is_empty(),
            "case {operation} {base}: expected a non-empty listing, got {whole:?}"
        );
        for size in [1_i64, 2, 3, 100] {
            let paged = walk_pages(&client, operation, base, items, Some(size)).await;
            assert_eq!(
                paged, whole,
                "case {operation} {base} MaxResults={size}: paging must yield the one listing, \
                 got {paged:?}"
            );
        }
    }

    // The catalog itself: the seeded services, in ServiceCode order, with no
    // duplicates. The membership is asserted in tests/all/default_quota_catalog.rs;
    // what this pins is that PAGING the catalog yields it once, in order.
    let services = walk_pages(&client, "ListServices", &json!({}), "Services", Some(2)).await;
    let codes: Vec<&str> = services
        .iter()
        .map(|service| service["ServiceCode"].as_str().unwrap_or_default())
        .collect();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        codes, sorted,
        "case ListServices: expected the catalog in ServiceCode order with no repeats, got \
         {codes:?}"
    );
    // Every service with a seeded quota set must be IN the catalog it is listed from.
    for seeded in ["dynamodb", "ec2", "lambda", "s3", "vpc"] {
        assert!(
            codes.contains(&seeded),
            "case ListServices: {seeded} has seeded quotas, so the catalog must list it, got \
             {} entries",
            codes.len()
        );
    }
    for service in &services {
        assert!(
            service["ServiceName"]
                .as_str()
                .is_some_and(|name| !name.is_empty()),
            "case ListServices: every service must report a ServiceName, got {service}"
        );
    }

    // A page that stops short carries a token, and the next page CONTINUES.
    let first = client.ok("ListServices", json!({"MaxResults": 2})).await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case ListServices MaxResults=2: expected a NextToken, got {first}"))
        .to_string();
    let second = client
        .ok("ListServices", json!({"MaxResults": 2, "NextToken": token}))
        .await;
    assert_ne!(
        first["Services"], second["Services"],
        "case ListServices MaxResults=2: the second page repeated the first, got {second}"
    );
}

/// For EVERY pair in the catalog, the single read and the listing agree, the default
/// read agrees on everything the default fixes, and the ARN is the pair's own.
#[tokio::test]
async fn a_single_quota_read_matches_the_entry_the_listing_returned() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let services = walk_pages(&client, "ListServices", &json!({}), "Services", None).await;
    let mut pairs = 0_usize;

    for service in &services {
        let service_code = service["ServiceCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case ListServices: a service without a code: {service}"))
            .to_string();
        let listed = walk_pages(
            &client,
            "ListServiceQuotas",
            &json!({"ServiceCode": service_code}),
            "Quotas",
            None,
        )
        .await;

        for quota in &listed {
            let quota_code = quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a quota without a code: {quota}"))
                .to_string();
            let case = format!("{service_code}/{quota_code}");
            let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});

            let got = client.ok("GetServiceQuota", pair.clone()).await;
            assert_eq!(
                got["Quota"], *quota,
                "case {case}: GetServiceQuota must return the listed entry, got {got}"
            );

            // A global quota's ARN carries no region; a regional one carries the
            // caller's.
            let global = quota["GlobalQuota"].as_bool().unwrap_or(false);
            let region = if global { "" } else { "us-east-1" };
            assert_eq!(
                quota["QuotaArn"].as_str(),
                Some(format!("arn:aws:servicequotas:{region}:{ACCOUNT}:{case}").as_str()),
                "case {case}: unexpected QuotaArn, got {quota}"
            );
            assert_eq!(
                quota["QuotaAppliedAtLevel"],
                json!("ACCOUNT"),
                "case {case}: an account-level quota must report it, got {quota}"
            );
            assert!(
                quota["Unit"].as_str().is_some_and(|unit| !unit.is_empty()),
                "case {case}: every quota reports a Unit, got {quota}"
            );

            let default = client.ok("GetAWSDefaultServiceQuota", pair.clone()).await;
            for member in [
                "QuotaName",
                "QuotaCode",
                "ServiceCode",
                "Unit",
                "Description",
                "Period",
                "UsageMetric",
            ] {
                assert_eq!(
                    default["Quota"][member], quota[member],
                    "case {case}: the default and the applied quota must agree on {member}, got \
                     {default} and {quota}"
                );
            }
            // The ONE member they must NOT agree on: an AWS default is published by
            // the service and owned by no account, so its ARN's account segment is
            // empty, while the applied quota's names the account (see
            // `quotas_catalog::an_aws_default_quota_arn_carries_no_account_id`). A
            // default is published per Region, so its ARN names the Region even for a
            // quota counted account-wide.
            assert_eq!(
                default["Quota"]["QuotaArn"].as_str(),
                Some(format!("arn:aws:servicequotas:us-east-1::{case}").as_str()),
                "case {case}: an AWS default quota's ARN carries no account, got {default}"
            );
            assert_eq!(
                default["Quota"]["Value"], quota["Value"],
                "case {case}: with nothing applied the two values must agree, got {default}"
            );
            assert_eq!(
                default["Quota"]["QuotaAppliedAtLevel"],
                Value::Null,
                "case {case}: a default quota is applied at no level, got {default}"
            );

            // And the default listing carries the same record as the default read.
            let default_listed = walk_pages(
                &client,
                "ListAWSDefaultServiceQuotas",
                &json!({"ServiceCode": service_code}),
                "Quotas",
                None,
            )
            .await;
            assert!(
                default_listed.contains(&default["Quota"]),
                "case {case}: ListAWSDefaultServiceQuotas must carry the default read, got \
                 {default_listed:?}"
            );
            pairs += 1;
        }
    }

    assert!(
        pairs >= 5,
        "case catalog: expected the seeded catalog to cover several quotas, covered {pairs}"
    );
}

/// RULE: a service whose published quotas this emulator seeds is discoverable by its
/// `ServiceCode` and every quota it reports reads back identically through
/// `GetServiceQuota` and `GetAWSDefaultServiceQuota`, with a `Value` and a `QuotaName`.
///
/// The rows are the catalogue's `ServiceCode`s themselves, taken from `ListServices`,
/// so a service added to the seed is covered without touching this test; `b2bi` is
/// named explicitly because a client discovers that quota before planning against it.
///
/// Coverage is per PAIR, not per service (see `seed_default_quotas`): a catalogued code
/// whose published defaults are not seeded resolves with an EMPTY list, which is the
/// rule `default_quota_catalog.rs` asserts. What this suite adds is that wherever a
/// quota IS reported, the three reads of it agree.
#[tokio::test]
async fn every_seeded_service_code_resolves_to_readable_quotas() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let services = walk_pages(&client, "ListServices", &json!({}), "Services", None).await;
    let codes: Vec<String> = services
        .iter()
        .filter_map(|service| service["ServiceCode"].as_str().map(str::to_string))
        .collect();
    assert!(
        codes.iter().any(|code| code == "b2bi"),
        "case catalog: AWS B2B Data Interchange publishes quotas, so ServiceCode 'b2bi' \
         must be in the catalogue, got {} entries",
        codes.len()
    );

    // How many (service, quota) pairs the sweep below actually read back, so an empty
    // catalogue cannot pass this test vacuously.
    let mut read_pairs = 0_usize;
    for service_code in &codes {
        let listed = walk_pages(
            &client,
            "ListServiceQuotas",
            &json!({"ServiceCode": service_code}),
            "Quotas",
            None,
        )
        .await;
        if service_code == "b2bi" {
            assert!(
                !listed.is_empty(),
                "case b2bi: the seeded profile quota must be reported, got {listed:?}"
            );
        }
        for quota in listed {
            read_pairs += 1;
            let quota_code = quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a quota without a code: {quota}"))
                .to_string();
            let case = format!("{service_code}/{quota_code}");
            assert!(
                quota["Value"].as_f64().is_some(),
                "case {case}: a quota must report a Value, got {quota}"
            );
            assert!(
                quota["QuotaName"].as_str().is_some_and(|n| !n.is_empty()),
                "case {case}: a quota must report a QuotaName, got {quota}"
            );
            let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});
            let read = client.ok("GetServiceQuota", pair.clone()).await["Quota"].clone();
            assert_eq!(
                read, quota,
                "case {case}: GetServiceQuota must answer the entry the listing returned"
            );
            let default = client.ok("GetAWSDefaultServiceQuota", pair).await["Quota"].clone();
            assert_eq!(
                default["Value"], quota["Value"],
                "case {case}: with no increase applied, the default value is the applied one, \
                 got {default}"
            );
        }
    }

    assert!(
        read_pairs >= 5,
        "case catalog: the sweep must have read several seeded quota pairs back, read \
         {read_pairs}"
    );
}

/// RULE: the quota a client plans against for AWS B2B Data Interchange profiles is the
/// documented five, published under ServiceCode `b2bi` with the `quotaCode` the b2bi
/// service's own `ServiceQuotaExceededException` names.
#[tokio::test]
async fn the_b2bi_profile_quota_publishes_the_documented_limit() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let quotas = walk_pages(
        &client,
        "ListServiceQuotas",
        &json!({"ServiceCode": "b2bi"}),
        "Quotas",
        None,
    )
    .await;

    // `CreateProfile` in the b2bi model: "You can have up to five customer profiles".
    let profiles = quotas
        .iter()
        .find(|quota| quota["QuotaCode"] == json!("ProfilesPerAccount"))
        .unwrap_or_else(|| panic!("case b2bi: expected a profiles quota, got {quotas:?}"));
    assert_eq!(
        profiles["Value"].as_f64(),
        Some(5.0),
        "case b2bi/ProfilesPerAccount: expected the documented limit of five profiles, \
         got {profiles}"
    );
    assert_eq!(
        profiles["ServiceName"],
        json!("AWS B2B Data Interchange"),
        "case b2bi: the catalogue must name the service, got {profiles}"
    );
    assert_eq!(
        profiles["QuotaArn"],
        json!(format!(
            "arn:aws:servicequotas:us-east-1:{ACCOUNT}:b2bi/ProfilesPerAccount"
        )),
        "case b2bi/ProfilesPerAccount: the ARN must name the pair, got {profiles}"
    );
}

/// RULE: the quota a client plans against for AWS User Notifications Contacts email
/// contacts is the published ten per account, published under ServiceCode
/// `notifications-contacts` with the `quotaCode` the notificationscontacts service's own
/// `ServiceQuotaExceededException` names.
#[tokio::test]
async fn the_email_contact_quota_publishes_the_documented_limit() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let quotas = walk_pages(
        &client,
        "ListServiceQuotas",
        &json!({"ServiceCode": "notifications-contacts"}),
        "Quotas",
        None,
    )
    .await;

    let contacts = quotas
        .iter()
        .find(|quota| quota["QuotaCode"] == json!("EmailContactsPerAccount"))
        .unwrap_or_else(|| {
            panic!("case notifications-contacts: expected an email-contact quota, got {quotas:?}")
        });
    assert_eq!(
        contacts["Value"].as_f64(),
        Some(10.0),
        "case notifications-contacts/EmailContactsPerAccount: expected the published limit \
         of ten email contacts, got {contacts}"
    );
    assert_eq!(
        contacts["QuotaName"],
        json!("Email contacts per account"),
        "case notifications-contacts/EmailContactsPerAccount: the published name must be the \
         one the code describes, got {contacts}"
    );
    assert_eq!(
        contacts["ServiceName"],
        json!("AWS User Notifications Contacts"),
        "case notifications-contacts: the catalogue must name the service, got {contacts}"
    );
    // The quota counts an ACCOUNT's contacts, so it is global and its ARN carries no
    // region whichever regional endpoint asked.
    assert_eq!(
        contacts["GlobalQuota"],
        json!(true),
        "case notifications-contacts/EmailContactsPerAccount: an account-wide quota must be \
         published as global, got {contacts}"
    );
    assert_eq!(
        contacts["QuotaArn"],
        json!(format!(
            "arn:aws:servicequotas::{ACCOUNT}:notifications-contacts/EmailContactsPerAccount"
        )),
        "case notifications-contacts/EmailContactsPerAccount: the ARN must name the pair, \
         got {contacts}"
    );
}

/// `QuotaAppliedAtLevel` keeps the quotas applied at that level, and `ALL` keeps
/// every one.
#[tokio::test]
async fn the_applied_level_filter_keeps_the_quotas_applied_at_that_level() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let all = walk_pages(
        &client,
        "ListServiceQuotas",
        &json!({"ServiceCode": "vpc"}),
        "Quotas",
        None,
    )
    .await;

    // The rows are the `AppliedLevelEnum` members (`APPLIED_LEVEL_ENUM_VALUES` in
    // `src/ops/types/shared_types.rs`), plus the unfiltered listing.
    let expectations = [("ACCOUNT", all.len()), ("ALL", all.len()), ("RESOURCE", 0)];
    for (level, expected) in expectations {
        let filtered = walk_pages(
            &client,
            "ListServiceQuotas",
            &json!({"ServiceCode": "vpc", "QuotaAppliedAtLevel": level}),
            "Quotas",
            None,
        )
        .await;
        assert_eq!(
            filtered.len(),
            expected,
            "case QuotaAppliedAtLevel={level}: expected {expected} quotas, got {filtered:?}"
        );
    }
}

/// The whole workflow: an increase is minted PENDING, read back from state, decided
/// by the clock, applied to the quota — and the AWS default never moves.
#[tokio::test]
async fn an_approved_increase_changes_the_applied_value_and_never_the_aws_default() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // One row per adjustable quota in the catalog, spanning a count quota, a
    // measured one (Unit=Gigabytes), a global one, and four different services.
    let rows = [
        ("vpc", "L-F678F1CE", 10.0_f64),
        ("vpc", "L-A4707A72", 25.0),
        ("lambda", "L-B99A9384", 2_000.0),
        ("lambda", "L-2ACBD22F", 150.0),
        ("dynamodb", "L-F98FE922", 5_000.0),
        ("s3", "L-DC2B2D3D", 20_000.0),
    ];

    let mut minted = Vec::new();
    for (service_code, quota_code, desired) in rows {
        let case = format!("{service_code}/{quota_code} -> {desired}");
        let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});

        let before = client
            .ok("GetAWSDefaultServiceQuota", pair.clone())
            .await["Quota"]["Value"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: the default quota must report a Value"));

        let response = client
            .ok(
                "RequestServiceQuotaIncrease",
                json!({
                    "ServiceCode": service_code,
                    "QuotaCode": quota_code,
                    "DesiredValue": desired,
                }),
            )
            .await;
        let requested = response["RequestedQuota"].clone();
        let id = requested["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a minted Id, got {requested}"))
            .to_string();

        assert_eq!(
            requested["Status"],
            json!("PENDING"),
            "case {case}: a new request is PENDING, got {requested}"
        );
        assert_eq!(
            requested["DesiredValue"].as_f64(),
            Some(desired),
            "case {case}: unexpected DesiredValue, got {requested}"
        );
        assert_eq!(
            requested["ServiceCode"],
            json!(service_code),
            "case {case}: unexpected ServiceCode, got {requested}"
        );
        assert_eq!(
            requested["QuotaCode"],
            json!(quota_code),
            "case {case}: unexpected QuotaCode, got {requested}"
        );
        assert!(
            requested["Created"].as_f64().is_some() && requested["LastUpdated"].as_f64().is_some(),
            "case {case}: Created and LastUpdated must both be set, got {requested}"
        );
        assert_eq!(
            requested["Requester"],
            json!(format!("arn:aws:iam::{ACCOUNT}:root")),
            "case {case}: unexpected Requester, got {requested}"
        );
        assert_eq!(
            requested["QuotaRequestedAtLevel"],
            json!("ACCOUNT"),
            "case {case}: an account-level request must report it, got {requested}"
        );

        // Read back from state, straight away: the SAME record, still awaiting its
        // decision.
        let fetched = client
            .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
            .await;
        assert_eq!(
            fetched["RequestedQuota"], requested,
            "case {case}: the stored request must read back unchanged, got {fetched}"
        );

        // The quota has not moved yet: a caller acting straight after the request
        // still sees the value in force.
        let applied_now = client.ok("GetServiceQuota", pair.clone()).await;
        assert_eq!(
            applied_now["Quota"]["Value"].as_f64(),
            Some(before),
            "case {case}: an undecided request must not change the applied value, got \
             {applied_now}"
        );

        // History: present, and filtered by the status it has now.
        let pending = walk_pages(
            &client,
            HISTORY,
            &json!({"ServiceCode": service_code, "QuotaCode": quota_code, "Status": "PENDING"}),
            "RequestedQuotas",
            None,
        )
        .await;
        assert!(
            pending
                .iter()
                .any(|request| request["Id"].as_str() == Some(id.as_str())),
            "case {case}: the PENDING history must carry the request, got {pending:?}"
        );
        let approved = walk_pages(
            &client,
            HISTORY,
            &json!({"ServiceCode": service_code, "QuotaCode": quota_code, "Status": "APPROVED"}),
            "RequestedQuotas",
            None,
        )
        .await;
        assert!(
            approved.is_empty(),
            "case {case}: nothing is APPROVED yet, got {approved:?}"
        );

        minted.push((service_code, quota_code, desired, before, id));
    }

    // Let the decision the clock owes come due. The emulator never sleeps: it stores
    // the due instant and settles on the next call past it.
    tokio::time::sleep(PAST_DECISION).await;

    for (service_code, quota_code, desired, before, id) in minted {
        let case = format!("{service_code}/{quota_code} -> {desired}");
        let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});

        let decided = client
            .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
            .await;
        assert_eq!(
            decided["RequestedQuota"]["Status"],
            json!("APPROVED"),
            "case {case}: a request past its decision is APPROVED, got {decided}"
        );
        assert_eq!(
            decided["RequestedQuota"]["DesiredValue"].as_f64(),
            Some(desired),
            "case {case}: a decision must not rewrite the request, got {decided}"
        );

        let applied = client.ok("GetServiceQuota", pair.clone()).await;
        assert_eq!(
            applied["Quota"]["Value"].as_f64(),
            Some(desired),
            "case {case}: the approved value must be in force, got {applied}"
        );

        // THE POINT of having both operations: the default is untouched.
        let default_after = client.ok("GetAWSDefaultServiceQuota", pair.clone()).await;
        assert_eq!(
            default_after["Quota"]["Value"].as_f64(),
            Some(before),
            "case {case}: the AWS default must be unchanged by an approved increase, got \
             {default_after}"
        );

        // Reading twice answers the same thing: a read decides nothing by itself.
        let again = client.ok("GetServiceQuota", pair.clone()).await;
        assert_eq!(
            again, applied,
            "case {case}: a second read must answer the same, got {again}"
        );

        // The listing agrees with the single read.
        let listed = walk_pages(
            &client,
            "ListServiceQuotas",
            &json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            "Quotas",
            None,
        )
        .await;
        assert_eq!(
            listed,
            vec![applied["Quota"].clone()],
            "case {case}: the listing must carry exactly the quota just read, got {listed:?}"
        );

        // And the history filters the other way round now.
        let approved = walk_pages(
            &client,
            HISTORY,
            &json!({"ServiceCode": service_code, "QuotaCode": quota_code, "Status": "APPROVED"}),
            "RequestedQuotas",
            None,
        )
        .await;
        assert!(
            approved
                .iter()
                .any(|request| request["Id"].as_str() == Some(id.as_str())),
            "case {case}: the APPROVED history must carry the request, got {approved:?}"
        );
        let pending = walk_pages(
            &client,
            HISTORY,
            &json!({"ServiceCode": service_code, "QuotaCode": quota_code, "Status": "PENDING"}),
            "RequestedQuotas",
            None,
        )
        .await;
        assert!(
            pending.is_empty(),
            "case {case}: nothing is PENDING any more, got {pending:?}"
        );
    }
}

/// The history is newest first, and one quota has at most one request awaiting a
/// decision.
#[tokio::test]
async fn the_history_is_newest_first_and_one_request_is_open_at_a_time() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let pair = json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"});

    let first = client
        .ok(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 10.0}),
        )
        .await["RequestedQuota"]["Id"]
        .as_str()
        .expect("the first request reports an Id")
        .to_string();

    // A second request while the first is open is refused.
    let (status, code) = client
        .refused(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 20.0}),
        )
        .await;
    assert_eq!(status, 400, "case second open request: got {status} {code}");
    assert!(
        code.contains("ResourceAlreadyExistsException"),
        "case second open request: expected ResourceAlreadyExistsException, got {code}"
    );

    tokio::time::sleep(PAST_DECISION).await;

    // With the first decided, a further increase is accepted, and it must be an
    // increase on the value NOW in force (10), not on the default (5).
    let (status, code) = client
        .refused(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 7.0}),
        )
        .await;
    assert_eq!(
        (status, code.contains("IllegalArgumentException")),
        (400, true),
        "case below the value now in force: got {status} {code}"
    );

    let second = client
        .ok(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 30.0}),
        )
        .await["RequestedQuota"]["Id"]
        .as_str()
        .expect("the second request reports an Id")
        .to_string();

    let history = walk_pages(&client, HISTORY, &pair, "RequestedQuotas", None).await;
    let ids: Vec<&str> = history
        .iter()
        .map(|request| request["Id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        [second.as_str(), first.as_str()],
        "case history order: expected the newest request first, got {ids:?}"
    );

    // Paging the history yields the same order.
    let paged = walk_pages(&client, HISTORY, &pair, "RequestedQuotas", Some(1)).await;
    assert_eq!(
        paged, history,
        "case history MaxResults=1: paging must yield the one ordered history, got {paged:?}"
    );
}

/// An increase is regional: it applies where it was requested and nowhere else.
#[tokio::test]
async fn an_increase_applies_only_in_the_region_it_was_requested_in() {
    let engine = emulator();
    let home = client(&engine, "us-east-1");
    let elsewhere = client(&engine, "eu-west-1");
    let pair = json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"});

    for (caller, region) in [(&home, "us-east-1"), (&elsewhere, "eu-west-1")] {
        let quota = caller.ok("GetServiceQuota", pair.clone()).await;
        assert_eq!(
            quota["Quota"]["QuotaArn"],
            json!(format!(
                "arn:aws:servicequotas:{region}:{ACCOUNT}:vpc/L-F678F1CE"
            )),
            "case {region}: the ARN must name the caller's region, got {quota}"
        );
    }

    let id = home
        .ok(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 20.0}),
        )
        .await["RequestedQuota"]["Id"]
        .as_str()
        .expect("the request reports an Id")
        .to_string();

    tokio::time::sleep(PAST_DECISION).await;

    let applied = home.ok("GetServiceQuota", pair.clone()).await;
    assert_eq!(
        applied["Quota"]["Value"].as_f64(),
        Some(20.0),
        "case us-east-1: the approved value must be in force, got {applied}"
    );
    let untouched = elsewhere.ok("GetServiceQuota", pair.clone()).await;
    assert_eq!(
        untouched["Quota"]["Value"].as_f64(),
        Some(5.0),
        "case eu-west-1: another region's increase must not apply here, got {untouched}"
    );

    let (status, code) = elsewhere
        .refused("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
        .await;
    assert_eq!(
        (status, code.contains("NoSuchResourceException")),
        (400, true),
        "case a request read from another region: got {status} {code}"
    );
}

/// An approved increase on a quota AWS counts account-wide is in force at every
/// regional endpoint, while a regional one is in force only where it was made.
#[tokio::test]
async fn an_approved_increase_on_a_global_quota_is_in_force_in_every_region() {
    let engine = emulator();
    let home = client(&engine, "us-east-1");
    let elsewhere = client(&engine, "ap-southeast-2");

    // (ServiceCode, QuotaCode, DesiredValue, in force elsewhere afterwards): the
    // global row's value crosses regions, the regional row's does not.
    let global = json!({"ServiceCode": "budgets", "QuotaCode": "Budgets"});
    let regional = json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"});

    for (pair, desired) in [(&global, 30_000.0_f64), (&regional, 20.0)] {
        let mut body = pair.clone();
        body["DesiredValue"] = json!(desired);
        let response = home.ok("RequestServiceQuotaIncrease", body).await;
        assert_eq!(
            response["RequestedQuota"]["Status"],
            json!("PENDING"),
            "case {pair}: expected an accepted request, got {response}"
        );
    }

    tokio::time::sleep(PAST_DECISION).await;

    // us-east-1 made both requests, so both are in force there.
    for (pair, desired) in [(&global, 30_000.0_f64), (&regional, 20.0)] {
        let quota = home.ok("GetServiceQuota", pair.clone()).await;
        assert_eq!(
            quota["Quota"]["Value"].as_f64(),
            Some(desired),
            "case {pair} in the region it was requested in: got {quota}"
        );
    }

    // Elsewhere: the account-wide quota carries the new value, the regional one
    // still carries its default.
    let global_elsewhere = elsewhere.ok("GetServiceQuota", global.clone()).await;
    assert_eq!(
        global_elsewhere["Quota"]["Value"].as_f64(),
        Some(30_000.0),
        "case a global quota read from another region: an account-wide increase must be in \
         force, got {global_elsewhere}"
    );
    let regional_elsewhere = elsewhere.ok("GetServiceQuota", regional.clone()).await;
    assert_eq!(
        regional_elsewhere["Quota"]["Value"].as_f64(),
        Some(5.0),
        "case a regional quota read from another region: another region's increase must not \
         apply, got {regional_elsewhere}"
    );

    // And the AWS default is untouched in both regions.
    for (caller, region) in [(&home, "us-east-1"), (&elsewhere, "ap-southeast-2")] {
        let default = caller
            .ok("GetAWSDefaultServiceQuota", global.clone())
            .await;
        assert_eq!(
            default["Quota"]["Value"].as_f64(),
            Some(20_000.0),
            "case the global quota's default in {region}: must be unchanged, got {default}"
        );
    }
}

/// An invalid request is refused with the status and error code the model declares
/// for it — never a 501, and never a 200.
#[tokio::test]
async fn an_invalid_request_is_refused_with_the_modelled_error() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let long_code = "v".repeat(64);

    // Each row: why it is invalid, the operation, the request, the expected status
    // and the expected error shape. Every one of these is a CLIENT fault, so this
    // awsJson front end serves it on HTTP 400 whatever `@httpError` the shape
    // carries (`crate::scaffolded::wire_http_status`); the body's `__type` is what
    // tells one refusal from another.
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "an unknown service code",
            "ListServiceQuotas",
            json!({"ServiceCode": "nosuchservice"}),
            400,
            "NoSuchResourceException",
        ),
        (
            "an unknown service code",
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": "nosuchservice"}),
            400,
            "NoSuchResourceException",
        ),
        (
            "an unknown quota code",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-NoSuchQuota"}),
            400,
            "NoSuchResourceException",
        ),
        (
            "an unknown quota code",
            "GetAWSDefaultServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-NoSuchQuota"}),
            400,
            "NoSuchResourceException",
        ),
        (
            "a quota code that belongs to another service",
            "GetServiceQuota",
            json!({"ServiceCode": "lambda", "QuotaCode": "L-F678F1CE"}),
            400,
            "NoSuchResourceException",
        ),
        (
            "a quota code that belongs to another service",
            "ListServiceQuotas",
            json!({"ServiceCode": "lambda", "QuotaCode": "L-F678F1CE"}),
            400,
            "NoSuchResourceException",
        ),
        (
            "an unknown request id",
            "GetRequestedServiceQuotaChange",
            json!({"RequestId": "0123456789abcdef0123456789abcdef"}),
            400,
            "NoSuchResourceException",
        ),
        (
            "an unknown quota in the history",
            HISTORY,
            json!({"ServiceCode": "vpc", "QuotaCode": "L-NoSuchQuota"}),
            400,
            "NoSuchResourceException",
        ),
        (
            "an increase on a quota AWS publishes as not adjustable",
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-085A6257", "DesiredValue": 5.0}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a desired value equal to the value in force",
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 5.0}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a desired value below the value in force",
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 1.0}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a desired value above the modelled range",
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 10_000_000_001.0_f64}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a context id for a quota adjustable at the account level only",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "ContextId": "vpc-0123456789abcdef0"}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a context id for a quota adjustable at the account level only",
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "DesiredValue": 10.0,
                "ContextId": "vpc-0123456789abcdef0",
            }),
            400,
            "IllegalArgumentException",
        ),
        (
            "a quota code outside the modelled pattern",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc", "QuotaCode": "1-bad code"}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a service code longer than the modelled maximum",
            "ListServiceQuotas",
            json!({"ServiceCode": long_code}),
            400,
            "IllegalArgumentException",
        ),
        (
            "an absent required member",
            "GetServiceQuota",
            json!({"ServiceCode": "vpc"}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a page size above the modelled range",
            "ListServices",
            json!({"MaxResults": 101}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a page size below the modelled range",
            "ListServices",
            json!({"MaxResults": 0}),
            400,
            "IllegalArgumentException",
        ),
    ];

    for (why, operation, body, expected_status, expected_code) in rows {
        let case = format!("{why} ({operation})");
        let (status, code) = client.refused(operation, body).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {code}"
        );
        assert!(
            code.contains(expected_code),
            "case {case}: expected {expected_code}, got {code}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with
/// `InvalidPaginationTokenException`, on every paginated listing.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Rows: a token that satisfies the modelled NextToken pattern but names no
    // position in the listing it is sent to, and why.
    let mut tokens = vec![
        ("notatoken".to_string(), "not base64 at all"),
        (base64_of("hello"), "base64 of something never issued"),
        (
            base64_of("ListServices:9999"),
            "a position past the end of the listing",
        ),
        (
            base64_of("ListServices:-1"),
            "a position that is not a position",
        ),
    ];
    // And a REAL token, issued by ONE listing: it must not resume another listing.
    let issued = client.ok("ListServices", json!({"MaxResults": 2})).await["NextToken"]
        .as_str()
        .expect("a short first page carries a NextToken")
        .to_string();
    tokens.push((issued, "a token another listing issued"));
    // Every paginated listing in this slice.
    let listings = [
        ("ListServices", json!({})),
        ("ListServiceQuotas", json!({"ServiceCode": "vpc"})),
        (
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": "vpc"}),
        ),
        (
            HISTORY,
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"}),
        ),
    ];

    for (operation, base) in &listings {
        for (token, why) in &tokens {
            // The one listing that DID issue the real token resumes with it.
            if *why == "a token another listing issued" && *operation == "ListServices" {
                continue;
            }
            let mut body = base.clone();
            body["NextToken"] = json!(token);
            let case = format!("{operation} with {why}");
            let (status, code) = client.refused(operation, body).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {code}"
            );
            assert!(
                code.contains("InvalidPaginationTokenException"),
                "case {case}: expected InvalidPaginationTokenException, got {code}"
            );
        }
    }
}

/// A credential scope naming a region this emulator does not know is served as the
/// default region, so the per-region state cannot grow a bucket per caller-chosen
/// string — and a per-region invariant cannot be side-stepped by varying it.
#[tokio::test]
async fn a_scope_region_this_emulator_does_not_know_is_served_as_the_default_region() {
    let engine = emulator();
    let home = client(&engine, "us-east-1");
    let pair = json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"});

    // Rows: region strings a caller can put in its own credential scope that are not
    // AWS region codes (`state::KNOWN_REGIONS`).
    for made_up in ["mars-central-1", "not-a-region", "US-EAST-1", "us-east-1x"] {
        let caller = client(&engine, made_up);
        let quota = caller.ok("GetServiceQuota", pair.clone()).await;
        assert_eq!(
            quota["Quota"]["QuotaArn"],
            json!(format!(
                "arn:aws:servicequotas:us-east-1:{ACCOUNT}:vpc/L-F678F1CE"
            )),
            "case scope region {made_up:?}: expected the default region's ARN, got {quota}"
        );
    }

    // A request made with a made-up scope region lands in the DEFAULT region's
    // store: the same record is readable from the default-region caller.
    let id = client(&engine, "mars-central-1")
        .ok(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 10.0}),
        )
        .await["RequestedQuota"]["Id"]
        .as_str()
        .expect("the request reports an Id")
        .to_string();
    let fetched = home
        .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
        .await;
    assert_eq!(
        fetched["RequestedQuota"]["Id"].as_str(),
        Some(id.as_str()),
        "case a request made with an unknown scope region: expected it in the default region's \
         store, got {fetched}"
    );

    // And the one-open-request-per-quota invariant still holds for a caller that
    // varies the string.
    let (status, code) = client(&engine, "moon-west-2")
        .refused(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 30.0}),
        )
        .await;
    assert_eq!(
        (status, code.contains("ResourceAlreadyExistsException")),
        (400, true),
        "case a second open request under another unknown scope region: got {status} {code}"
    );
}

/// The append-only request store has a ceiling, and a request past it is refused
/// with the `QuotaExceededException` the model declares.
#[tokio::test]
async fn the_request_store_is_bounded_and_refuses_past_its_ceiling() {
    use service_com_amazonaws_servicequotas_servicequotasv20190624::state;

    let engine = emulator();
    let caller = client(&engine, "us-east-1");
    let store = state::service_state(&*engine).expect("this service's state is reachable");
    let requester = format!("arn:aws:iam::{ACCOUNT}:root");

    // Fill one region to its ceiling through the same insertion point the handler
    // uses — it is the ceiling that is under test, not the path to it.
    {
        let mut guard = store.lock();
        for index in 0..state::MAX_REQUESTS_PER_REGION {
            let stored = guard.record_request(
                "us-east-1",
                "vpc",
                "L-A4707A72",
                100.0 + index as f64,
                state::SUPPORT_CASE_ALLOWED_BY_DEFAULT,
                requester.clone(),
                None,
                std::time::SystemTime::now(),
            );
            assert!(
                stored.is_some(),
                "case fill {index}: the store must accept a request below its ceiling"
            );
        }
        assert!(
            guard
                .record_request(
                    "us-east-1",
                    "vpc",
                    "L-A4707A72",
                    99_999.0,
                    state::SUPPORT_CASE_ALLOWED_BY_DEFAULT,
                    requester.clone(),
                    None,
                    std::time::SystemTime::now(),
                )
                .is_none(),
            "case at the ceiling: the insertion point itself must refuse, got a stored request"
        );
        assert_eq!(
            guard.stored_request_count("us-east-1"),
            state::MAX_REQUESTS_PER_REGION,
            "case at the ceiling: the store must hold exactly its ceiling, got {}",
            guard.stored_request_count("us-east-1")
        );
    }

    // Over the wire the caller sees the modelled error, not a 500 and not a silent
    // success.
    let (status, code) = caller
        .refused(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 10.0}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a request past the ceiling: expected HTTP 400, got {status} with {code}"
    );
    assert!(
        code.contains("QuotaExceededException"),
        "case a request past the ceiling: expected QuotaExceededException, got {code}"
    );

    // Another region is unaffected: the ceiling is per region.
    let elsewhere = client(&engine, "eu-west-1");
    let accepted = elsewhere
        .ok(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": 10.0}),
        )
        .await;
    assert_eq!(
        accepted["RequestedQuota"]["Status"],
        json!("PENDING"),
        "case another region at the ceiling of the first: expected an accepted request, got \
         {accepted}"
    );
}
