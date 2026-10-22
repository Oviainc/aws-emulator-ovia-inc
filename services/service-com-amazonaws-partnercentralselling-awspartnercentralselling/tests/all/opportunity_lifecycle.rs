// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Opportunity lifecycle: `CreateOpportunity`,
//! `GetOpportunity`, `UpdateOpportunity`, `AssignOpportunity`, `SubmitOpportunity` and
//! `ListOpportunities`.
//!
//! Every call goes through the same [`MultiServiceRouter`] the production front door
//! mounts, as an `awsJson1_0` `POST /` with an `X-Amz-Target` header, so what is
//! asserted is the wire behaviour and not a handler's return value.
//!
//! Each test is a TABLE over the class its rule covers — the rows differ in catalog,
//! in which optional members a request carries, in list sizes, in page size, in sort
//! key and in filter shape — because the rules under test ("whatever was created comes
//! back", "a page bound is honoured and resumable", "a stale version token is a
//! conflict") hold over that whole class and not just over one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralselling_awspartnercentralselling::state::{
    service_state, MAX_CREATE_REPLAYS, MAX_OPPORTUNITIES,
};
use service_com_amazonaws_partnercentralselling_awspartnercentralselling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every test signs for; it is also the region the ARNs must carry.
const REGION: &str = "us-east-1";

/// A body long enough for `Project.CustomerBusinessProblem` (`^(?s).{20,2000}$`).
const BUSINESS_PROBLEM: &str = "The customer needs to migrate a monolith off premises.";

/// The emulator under test: one shared engine, this service mounted on the router.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the engine handle, for a test that has to look at the
/// store itself rather than only at what the wire reports.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise operation behaviour, not SigV4: the signature a real client
    // sends is verified by the emulator, never by a service.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One answered call: the HTTP status and the parsed JSON body.
struct Call {
    status: u16,
    body: Value,
}

impl Call {
    /// The body of a call that must have succeeded.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case}: expected HTTP 200, got {} with body {}",
            self.status, self.body
        );
        &self.body
    }

    /// The wire error code (`__type`), with any namespace prefix stripped.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// Assert this call failed with `status` and `code`.
    fn expect_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code().as_str()),
            (status, code),
            "case {case}: expected HTTP {status} {code}, got HTTP {} {} with body {}",
            self.status,
            self.error_code(),
            self.body
        );
    }

    /// The record version a response reports: the `LastModifiedDate` member, as the
    /// JSON value a client echoes back into `UpdateOpportunity`. It is a modelled
    /// `DateTime`, so the test never assumes which wire form it takes.
    fn version(&self, case: &str) -> Value {
        let value = self
            .body
            .get("LastModifiedDate")
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            !value.is_null(),
            "case {case}: expected a LastModifiedDate in the response, got {}",
            self.body
        );
        value
    }

    /// A string member of the body.
    fn field(&self, case: &str, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a string `{member}` in the response, got {}",
                    self.body
                )
            })
            .to_string()
    }
}

/// Send one operation call.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Call {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/partnercentral-selling/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("https://partnercentral-selling.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSPartnerCentralSelling.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the test body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Call { status, body }
}

/// Wait out the millisecond the timestamps are stamped at.
///
/// `CreatedDate` and `LastModifiedDate` come from the real clock, so two calls inside
/// one millisecond carry the same instant and a test asserting a time ORDERING would
/// be deciding on the tie-break instead. Only the test that asserts a time order
/// calls this; nothing in the service sleeps.
async fn separate_instants() {
    tokio::time::sleep(std::time::Duration::from_millis(3)).await;
}

/// A `CreateOpportunity` request: `members` plus the two required members.
fn create_body(catalog: &str, client_token: &str, members: &Value) -> Value {
    let mut body = members.clone();
    body["Catalog"] = json!(catalog);
    body["ClientToken"] = json!(client_token);
    body
}

/// Create one opportunity and return its id.
async fn create_opportunity(
    router: &MultiServiceRouter,
    case: &str,
    catalog: &str,
    client_token: &str,
    members: &Value,
) -> String {
    let created = call(
        router,
        "CreateOpportunity",
        &create_body(catalog, client_token, members),
    )
    .await;
    created.ok(case);
    created.field(case, "Id")
}

/// Read one opportunity back.
async fn get_opportunity(router: &MultiServiceRouter, catalog: &str, id: &str) -> Call {
    call(
        router,
        "GetOpportunity",
        &json!({ "Catalog": catalog, "Identifier": id }),
    )
    .await
}

