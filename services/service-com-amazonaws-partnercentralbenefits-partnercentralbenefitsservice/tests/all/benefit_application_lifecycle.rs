// SPDX-License-Identifier: CC0-1.0

//! The benefit-application lifecycle, end to end over the wire.
//!
//! Every assertion goes through the real request path — `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The tests are tables: each row is one input drawn from the class the rule covers
//! (every seeded catalog, every modelled `FulfillmentType` and `BenefitStatus`, every
//! file extension the `FileType` enum names, several document shapes, several
//! application-count/`MaxResults` combinations, each published `MaxResults` bound and
//! one past it, every identifier form the modelled patterns admit). The one call the
//! bootstrap issue happened to name is one row among them.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::{
    BENEFIT_STATUS_VALUES, FILE_TYPE_VALUES, FULFILLMENT_TYPE_VALUES,
};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    service_state, APPLICATION_ID_PREFIX, APPLICATION_RESOURCE, BENEFIT_ID_PREFIX,
    BENEFIT_RESOURCE, ID_BODY_LEN, MAX_PAGE_SIZE, SEEDED_CATALOGS,
};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for; the ARNs the service returns must carry it.
/// It is also a value the modelled `Arn` region field admits
/// (`([a-z]{2}(-gov)?-[a-z]+-\d{1})?`).
pub(crate) const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// This service's sigv4 signing name, the 4th field of the credential scope the router
/// resolves the service from (`OPERATIONS.md`, "Wire protocol").
const SIGNING_NAME: &str = "partnercentral-benefits";

/// The `X-Amz-Target` prefix this service answers on (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "PartnerCentralBenefitsService";

/// The production catalog, one of the two the service serves.
pub(crate) const CATALOG: &str = "AWS";

/// One emulator holding this service on one shared engine.
///
/// Reused by `tests/all/service_error_rules.rs` and
/// `tests/all/recall_benefit_application.rs`, which need the same wire path — the latter
/// as an entitled partner, the former for an account that is NOT one (hence
/// [`Harness::unentitled`]).
pub(crate) struct Harness {
    router: MultiServiceRouter,
    /// The same engine the registered service holds, so a test can place a stored
    /// application into a status AWS's own review workflow produces and no operation in
    /// this slice reaches — see [`Harness::place_status`].
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    /// An emulator whose account holds an active AWS partner benefit, so the service's
    /// entitlement gate admits its requests.
    pub(crate) fn new() -> Self {
        Self::with_entitlement(true)
    }

    /// An emulator whose account holds NO active AWS partner benefit — the state this
    /// service's store starts in, and what every caller from the wire is.
    pub(crate) fn unentitled() -> Self {
        Self::with_entitlement(false)
    }

    fn with_entitlement(entitled: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The lifecycle tests are about the lifecycle, so they call as an ENTITLED
        // partner account: the service's entitlement gate refuses every request from an
        // account with no active AWS partner benefit, and no operation grants one (the
        // API models no partner registration), so the entitlement is recorded in process
        // here. The gate itself is pinned in `tests/all/service_error_rules.rs`.
        if entitled {
            service_state(&*engine)
                .expect("the in-memory engine carries this service's state")
                .lock()
                .grant_partner_entitlement(ACCOUNT);
        }
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential
        // below is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Put one stored application into `status`, directly in the store.
    ///
    /// `APPROVED`, `REJECTED` and `ACTION_REQUIRED` are set by AWS's review workflow, and
    /// no operation this emulator serves reaches them, so a rule stated over every
    /// `BenefitApplicationStatus` can only be exercised by placing the status here. The
    /// behaviour under test is still read and asserted over the wire.
    pub(crate) fn place_status(&self, case: &str, catalog: &str, identifier: &str, status: &str) {
        let state = service_state(&*self.engine)
            .expect("the in-memory engine carries this service's state");
        let mut store = state.lock();
        let application = store
            .application_mut(catalog, identifier)
            .unwrap_or_else(|| panic!("case {case:?}: {identifier:?} names no stored application"));
        application.status = status.to_string();
    }

    /// Call one operation and return `(http status, parsed body)`.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/{SIGNING_NAME}/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid HTTP request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call one operation that must answer 200, and return its body.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, out) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {out:?}"
        );
        out
    }

    /// Every `BenefitSummary` of one catalog, unfiltered.
    pub(crate) async fn benefits(&self, catalog: &str) -> Vec<Value> {
        let out = self
            .ok(catalog, "ListBenefits", &json!({ "Catalog": catalog }))
            .await;
        out["BenefitSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {catalog:?}: ListBenefits returned no list: {out:?}"))
            .clone()
    }

    /// The first `ACTIVE` benefit of [`CATALOG`], which every create below applies for.
    pub(crate) async fn active_benefit(&self) -> Value {
        self.active_benefit_of(CATALOG).await
    }

    /// The first `ACTIVE` benefit of `catalog`: the one implementation of that filter,
    /// which `tests/all/recall_benefit_application.rs` reaches for the other catalogs.
    pub(crate) async fn active_benefit_of(&self, catalog: &str) -> Value {
        self.benefits(catalog)
            .await
            .into_iter()
            .find(|benefit| benefit["Status"] == json!("ACTIVE"))
            .unwrap_or_else(|| panic!("case {catalog:?}: the seeded catalog holds no ACTIVE benefit"))
    }

    /// Create an application that must succeed, and return its create response.
    pub(crate) async fn create(&self, case: &str, body: &Value) -> Value {
        self.ok(case, "CreateBenefitApplication", body).await
    }

    /// Read one application of [`CATALOG`] back, asserting the read succeeded.
    pub(crate) async fn get(&self, case: &str, identifier: &str) -> Value {
        self.get_in(case, CATALOG, identifier).await
    }

    /// Read one application of any catalog back, asserting the read succeeded.
    pub(crate) async fn get_in(&self, case: &str, catalog: &str, identifier: &str) -> Value {
        self.ok(
            case,
            "GetBenefitApplication",
            &json!({ "Catalog": catalog, "Identifier": identifier }),
        )
        .await
    }
}

