// SPDX-License-Identifier: CC0-1.0

//! The quota request TEMPLATE plane, end to end over the wire.
//!
//! The rule under test: a quota request template is an AWS ORGANIZATION-wide object —
//! `AssociateServiceQuotaTemplate` documents it as "Associates your quota request
//! template with your organization. When a new Amazon Web Services account is created in
//! your organization, the quota increase requests in the template are automatically
//! applied to the account" — so the plane is served exactly while the calling account is
//! a member of an organization with ALL features enabled and the call is made at the
//! Region the plane is served from, and refused otherwise; past that gate every template
//! operation computes its answer from the stored template, so what a write stores is what
//! the reads report and what a delete removes stops being reported.
//!
//! That rule spans two services, so both are mounted on ONE engine: Service Quotas is
//! driven over the wire the way a client drives it, and AWS Organizations — which owns
//! the organization the template belongs to — over the same wire, through the in-process
//! callback the template gate itself reads membership over.
//!
//! The rows span the class the rule generalises over: the three membership states (no
//! organization, an organization with only consolidated billing, an all-features
//! organization), every one of the seven template operations, Regions inside and outside
//! the plane's home Region, entries differing in service, quota, Region, unit, whether
//! the quota is counted account-wide, and value, a template filtered by each of its two
//! filters and by both, pages at a size of one, and each way a put can be refused.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The routing header Service Quotas is reached by, from this crate's `OPERATIONS.md`.
const QUOTAS_TARGET_PREFIX: &str = "ServiceQuotasV20190624.";

/// The routing header AWS Organizations is reached by, from that crate's
/// `OPERATIONS.md`: `POST /` with `X-Amz-Target: AWSOrganizationsV20161128.<Op>`.
const ORGANIZATIONS_TARGET_PREFIX: &str = "AWSOrganizationsV20161128.";

/// The Region the quota request template plane is served from
/// (`crate::state::TEMPLATE_HOME_REGION`).
pub(crate) const HOME_REGION: &str = "us-east-1";

/// Service Quotas and AWS Organizations over one shared engine.
///
/// `pub(crate)` so the other suites in this target that drive the template plane share
/// ONE harness instead of each growing a copy of it: the plane's gate reads the
/// account's organization out of AWS Organizations, so both services have to be mounted
/// on the same engine to reach it at all.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// Both services on one engine, with the Organizations store put back into the state
    /// of an account that belongs to NO organization.
    ///
    /// Mounting the Organizations control plane establishes the organization the emulated
    /// account belongs to, so the membership states BELOW the all-features one — no
    /// organization at all, and a consolidated-billing organization — are reached by
    /// clearing that store after both services are mounted and creating what the case
    /// needs.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These callers sign nothing, so they must not be refused for it.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_servicequotas_servicequotasv20190624::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        *engine
            .state::<service_com_amazonaws_organizations_awsorganizationsv20161128::state::ComAmazonawsOrganizationsAwsorganizationsv20161128State>()
            .lock() = Default::default();
        Self { router }
    }

    /// Both services, with the account made the management account of an organization
    /// whose `FeatureSet` is `feature_set`.
    pub(crate) async fn in_organization(feature_set: &str) -> Self {
        let emulator = Self::new();
        let created = emulator
            .call(
                ORGANIZATIONS_TARGET_PREFIX,
                "organizations",
                HOME_REGION,
                "CreateOrganization",
                &json!({"FeatureSet": feature_set}),
            )
            .await;
        assert_eq!(
            created.0, 200,
            "case an organization with FeatureSet {feature_set}: expected it to be created, \
             got {} with {}",
            created.0, created.1
        );
        emulator
    }

    /// Dispatch one awsJson1_1 call and return its status and parsed body.
    async fn call(
        &self,
        target_prefix: &str,
        signing_name: &str,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}{operation}"))
            // The credential scope is what routes the request to a service and names the
            // Region it is addressed to.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTCLIENT/20260101/{region}/\
                     {signing_name}/aws4_request, SignedHeaders=host;x-amz-date, \
                     Signature=notchecked"
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

    /// One Service Quotas call at `region`.
    async fn quotas(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        self.call(
            QUOTAS_TARGET_PREFIX,
            "servicequotas",
            region,
            operation,
            &body,
        )
        .await
    }

    /// One Service Quotas call at the template plane's home Region, required to answer
    /// 200.
    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.quotas(HOME_REGION, operation, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One Service Quotas call at `region` that must be refused, as its status and the
    /// error code the envelope carries.
    async fn refused(&self, region: &str, operation: &str, body: Value) -> (u16, String) {
        let (status, parsed) = self.quotas(region, operation, body).await;
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .to_string();
        (status, code)
    }

    /// One Service Quotas call at the template plane's home Region that must be refused,
    /// as its status, the error code the envelope carries and the `Message` it states the
    /// fact in.
    ///
    /// `pub(crate)` so a suite that asserts the WORDING of a template-plane refusal
    /// reaches it through this harness instead of growing a second copy of the two
    /// services on one engine.
    pub(crate) async fn refused_at_home(
        &self,
        operation: &str,
        body: Value,
    ) -> (u16, String, String) {
        let (status, parsed) = self.quotas(HOME_REGION, operation, body).await;
        let member = |name: &str, missing: &str| {
            parsed
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or(missing)
                .to_string()
        };
        (
            status,
            member("__type", "<no __type>"),
            member("Message", "<no Message>"),
        )
    }
}