/// The modelled `OpportunityIdentifier` pattern, `^O[0-9]{1,19}$`.
fn is_opportunity_id(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('O') else {
        return false;
    };
    (1..=19).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// Assert every member `sent` carries is present in `got` with the same value,
/// recursing into nested objects so a response that adds service-owned members (a
/// `ReviewStatus`, a history) still satisfies "whatever was created comes back".
fn assert_echoed(case: &str, path: &str, sent: &Value, got: &Value) {
    match sent {
        Value::Object(members) => {
            let Some(object) = got.as_object() else {
                panic!("case {case}: expected an object at {path}, got {got}");
            };
            for (member, value) in members {
                let child = object.get(member).unwrap_or(&Value::Null);
                assert_echoed(case, &format!("{path}.{member}"), value, child);
            }
        }
        _ => assert_eq!(
            sent, got,
            "case {case}: at {path} expected {sent}, got {got}"
        ),
    }
}

/// The members a create request carries that no `GetOpportunity` response declares
/// (`Origin` and `Tags` are not on `GetOpportunityResponse`; `ClientToken` is the
/// idempotency token), so they are excluded from the echo check.
const NOT_ON_THE_READ: [&str; 3] = ["ClientToken", "Origin", "Tags"];

/// The payload rows every create/read test spans.
fn create_payloads() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "only the required members, AWS catalog",
            "AWS",
            json!({}),
        ),
        (
            "customer, project and lifecycle, Sandbox catalog",
            "Sandbox",
            json!({
                "Customer": {
                    "Account": {
                        "CompanyName": "Acme",
                        "WebsiteUrl": "https://acme.test",
                        "Industry": "Software and Internet",
                        "Address": { "City": "Seattle", "CountryCode": "US", "PostalCode": "98101" }
                    }
                },
                "Project": { "Title": "Migration", "CustomerBusinessProblem": BUSINESS_PROBLEM },
                "LifeCycle": { "Stage": "Prospect", "TargetCloseDate": "2027-01-31" }
            }),
        ),
        (
            "every top-level member, lists with several entries",
            "AWS",
            json!({
                "Customer": {
                    "Account": { "CompanyName": "Globex", "WebsiteUrl": "https://globex.test" },
                    "Contacts": [
                        { "Email": "buyer@globex.test", "FirstName": "Bea", "LastName": "Buyer" }
                    ]
                },
                "Project": {
                    "Title": "Data lake",
                    "CustomerBusinessProblem": BUSINESS_PROBLEM,
                    "DeliveryModels": ["SaaS or PaaS", "Managed Services"]
                },
                "LifeCycle": {
                    "Stage": "Qualified",
                    "TargetCloseDate": "2027-06-30",
                    "NextSteps": "Schedule the architecture review"
                },
                "OpportunityType": "Net New Business",
                "Origin": "Partner Referral",
                "NationalSecurity": "No",
                "PartnerOpportunityIdentifier": "crm-4711",
                "PrimaryNeedsFromAws": [
                    "Co-Sell - Architectural Validation",
                    "Co-Sell - Pricing Assistance"
                ],
                "OpportunityTeam": [
                    {
                        "Email": "pam@partner.test",
                        "FirstName": "Pam",
                        "LastName": "Manager",
                        "BusinessTitle": "PartnerAccountManager"
                    },
                    {
                        "Email": "owen@partner.test",
                        "FirstName": "Owen",
                        "LastName": "Owner",
                        "BusinessTitle": "OpportunityOwner"
                    }
                ],
                "Marketing": { "CampaignName": "summer-cosell" },
                "SoftwareRevenue": {
                    "Value": { "Amount": "1200.50", "CurrencyCode": "USD" },
                    "EffectiveDate": "2027-01-01",
                    "ExpirationDate": "2027-12-31"
                },
                "Tags": [{ "Key": "team", "Value": "cosell" }]
            }),
        ),
    ]
}

#[tokio::test]
async fn create_then_get_returns_every_created_member() {
    let router = emulator();

    for (index, (case, catalog, members)) in create_payloads().into_iter().enumerate() {
        let token = format!("token-{index}");
        let created = call(
            &router,
            "CreateOpportunity",
            &create_body(catalog, &token, &members),
        )
        .await;
        created.ok(case);

        let id = created.field(case, "Id");
        assert!(
            is_opportunity_id(&id),
            "case {case}: Id must match ^O[0-9]{{1,19}}$, got {id:?}"
        );
        assert_eq!(
            created.body.get("PartnerOpportunityIdentifier"),
            members.get("PartnerOpportunityIdentifier"),
            "case {case}: the create response must echo the stored PartnerOpportunityIdentifier, got {}",
            created.body
        );

        let read = get_opportunity(&router, catalog, &id).await;
        let body = read.ok(case).clone();

        // Everything the request carried and the read declares comes back.
        let mut expected = members.clone();
        if let Some(object) = expected.as_object_mut() {
            for member in NOT_ON_THE_READ {
                object.remove(member);
            }
        }
        assert_echoed(case, "GetOpportunity", &expected, &body);

        assert_eq!(
            body.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: the read must report the created Id, got {body}"
        );
        assert_eq!(
            body.get("Catalog").and_then(Value::as_str),
            Some(catalog),
            "case {case}: the read must report the requested catalog, got {body}"
        );
        let arn = read.field(case, "Arn");
        assert_eq!(
            arn,
            format!("arn:aws:partnercentral:{REGION}:000000000000:catalog/{catalog}/opportunity/{id}"),
            "case {case}: unexpected Arn, got {arn:?}"
        );
        assert_eq!(
            body.get("CreatedDate"),
            body.get("LastModifiedDate"),
            "case {case}: a created opportunity's LastModifiedDate is its CreatedDate, got {body}"
        );
        assert_eq!(
            body.pointer("/LifeCycle/ReviewStatus").and_then(Value::as_str),
            Some("Pending Submission"),
            "case {case}: creating an opportunity sets LifeCycle.ReviewStatus to Pending Submission, got {body}"
        );
        // Required on the response, and empty while nothing is associated.
        let related = body
            .get("RelatedEntityIdentifiers")
            .unwrap_or_else(|| panic!("case {case}: RelatedEntityIdentifiers is required, got {body}"));
        for member in [
            "Solutions",
            "AwsProducts",
            "AwsMarketplaceOffers",
            "AwsMarketplaceOfferSets",
            "AwsMarketplaceSolutions",
            "AwsMarketplaceProducts",
        ] {
            assert_eq!(
                related.get(member).and_then(Value::as_array).map(Vec::len),
                Some(0),
                "case {case}: RelatedEntityIdentifiers.{member} must be an empty list, got {related}"
            );
        }
    }
}