/// The wire error code of a failure body: the `__type` member, without its namespace.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// Assert `id` is `prefix` followed by [`ID_BODY_LEN`] `[0-9a-z]` characters, which is
/// what the modelled `BenefitApplicationId` / `BenefitSummary$Id` patterns require.
fn assert_id(case: &str, id: &str, prefix: &str) {
    let body = id
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("case {case:?}: expected an id starting {prefix:?}, got {id:?}"));
    assert_eq!(
        body.len(),
        ID_BODY_LEN,
        "case {case:?}: expected {ID_BODY_LEN} characters after {prefix:?}, got {id:?}"
    );
    assert!(
        body.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "case {case:?}: expected only [0-9a-z] after {prefix:?}, got {id:?}"
    );
}

/// The ARN this service gives the `resource` named `id` in `catalog`.
fn expected_arn(catalog: &str, resource: &str, id: &str) -> String {
    format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/{catalog}/{resource}/{id}")
}

/// A member of a JSON object as a string, failing the test when it is absent.
pub(crate) fn text(case: &str, body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {body:?}"))
        .to_string()
}

/// A timestamp member as epoch milliseconds, so two instants compare by value rather
/// than by their rendering.
///
/// `awsJson1_0` carries a `Timestamp` as epoch seconds by default and as an RFC-3339
/// string where the model asks for `date-time`; both forms are accepted here so the
/// comparison does not depend on which one the protocol chose.
pub(crate) fn instant(case: &str, body: &Value, member: &str) -> i64 {
    match &body[member] {
        Value::String(rendered) => DateTime::from_str(rendered, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {member} {rendered:?} is not a date-time: {error}")
            })
            .to_millis()
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {member} {rendered:?} has no millis: {error}")
            }),
        Value::Number(seconds) => {
            let seconds = seconds.as_f64().unwrap_or_else(|| {
                panic!("case {case:?}: {member} {seconds:?} is not a number of seconds")
            });
            (seconds * 1000.0).round() as i64
        }
        other => panic!("case {case:?}: expected a {member} timestamp, got {other:?}"),
    }
}

/// The string entries of a list member, in the order the service reported them.
fn list(body: &Value, member: &str) -> Vec<String> {
    body[member]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// ListBenefits — the service-owned catalog
// ---------------------------------------------------------------------------

/// Every catalog the service serves reports its seeded benefits, each with a modelled
/// id, the ARN this service builds for it, and values drawn from the model's own enums.
///
/// The row set is `SEEDED_CATALOGS`, so it cannot drift from the store, and the
/// enum catalogs are the generated `BENEFIT_STATUS_VALUES` / `FULFILLMENT_TYPE_VALUES`
/// (`smithy_query(query="shape",shape="BenefitStatus,FulfillmentType")`).
#[tokio::test]
async fn list_benefits_reports_the_seeded_catalog_of_every_catalog() {
    let harness = Harness::new();
    let mut ids_by_catalog: HashMap<&str, BTreeSet<String>> = HashMap::new();

    for catalog in SEEDED_CATALOGS {
        let summaries = harness.benefits(catalog).await;
        assert!(
            !summaries.is_empty(),
            "case {catalog:?}: expected a non-empty BenefitSummaries, got {summaries:?}"
        );
        let mut ids = BTreeSet::new();
        for summary in &summaries {
            let case = format!("{catalog}/{}", text(catalog, summary, "Name"));
            let id = text(&case, summary, "Id");
            assert_id(&case, &id, BENEFIT_ID_PREFIX);
            assert_eq!(
                text(&case, summary, "Arn"),
                expected_arn(catalog, BENEFIT_RESOURCE, &id),
                "case {case:?}: unexpected benefit ARN in {summary:?}"
            );
            assert_eq!(
                summary["Catalog"],
                json!(catalog),
                "case {case:?}: the summary must report its own catalog, got {summary:?}"
            );
            let status = text(&case, summary, "Status");
            assert!(
                BENEFIT_STATUS_VALUES.contains(&status.as_str()),
                "case {case:?}: Status {status:?} is not a modelled BenefitStatus"
            );
            for fulfillment in list(summary, "FulfillmentTypes") {
                assert!(
                    FULFILLMENT_TYPE_VALUES.contains(&fulfillment.as_str()),
                    "case {case:?}: FulfillmentTypes carries {fulfillment:?}, not a modelled FulfillmentType"
                );
            }
            assert!(
                !text(&case, summary, "Description").is_empty(),
                "case {case:?}: expected a description, got {summary:?}"
            );
            ids.insert(id);
        }
        ids_by_catalog.insert(catalog, ids);
    }

    // A benefit belongs to ONE catalog, so no id is shared between two of them.
    for (left, left_ids) in &ids_by_catalog {
        for (right, right_ids) in &ids_by_catalog {
            if left >= right {
                continue;
            }
            let shared: Vec<&String> = left_ids.intersection(right_ids).collect();
            assert!(
                shared.is_empty(),
                "case {left:?}/{right:?}: catalogs must not share benefit ids, got {shared:?}"
            );
        }
    }
}

/// Each list filter selects exactly the benefits whose own member satisfies it.
///
/// The expectation is computed from the UNFILTERED listing rather than written down, so
/// the rule is checked against the store itself: for every row, the filtered ids are the
/// unfiltered ids whose member overlaps the filter. The rows span every modelled
/// `BenefitStatus` and `FulfillmentType` value plus every program the catalog holds.
#[tokio::test]
async fn list_benefits_filters_by_status_fulfillment_type_and_program() {
    let harness = Harness::new();
    let all = harness.benefits(CATALOG).await;

    let mut rows: Vec<(&str, Vec<String>)> = Vec::new();
    for status in BENEFIT_STATUS_VALUES {
        rows.push(("Status", vec![(*status).to_string()]));
    }
    for fulfillment in FULFILLMENT_TYPE_VALUES {
        rows.push(("FulfillmentTypes", vec![(*fulfillment).to_string()]));
    }
    let mut programs: BTreeSet<String> = BTreeSet::new();
    for summary in &all {
        programs.extend(list(summary, "Programs"));
    }
    for program in &programs {
        rows.push(("Programs", vec![program.clone()]));
    }
    // A filter naming a value nothing carries selects nothing.
    rows.push(("Programs", vec!["NO_SUCH_PROGRAM".to_string()]));

    for (member, filter) in rows {
        let case = format!("{member}={filter:?}");
        let expected: BTreeSet<String> = all
            .iter()
            .filter(|summary| {
                let mine = if member == "Status" {
                    vec![text(&case, summary, "Status")]
                } else {
                    list(summary, member)
                };
                filter.iter().any(|wanted| mine.contains(wanted))
            })
            .map(|summary| text(&case, summary, "Id"))
            .collect();

        let out = harness
            .ok(
                &case,
                "ListBenefits",
                &json!({ "Catalog": CATALOG, member: filter }),
            )
            .await;
        let actual: BTreeSet<String> = out["BenefitSummaries"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|summary| text(&case, summary, "Id"))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: the filter must select exactly the matching benefits, got {out:?}"
        );
    }
}