/// Every template operation, with a request the model accepts: the three association
/// operations take no members, the entry operations take a quota and a Region, and the
/// listing takes optional filters.
fn every_template_call() -> Vec<(&'static str, Value)> {
    vec![
        ("AssociateServiceQuotaTemplate", json!({})),
        ("DisassociateServiceQuotaTemplate", json!({})),
        ("GetAssociationForServiceQuotaTemplate", json!({})),
        (
            "PutServiceQuotaIncreaseRequestIntoTemplate",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "AwsRegion": "us-east-1",
                "DesiredValue": 100.0,
            }),
        ),
        (
            "GetServiceQuotaIncreaseRequestFromTemplate",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "AwsRegion": "us-east-1"}),
        ),
        (
            "DeleteServiceQuotaIncreaseRequestFromTemplate",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "AwsRegion": "us-east-1"}),
        ),
        ("ListServiceQuotaIncreaseRequestsInTemplate", json!({})),
    ]
}

/// The plane is reachable only for an account whose organization has all features: an
/// account in none is refused for not being in an organization, and one whose
/// organization enables only consolidated billing is refused for the feature set.
#[tokio::test]
async fn the_template_plane_is_refused_until_the_account_is_in_an_all_features_organization() {
    // The rows are the three membership states, each with the condition the gate must
    // state for it. An operation that models the all-features condition states it; one
    // that does not states the Service Access that condition denies, which every
    // template operation models.
    let rows: Vec<(&str, Option<&str>, &str, &str)> = vec![
        (
            "an account in no organization",
            None,
            "NoAvailableOrganizationException",
            "NoAvailableOrganizationException",
        ),
        (
            "an organization with consolidated billing only",
            Some("CONSOLIDATED_BILLING"),
            "OrganizationNotInAllFeaturesModeException",
            "AWSServiceAccessNotEnabledException",
        ),
    ];

    for (why, feature_set, associate_code, other_code) in rows {
        let emulator = match feature_set {
            Some(feature_set) => Emulator::in_organization(feature_set).await,
            None => Emulator::new(),
        };
        for (operation, body) in every_template_call() {
            let case = format!("{why}: {operation}");
            let expected = if operation == "AssociateServiceQuotaTemplate" {
                associate_code
            } else {
                other_code
            };
            let (status, code) = emulator.refused(HOME_REGION, operation, body).await;
            // Each of these is a client fault, which this awsJson front end serves on
            // 400 whatever `@httpError` the shape declares.
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {code}"
            );
            assert!(
                code.contains(expected),
                "case {case}: expected {expected}, got {code}"
            );
            // Never the scaffold's 501: the gate IS the behaviour here.
            assert!(
                !code.contains("NotImplemented"),
                "case {case}: the gate must be evaluated, got {code}"
            );
        }
    }
}