#[tokio::test]
async fn next_steps_are_recorded_in_the_lifecycle_history() {
    let router = emulator();
    // The rows differ in whether the created opportunity already had NextSteps, which
    // is what decides how many entries the history holds after one update.
    let cases = [
        ("created without NextSteps", Value::Null, 1_usize),
        ("created with NextSteps", json!("Confirm the budget"), 2),
    ];

    for (index, (case, created_next_steps, expected_entries)) in cases.into_iter().enumerate() {
        let mut life_cycle = json!({ "Stage": "Prospect" });
        if let Some(value) = created_next_steps.as_str() {
            life_cycle["NextSteps"] = json!(value);
        }
        let id = create_opportunity(
            &router,
            case,
            "AWS",
            &format!("history-{index}"),
            &json!({ "LifeCycle": life_cycle }),
        )
        .await;

        let read = get_opportunity(&router, "AWS", &id).await;
        let version = read.version(case);
        let updated = call(
            &router,
            "UpdateOpportunity",
            &json!({
                "Catalog": "AWS",
                "Identifier": id,
                "LastModifiedDate": version,
                "LifeCycle": { "Stage": "Qualified", "NextSteps": "Book the technical validation" }
            }),
        )
        .await;
        updated.ok(case);

        let read = get_opportunity(&router, "AWS", &id).await;
        let body = read.ok(case).clone();
        let history = body
            .pointer("/LifeCycle/NextStepsHistory")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            history.len(),
            expected_entries,
            "case {case}: expected {expected_entries} NextStepsHistory entries, got {body}"
        );
        let last = history.last().cloned().unwrap_or(Value::Null);
        assert_eq!(
            last.get("Value").and_then(Value::as_str),
            Some("Book the technical validation"),
            "case {case}: the newest history entry must carry the new NextSteps, got {last}"
        );
        assert!(
            last.get("Time").is_some(),
            "case {case}: a history entry carries the time it was recorded, got {last}"
        );
    }
}