/// A catalog the service does not serve is a 404, and a catalog outside the modelled
/// `CatalogName` pattern is a 400 — on every operation that takes one.
#[tokio::test]
async fn an_unknown_catalog_is_404_and_a_malformed_one_is_400() {
    let harness = Harness::new();
    let benefit = harness.active_benefit().await;
    let benefit_id = text("seed", &benefit, "Id");

    let bodies: Vec<(&str, Value)> = vec![
        ("ListBenefits", json!({})),
        ("ListBenefitApplications", json!({})),
        (
            "CreateBenefitApplication",
            json!({ "ClientToken": "tok-catalog", "BenefitIdentifier": benefit_id }),
        ),
        (
            "GetBenefitApplication",
            json!({ "Identifier": "benappl-00000000000000" }),
        ),
    ];

    for (operation, extra) in bodies {
        for (catalog, expected_status, expected_code) in [
            ("NotACatalog", 404_u16, "ResourceNotFoundException"),
            ("bad catalog!", 400, "ValidationException"),
        ] {
            let case = format!("{operation}/{catalog}");
            let mut body = extra.clone();
            body["Catalog"] = json!(catalog);
            let (status, out) = harness.call(operation, &body).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected {expected_status}, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                expected_code,
                "case {case:?}: expected {expected_code}, got {out:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CreateBenefitApplication -> GetBenefitApplication
// ---------------------------------------------------------------------------

/// A create stores every member it was given, and the Get reports them back.
///
/// The rows differ along what the rule generalises over: which optional members are
/// present, how many fulfillment types, and the SHAPE of the `BenefitApplicationDetails`
/// document (object, nested object, array, scalar, absent).
#[tokio::test]
async fn create_then_get_round_trips_every_supplied_member() {
    let harness = Harness::new();
    let benefit = harness.active_benefit().await;
    let benefit_id = text("seed", &benefit, "Id");
    let benefit_programs = list(&benefit, "Programs");

    let rows: Vec<(&str, Value)> = vec![
        ("only the required members", json!({})),
        (
            "name and description",
            json!({ "Name": "demo", "Description": "demo app" }),
        ),
        (
            "two fulfillment types",
            json!({ "Name": "two-types", "FulfillmentTypes": ["CREDITS", "CASH"] }),
        ),
        (
            "an object details document",
            json!({
                "Name": "object-details",
                "BenefitApplicationDetails": { "CampaignName": "spring", "RequestedAmount": 2500 },
            }),
        ),
        (
            "a nested details document",
            json!({
                "Name": "nested-details",
                "BenefitApplicationDetails": {
                    "Campaign": { "Name": "spring", "Regions": ["us-west-2", "eu-west-1"] },
                },
            }),
        ),
        (
            "an array details document",
            json!({ "Name": "array-details", "BenefitApplicationDetails": [1, 2, 3] }),
        ),
        (
            "a scalar details document",
            json!({ "Name": "scalar-details", "BenefitApplicationDetails": "free form" }),
        ),
        (
            "contacts, resources, files and tags",
            json!({
                "Name": "everything",
                "Description": "every optional member at once",
                "FulfillmentTypes": ["ACCESS"],
                "PartnerContacts": [{
                    "Email": "partner@example.com",
                    "FirstName": "Ada",
                    "LastName": "Lovelace",
                    "BusinessTitle": "Principal",
                    "Phone": "+15555550100",
                }],
                "AssociatedResources": [
                    "arn:aws:partnercentral:us-west-2:000000000000:catalog/AWS/opportunity/O-1",
                ],
                "FileDetails": [{
                    "FileURI": "s3://partner-bucket/proposals/plan.pdf",
                    "BusinessUseCase": "funding request",
                }],
                "Tags": [{ "Key": "team", "Value": "partner-ops" }],
            }),
        ),
    ];

    for (index, (case, extra)) in rows.iter().enumerate() {
        let mut body = extra.clone();
        body["Catalog"] = json!(CATALOG);
        body["ClientToken"] = json!(format!("tok-roundtrip-{index}"));
        body["BenefitIdentifier"] = json!(benefit_id);

        let created = harness.create(case, &body).await;
        let id = text(case, &created, "Id");
        assert_id(case, &id, APPLICATION_ID_PREFIX);
        assert_eq!(
            text(case, &created, "Arn"),
            expected_arn(CATALOG, APPLICATION_RESOURCE, &id),
            "case {case:?}: unexpected application ARN in {created:?}"
        );
        assert_eq!(
            text(case, &created, "Revision"),
            "1",
            "case {case:?}: a newly created application must carry its first revision, got {created:?}"
        );

        let got = harness.get(case, &id).await;
        assert_eq!(
            got["Id"], created["Id"],
            "case {case:?}: the Get must report the created id, got {got:?}"
        );
        assert_eq!(
            got["Arn"], created["Arn"],
            "case {case:?}: the Get must report the created ARN, got {got:?}"
        );
        assert_eq!(
            got["Catalog"],
            json!(CATALOG),
            "case {case:?}: the Get must report the catalog it was created in, got {got:?}"
        );
        assert_eq!(
            got["BenefitId"],
            json!(benefit_id),
            "case {case:?}: the Get must report the benefit applied for, got {got:?}"
        );
        assert_eq!(
            got["Status"],
            json!("PENDING_SUBMISSION"),
            "case {case:?}: a new application must be PENDING_SUBMISSION, got {got:?}"
        );
        assert_eq!(
            got["Revision"], created["Revision"],
            "case {case:?}: the Get must report the created revision, got {got:?}"
        );
        assert_eq!(
            list(&got, "Programs"),
            benefit_programs,
            "case {case:?}: the application must carry its benefit's programs, got {got:?}"
        );
        assert_eq!(
            instant(case, &got, "CreatedAt"),
            instant(case, &got, "UpdatedAt"),
            "case {case:?}: an application nothing has changed must report one instant twice, got {got:?}"
        );

        // Every member the request carried comes back unchanged; one it did not carry is
        // absent rather than zeroed.
        for member in [
            "Name",
            "Description",
            "FulfillmentTypes",
            "BenefitApplicationDetails",
            "AssociatedResources",
        ] {
            match body.get(member) {
                Some(sent) => assert_eq!(
                    &got[member], sent,
                    "case {case:?}: {member} must round-trip, got {got:?}"
                ),
                None => assert!(
                    got.get(member).is_none_or(Value::is_null),
                    "case {case:?}: {member} was never sent, so it must be absent, got {got:?}"
                ),
            }
        }
        if let Some(contacts) = body.get("PartnerContacts") {
            assert_eq!(
                &got["PartnerContacts"], contacts,
                "case {case:?}: PartnerContacts must round-trip, got {got:?}"
            );
        }
    }
}

/// Replaying one `ClientToken` answers with the first call's id, ARN and revision and
/// creates nothing; a different token creates a second application.
#[tokio::test]
async fn create_is_idempotent_in_its_client_token() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    let request = json!({
        "Catalog": CATALOG,
        "ClientToken": "tok-1",
        "BenefitIdentifier": benefit_id,
        "Name": "demo",
        "Description": "demo app",
    });

    let first = harness.create("first create", &request).await;
    let replay = harness.create("replayed create", &request).await;
    assert_eq!(
        replay, first,
        "case \"replayed create\": a retried create must answer with the first response, got {replay:?}"
    );

    // Even after the application has moved on, the retry still answers with what the
    // create returned.
    let id = text("first create", &first, "Id");
    harness
        .ok(
            "submit",
            "SubmitBenefitApplication",
            &json!({ "Catalog": CATALOG, "Identifier": id }),
        )
        .await;
    let late_replay = harness.create("late replayed create", &request).await;
    assert_eq!(
        late_replay, first,
        "case \"late replayed create\": a retry must still answer with the first response, got {late_replay:?}"
    );

    let mut second = request.clone();
    second["ClientToken"] = json!("tok-2");
    let other = harness.create("second create", &second).await;
    assert_ne!(
        other["Id"], first["Id"],
        "case \"second create\": a different ClientToken must mint a different application, got {other:?}"
    );

    let listed = harness
        .ok(
            "after two creates",
            "ListBenefitApplications",
            &json!({ "Catalog": CATALOG }),
        )
        .await;
    let ids: BTreeSet<String> = listed["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| text("after two creates", summary, "Id"))
        .collect();
    assert_eq!(
        ids,
        BTreeSet::from([
            text("first create", &first, "Id"),
            text("second create", &other, "Id")
        ]),
        "case \"after two creates\": the store must hold exactly the two applications created, got {listed:?}"
    );
}

/// An unknown `BenefitIdentifier` is a 404, whichever form the modelled `BenefitId`
/// pattern admits it in.
#[tokio::test]
async fn a_create_for_an_unknown_benefit_is_404() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");

    let rows = [
        ("an id no catalog holds", "ben-00000000000000".to_string()),
        (
            "an ARN of another catalog",
            expected_arn("Sandbox", BENEFIT_RESOURCE, &benefit_id),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:catalog/AWS/benefit/{benefit_id}"),
        ),
        (
            "an ARN naming no benefit resource",
            expected_arn(CATALOG, APPLICATION_RESOURCE, &benefit_id),
        ),
    ];

    for (index, (case, identifier)) in rows.iter().enumerate() {
        let (status, out) = harness
            .call(
                "CreateBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-unknown-{index}"),
                    "BenefitIdentifier": identifier,
                }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {identifier:?}, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {out:?}"
        );
    }
}