/// The plane is served from ONE Region: an all-features organization reaches it at the
/// home Region and nowhere else.
#[tokio::test]
async fn the_template_plane_is_served_from_its_home_region_only() {
    let emulator = Emulator::in_organization("ALL").await;

    // The rows differ in partition and in distance from the home Region.
    for region in ["us-west-2", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        for (operation, body) in every_template_call() {
            let case = format!("{operation} from {region}");
            let (status, code) = emulator.refused(region, operation, body).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {code}"
            );
            assert!(
                code.contains("TemplatesNotAvailableInRegionException"),
                "case {case}: expected TemplatesNotAvailableInRegionException, got {code}"
            );
        }
    }

    // At the home Region the same calls are served: the association operations answer,
    // and the entry reads answer about the template rather than about the plane.
    let associated = emulator.ok("AssociateServiceQuotaTemplate", json!({})).await;
    assert_eq!(
        associated,
        json!({}),
        "case AssociateServiceQuotaTemplate at the home Region: expected an empty 200, got \
         {associated}"
    );
    let listing = emulator
        .ok("ListServiceQuotaIncreaseRequestsInTemplate", json!({}))
        .await;
    assert_eq!(
        listing["ServiceQuotaIncreaseRequestInTemplateList"],
        json!([]),
        "case a listing of an empty template at the home Region: got {listing}"
    );
}

/// One row of the template-entry table: a quota, the Region the entry applies in, and
/// the value it asks for.
struct EntryRow {
    /// Why this row is in the table.
    why: &'static str,
    service_code: &'static str,
    quota_code: &'static str,
    aws_region: &'static str,
    desired: f64,
}

impl EntryRow {
    /// The `(ServiceCode, QuotaCode, AwsRegion)` body a read or a delete of this entry
    /// takes.
    fn triple(&self) -> Value {
        json!({
            "ServiceCode": self.service_code,
            "QuotaCode": self.quota_code,
            "AwsRegion": self.aws_region,
        })
    }

    /// The body a put of this entry takes.
    fn put_body(&self, desired: f64) -> Value {
        json!({
            "ServiceCode": self.service_code,
            "QuotaCode": self.quota_code,
            "AwsRegion": self.aws_region,
            "DesiredValue": desired,
        })
    }

    /// How a failed assertion names this row.
    fn case(&self) -> String {
        format!(
            "{} ({}/{} in {})",
            self.why, self.service_code, self.quota_code, self.aws_region
        )
    }
}

/// The entries the table drives, spanning the class: two Regions for ONE quota, two
/// other services' quotas, a quota AWS counts account-wide, and values of different
/// magnitudes. The quotas are seeded rows of this service's own catalog
/// (`src/state.rs::seed_quota_catalog`), each named by the `QuotaCode` AWS publishes.
fn entry_rows() -> Vec<EntryRow> {
    vec![
        EntryRow {
            why: "a per-Region quota in the home Region",
            service_code: "vpc",
            quota_code: "L-F678F1CE",
            aws_region: "us-east-1",
            desired: 100.0,
        },
        EntryRow {
            why: "the SAME quota in another Region",
            service_code: "vpc",
            quota_code: "L-F678F1CE",
            aws_region: "eu-west-1",
            desired: 50.0,
        },
        EntryRow {
            why: "another service's quota in the home Region",
            service_code: "ec2",
            quota_code: "L-0263D0A3",
            aws_region: "us-east-1",
            desired: 20.0,
        },
        EntryRow {
            why: "a quota AWS counts account-wide",
            service_code: "s3",
            quota_code: "L-DC2B2D3D",
            aws_region: "us-west-2",
            desired: 20_000.0,
        },
        EntryRow {
            why: "a quota whose default is in the thousands",
            service_code: "dynamodb",
            quota_code: "L-F98FE922",
            aws_region: "ap-southeast-2",
            desired: 100_000.0,
        },
    ]
}

/// The `(ServiceCode, QuotaCode, AwsRegion, DesiredValue)` an entry on the wire reports,
/// for comparing a listing against what was stored. The value is kept in its JSON form,
/// so a selection can be ordered and compared as a whole.
fn entry_identity(entry: &Value) -> (String, String, String, String) {
    (
        entry["ServiceCode"].as_str().unwrap_or_default().to_string(),
        entry["QuotaCode"].as_str().unwrap_or_default().to_string(),
        entry["AwsRegion"].as_str().unwrap_or_default().to_string(),
        entry["DesiredValue"].to_string(),
    )
}

