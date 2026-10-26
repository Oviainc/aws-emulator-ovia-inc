// SPDX-License-Identifier: CC0-1.0

//! The tenant-resource association surface, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: `CreateTenantResourceAssociation` resolves the request's
//! `TenantName` against the account's tenants BEFORE it writes anything — a name the
//! account does not hold is refused with the modelled terminal 404 and stores nothing —
//! and an accepted call records the association, so `ListResourceTenants` for that
//! `ResourceArn` reports the tenant it was made for, with the tenant's own id and the
//! instant it was associated. The tables below span what the rule generalises over: the
//! kind of resource the ARN names, the region it carries, the form of the tenant name,
//! how many tenants share one resource and how many resources one tenant holds, the
//! order associations are made in, the page size a listing asks for, and each refusal
//! leg.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulated ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// The longest `TenantName` SES accepts, taken from the crate's own constant so the
/// bound is stated once: its documentation states the name "can contain up to 64
/// alphanumeric characters, including letters, numbers, hyphens (-) and underscores
/// (_) only" (`smithy_query(query="shape",shape="TenantName")`).
const MAX_TENANT_NAME_CHARS: usize =
    service_com_amazonaws_sesv2_simpleemailservice_v2::state::MAX_TENANT_NAME_CHARS;

/// How many associations the store holds, taken from the crate's own constant so the
/// bound is stated once. It is the emulator's own ceiling on a table a request grows,
/// not an AWS quota.
const MAX_TENANT_RESOURCE_ASSOCIATIONS: usize =
    service_com_amazonaws_sesv2_simpleemailservice_v2::state::MAX_TENANT_RESOURCE_ASSOCIATIONS;

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A string member of the body, or `""` when absent.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `ResourceTenants` rows a listing reported, in the order reported.
    fn resource_tenants(&self) -> Vec<Value> {
        self.body
            .get("ResourceTenants")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `TenantName` of every reported row, in the order reported.
    fn listed_tenants(&self) -> Vec<String> {
        self.resource_tenants()
            .iter()
            .map(|row| {
                row.get("TenantName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The `NextToken` the listing handed back, when it handed one back.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, path: &str, region: &str, body: Value) -> Reply {
        let payload = bytes::Bytes::from(serde_json::to_vec(&body).expect("serialize body"));
        let host = format!("email.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name, and what names the region an ARN is minted in; the signature itself
            // is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{region}/ses/aws4_request, SignedHeaders=host, Signature=00"),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create_tenant(&self, name: &str) -> Reply {
        self.call(
            "/v2/email/tenants",
            "us-east-1",
            json!({"TenantName": name}),
        )
        .await
    }

    async fn associate(&self, tenant_name: &str, resource_arn: &str) -> Reply {
        self.call(
            "/v2/email/tenants/resources",
            "us-east-1",
            json!({"TenantName": tenant_name, "ResourceArn": resource_arn}),
        )
        .await
    }

    async fn list_resource_tenants(&self, resource_arn: &str) -> Reply {
        self.call(
            "/v2/email/resources/tenants/list",
            "us-east-1",
            json!({"ResourceArn": resource_arn}),
        )
        .await
    }

    async fn list_resource_tenants_page(&self, body: Value) -> Reply {
        self.call("/v2/email/resources/tenants/list", "us-east-1", body)
            .await
    }
}

/// One accepted association: the tenant to create and the resource ARN to associate it
/// with.
struct AssociationCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `TenantName` to create and associate.
    tenant_name: String,
    /// The `ResourceArn` the association is made for.
    resource_arn: String,
}

/// The accepted-association table.
///
/// The resource kinds are the three `ResourceType` values the model names
/// (`smithy_query(query="shape",shape="ResourceType")`: `EMAIL_IDENTITY`,
/// `CONFIGURATION_SET`, `EMAIL_TEMPLATE`), each written as the ARN SES uses for it, and
/// the tenant names span the documented form from its shortest to its 64-character
/// bound.
fn association_cases() -> Vec<AssociationCase> {
    vec![
        AssociationCase {
            case: "a configuration set, and the shortest tenant name the model allows",
            tenant_name: "a".to_string(),
            resource_arn: format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/nightly"),
        },
        AssociationCase {
            case: "an email identity that is an address",
            tenant_name: "Tenant_01-beta".to_string(),
            resource_arn: format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:identity/sender@example.com"),
        },
        AssociationCase {
            case: "an email identity that is a domain",
            tenant_name: "domain_tenant".to_string(),
            resource_arn: format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:identity/example.com"),
        },
        AssociationCase {
            case: "an email template, and a tenant name at the documented 64-character bound",
            tenant_name: format!(
                "tenant-{}",
                "x".repeat(MAX_TENANT_NAME_CHARS - "tenant-".len())
            ),
            resource_arn: format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:template/welcome"),
        },
        AssociationCase {
            case: "a configuration set in another region",
            tenant_name: "eu-tenant".to_string(),
            resource_arn: format!("arn:aws:ses:eu-west-1:{ACCOUNT_ID}:configuration-set/nightly"),
        },
        AssociationCase {
            case: "a resource ARN that is not one this account holds",
            tenant_name: "foreign-resource".to_string(),
            resource_arn: format!("arn:aws:ses:ap-southeast-2:{ACCOUNT_ID}:configuration-set/x_1"),
        },
    ]
}

#[tokio::test]
async fn an_association_round_trips_through_the_resources_tenant_listing() {
    let ses = Ses::new();
    let mut seen_arns = BTreeSet::new();

    for case in association_cases() {
        let created = ses.create_tenant(&case.tenant_name).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
            case.case, created.status, created.body
        );
        let tenant_id = created.text("TenantId");

        // Before the association, the resource has no tenants at all.
        let before = ses.list_resource_tenants(&case.resource_arn).await;
        assert_eq!(
            before.status, 200,
            "case {:?}: expected HTTP 200 from ListResourceTenants, got {} {:?}",
            case.case, before.status, before.body
        );
        assert!(
            before.resource_tenants().is_empty(),
            "case {:?}: an unassociated resource has no tenants, got {:?}",
            case.case,
            before.body
        );

        let associated = ses.associate(&case.tenant_name, &case.resource_arn).await;
        assert_eq!(
            associated.status, 200,
            "case {:?}: expected HTTP 200 from CreateTenantResourceAssociation, got {} {:?}",
            case.case, associated.status, associated.body
        );
        assert!(
            associated
                .body
                .as_object()
                .is_none_or(|members| members.is_empty()),
            "case {:?}: the response models no members, got {:?}",
            case.case,
            associated.body
        );

        let listed = ses.list_resource_tenants(&case.resource_arn).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected HTTP 200 from ListResourceTenants, got {} {:?}",
            case.case, listed.status, listed.body
        );
        let rows = listed.resource_tenants();
        assert_eq!(
            rows.len(),
            1,
            "case {:?}: the resource must report exactly the one tenant it was \
             associated with, got {:?}",
            case.case,
            listed.body
        );
        let row = &rows[0];
        assert_eq!(
            row.get("TenantName").and_then(Value::as_str),
            Some(case.tenant_name.as_str()),
            "case {:?}: the row must name the tenant the association was made for, got {row:?}",
            case.case
        );
        assert_eq!(
            row.get("TenantId").and_then(Value::as_str),
            Some(tenant_id.as_str()),
            "case {:?}: the row must carry the id CreateTenant minted, got {row:?}",
            case.case
        );
        assert_eq!(
            row.get("ResourceArn").and_then(Value::as_str),
            Some(case.resource_arn.as_str()),
            "case {:?}: the row must echo the resource ARN as the request spelled it, \
             got {row:?}",
            case.case
        );
        let associated_at = row.get("AssociatedTimestamp").and_then(Value::as_f64);
        assert!(
            associated_at.is_some_and(|seconds| seconds > 0.0),
            "case {:?}: the row must report when the resource was associated, got {row:?}",
            case.case
        );

        // Reading does not change what a later caller sees: the same rows, including the
        // association instant, which is read off the record rather than recomputed.
        let again = ses.list_resource_tenants(&case.resource_arn).await;
        assert_eq!(
            again.resource_tenants(),
            rows,
            "case {:?}: two reads of one resource's tenants must agree, got {:?} then {:?}",
            case.case,
            listed.body,
            again.body
        );

        // Every row this suite stored is reached under its own ARN only, so the listing
        // is keyed by the resource and not by the account.
        assert!(
            seen_arns.insert(case.resource_arn.clone()),
            "case {:?}: the table must not reuse an ARN, {} appears twice",
            case.case,
            case.resource_arn
        );
    }

    // Each resource still reports only its own tenant after every row was stored.
    for case in association_cases() {
        let listed = ses.list_resource_tenants(&case.resource_arn).await;
        assert_eq!(
            listed.listed_tenants(),
            vec![case.tenant_name.clone()],
            "case {:?}: a resource reports only the tenants associated with it, got {:?}",
            case.case,
            listed.body
        );
    }
}

#[tokio::test]
async fn a_tenant_the_account_does_not_hold_is_refused_and_nothing_is_associated() {
    let ses = Ses::new();
    let resource_arn = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/nightly");
    // A tenant that does exist, so the refusals below are about the name that was asked
    // for and not about an empty account.
    let created = ses.create_tenant("Mixed_Case").await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
        "Mixed_Case", created.status, created.body
    );

    for (case, name) in [
        ("a tenant nothing ever created", "never-created".to_string()),
        ("the same name in another case", "mixed_case".to_string()),
        (
            "a name one character longer than a stored one",
            "Mixed_Cases".to_string(),
        ),
        (
            "a name at the documented bound that was never created",
            "z".repeat(MAX_TENANT_NAME_CHARS),
        ),
    ] {
        let refused = ses.associate(&name, &resource_arn).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 from CreateTenantResourceAssociation, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?} {:?}",
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            format!("The requested tenant <{name}> does not exist."),
            "case {case:?}: the refusal must name the tenant it did not find, got {:?}",
            refused.message()
        );

        // The refusal had no effect: the resource is still associated with nothing.
        let listed = ses.list_resource_tenants(&resource_arn).await;
        assert!(
            listed.resource_tenants().is_empty(),
            "case {case:?}: a refused association must store nothing, got {:?}",
            listed.body
        );
    }

    // The positive control on the very same ARN: once the tenant exists, the same call
    // succeeds and the tenant appears under that resource.
    let created = ses.create_tenant("never-created").await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
        "never-created", created.status, created.body
    );
    let associated = ses.associate("never-created", &resource_arn).await;
    assert_eq!(
        associated.status, 200,
        "case {:?}: expected HTTP 200 once the tenant exists, got {} {:?}",
        "never-created", associated.status, associated.body
    );
    let listed = ses.list_resource_tenants(&resource_arn).await;
    assert_eq!(
        listed.listed_tenants(),
        vec!["never-created".to_string()],
        "case {:?}: the resource must report the tenant just associated, got {:?}",
        "never-created",
        listed.body
    );
}