/// A stored `FileDetail` reports the caller's URI and use case, and derives the file's
/// name and type from that URI.
///
/// The extension -> `FileType` rows are every media type the model's `FileType` enum
/// declares (the generated `FILE_TYPE_VALUES`), plus an extension the enum does not name
/// and a URI with no extension at all — for which the type is absent rather than guessed.
#[tokio::test]
async fn file_details_derive_the_file_name_and_type_from_the_uri() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");

    let rows: [(&str, &str, Option<&str>); 13] = [
        ("s3://b/plan.pdf", "plan.pdf", Some("application/pdf")),
        ("s3://b/old.doc", "old.doc", Some("application/msword")),
        (
            "s3://b/brief.docx",
            "brief.docx",
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        ),
        (
            "s3://b/budget.xlsx",
            "budget.xlsx",
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        ),
        (
            "s3://b/deck.pptx",
            "deck.pptx",
            Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        ),
        ("s3://b/photo.jpeg", "photo.jpeg", Some("image/jpeg")),
        ("s3://b/photo.jpg", "photo.jpg", Some("image/jpeg")),
        ("s3://b/logo.png", "logo.png", Some("image/png")),
        ("s3://b/logo.svg", "logo.svg", Some("image/svg+xml")),
        ("s3://b/rows.csv", "rows.csv", Some("text/csv")),
        // The extension is matched case-insensitively, and an https URI is as valid as
        // an s3 one under the modelled `FileURI` pattern.
        (
            "https://files.example.com/a/b/REPORT.PDF",
            "REPORT.PDF",
            Some("application/pdf"),
        ),
        // Not a media type the `FileType` enum names.
        ("s3://b/archive.zip", "archive.zip", None),
        ("s3://b/no-extension", "no-extension", None),
    ];

    for (index, (uri, expected_name, expected_type)) in rows.iter().enumerate() {
        if let Some(file_type) = expected_type {
            assert!(
                FILE_TYPE_VALUES.contains(file_type),
                "case {uri:?}: {file_type:?} is not a modelled FileType, so the expectation is wrong"
            );
        }
        let created = harness
            .create(
                uri,
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-file-{index}"),
                    "BenefitIdentifier": benefit_id,
                    "FileDetails": [{ "FileURI": uri, "BusinessUseCase": "evidence" }],
                }),
            )
            .await;
        let got = harness.get(uri, &text(uri, &created, "Id")).await;
        let files = got["FileDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {uri:?}: expected FileDetails, got {got:?}"))
            .clone();
        assert_eq!(
            files.len(),
            1,
            "case {uri:?}: expected one file, got {files:?}"
        );
        let file = &files[0];
        assert_eq!(
            file["FileURI"],
            json!(uri),
            "case {uri:?}: the URI must round-trip, got {file:?}"
        );
        assert_eq!(
            file["BusinessUseCase"],
            json!("evidence"),
            "case {uri:?}: the use case must round-trip, got {file:?}"
        );
        assert_eq!(
            file["FileName"],
            json!(expected_name),
            "case {uri:?}: unexpected FileName, got {file:?}"
        );
        match expected_type {
            Some(file_type) => assert_eq!(
                file["FileType"],
                json!(file_type),
                "case {uri:?}: unexpected FileType, got {file:?}"
            ),
            None => assert!(
                file.get("FileType").is_none_or(Value::is_null),
                "case {uri:?}: an unlisted extension must carry no FileType, got {file:?}"
            ),
        }
        assert!(
            !text(uri, file, "CreatedBy").is_empty(),
            "case {uri:?}: expected the attaching principal, got {file:?}"
        );
        // Nothing processed the attachment, so there is no status to report.
        assert!(
            file.get("FileStatus").is_none_or(Value::is_null),
            "case {uri:?}: no file status may be invented, got {file:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateBenefitApplication
// ---------------------------------------------------------------------------

/// An update applies the members it carries, leaves the rest alone, and bumps the
/// revision and `UpdatedAt`; the next Get reports exactly that.
#[tokio::test]
async fn update_applies_supplied_members_and_bumps_the_revision() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-update",
                "BenefitIdentifier": benefit_id,
                "Name": "demo",
                "Description": "demo app",
                "BenefitApplicationDetails": { "CampaignName": "spring" },
            }),
        )
        .await;
    let id = text("seed", &created, "Id");

    // Each row changes a different subset; the members a row omits must survive it.
    let rows: Vec<(&str, Value)> = vec![
        ("the name only", json!({ "Name": "demo-2" })),
        (
            "the description only",
            json!({ "Description": "a longer description" }),
        ),
        (
            "the details document",
            json!({ "BenefitApplicationDetails": { "CampaignName": "summer", "RequestedAmount": 10 } }),
        ),
        (
            "contacts and files",
            json!({
                "PartnerContacts": [{ "Email": "new@example.com", "FirstName": "Grace" }],
                "FileDetails": [{ "FileURI": "s3://b/updated.csv" }],
            }),
        ),
        (
            "every updatable member at once",
            json!({
                "Name": "demo-3",
                "Description": "final",
                "BenefitApplicationDetails": { "CampaignName": "autumn" },
            }),
        ),
    ];

    let mut before = harness.get("seed", &id).await;
    for (index, (case, change)) in rows.iter().enumerate() {
        let revision = text(case, &before, "Revision");
        let mut body = change.clone();
        body["Catalog"] = json!(CATALOG);
        body["ClientToken"] = json!(format!("tok-update-{index}"));
        body["Identifier"] = json!(id);
        body["Revision"] = json!(revision);

        let updated = harness.ok(case, "UpdateBenefitApplication", &body).await;
        let new_revision = text(case, &updated, "Revision");
        assert_ne!(
            new_revision, revision,
            "case {case:?}: an update must bump the revision, got {updated:?}"
        );
        assert_eq!(
            updated["Id"],
            json!(id),
            "case {case:?}: the update must report the application it changed, got {updated:?}"
        );

        let after = harness.get(case, &id).await;
        assert_eq!(
            text(case, &after, "Revision"),
            new_revision,
            "case {case:?}: the Get must report the new revision, got {after:?}"
        );
        assert!(
            instant(case, &after, "UpdatedAt") > instant(case, &after, "CreatedAt"),
            "case {case:?}: UpdatedAt must move past CreatedAt, got {after:?}"
        );
        for (member, sent) in change.as_object().expect("each row is an object") {
            if member == "FileDetails" {
                // `FileInput` carries only the URI and use case; the stored `FileDetail`
                // adds the members derived from that URI, so the two are compared by the
                // URI they share.
                let stored: Vec<Value> = after["FileDetails"]
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .map(|file| file["FileURI"].clone())
                    .collect();
                let wanted: Vec<Value> = sent
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .map(|file| file["FileURI"].clone())
                    .collect();
                assert_eq!(
                    stored, wanted,
                    "case {case:?}: FileDetails must be stored, got {after:?}"
                );
                continue;
            }
            assert_eq!(
                &after[member.as_str()], sent,
                "case {case:?}: {member} must be stored, got {after:?}"
            );
        }
        for member in ["Name", "Description", "BenefitApplicationDetails"] {
            if change.get(member).is_none() {
                assert_eq!(
                    after[member], before[member],
                    "case {case:?}: {member} was not in the request, so it must not change, got {after:?}"
                );
            }
        }
        assert_eq!(
            after["Status"],
            json!("PENDING_SUBMISSION"),
            "case {case:?}: an update must not move the status, got {after:?}"
        );
        before = after;
    }
}