#[tokio::test]
async fn create_is_idempotent_on_its_client_token() {
    let router = emulator();
    let mut minted = Vec::new();

    for (index, (case, catalog, members)) in create_payloads().into_iter().enumerate() {
        let token = format!("replay-{index}");
        let body = create_body(catalog, &token, &members);

        let first = call(&router, "CreateOpportunity", &body).await;
        first.ok(case);
        let id = first.field(case, "Id");
        minted.push((catalog, id.clone()));

        // Same token, same body: the same record, not a second one.
        let replay = call(&router, "CreateOpportunity", &body).await;
        replay.ok(case);
        assert_eq!(
            replay.field(case, "Id"),
            id,
            "case {case}: replaying a ClientToken with the same body must return the same Id"
        );

        // Same token, different body: the documented conflict.
        let mut changed = body.clone();
        changed["PartnerOpportunityIdentifier"] = json!(format!("crm-changed-{index}"));
        call(&router, "CreateOpportunity", &changed)
            .await
            .expect_error(case, 409, "ConflictException");

        // A fresh token with the same body is a new opportunity.
        let again = call(
            &router,
            "CreateOpportunity",
            &create_body(catalog, &format!("fresh-{index}"), &members),
        )
        .await;
        again.ok(case);
        let second_id = again.field(case, "Id");
        assert_ne!(
            second_id, id,
            "case {case}: a different ClientToken must mint a new Id"
        );
        minted.push((catalog, second_id));
    }

    // The store holds exactly the records the tokens minted — a replay added none.
    for catalog in ["AWS", "Sandbox"] {
        let expected: Vec<String> = minted
            .iter()
            .filter(|(record_catalog, _)| *record_catalog == catalog)
            .map(|(_, id)| id.clone())
            .collect();
        let listed = list_ids(&router, catalog, 100).await;
        let mut sorted = listed.clone();
        sorted.sort();
        let mut want = expected.clone();
        want.sort();
        assert_eq!(
            sorted, want,
            "case {catalog}: the catalog must hold exactly the minted opportunities, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_not_found() {
    let router = emulator();
    let sandbox_id = create_opportunity(&router, "setup", "Sandbox", "nf-1", &json!({})).await;
    let version = get_opportunity(&router, "Sandbox", &sandbox_id)
        .await
        .version("setup");

    // The rows span the operations that resolve an opportunity, and both ways a
    // lookup can miss: an id nothing ever minted, and an id that exists in ANOTHER
    // catalog.
    let assignee = json!({
        "Email": "rep@partner.test",
        "FirstName": "Rep",
        "LastName": "One",
        "BusinessTitle": "OpportunityOwner"
    });
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "GetOpportunity, id never minted",
            "GetOpportunity",
            json!({ "Catalog": "Sandbox", "Identifier": "O999999999" }),
        ),
        (
            "GetOpportunity, id of another catalog",
            "GetOpportunity",
            json!({ "Catalog": "AWS", "Identifier": sandbox_id }),
        ),
        (
            "UpdateOpportunity, id never minted",
            "UpdateOpportunity",
            json!({ "Catalog": "Sandbox", "Identifier": "O999999999", "LastModifiedDate": version }),
        ),
        (
            "UpdateOpportunity, id of another catalog",
            "UpdateOpportunity",
            json!({ "Catalog": "AWS", "Identifier": sandbox_id, "LastModifiedDate": version }),
        ),
        (
            "AssignOpportunity, id never minted",
            "AssignOpportunity",
            json!({ "Catalog": "Sandbox", "Identifier": "O999999999", "Assignee": assignee }),
        ),
        (
            "SubmitOpportunity, id of another catalog",
            "SubmitOpportunity",
            json!({ "Catalog": "AWS", "Identifier": sandbox_id, "InvolvementType": "Co-Sell" }),
        ),
    ];

    for (case, operation, body) in cases {
        call(&router, operation, &body)
            .await
            .expect_error(case, 404, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn update_requires_the_current_last_modified_date() {
    let router = emulator();

    for (index, (case, catalog, members)) in create_payloads().into_iter().enumerate() {
        let id = create_opportunity(&router, case, catalog, &format!("cc-{index}"), &members).await;
        let read = get_opportunity(&router, catalog, &id).await;
        let first_version = read.version(case);

        let update = json!({
            "Catalog": catalog,
            "Identifier": id,
            "LastModifiedDate": first_version,
            "Project": { "Title": "Migration v2", "CustomerBusinessProblem": BUSINESS_PROBLEM }
        });
        let updated = call(&router, "UpdateOpportunity", &update).await;
        updated.ok(case);
        assert_eq!(
            updated.field(case, "Id"),
            id,
            "case {case}: the update response must report the updated Id"
        );
        let second_version = updated.version(case);
        assert_ne!(
            second_version, first_version,
            "case {case}: an update must advance LastModifiedDate"
        );

        // The stale token is refused, the new one is accepted: that is what "advanced"
        // means for a concurrency token.
        call(&router, "UpdateOpportunity", &update)
            .await
            .expect_error(case, 409, "ConflictException");
        let mut retry = update.clone();
        retry["LastModifiedDate"] = json!(second_version);
        call(&router, "UpdateOpportunity", &retry).await.ok(case);

        // The whole payload replaces the stored members: the update above carried no
        // Customer, so the read reports none, while the service-owned ReviewStatus
        // survives.
        let read = get_opportunity(&router, catalog, &id).await;
        let body = read.ok(case).clone();
        assert_eq!(
            body.pointer("/Project/Title").and_then(Value::as_str),
            Some("Migration v2"),
            "case {case}: the update must be visible on the read, got {body}"
        );
        assert!(
            body.get("Customer").is_none(),
            "case {case}: a member omitted from an update is set to null, got {body}"
        );
        assert_eq!(
            body.pointer("/LifeCycle/ReviewStatus").and_then(Value::as_str),
            Some("Pending Submission"),
            "case {case}: ReviewStatus is service-owned and survives an update, got {body}"
        );
    }
}

#[tokio::test]
async fn assigning_an_opportunity_records_the_owner_on_the_team() {
    let router = emulator();
    let created_team = json!([
        {
            "Email": "pam@partner.test",
            "FirstName": "Pam",
            "LastName": "Manager",
            "BusinessTitle": "PartnerAccountManager"
        }
    ]);

    // The rows differ in the assignee's members (with and without a phone) and run in
    // sequence, so the second row is a REASSIGNMENT of the same opportunity.
    let assignees = [
        json!({
            "Email": "first@partner.test",
            "FirstName": "First",
            "LastName": "Owner",
            "BusinessTitle": "OpportunityOwner"
        }),
        json!({
            "Email": "second@partner.test",
            "FirstName": "Second",
            "LastName": "Owner",
            "BusinessTitle": "OpportunityOwner",
            "Phone": "+14255550123"
        }),
    ];

    let id = create_opportunity(
        &router,
        "assign setup",
        "AWS",
        "assign-1",
        &json!({ "OpportunityTeam": created_team }),
    )
    .await;

    for (index, assignee) in assignees.iter().enumerate() {
        let case = format!("assignee {index}");
        let before = get_opportunity(&router, "AWS", &id).await.version(&case);

        call(
            &router,
            "AssignOpportunity",
            &json!({ "Catalog": "AWS", "Identifier": id, "Assignee": assignee }),
        )
        .await
        .ok(&case);

        let read = get_opportunity(&router, "AWS", &id).await;
        let body = read.ok(&case).clone();
        let team = body
            .get("OpportunityTeam")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            team.first().cloned().unwrap_or(Value::Null),
            *assignee,
            "case {case}: the assignee must be the opportunity's owner contact, got {body}"
        );
        assert!(
            team.iter()
                .any(|member| member.get("Email").and_then(Value::as_str)
                    == Some("pam@partner.test")),
            "case {case}: assigning must not drop the created team members, got {body}"
        );
        assert_eq!(
            team.len(),
            2,
            "case {case}: reassignment replaces the previous owner rather than appending, got {body}"
        );
        assert_ne!(
            body.get("LastModifiedDate").cloned().unwrap_or(Value::Null),
            before,
            "case {case}: assigning must advance LastModifiedDate, got {body}"
        );
    }
}

#[tokio::test]
async fn submitting_an_opportunity_moves_its_review_status() {
    let router = emulator();
    // The rows span the `SalesInvolvementType` enum and both `Visibility` values,
    // plus an absent `Visibility`.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("co-sell, full visibility", "Co-Sell", Some("Full")),
        (
            "visibility only, limited visibility",
            "For Visibility Only",
            Some("Limited"),
        ),
        ("co-sell, no visibility member", "Co-Sell", None),
    ];

    for (index, (case, involvement, visibility)) in cases.into_iter().enumerate() {
        let id = create_opportunity(
            &router,
            case,
            "Sandbox",
            &format!("submit-{index}"),
            &json!({ "LifeCycle": { "Stage": "Qualified", "TargetCloseDate": "2027-03-31" } }),
        )
        .await;
        let read = get_opportunity(&router, "Sandbox", &id).await;
        let before = read.version(case);
        assert_eq!(
            read.body.pointer("/LifeCycle/ReviewStatus").and_then(Value::as_str),
            Some("Pending Submission"),
            "case {case}: an unsubmitted opportunity is Pending Submission, got {}",
            read.body
        );

        let mut submit = json!({
            "Catalog": "Sandbox",
            "Identifier": id,
            "InvolvementType": involvement
        });
        if let Some(visibility) = visibility {
            submit["Visibility"] = json!(visibility);
        }
        call(&router, "SubmitOpportunity", &submit).await.ok(case);

        let read = get_opportunity(&router, "Sandbox", &id).await;
        let body = read.ok(case).clone();
        assert_eq!(
            body.pointer("/LifeCycle/ReviewStatus").and_then(Value::as_str),
            Some("Submitted"),
            "case {case}: submitting must move LifeCycle.ReviewStatus to Submitted, got {body}"
        );
        assert_ne!(
            body.get("LastModifiedDate").cloned().unwrap_or(Value::Null),
            before,
            "case {case}: submitting must advance LastModifiedDate, got {body}"
        );

        // A submitted opportunity is under review: it can be neither resubmitted nor
        // edited until the review completes.
        call(&router, "SubmitOpportunity", &submit)
            .await
            .expect_error(case, 400, "ValidationException");
        let version = read.version(case);
        call(
            &router,
            "UpdateOpportunity",
            &json!({
                "Catalog": "Sandbox",
                "Identifier": id,
                "LastModifiedDate": version,
                "Project": { "Title": "edited after submission" }
            }),
        )
        .await
        .expect_error(case, 400, "ValidationException");
    }
}

/// The ids one `ListOpportunities` page of at most `max_results` reports.
async fn list_ids(router: &MultiServiceRouter, catalog: &str, max_results: i64) -> Vec<String> {
    let listed = call(
        router,
        "ListOpportunities",
        &json!({ "Catalog": catalog, "MaxResults": max_results }),
    )
    .await;
    listed.ok("list");
    summaries_ids(&listed.body)
}

/// The ids of a `ListOpportunities` response's summaries.
fn summaries_ids(body: &Value) -> Vec<String> {
    body.get("OpportunitySummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("Id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn listing_pages_through_every_opportunity_exactly_once() {
    let router = emulator();
    const RECORDS: usize = 5;
    for index in 0..RECORDS {
        create_opportunity(
            &router,
            "page setup",
            "AWS",
            &format!("page-{index}"),
            &json!({ "Project": { "Title": format!("project {index}") } }),
        )
        .await;
    }
    // One opportunity in another catalog: it must never appear in the AWS pages.
    let other = create_opportunity(&router, "page setup", "Sandbox", "page-other", &json!({})).await;

    let whole = list_ids(&router, "AWS", 100).await;
    assert_eq!(
        whole.len(),
        RECORDS,
        "an unbounded page must report every opportunity of the catalog, got {whole:?}"
    );
    assert!(
        !whole.contains(&other),
        "a list must not cross catalogs, got {whole:?}"
    );

    // The rows span page sizes from one record per page to more than the whole list.
    for page_size in [1_usize, 2, 3, 5, 7] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({ "Catalog": "AWS", "MaxResults": page_size });
            if let Some(token) = next_token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let listed = call(&router, "ListOpportunities", &request).await;
            let body = listed.ok(&case).clone();
            let ids = summaries_ids(&body);
            pages += 1;
            assert!(
                ids.len() <= page_size,
                "case {case}: a page must honour MaxResults, got {} ids",
                ids.len()
            );
            let remaining = RECORDS - seen.len();
            assert_eq!(
                ids.len(),
                remaining.min(page_size),
                "case {case}: expected a full page while records remain, got {ids:?}"
            );
            seen.extend(ids);
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                next_token.is_some(),
                seen.len() < RECORDS,
                "case {case}: NextToken is present exactly while records remain, got {body}"
            );
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= RECORDS + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen, whole,
            "case {case}: paging must report the same records, in the same order, as one page"
        );
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case}: paging repeated a record, got {seen:?}"
        );
    }

    // A token this service did not issue is refused rather than silently restarting.
    for (case, token) in [
        ("not base64", "!!!not-a-token!!!"),
        ("base64 of foreign content", "Zm9yZ2VkOjEyMw=="),
    ] {
        call(
            &router,
            "ListOpportunities",
            &json!({ "Catalog": "AWS", "NextToken": token }),
        )
        .await
        .expect_error(case, 400, "ValidationException");
    }
}