#[tokio::test]
async fn a_tenant_name_outside_the_documented_form_is_refused() {
    let ses = Ses::new();
    let resource_arn = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/nightly");

    // Each row is a name the documented form excludes: too long, or carrying a character
    // that is not a letter, a digit, a hyphen or an underscore. A name no tenant could
    // carry is a bad request, not a missing tenant.
    for (case, name) in [
        (
            "one character past the documented bound",
            "x".repeat(MAX_TENANT_NAME_CHARS + 1),
        ),
        ("a space", "has space".to_string()),
        ("a dot", "dot.name".to_string()),
        ("a slash", "slash/name".to_string()),
        ("a plus sign", "plus+name".to_string()),
        ("a character outside the ASCII range", "ünïcode".to_string()),
    ] {
        let refused = ses.associate(&name, &resource_arn).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from CreateTenantResourceAssociation, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?} {:?}",
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.message().contains(&name),
            "case {case:?}: the refusal must name the tenant it refused, got {:?}",
            refused.message()
        );
    }

    let listed = ses.list_resource_tenants(&resource_arn).await;
    assert!(
        listed.resource_tenants().is_empty(),
        "case {:?}: a refused association must store nothing, got {:?}",
        "malformed names",
        listed.body
    );
}

#[tokio::test]
async fn a_resource_is_shared_by_its_tenants_in_name_order() {
    let ses = Ses::new();
    let shared_arn = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/shared");
    // Associated in an order that is not the name order, so the listing's order is the
    // table's and not the order the calls arrived in.
    let order_made = ["zeta", "alpha", "Mid_1", "beta-2"];

    let mut ids = Vec::new();
    for name in order_made {
        let created = ses.create_tenant(name).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
            created.status, created.body
        );
        ids.push((name, created.text("TenantId")));
        let associated = ses.associate(name, &shared_arn).await;
        assert_eq!(
            associated.status, 200,
            "case {name:?}: expected HTTP 200 from CreateTenantResourceAssociation, got {} {:?}",
            associated.status, associated.body
        );
    }

    let listed = ses.list_resource_tenants(&shared_arn).await;
    let mut expected: Vec<String> = order_made.iter().map(|name| name.to_string()).collect();
    expected.sort();
    assert_eq!(
        listed.listed_tenants(),
        expected,
        "case {:?}: one resource reports every tenant it is shared with, in name order, \
         got {:?}",
        "four tenants sharing one configuration set",
        listed.body
    );
    for (name, id) in &ids {
        let row = listed
            .resource_tenants()
            .into_iter()
            .find(|row| row.get("TenantName").and_then(Value::as_str) == Some(name))
            .unwrap_or(Value::Null);
        assert_eq!(
            row.get("TenantId").and_then(Value::as_str),
            Some(id.as_str()),
            "case {name:?}: each row carries that tenant's own id, got {row:?}"
        );
    }

    // The other half of the rule: one tenant may hold several resources, and each
    // resource's listing reports only itself.
    let own_arns: Vec<String> = ["alpha-only-a", "alpha-only-b", "alpha-only-c"]
        .iter()
        .map(|suffix| format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/{suffix}"))
        .collect();
    for arn in &own_arns {
        let associated = ses.associate("alpha", arn).await;
        assert_eq!(
            associated.status, 200,
            "case {arn:?}: expected HTTP 200 from CreateTenantResourceAssociation, got {} {:?}",
            associated.status, associated.body
        );
    }
    for arn in &own_arns {
        let listed = ses.list_resource_tenants(arn).await;
        assert_eq!(
            listed.listed_tenants(),
            vec!["alpha".to_string()],
            "case {arn:?}: a resource reports only its own tenants, got {:?}",
            listed.body
        );
    }
    // And the shared resource is unchanged by those associations.
    let shared = ses.list_resource_tenants(&shared_arn).await;
    assert_eq!(
        shared.listed_tenants(),
        expected,
        "case {:?}: associating other resources leaves this one's tenants alone, got {:?}",
        "four tenants sharing one configuration set",
        shared.body
    );
}