/// An update that does not name the application's current revision is a 409, and changes
/// nothing.
#[tokio::test]
async fn update_refuses_a_revision_that_is_not_the_current_one() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-conflict",
                "BenefitIdentifier": benefit_id,
                "Name": "demo",
            }),
        )
        .await;
    let id = text("seed", &created, "Id");

    // Bump it once, so the create's own revision is now stale.
    let current = text(
        "first update",
        &harness
            .ok(
                "first update",
                "UpdateBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": "tok-conflict-1",
                    "Identifier": id,
                    "Revision": text("seed", &created, "Revision"),
                    "Name": "demo-2",
                }),
            )
            .await,
        "Revision",
    );

    for stale in [
        text("seed", &created, "Revision"),
        "0".to_string(),
        "99999".to_string(),
        "not-a-revision".to_string(),
    ] {
        let case = format!("revision {stale:?}");
        let (status, out) = harness
            .call(
                "UpdateBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": "tok-conflict-stale",
                    "Identifier": id,
                    "Revision": stale,
                    "Name": "should-not-apply",
                }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {out:?}"
        );
        let got = harness.get(&case, &id).await;
        assert_eq!(
            got["Name"],
            json!("demo-2"),
            "case {case:?}: a refused update must change nothing, got {got:?}"
        );
        assert_eq!(
            text(&case, &got, "Revision"),
            current,
            "case {case:?}: a refused update must not bump the revision, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// SubmitBenefitApplication / CancelBenefitApplication
// ---------------------------------------------------------------------------

/// Submitting moves a `PENDING_SUBMISSION` application to `IN_REVIEW` and advances
/// `UpdatedAt`; submitting from any other status is a 409.
#[tokio::test]
async fn submit_transitions_only_from_pending_submission() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");

    let created = harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-submit",
                "BenefitIdentifier": benefit_id,
                "Name": "demo",
            }),
        )
        .await;
    let id = text("seed", &created, "Id");
    let before = harness.get("seed", &id).await;

    harness
        .ok(
            "submit",
            "SubmitBenefitApplication",
            &json!({ "Catalog": CATALOG, "Identifier": id }),
        )
        .await;
    let after = harness.get("submit", &id).await;
    assert_eq!(
        after["Status"],
        json!("IN_REVIEW"),
        "case \"submit\": a submitted application must be IN_REVIEW, got {after:?}"
    );
    assert!(
        instant("submit", &after, "UpdatedAt") > instant("seed", &before, "UpdatedAt"),
        "case \"submit\": a submit must advance UpdatedAt, got {after:?}"
    );

    // The transition is observable through the listing too, not only the Get.
    let listed = harness
        .ok(
            "submit",
            "ListBenefitApplications",
            &json!({ "Catalog": CATALOG, "Status": ["IN_REVIEW"] }),
        )
        .await;
    let ids: Vec<String> = listed["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| text("submit", summary, "Id"))
        .collect();
    assert_eq!(
        ids,
        vec![id.clone()],
        "case \"submit\": the IN_REVIEW filter must report the submitted application, got {listed:?}"
    );

    // From IN_REVIEW, and from CANCELED, a submit is refused.
    let mut refusals: Vec<(&str, String)> = vec![("already IN_REVIEW", id.clone())];
    let cancelled = harness
        .create(
            "cancelled seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-submit-cancelled",
                "BenefitIdentifier": benefit_id,
            }),
        )
        .await;
    let cancelled_id = text("cancelled seed", &cancelled, "Id");
    harness
        .ok(
            "cancel",
            "CancelBenefitApplication",
            &json!({ "Catalog": CATALOG, "ClientToken": "tok-cancel-0", "Identifier": cancelled_id }),
        )
        .await;
    refusals.push(("already CANCELED", cancelled_id));

    for (case, identifier) in refusals {
        let (status, out) = harness
            .call(
                "SubmitBenefitApplication",
                &json!({ "Catalog": CATALOG, "Identifier": identifier }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {out:?}"
        );
    }
}