#[tokio::test]
async fn listing_applies_the_requested_filters() {
    let router = emulator();
    // Three opportunities that differ in stage and customer, so each filter has both
    // matching and non-matching rows to separate.
    let acme = create_opportunity(
        &router,
        "filter setup",
        "AWS",
        "filter-1",
        &json!({
            "Customer": { "Account": { "CompanyName": "Acme" } },
            "LifeCycle": { "Stage": "Prospect", "TargetCloseDate": "2027-01-31" }
        }),
    )
    .await;
    let globex = create_opportunity(
        &router,
        "filter setup",
        "AWS",
        "filter-2",
        &json!({
            "Customer": { "Account": { "CompanyName": "Globex" } },
            "LifeCycle": { "Stage": "Qualified", "TargetCloseDate": "2027-06-30" }
        }),
    )
    .await;
    let initech = create_opportunity(
        &router,
        "filter setup",
        "AWS",
        "filter-3",
        &json!({
            "Customer": { "Account": { "CompanyName": "Initech" } },
            "LifeCycle": { "Stage": "Qualified", "TargetCloseDate": "2028-02-29" }
        }),
    )
    .await;
    call(
        &router,
        "SubmitOpportunity",
        &json!({ "Catalog": "AWS", "Identifier": globex, "InvolvementType": "Co-Sell" }),
    )
    .await
    .ok("filter setup");

    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "no filter",
            json!({}),
            vec![&acme, &globex, &initech],
        ),
        (
            "Identifier, one value",
            json!({ "Identifier": [globex] }),
            vec![&globex],
        ),
        (
            "Identifier, two values",
            json!({ "Identifier": [acme, initech] }),
            vec![&acme, &initech],
        ),
        (
            "Identifier, an id in no catalog",
            json!({ "Identifier": ["O999999999"] }),
            vec![],
        ),
        (
            "Identifier, an empty list constrains nothing",
            json!({ "Identifier": [] }),
            vec![&acme, &globex, &initech],
        ),
        (
            "LifeCycleStage, one value",
            json!({ "LifeCycleStage": ["Qualified"] }),
            vec![&globex, &initech],
        ),
        (
            "LifeCycleStage, a stage nothing is in",
            json!({ "LifeCycleStage": ["Launched"] }),
            vec![],
        ),
        (
            "LifeCycleReviewStatus, the submitted one",
            json!({ "LifeCycleReviewStatus": ["Submitted"] }),
            vec![&globex],
        ),
        (
            "LifeCycleReviewStatus, the unsubmitted ones",
            json!({ "LifeCycleReviewStatus": ["Pending Submission"] }),
            vec![&acme, &initech],
        ),
        (
            "CustomerCompanyName",
            json!({ "CustomerCompanyName": ["Initech"] }),
            vec![&initech],
        ),
        (
            "TargetCloseDate, before a bound",
            json!({ "TargetCloseDate": { "BeforeTargetCloseDate": "2027-12-31" } }),
            vec![&acme, &globex],
        ),
        (
            "TargetCloseDate, after a bound",
            json!({ "TargetCloseDate": { "AfterTargetCloseDate": "2027-06-30" } }),
            vec![&globex, &initech],
        ),
        (
            "two filters together",
            json!({
                "LifeCycleStage": ["Qualified"],
                "CustomerCompanyName": ["Globex"]
            }),
            vec![&globex],
        ),
    ];

    for (case, filters, expected) in cases {
        let mut request = filters;
        request["Catalog"] = json!("AWS");
        request["MaxResults"] = json!(100);
        let listed = call(&router, "ListOpportunities", &request).await;
        listed.ok(case);
        let mut got = summaries_ids(&listed.body);
        got.sort();
        let mut want: Vec<String> = expected.into_iter().cloned().collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case}: unexpected filtered ids, body was {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn listing_sorts_by_the_requested_key() {
    let router = emulator();
    let first = create_opportunity(
        &router,
        "sort setup",
        "AWS",
        "sort-1",
        &json!({ "Customer": { "Account": { "CompanyName": "Charlie" } } }),
    )
    .await;
    separate_instants().await;
    let second = create_opportunity(
        &router,
        "sort setup",
        "AWS",
        "sort-2",
        &json!({ "Customer": { "Account": { "CompanyName": "Alpha" } } }),
    )
    .await;
    separate_instants().await;
    let third = create_opportunity(
        &router,
        "sort setup",
        "AWS",
        "sort-3",
        &json!({ "Customer": { "Account": { "CompanyName": "Bravo" } } }),
    )
    .await;
    separate_instants().await;

    // Touch the FIRST record last, so the default LastModifiedDate ordering differs
    // from the creation order. The update carries the whole payload, as the operation
    // documents, so the customer the sort key reads is unchanged.
    let version = get_opportunity(&router, "AWS", &first)
        .await
        .version("sort setup");
    call(
        &router,
        "UpdateOpportunity",
        &json!({
            "Catalog": "AWS",
            "Identifier": first,
            "LastModifiedDate": version,
            "Customer": { "Account": { "CompanyName": "Charlie" } },
            "Project": { "Title": "touched" }
        }),
    )
    .await
    .ok("sort setup");

    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "default sort is LastModifiedDate descending",
            json!({}),
            vec![&first, &third, &second],
        ),
        (
            "Identifier ascending",
            json!({ "Sort": { "SortBy": "Identifier", "SortOrder": "ASCENDING" } }),
            vec![&first, &second, &third],
        ),
        (
            "Identifier descending",
            json!({ "Sort": { "SortBy": "Identifier", "SortOrder": "DESCENDING" } }),
            vec![&third, &second, &first],
        ),
        (
            "CustomerCompanyName ascending",
            json!({ "Sort": { "SortBy": "CustomerCompanyName", "SortOrder": "ASCENDING" } }),
            vec![&second, &third, &first],
        ),
        (
            "CreatedDate ascending",
            json!({ "Sort": { "SortBy": "CreatedDate", "SortOrder": "ASCENDING" } }),
            vec![&first, &second, &third],
        ),
    ];

    for (case, sort, expected) in cases {
        let mut request = sort;
        request["Catalog"] = json!("AWS");
        request["MaxResults"] = json!(100);
        let listed = call(&router, "ListOpportunities", &request).await;
        listed.ok(case);
        let got = summaries_ids(&listed.body);
        let want: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            got, want,
            "case {case}: unexpected order, body was {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn list_summaries_report_the_stored_record() {
    let router = emulator();
    let members = json!({
        "Customer": {
            "Account": {
                "CompanyName": "Acme",
                "Industry": "Software and Internet",
                "WebsiteUrl": "https://acme.test",
                "Address": { "City": "Seattle", "CountryCode": "US", "PostalCode": "98101" }
            }
        },
        "Project": {
            "Title": "Migration",
            "DeliveryModels": ["SaaS or PaaS"],
            "CustomerBusinessProblem": BUSINESS_PROBLEM
        },
        "LifeCycle": { "Stage": "Qualified", "TargetCloseDate": "2027-06-30" },
        "OpportunityType": "Expansion",
        "PartnerOpportunityIdentifier": "crm-99"
    });
    let id = create_opportunity(&router, "summary", "AWS", "summary-1", &members).await;

    let listed = call(
        &router,
        "ListOpportunities",
        &json!({ "Catalog": "AWS", "MaxResults": 1 }),
    )
    .await;
    listed.ok("summary");
    let summary = listed
        .body
        .pointer("/OpportunitySummaries/0")
        .cloned()
        .unwrap_or(Value::Null);

    let read = get_opportunity(&router, "AWS", &id).await;
    let full = read.ok("summary").clone();

    // Every member the summary shapes declare carries the stored value.
    for member in [
        "Catalog",
        "Id",
        "Arn",
        "OpportunityType",
        "PartnerOpportunityIdentifier",
        "CreatedDate",
        "LastModifiedDate",
    ] {
        assert_eq!(
            summary.get(member),
            full.get(member),
            "case summary: {member} must match the stored record, got {summary}"
        );
    }
    for (path, expected) in [
        ("/LifeCycle/Stage", json!("Qualified")),
        ("/LifeCycle/TargetCloseDate", json!("2027-06-30")),
        ("/LifeCycle/ReviewStatus", json!("Pending Submission")),
        ("/Customer/Account/CompanyName", json!("Acme")),
        ("/Customer/Account/Industry", json!("Software and Internet")),
        ("/Customer/Account/Address/City", json!("Seattle")),
        ("/Project/Title", json!("Migration")),
    ] {
        // `ProjectSummary` declares no `Title`, so that one must be absent.
        let found = summary.pointer(path).cloned().unwrap_or(Value::Null);
        if path == "/Project/Title" {
            assert_eq!(
                found,
                Value::Null,
                "case summary: ProjectSummary declares no Title, got {summary}"
            );
        } else {
            assert_eq!(
                found, expected,
                "case summary: {path} must match the stored record, got {summary}"
            );
        }
    }
    assert_eq!(
        summary.pointer("/Project/DeliveryModels"),
        Some(&json!(["SaaS or PaaS"])),
        "case summary: ProjectSummary.DeliveryModels must match the stored record, got {summary}"
    );
}