#[tokio::test]
async fn an_association_that_already_exists_is_refused_and_leaves_one_row() {
    let ses = Ses::new();
    let resource_arn = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/nightly");
    let created = ses.create_tenant("marketing").await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
        "marketing", created.status, created.body
    );
    let first = ses.associate("marketing", &resource_arn).await;
    assert_eq!(
        first.status, 200,
        "case {:?}: expected HTTP 200 from the first association, got {} {:?}",
        "marketing", first.status, first.body
    );
    let stored = ses.list_resource_tenants(&resource_arn).await;

    // A second association of the same pair is a repeat, whichever order the pair is
    // reached in: the same tenant on another resource, and another tenant on the same
    // resource, are not repeats and are accepted below.
    let again = ses.associate("marketing", &resource_arn).await;
    assert_eq!(
        again.status, 400,
        "case {:?}: expected HTTP 400 from the repeated association, got {} {:?}",
        "marketing", again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "AlreadyExistsException",
        "case {:?}: expected AlreadyExistsException, got {:?} {:?}",
        "marketing",
        again.error_code(),
        again.body
    );
    assert!(
        again.message().contains("marketing") && again.message().contains(&resource_arn),
        "case {:?}: the refusal must name the tenant and the resource, got {:?}",
        "marketing",
        again.message()
    );

    // The refused repeat stored no second row and did not re-stamp the first one.
    let after = ses.list_resource_tenants(&resource_arn).await;
    assert_eq!(
        after.resource_tenants(),
        stored.resource_tenants(),
        "case {:?}: a refused repeat must leave the stored association untouched, got {:?}",
        "marketing",
        after.body
    );

    // Neither leg of the pair alone makes a repeat.
    let other_resource = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:identity/example.com");
    let accepted = ses.associate("marketing", &other_resource).await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: the same tenant on another resource is not a repeat, got {} {:?}",
        "marketing", accepted.status, accepted.body
    );
    let created = ses.create_tenant("support").await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
        "support", created.status, created.body
    );
    let accepted = ses.associate("support", &resource_arn).await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: another tenant on the same resource is not a repeat, got {} {:?}",
        "support", accepted.status, accepted.body
    );
    let after = ses.list_resource_tenants(&resource_arn).await;
    assert_eq!(
        after.listed_tenants(),
        vec!["marketing".to_string(), "support".to_string()],
        "case {:?}: the resource now reports both tenants, got {:?}",
        "marketing and support",
        after.body
    );
}