/// What a put stores is what every read of the template reports: the entry round-trips
/// through the store, carrying the quota's own catalog facts, and a put for a triple the
/// template already holds replaces that entry rather than adding a second one.
#[tokio::test]
async fn a_template_entry_round_trips_through_the_store() {
    let emulator = Emulator::in_organization("ALL").await;

    for row in entry_rows() {
        let case = row.case();
        let stored = emulator
            .ok(
                "PutServiceQuotaIncreaseRequestIntoTemplate",
                row.put_body(row.desired),
            )
            .await["ServiceQuotaIncreaseRequestInTemplate"]
            .clone();
        // The AWS default quota is where the entry's `ServiceName`, `QuotaName`, `Unit`
        // and `GlobalQuota` come from: they are the quota's facts, not the request's, so
        // they must agree with what a quota read reports for the same pair.
        let default = emulator
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code}),
            )
            .await["Quota"]
            .clone();
        let expected = json!({
            "ServiceCode": row.service_code,
            "ServiceName": default["ServiceName"],
            "QuotaCode": row.quota_code,
            "QuotaName": default["QuotaName"],
            "DesiredValue": row.desired,
            "AwsRegion": row.aws_region,
            "Unit": default["Unit"],
            "GlobalQuota": default["GlobalQuota"],
        });
        assert_eq!(
            stored, expected,
            "case {case}: the stored entry must report the quota's own facts, got {stored}"
        );

        // Read back through the modelled reader: the same record, and reading it twice
        // does not change it.
        let read = emulator
            .ok("GetServiceQuotaIncreaseRequestFromTemplate", row.triple())
            .await["ServiceQuotaIncreaseRequestInTemplate"]
            .clone();
        assert_eq!(
            read, stored,
            "case {case}: the read must report the entry as stored, got {read}"
        );
        let again = emulator
            .ok("GetServiceQuotaIncreaseRequestFromTemplate", row.triple())
            .await["ServiceQuotaIncreaseRequestInTemplate"]
            .clone();
        assert_eq!(
            again, read,
            "case {case}: a second read must report the same entry, got {again}"
        );
    }

    // A put for a triple the template already holds REPLACES its value: the triple
    // identifies the entry, so the template never holds two increases for one quota in
    // one Region.
    let row = &entry_rows()[0];
    let replaced = emulator
        .ok(
            "PutServiceQuotaIncreaseRequestIntoTemplate",
            row.put_body(row.desired + 1.0),
        )
        .await["ServiceQuotaIncreaseRequestInTemplate"]["DesiredValue"]
        .as_f64();
    assert_eq!(
        replaced,
        Some(row.desired + 1.0),
        "case replacing {}: expected the new value, got {replaced:?}",
        row.case()
    );
    let listed = emulator
        .ok(
            "ListServiceQuotaIncreaseRequestsInTemplate",
            json!({"ServiceCode": row.service_code, "AwsRegion": row.aws_region}),
        )
        .await["ServiceQuotaIncreaseRequestInTemplateList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "case replacing {}: the template must hold ONE entry for the triple, got {listed:?}",
        row.case()
    );
    assert_eq!(
        listed[0]["DesiredValue"].as_f64(),
        Some(row.desired + 1.0),
        "case replacing {}: the listing must report the new value, got {listed:?}",
        row.case()
    );
}