#[tokio::test]
async fn max_results_outside_its_modelled_range_is_refused() {
    let router = emulator();
    create_opportunity(&router, "range setup", "AWS", "range-1", &json!({})).await;

    // The modelled range is 1..100: each published bound is accepted and one past it
    // on either side is refused.
    for (case, max_results, expected_status) in [
        ("below the minimum", 0, 400),
        ("at the minimum", 1, 200),
        ("at the maximum", 100, 200),
        ("above the maximum", 101, 400),
    ] {
        let listed = call(
            &router,
            "ListOpportunities",
            &json!({ "Catalog": "AWS", "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            listed.status, expected_status,
            "case {case}: MaxResults {max_results} expected HTTP {expected_status}, got HTTP {} with body {}",
            listed.status, listed.body
        );
        if expected_status == 400 {
            assert_eq!(
                listed.error_code(),
                "ValidationException",
                "case {case}: expected a ValidationException, got {}",
                listed.body
            );
        }
    }
}

#[tokio::test]
async fn a_catalog_outside_the_allowlist_is_refused() {
    let router = emulator();
    let id = create_opportunity(&router, "catalog setup", "AWS", "catalog-1", &json!({})).await;
    let version = get_opportunity(&router, "AWS", &id)
        .await
        .version("catalog setup");

    // `^[a-zA-Z]+$` is all the model pins, so every row below reaches the service's
    // own allowlist check: the catalog is "AWS" or "Sandbox", case included.
    for catalog in ["Unknown", "aws", "sandbox", "AWSSandbox"] {
        let cases: Vec<(String, &str, Value)> = vec![
            (
                format!("CreateOpportunity, catalog {catalog}"),
                "CreateOpportunity",
                json!({ "Catalog": catalog, "ClientToken": "catalog-bad" }),
            ),
            (
                format!("GetOpportunity, catalog {catalog}"),
                "GetOpportunity",
                json!({ "Catalog": catalog, "Identifier": id }),
            ),
            (
                format!("UpdateOpportunity, catalog {catalog}"),
                "UpdateOpportunity",
                json!({ "Catalog": catalog, "Identifier": id, "LastModifiedDate": version }),
            ),
            (
                format!("AssignOpportunity, catalog {catalog}"),
                "AssignOpportunity",
                json!({
                    "Catalog": catalog,
                    "Identifier": id,
                    "Assignee": {
                        "Email": "rep@partner.test",
                        "FirstName": "Rep",
                        "LastName": "One",
                        "BusinessTitle": "OpportunityOwner"
                    }
                }),
            ),
            (
                format!("SubmitOpportunity, catalog {catalog}"),
                "SubmitOpportunity",
                json!({ "Catalog": catalog, "Identifier": id, "InvolvementType": "Co-Sell" }),
            ),
            (
                format!("ListOpportunities, catalog {catalog}"),
                "ListOpportunities",
                json!({ "Catalog": catalog }),
            ),
        ];
        for (case, operation, body) in cases {
            call(&router, operation, &body)
                .await
                .expect_error(&case, 400, "ValidationException");
        }
    }

    // And the opportunity the refused calls named is untouched.
    let read = get_opportunity(&router, "AWS", &id).await;
    assert_eq!(
        read.version("catalog setup"),
        version,
        "a refused request must not change the record, got {}",
        read.body
    );
}

#[tokio::test]
async fn the_store_and_the_idempotency_ledger_stay_bounded() {
    let (router, engine) = emulator_with_engine();

    // Past the ledger's cap the OLDEST token is forgotten, while every record it
    // created stays: the ledger is bounded by its own rule, not by the store's.
    let overshoot = MAX_CREATE_REPLAYS + 2;
    for index in 0..overshoot {
        let case = format!("create {index}");
        call(
            &router,
            "CreateOpportunity",
            &create_body("AWS", &format!("bound-{index}"), &json!({})),
        )
        .await
        .ok(&case);
    }
    {
        let store = service_state(&*engine).expect("the service store is reachable");
        let store = store.lock();
        assert_eq!(
            store.create_replays.len(),
            MAX_CREATE_REPLAYS,
            "the idempotency ledger must stay at its cap of {MAX_CREATE_REPLAYS}"
        );
        assert_eq!(
            store.opportunities.len(),
            overshoot,
            "evicting a token must not drop the opportunity it created"
        );
    }

    // Replaying a token the ledger has forgotten mints a new opportunity, exactly as
    // a token whose idempotency window has passed would.
    let replayed = call(
        &router,
        "CreateOpportunity",
        &create_body("AWS", "bound-0", &json!({})),
    )
    .await;
    replayed.ok("evicted token");
    assert_eq!(
        replayed.field("evicted token", "Id"),
        format!("O{}", overshoot + 1),
        "a forgotten token must mint the next opportunity"
    );

    // And the store's own ceiling refuses a create past it, with the quota-shaped
    // modelled error, instead of growing.
    let mut created = overshoot + 1;
    while created < MAX_OPPORTUNITIES {
        let case = format!("fill {created}");
        call(
            &router,
            "CreateOpportunity",
            &create_body("AWS", &format!("fill-{created}"), &json!({})),
        )
        .await
        .ok(&case);
        created += 1;
    }
    call(
        &router,
        "CreateOpportunity",
        &create_body("AWS", "one-too-many", &json!({})),
    )
    .await
    .expect_error("past the ceiling", 429, "ThrottlingException");

    let store = service_state(&*engine).expect("the service store is reachable");
    let store = store.lock();
    assert_eq!(
        store.opportunities.len(),
        MAX_OPPORTUNITIES,
        "the store must stay at its ceiling of {MAX_OPPORTUNITIES}"
    );
    assert!(
        store.create_replays.len() <= MAX_CREATE_REPLAYS,
        "the idempotency ledger must stay within its cap, got {}",
        store.create_replays.len()
    );
}