#[tokio::test]
async fn a_walk_of_a_resources_tenants_reports_each_one_exactly_once() {
    let resource_arn = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/paged");

    // The pagination rule spans the row count and the page size independently: an empty,
    // a single and a multi-row resource, with no page size, a page size of one, one that
    // divides the count, one that does not, and one larger than the count.
    for (case, stored, page_size) in [
        ("nothing associated, no page size", 0usize, None),
        ("nothing associated, page size 1", 0, Some(1)),
        ("one tenant, page size 1", 1, Some(1)),
        ("one tenant, no page size", 1, None),
        ("two tenants, page size 1", 2, Some(1)),
        ("seven tenants, page size 1", 7, Some(1)),
        ("seven tenants, page size 7", 7, Some(7)),
        ("seven tenants, page size 3", 7, Some(3)),
        ("seven tenants, page size 100", 7, Some(100)),
        ("seven tenants, no page size", 7, None),
        // A non-positive page size cannot truncate, or a walk would never advance.
        ("seven tenants, page size 0", 7, Some(0)),
    ] {
        let ses = Ses::new();
        let mut expected = Vec::new();
        for index in 0..stored {
            let name = format!("tenant-{index:02}");
            let created = ses.create_tenant(&name).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
                created.status, created.body
            );
            let associated = ses.associate(&name, &resource_arn).await;
            assert_eq!(
                associated.status, 200,
                "case {case:?}: expected HTTP 200 from CreateTenantResourceAssociation, \
                 got {} {:?}",
                associated.status, associated.body
            );
            expected.push(name);
        }
        expected.sort();

        let mut walked = Vec::new();
        let mut token: Option<String> = None;
        // A walk may not outlive its rows: one request per row plus one for the final
        // page is the most any page size can need.
        let budget = stored + 2;
        for request in 0..=budget {
            assert!(
                request < budget,
                "case {case:?}: the walk did not terminate within {budget} requests, \
                 got {walked:?}"
            );
            let mut body = json!({"ResourceArn": resource_arn});
            if let Some(size) = page_size {
                body["PageSize"] = json!(size);
            }
            if let Some(resume) = &token {
                body["NextToken"] = json!(resume);
            }
            let page = ses.list_resource_tenants_page(body).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200 from ListResourceTenants, got {} {:?}",
                page.status, page.body
            );
            if let Some(size) = page_size.filter(|size| *size > 0) {
                assert!(
                    page.resource_tenants().len() <= size,
                    "case {case:?}: a page may not exceed the page size, got {:?}",
                    page.body
                );
            }
            walked.extend(page.listed_tenants());
            token = page.next_token();
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: the walk must report every associated tenant exactly once, \
             in name order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_listing_never_issued_is_refused() {
    let ses = Ses::new();
    let resource_arn = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/nightly");
    let created = ses.create_tenant("marketing").await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
        "marketing", created.status, created.body
    );
    let associated = ses.associate("marketing", &resource_arn).await;
    assert_eq!(
        associated.status, 200,
        "case {:?}: expected HTTP 200 from CreateTenantResourceAssociation, got {} {:?}",
        "marketing", associated.status, associated.body
    );

    for (case, token) in [
        ("a token that is not base64 at all", "not-a-token".to_string()),
        (
            "base64 of text that is no listing's position",
            aws_smithy_types::base64::encode("marketing"),
        ),
        (
            "a token another listing of this service issued",
            service_com_amazonaws_sesv2_simpleemailservice_v2::state::encode_tagged_page_token(
                service_com_amazonaws_sesv2_simpleemailservice_v2::state::CONTACT_LISTS_PAGE_TOKEN_TAG,
                "marketing",
            ),
        ),
    ] {
        let refused = ses
            .list_resource_tenants_page(json!({
                "ResourceArn": resource_arn,
                "NextToken": token
            }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from ListResourceTenants, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?} {:?}",
            refused.error_code(),
            refused.body
        );
    }

    // This listing's own token is accepted, and resumes after the row it names.
    let resumed = ses
        .list_resource_tenants_page(json!({
            "ResourceArn": resource_arn,
            "NextToken": service_com_amazonaws_sesv2_simpleemailservice_v2::state::encode_tagged_page_token(
                service_com_amazonaws_sesv2_simpleemailservice_v2::state::RESOURCE_TENANTS_PAGE_TOKEN_TAG,
                "marketing",
            )
        }))
        .await;
    assert_eq!(
        resumed.status, 200,
        "case {:?}: expected HTTP 200 from ListResourceTenants, got {} {:?}",
        "this listing's own token", resumed.status, resumed.body
    );
    assert!(
        resumed.resource_tenants().is_empty(),
        "case {:?}: resuming after the last row reports nothing more, got {:?}",
        "this listing's own token",
        resumed.body
    );
}