/// The listing reports exactly the entries its filters select, in a stable order, over
/// pages the caller's `MaxResults` sizes; a token from another selection is refused.
#[tokio::test]
async fn the_listing_filters_and_pages_the_stored_entries() {
    let emulator = Emulator::in_organization("ALL").await;
    let rows = entry_rows();
    for row in &rows {
        emulator
            .ok(
                "PutServiceQuotaIncreaseRequestIntoTemplate",
                row.put_body(row.desired),
            )
            .await;
    }

    // Each filter row: the body, and the entries it must select.
    let filters: Vec<(&str, Value, Vec<&EntryRow>)> = vec![
        ("no filter: the whole template", json!({}), rows.iter().collect()),
        (
            "by ServiceCode, which two entries share",
            json!({"ServiceCode": "vpc"}),
            rows.iter().filter(|row| row.service_code == "vpc").collect(),
        ),
        (
            "by AwsRegion, which two entries share",
            json!({"AwsRegion": "us-east-1"}),
            rows.iter()
                .filter(|row| row.aws_region == "us-east-1")
                .collect(),
        ),
        (
            "by both filters together",
            json!({"ServiceCode": "vpc", "AwsRegion": "us-east-1"}),
            rows.iter()
                .filter(|row| row.service_code == "vpc" && row.aws_region == "us-east-1")
                .collect(),
        ),
        (
            "by a ServiceCode no entry names",
            json!({"ServiceCode": "lambda"}),
            Vec::new(),
        ),
    ];

    for (why, body, expected_rows) in filters {
        let mut expected: Vec<(String, String, String, String)> = expected_rows
            .iter()
            .map(|row| {
                (
                    row.service_code.to_string(),
                    row.quota_code.to_string(),
                    row.aws_region.to_string(),
                    json!(row.desired).to_string(),
                )
            })
            .collect();
        expected.sort();

        // The whole selection in one page (the listing serves it all when no page size
        // is asked for), and the same selection walked a page at a time.
        let whole = emulator
            .ok("ListServiceQuotaIncreaseRequestsInTemplate", body.clone())
            .await;
        assert!(
            whole["NextToken"].is_null(),
            "case {why}: an unasked page size serves the whole selection, got {whole}"
        );
        let in_one_page: Vec<(String, String, String, String)> = whole
            ["ServiceQuotaIncreaseRequestInTemplateList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(entry_identity)
            .collect();
        let mut sorted = in_one_page.clone();
        sorted.sort();
        assert_eq!(
            sorted, expected,
            "case {why}: expected exactly the selected entries, got {in_one_page:?}"
        );

        let paged = walk_pages(&emulator, body.clone(), 1, &format!("{why}, a page at a time")).await;
        assert_eq!(
            paged, in_one_page,
            "case {why}: paging a page at a time must yield the same entries in the same \
             order, got {paged:?}"
        );
    }

    // A token names a position in ONE selection, so another selection refuses it rather
    // than resuming at an unrelated position.
    let first = emulator
        .ok(
            "ListServiceQuotaIncreaseRequestsInTemplate",
            json!({"MaxResults": 1}),
        )
        .await;
    let token = first["NextToken"]
        .as_str()
        .expect("the first page of five entries carries a NextToken")
        .to_string();
    let (status, code) = emulator
        .refused(
            HOME_REGION,
            "ListServiceQuotaIncreaseRequestsInTemplate",
            json!({"ServiceCode": "vpc", "MaxResults": 1, "NextToken": token}),
        )
        .await;
    assert_eq!(
        (status, code.contains("IllegalArgumentException")),
        (400, true),
        "case another selection's token: got {status} {code}"
    );
}

/// Walk the listing `body` selects, `page_size` entries at a time, and answer every
/// entry in the order the pages reported them.
async fn walk_pages(
    emulator: &Emulator,
    body: Value,
    page_size: usize,
    case: &str,
) -> Vec<(String, String, String, String)> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let mut page_body = body.clone();
        page_body["MaxResults"] = json!(page_size);
        if let Some(next) = &token {
            page_body["NextToken"] = json!(next);
        }
        let page = emulator
            .ok("ListServiceQuotaIncreaseRequestsInTemplate", page_body)
            .await;
        let items = page["ServiceQuotaIncreaseRequestInTemplateList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            items.len() <= page_size,
            "case {case}: a page must hold at most {page_size} entries, got {items:?}"
        );
        entries.extend(items.iter().map(entry_identity));
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return entries;
        }
    }
    panic!("case {case}: the walk must terminate");
}