/// Cancelling records the reason as `StatusReason`, bumps the revision, and is terminal.
///
/// The rows are the statuses an application can be cancelled from, each with and without
/// a `Reason`.
#[tokio::test]
async fn cancel_records_the_reason_and_is_terminal() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");

    let rows: [(&str, bool, Option<&str>); 4] = [
        ("pending, with a reason", false, Some("no longer needed")),
        ("pending, without a reason", false, None),
        ("in review, with a reason", true, Some("customer withdrew")),
        ("in review, without a reason", true, None),
    ];

    for (index, (case, submit_first, reason)) in rows.iter().enumerate() {
        let created = harness
            .create(
                case,
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-cancel-seed-{index}"),
                    "BenefitIdentifier": benefit_id,
                    "Name": format!("cancel-{index}"),
                }),
            )
            .await;
        let id = text(case, &created, "Id");
        if *submit_first {
            harness
                .ok(
                    case,
                    "SubmitBenefitApplication",
                    &json!({ "Catalog": CATALOG, "Identifier": id }),
                )
                .await;
        }
        let before = harness.get(case, &id).await;

        let mut body = json!({
            "Catalog": CATALOG,
            "ClientToken": format!("tok-cancel-{index}"),
            "Identifier": id,
        });
        if let Some(reason) = reason {
            body["Reason"] = json!(reason);
        }
        harness.ok(case, "CancelBenefitApplication", &body).await;

        let after = harness.get(case, &id).await;
        assert_eq!(
            after["Status"],
            json!("CANCELED"),
            "case {case:?}: a cancelled application must be CANCELED, got {after:?}"
        );
        match reason {
            Some(reason) => assert_eq!(
                after["StatusReason"],
                json!(reason),
                "case {case:?}: the reason must be stored as StatusReason, got {after:?}"
            ),
            None => assert!(
                after.get("StatusReason").is_none_or(Value::is_null),
                "case {case:?}: no reason was given, so none may be invented, got {after:?}"
            ),
        }
        assert_ne!(
            text(case, &after, "Revision"),
            text(case, &before, "Revision"),
            "case {case:?}: a cancel must bump the revision, got {after:?}"
        );
        assert!(
            instant(case, &after, "UpdatedAt") > instant(case, &before, "UpdatedAt"),
            "case {case:?}: a cancel must advance UpdatedAt, got {after:?}"
        );

        let (status, out) = harness.call("CancelBenefitApplication", &body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a second cancel must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {out:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListBenefitApplications
// ---------------------------------------------------------------------------

/// Paging reports every stored application exactly once, in pages no larger than
/// `MaxResults`, and the last page carries no `NextToken`.
///
/// The rows vary both what the rule generalises over: how many applications are stored
/// and how big a page is asked for — including the published `MaxResults` bounds.
#[tokio::test]
async fn paging_reports_every_application_exactly_once() {
    let rows: [(usize, i32); 6] = [
        (1, 1),
        (3, 1),
        (5, 2),
        (4, 3),
        (2, MAX_PAGE_SIZE),
        (7, MAX_PAGE_SIZE),
    ];

    for (total, max_results) in rows {
        let case = format!("{total} applications, MaxResults {max_results}");
        let harness = Harness::new();
        let benefit_id = text(&case, &harness.active_benefit().await, "Id");
        let mut created = BTreeSet::new();
        for index in 0..total {
            let response = harness
                .create(
                    &case,
                    &json!({
                        "Catalog": CATALOG,
                        "ClientToken": format!("tok-page-{index}"),
                        "BenefitIdentifier": benefit_id,
                        "Name": format!("page-{index}"),
                    }),
                )
                .await;
            created.insert(text(&case, &response, "Id"));
        }

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "Catalog": CATALOG, "MaxResults": max_results });
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let out = harness.ok(&case, "ListBenefitApplications", &body).await;
            let page = out["BenefitApplicationSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a page, got {out:?}"))
                .clone();
            assert!(
                !page.is_empty(),
                "case {case:?}: a page carrying a NextToken must not be empty, got {out:?}"
            );
            assert!(
                i32::try_from(page.len()).unwrap_or(i32::MAX) <= max_results,
                "case {case:?}: a page must hold at most {max_results} summaries, got {} ",
                page.len()
            );
            for summary in &page {
                seen.push(text(&case, summary, "Id"));
                assert_eq!(
                    summary["Catalog"],
                    json!(CATALOG),
                    "case {case:?}: every summary must report its catalog, got {summary:?}"
                );
                assert_eq!(
                    summary["BenefitId"],
                    json!(benefit_id),
                    "case {case:?}: every summary must report its benefit, got {summary:?}"
                );
            }
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            next_token = out
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: no application may be reported twice, got {seen:?}"
        );
        assert_eq!(
            unique, created,
            "case {case:?}: paging must report exactly the stored applications, got {seen:?}"
        );
    }
}