#[tokio::test]
async fn the_association_table_stops_growing_at_its_ceiling() {
    let ses = Ses::new();
    let created = ses.create_tenant("ceiling").await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
        "ceiling", created.status, created.body
    );

    // The resource ARN is caller-supplied, so the table only stops growing because the
    // handler refuses past the ceiling: fill it to the brim one ARN at a time.
    for index in 0..MAX_TENANT_RESOURCE_ASSOCIATIONS {
        let arn = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/set-{index:05}");
        let accepted = ses.associate("ceiling", &arn).await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: association {index} is within the ceiling, got {} {:?}",
            "filling the table", accepted.status, accepted.body
        );
    }

    let over = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/one-too-many");
    let refused = ses.associate("ceiling", &over).await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected HTTP 400 past the ceiling, got {} {:?}",
        "one past the ceiling", refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "BadRequestException",
        "case {:?}: expected BadRequestException, got {:?} {:?}",
        "one past the ceiling",
        refused.error_code(),
        refused.body
    );
    let listed = ses.list_resource_tenants(&over).await;
    assert!(
        listed.resource_tenants().is_empty(),
        "case {:?}: the refused association stored nothing, got {:?}",
        "one past the ceiling",
        listed.body
    );

    // A full table still tells a repeat from an overflow: the pair is already there, so
    // that is the refusal the caller gets.
    let first_arn = format!("arn:aws:ses:us-east-1:{ACCOUNT_ID}:configuration-set/set-00000");
    let repeated = ses.associate("ceiling", &first_arn).await;
    assert_eq!(
        repeated.error_code(),
        "AlreadyExistsException",
        "case {:?}: a repeat is a repeat even at the ceiling, got {:?} {:?}",
        "a repeat at the ceiling",
        repeated.error_code(),
        repeated.body
    );
}