/// A delete removes exactly the entry it names: the later read 404s, the listing stops
/// reporting it, and every other entry is left where it was.
#[tokio::test]
async fn a_delete_removes_exactly_the_entry_it_names() {
    let emulator = Emulator::in_organization("ALL").await;
    let rows = entry_rows();
    for row in &rows {
        emulator
            .ok(
                "PutServiceQuotaIncreaseRequestIntoTemplate",
                row.put_body(row.desired),
            )
            .await;
    }

    let mut remaining = rows.len();
    for row in &rows {
        let case = row.case();
        emulator
            .ok("DeleteServiceQuotaIncreaseRequestFromTemplate", row.triple())
            .await;
        remaining -= 1;

        let (status, code) = emulator
            .refused(
                HOME_REGION,
                "GetServiceQuotaIncreaseRequestFromTemplate",
                row.triple(),
            )
            .await;
        assert_eq!(
            (status, code.contains("NoSuchResourceException")),
            (400, true),
            "case reading {case} after its delete: got {status} {code}"
        );
        // Deleting it again has nothing to delete, and says so the same way.
        let (status, code) = emulator
            .refused(
                HOME_REGION,
                "DeleteServiceQuotaIncreaseRequestFromTemplate",
                row.triple(),
            )
            .await;
        assert_eq!(
            (status, code.contains("NoSuchResourceException")),
            (400, true),
            "case deleting {case} twice: got {status} {code}"
        );

        let listed = emulator
            .ok("ListServiceQuotaIncreaseRequestsInTemplate", json!({}))
            .await["ServiceQuotaIncreaseRequestInTemplateList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            remaining,
            "case the template after deleting {case}: expected {remaining} entries, got \
             {listed:?}"
        );
        assert!(
            !listed
                .iter()
                .any(|entry| entry["ServiceCode"] == json!(row.service_code)
                    && entry["QuotaCode"] == json!(row.quota_code)
                    && entry["AwsRegion"] == json!(row.aws_region)),
            "case the template after deleting {case}: the entry must be gone, got {listed:?}"
        );
    }
}

/// The association status is the association: never associated has none to report, an
/// association reports `ASSOCIATED`, ending it reports `DISASSOCIATED`, and neither
/// touches the template's entries.
#[tokio::test]
async fn the_association_status_follows_the_association() {
    let emulator = Emulator::in_organization("ALL").await;
    let row = &entry_rows()[0];
    emulator
        .ok(
            "PutServiceQuotaIncreaseRequestIntoTemplate",
            row.put_body(row.desired),
        )
        .await;

    // A template that was never associated is not in use, so there is no status for it.
    let (status, code) = emulator
        .refused(
            HOME_REGION,
            "GetAssociationForServiceQuotaTemplate",
            json!({}),
        )
        .await;
    assert_eq!(
        (status, code.contains("ServiceQuotaTemplateNotInUseException")),
        (400, true),
        "case the association of a template that was never associated: got {status} {code}"
    );
    // Nor is there an association to end.
    let (status, code) = emulator
        .refused(HOME_REGION, "DisassociateServiceQuotaTemplate", json!({}))
        .await;
    assert_eq!(
        (status, code.contains("ServiceQuotaTemplateNotInUseException")),
        (400, true),
        "case disassociating a template that was never associated: got {status} {code}"
    );

    // Associating it is reported as `ASSOCIATED`, and associating an already associated
    // template leaves it associated.
    for attempt in ["the first association", "the same association again"] {
        emulator.ok("AssociateServiceQuotaTemplate", json!({})).await;
        let reported = emulator
            .ok("GetAssociationForServiceQuotaTemplate", json!({}))
            .await["ServiceQuotaTemplateAssociationStatus"]
            .as_str()
            .map(str::to_string);
        assert_eq!(
            reported.as_deref(),
            Some("ASSOCIATED"),
            "case {attempt}: expected ASSOCIATED, got {reported:?}"
        );
    }

    // Ending it is reported as `DISASSOCIATED`, and there is then no association to end
    // a second time.
    emulator
        .ok("DisassociateServiceQuotaTemplate", json!({}))
        .await;
    let reported = emulator
        .ok("GetAssociationForServiceQuotaTemplate", json!({}))
        .await["ServiceQuotaTemplateAssociationStatus"]
        .as_str()
        .map(str::to_string);
    assert_eq!(
        reported.as_deref(),
        Some("DISASSOCIATED"),
        "case the association after it was ended: expected DISASSOCIATED, got {reported:?}"
    );
    let (status, code) = emulator
        .refused(HOME_REGION, "DisassociateServiceQuotaTemplate", json!({}))
        .await;
    assert_eq!(
        (status, code.contains("ServiceQuotaTemplateNotInUseException")),
        (400, true),
        "case disassociating a template that is already disassociated: got {status} {code}"
    );

    // Through all of it the template's entries are untouched: disabling a template does
    // not empty it.
    let read = emulator
        .ok("GetServiceQuotaIncreaseRequestFromTemplate", row.triple())
        .await["ServiceQuotaIncreaseRequestInTemplate"]["DesiredValue"]
        .as_f64();
    assert_eq!(
        read,
        Some(row.desired),
        "case the entries after the association changed: expected {} still stored, got \
         {read:?}",
        row.desired
    );
}