/// A `MaxResults` outside the published 1..100 range is refused, and so is a `NextToken`
/// this service never issued.
#[tokio::test]
async fn paging_refuses_a_max_results_past_a_bound_and_a_token_it_never_issued() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-bounds",
                "BenefitIdentifier": benefit_id,
            }),
        )
        .await;

    // Each published bound is accepted...
    for max_results in [1, MAX_PAGE_SIZE] {
        let case = format!("MaxResults {max_results}");
        harness
            .ok(
                &case,
                "ListBenefitApplications",
                &json!({ "Catalog": CATALOG, "MaxResults": max_results }),
            )
            .await;
        harness
            .ok(
                &case,
                "ListBenefits",
                &json!({ "Catalog": CATALOG, "MaxResults": max_results }),
            )
            .await;
    }

    // ...and one past either bound is refused, on both list operations.
    for operation in ["ListBenefitApplications", "ListBenefits"] {
        for max_results in [0, MAX_PAGE_SIZE + 1] {
            let case = format!("{operation} MaxResults {max_results}");
            let (status, out) = harness
                .call(
                    operation,
                    &json!({ "Catalog": CATALOG, "MaxResults": max_results }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {out:?}"
            );
        }
    }

    for operation in ["ListBenefitApplications", "ListBenefits"] {
        for token in ["not-a-token", "6162", "benappl-00000000000000"] {
            let case = format!("{operation} NextToken {token:?}");
            let (status, out) = harness
                .call(
                    operation,
                    &json!({ "Catalog": CATALOG, "NextToken": token }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {out:?}"
            );
        }
    }
}

/// The listing's filters select exactly the applications whose own members satisfy them,
/// and its summaries report the stored record.
#[tokio::test]
async fn list_benefit_applications_honours_its_filters() {
    let harness = Harness::new();
    let benefits = harness.benefits(CATALOG).await;
    let first = text("seed", &benefits[0], "Id");
    let second = text("seed", &benefits[1], "Id");

    // One application per (benefit, status) combination the slice can reach.
    let mut ids: HashMap<&str, String> = HashMap::new();
    for (key, benefit, submit) in [
        ("first-pending", &first, false),
        ("first-review", &first, true),
        ("second-pending", &second, false),
    ] {
        let created = harness
            .create(
                key,
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-filter-{key}"),
                    "BenefitIdentifier": benefit,
                    "Name": key,
                    "FulfillmentTypes": ["CREDITS"],
                    "BenefitApplicationDetails": { "CampaignName": key },
                    "AssociatedResources": [
                        format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/AWS/opportunity/{key}"),
                    ],
                }),
            )
            .await;
        let id = text(key, &created, "Id");
        if submit {
            harness
                .ok(
                    key,
                    "SubmitBenefitApplication",
                    &json!({ "Catalog": CATALOG, "Identifier": id }),
                )
                .await;
        }
        ids.insert(key, id);
    }

    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter", json!({}), vec!["first-pending", "first-review", "second-pending"]),
        (
            "PENDING_SUBMISSION",
            json!({ "Status": ["PENDING_SUBMISSION"] }),
            vec!["first-pending", "second-pending"],
        ),
        (
            "IN_REVIEW",
            json!({ "Status": ["IN_REVIEW"] }),
            vec!["first-review"],
        ),
        (
            "APPROVED",
            json!({ "Status": ["APPROVED"] }),
            vec![],
        ),
        (
            "the first benefit",
            json!({ "BenefitIdentifiers": [first.clone()] }),
            vec!["first-pending", "first-review"],
        ),
        (
            "the second benefit",
            json!({ "BenefitIdentifiers": [second.clone()] }),
            vec!["second-pending"],
        ),
        (
            "the second benefit by ARN",
            json!({ "BenefitIdentifiers": [expected_arn(CATALOG, BENEFIT_RESOURCE, &second)] }),
            vec!["second-pending"],
        ),
        (
            "a benefit no catalog holds",
            json!({ "BenefitIdentifiers": ["ben-00000000000000"] }),
            vec![],
        ),
        (
            "a fulfillment type every row carries",
            json!({ "FulfillmentTypes": ["CREDITS"] }),
            vec!["first-pending", "first-review", "second-pending"],
        ),
        (
            "a fulfillment type no row carries",
            json!({ "FulfillmentTypes": ["ACCESS"] }),
            vec![],
        ),
        (
            "one associated resource ARN",
            json!({ "AssociatedResourceArns": [
                format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/AWS/opportunity/first-review"),
            ] }),
            vec!["first-review"],
        ),
        (
            "a stage no application has",
            json!({ "Stages": ["QUALIFY"] }),
            vec![],
        ),
    ];

    for (case, filter, expected) in rows {
        let mut body = filter.clone();
        body["Catalog"] = json!(CATALOG);
        let out = harness.ok(case, "ListBenefitApplications", &body).await;
        let actual: BTreeSet<String> = out["BenefitApplicationSummaries"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|summary| text(case, summary, "Id"))
            .collect();
        let wanted: BTreeSet<String> = expected
            .iter()
            .map(|key| ids[key].clone())
            .collect::<BTreeSet<String>>();
        assert_eq!(
            actual, wanted,
            "case {case:?}: the filter must select exactly the matching applications, got {out:?}"
        );
    }

    // The summary reports the stored record, including the details document projected
    // into the `map<String, String>` the summary models.
    let out = harness
        .ok(
            "summary shape",
            "ListBenefitApplications",
            &json!({ "Catalog": CATALOG, "BenefitIdentifiers": [second] }),
        )
        .await;
    let summary = &out["BenefitApplicationSummaries"][0];
    assert_eq!(
        summary["Id"],
        json!(ids["second-pending"]),
        "case \"summary shape\": unexpected application, got {out:?}"
    );
    assert_eq!(
        summary["Name"],
        json!("second-pending"),
        "case \"summary shape\": the summary must report the stored name, got {summary:?}"
    );
    assert_eq!(
        summary["Arn"],
        json!(expected_arn(
            CATALOG,
            APPLICATION_RESOURCE,
            &ids["second-pending"]
        )),
        "case \"summary shape\": the summary must report the stored ARN, got {summary:?}"
    );
    assert_eq!(
        summary["BenefitApplicationDetails"],
        json!({ "CampaignName": "second-pending" }),
        "case \"summary shape\": the summary must project the stored details, got {summary:?}"
    );
}

// ---------------------------------------------------------------------------
// Identifier resolution
// ---------------------------------------------------------------------------

/// Every operation that takes an `Identifier` resolves the same application from its
/// bare id and from its ARN, and answers 404 for an identifier naming nothing.
#[tokio::test]
async fn identifiers_resolve_by_id_and_by_arn() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-identifier",
                "BenefitIdentifier": benefit_id,
                "Name": "demo",
            }),
        )
        .await;
    let id = text("seed", &created, "Id");
    let arn = text("seed", &created, "Arn");

    // Both forms resolve to the one record.
    for (case, identifier) in [("the bare id", id.clone()), ("the full ARN", arn.clone())] {
        let got = harness.get(case, &identifier).await;
        assert_eq!(
            got["Id"],
            json!(id),
            "case {case:?}: both identifier forms must resolve to one record, got {got:?}"
        );
    }

    // And an identifier naming nothing is a 404 — including an ARN that differs from the
    // stored one in exactly one field.
    let unknown: Vec<(&str, String, &str)> = vec![
        ("an id nothing was minted for", "benappl-00000000000000".to_string(), CATALOG),
        (
            "an ARN of another region",
            arn.replace(REGION, "eu-west-1"),
            CATALOG,
        ),
        (
            "an ARN of another account",
            arn.replace(ACCOUNT, "123456789012"),
            CATALOG,
        ),
        (
            "an ARN of another partition",
            arn.replace("arn:aws:", "arn:aws-cn:"),
            CATALOG,
        ),
        ("the right ARN in the wrong catalog", arn.clone(), "Sandbox"),
        ("the right id in the wrong catalog", id.clone(), "Sandbox"),
    ];

    for (case, identifier, catalog) in unknown {
        for operation in [
            "GetBenefitApplication",
            "SubmitBenefitApplication",
            "CancelBenefitApplication",
            "UpdateBenefitApplication",
        ] {
            let case = format!("{operation}: {case}");
            let (status, out) = harness
                .call(
                    operation,
                    &json!({
                        "Catalog": catalog,
                        "Identifier": identifier,
                        "ClientToken": "tok-identifier-unknown",
                        "Revision": "1",
                    }),
                )
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for {identifier:?}, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {out:?}"
            );
        }
    }

    // An identifier outside the modelled pattern never reaches a handler.
    for identifier in ["nope", "benappl-SHOUTING00000", ""] {
        let case = format!("identifier {identifier:?}");
        let (status, out) = harness
            .call(
                "GetBenefitApplication",
                &json!({ "Catalog": CATALOG, "Identifier": identifier }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
    }
}

/// Applications live in the catalog they were created in: one catalog's listing never
/// reports another's.
#[tokio::test]
async fn applications_are_scoped_to_their_catalog() {
    let harness = Harness::new();
    let mut ids: HashMap<&str, String> = HashMap::new();

    for catalog in SEEDED_CATALOGS {
        let benefit = harness
            .benefits(catalog)
            .await
            .into_iter()
            .next()
            .expect("every seeded catalog holds a benefit");
        let created = harness
            .create(
                catalog,
                &json!({
                    "Catalog": catalog,
                    "ClientToken": format!("tok-scope-{catalog}"),
                    "BenefitIdentifier": text(catalog, &benefit, "Id"),
                    "Name": format!("in-{catalog}"),
                }),
            )
            .await;
        ids.insert(catalog, text(catalog, &created, "Id"));
    }

    for catalog in SEEDED_CATALOGS {
        let out = harness
            .ok(
                catalog,
                "ListBenefitApplications",
                &json!({ "Catalog": catalog }),
            )
            .await;
        let listed: Vec<String> = out["BenefitApplicationSummaries"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|summary| text(catalog, summary, "Id"))
            .collect();
        assert_eq!(
            listed,
            vec![ids[catalog].clone()],
            "case {catalog:?}: a catalog must list only its own applications, got {out:?}"
        );
    }
}