/// A put outside the rule is refused, and nothing is stored for it.
#[tokio::test]
async fn a_put_outside_the_rule_is_refused_and_stores_nothing() {
    let emulator = Emulator::in_organization("ALL").await;

    // The rows are the ways a put can name something the rule does not admit: a service
    // and a quota the catalog does not hold, a Region that is not one, a quota AWS
    // publishes as not adjustable (`vpc` `L-2C462E13`, "Expiry time for an unaccepted VPC
    // peering connection request"), and a value that is not an increase on the quota's
    // AWS default (`vpc` `L-F678F1CE` defaults to 5), at the default itself and below it.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a ServiceCode the catalog does not hold",
            json!({
                "ServiceCode": "nosuchservice",
                "QuotaCode": "L-F678F1CE",
                "AwsRegion": "us-east-1",
                "DesiredValue": 100.0,
            }),
            "NoSuchResourceException",
        ),
        (
            "a QuotaCode the service does not have",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-00000000",
                "AwsRegion": "us-east-1",
                "DesiredValue": 100.0,
            }),
            "NoSuchResourceException",
        ),
        (
            "an AwsRegion that is not a Region",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "AwsRegion": "zz-nowhere-9",
                "DesiredValue": 100.0,
            }),
            "IllegalArgumentException",
        ),
        (
            "a quota AWS publishes as not adjustable",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-2C462E13",
                "AwsRegion": "us-east-1",
                "DesiredValue": 336.0,
            }),
            "IllegalArgumentException",
        ),
        (
            "a DesiredValue equal to the quota's default",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "AwsRegion": "us-east-1",
                "DesiredValue": 5.0,
            }),
            "IllegalArgumentException",
        ),
        (
            "a DesiredValue below the quota's default",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "AwsRegion": "us-east-1",
                "DesiredValue": 1.0,
            }),
            "IllegalArgumentException",
        ),
    ];

    for (why, body, expected) in rows {
        let (status, code) = emulator
            .refused(
                HOME_REGION,
                "PutServiceQuotaIncreaseRequestIntoTemplate",
                body,
            )
            .await;
        assert_eq!(
            status, 400,
            "case {why}: expected HTTP 400, got {status} with {code}"
        );
        assert!(
            code.contains(expected),
            "case {why}: expected {expected}, got {code}"
        );
    }

    // Nothing any of them named was stored.
    let listed = emulator
        .ok("ListServiceQuotaIncreaseRequestsInTemplate", json!({}))
        .await["ServiceQuotaIncreaseRequestInTemplateList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        listed.is_empty(),
        "case the template after only refused puts: expected it empty, got {listed:?}"
    );
}

/// The template plane is the account's own state: a write to it changes no quota value,
/// and the rest of Service Quotas keeps working around it.
#[tokio::test]
async fn a_template_entry_changes_no_quota_value() {
    let emulator = Emulator::in_organization("ALL").await;
    let pair = json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"});
    let before = emulator.ok("GetServiceQuota", pair.clone()).await;

    emulator
        .ok(
            "PutServiceQuotaIncreaseRequestIntoTemplate",
            json!({
                "ServiceCode": "vpc",
                "QuotaCode": "L-F678F1CE",
                "AwsRegion": "us-east-1",
                "DesiredValue": 1_000.0,
            }),
        )
        .await;
    emulator.ok("AssociateServiceQuotaTemplate", json!({})).await;

    // An entry is a request applied to accounts created in the organization later, so it
    // puts nothing in force for THIS account.
    let after = emulator.ok("GetServiceQuota", pair.clone()).await;
    assert_eq!(
        after, before,
        "case the quota a template entry names: expected it unchanged, got {after}"
    );
    let default = emulator.ok("GetAWSDefaultServiceQuota", pair).await;
    assert!(
        default["Quota"]["Value"].as_f64().is_some(),
        "case the default read after a template write: got {default}"
    );
}
