// SPDX-License-Identifier: CC0-1.0

//! The assessment lifecycle, end to end over HTTP: create, read, list, update,
//! flip status, delete — plus the framework catalog a create resolves against.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`), so a
//! response here is the response a client gets: routing, restJson1
//! (de)serialization, modelled constraints and the handler, over the one shared
//! engine.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_auditmanager_bedrockassessmentmanagerlambda::state;
use service_com_amazonaws_auditmanager_bedrockassessmentmanagerlambda::EmulatedService;

/// The region every request in this suite signs for, so an ARN assertion can name
/// the region it expects.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// One emulator: a shared engine with this service mounted on a router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs with a placeholder key, so signature checking is off;
        // the credential scope is still what routes the request and names the
        // region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one request and return its status and parsed JSON body (`Null` for an
    /// empty body).
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let bytes = match &body {
            Some(value) => bytes::Bytes::from(
                serde_json::to_vec(value).expect("a test request body always serializes"),
            ),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("auditmanager.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{REGION}/auditmanager/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes)
            .expect("a test request always builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// Walk a dotted path into a JSON value; a missing member reads as `Null`.
fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for segment in path.split('.') {
        current = match segment.parse::<usize>() {
            Ok(index) => &current[index],
            Err(_) => &current[segment],
        };
    }
    current
}

/// A string member, or `""` when it is absent.
fn text(value: &Value, path: &str) -> String {
    at(value, path).as_str().unwrap_or_default().to_string()
}

/// A timestamp member as epoch seconds (restJson1 renders `Timestamp` as a
/// number), or `-1.0` when it is absent.
fn instant(value: &Value, path: &str) -> f64 {
    at(value, path).as_f64().unwrap_or(-1.0)
}

/// Does `candidate` satisfy the model's `UUID` pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`?
fn is_model_uuid(candidate: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let parts: Vec<&str> = candidate.split('-').collect();
    parts.len() == GROUPS.len()
        && parts.iter().zip(GROUPS).all(|(part, width)| {
            part.len() == width
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

/// Does `token` satisfy the model's `Token` pattern `^[A-Za-z0-9+/=]*$`?
fn is_model_token(token: &str) -> bool {
    token
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/' || byte == b'=')
}

/// One `CreateAssessment` request, as a row of the table the create cases span.
struct CreateCase {
    label: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    /// Accounts in scope.
    accounts: &'static [&'static str],
    /// Services the request names in scope, which Audit Manager drops.
    services: &'static [&'static str],
    roles: &'static [(&'static str, &'static str)],
    tags: &'static [(&'static str, &'static str)],
    destination: (&'static str, &'static str),
}

/// Rows differ along everything `CreateAssessment` persists: name length and
/// character set, a description present or absent, the number of accounts in
/// scope, services named or not, the number of roles and of tags.
const CREATE_CASES: &[CreateCase] = &[
    CreateCase {
        label: "minimal: one account, no description, no tags",
        name: "a",
        description: None,
        accounts: &["111122223333"],
        services: &[],
        roles: &[(
            "PROCESS_OWNER",
            "arn:aws:iam::111122223333:role/audit-owner",
        )],
        tags: &[],
        destination: ("S3", "s3://audit-reports/minimal"),
    },
    CreateCase {
        label: "description, one tag, services named in scope",
        name: "demo-assessment",
        description: Some("the assessment a client creates first"),
        accounts: &["111122223333"],
        services: &["s3", "CloudTrail"],
        roles: &[(
            "PROCESS_OWNER",
            "arn:aws:iam::111122223333:role/audit-owner",
        )],
        tags: &[("env", "test")],
        destination: ("S3", "s3://audit-reports/demo"),
    },
    CreateCase {
        label: "three accounts, three tags, two roles",
        name: "quarterly review (Q4) — 2026",
        description: Some(""),
        accounts: &["111122223333", "222233334444", "333344445555"],
        services: &[],
        roles: &[
            (
                "PROCESS_OWNER",
                "arn:aws:iam::111122223333:role/audit-owner",
            ),
            (
                "PROCESS_OWNER",
                "arn:aws:iam::222233334444:role/second-owner",
            ),
        ],
        tags: &[("env", "prod"), ("team", "security"), ("cost-center", "42")],
        destination: ("S3", "s3://audit-reports/quarterly/q4"),
    },
    CreateCase {
        label: "a 300-character name, the model's maximum",
        name: "n300-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        description: Some("a description that carries punctuation: commas, colons; and (parentheses)."),
        accounts: &["999988887777"],
        services: &["Amazon EC2"],
        roles: &[(
            "PROCESS_OWNER",
            "arn:aws:iam::999988887777:role/long-name-owner",
        )],
        tags: &[("owner", "platform")],
        destination: ("S3", "s3://audit-reports/long"),
    },
];

/// The request body for a create case.
fn create_body(case: &CreateCase, framework_id: &str) -> Value {
    let mut body = json!({
        "name": case.name,
        "frameworkId": framework_id,
        "assessmentReportsDestination": {
            "destinationType": case.destination.0,
            "destination": case.destination.1,
        },
        "scope": {
            "awsAccounts": case.accounts.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
            "awsServices": case.services.iter().map(|name| json!({"serviceName": name})).collect::<Vec<_>>(),
        },
        "roles": case.roles.iter().map(|(role_type, role_arn)| json!({"roleType": role_type, "roleArn": role_arn})).collect::<Vec<_>>(),
    });
    if let Some(description) = case.description {
        body["description"] = json!(description);
    }
    if !case.tags.is_empty() {
        body["tags"] = Value::Object(
            case.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    body
}

/// Percent-encode a query value the way an AWS SDK does: `+`, `/` and `=` are
/// not safe in a query component (a raw `+` decodes to a space there), and a
/// base64 page token can carry all three.
fn encode_query_value(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '+' => "%2B".to_string(),
            '/' => "%2F".to_string(),
            '=' => "%3D".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Every framework the account publishes under `frameworkType`, walked across
/// every page.
async fn list_frameworks(
    emulator: &Emulator,
    framework_type: &str,
    page_size: Option<i32>,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let mut path = format!("/assessmentFrameworks?frameworkType={framework_type}");
        if let Some(size) = page_size {
            path.push_str(&format!("&maxResults={size}"));
        }
        if let Some(cursor) = &token {
            path.push_str(&format!("&nextToken={}", encode_query_value(cursor)));
        }
        let (status, body) = emulator.send("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "ListAssessmentFrameworks({framework_type}, maxResults={page_size:?}): expected 200, got {status} {body}"
        );
        if let Some(items) = at(&body, "frameworkMetadataList").as_array() {
            collected.extend(items.iter().cloned());
        }
        match at(&body, "nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return collected,
        }
    }
    panic!("ListAssessmentFrameworks({framework_type}) never reached a last page");
}

/// One `ListAssessments` page.
async fn list_assessments_page(
    emulator: &Emulator,
    status_filter: Option<&str>,
    page_size: Option<i32>,
    token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut path = "/assessments".to_string();
    let mut query: Vec<String> = Vec::new();
    if let Some(status) = status_filter {
        query.push(format!("status={status}"));
    }
    if let Some(size) = page_size {
        query.push(format!("maxResults={size}"));
    }
    if let Some(cursor) = token {
        query.push(format!("nextToken={}", encode_query_value(cursor)));
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }
    let (status, body) = emulator.send("GET", &path, None).await;
    assert_eq!(
        status, 200,
        "ListAssessments(status={status_filter:?}, maxResults={page_size:?}): expected 200, got {status} {body}"
    );
    let items = at(&body, "assessmentMetadata")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let next = at(&body, "nextToken").as_str().map(ToString::to_string);
    (items, next)
}

/// Every assessment `ListAssessments` reports for a filter, with the page count.
async fn list_assessments_all(
    emulator: &Emulator,
    status_filter: Option<&str>,
    page_size: Option<i32>,
) -> (Vec<Value>, usize) {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 1..=64 {
        let (items, next) =
            list_assessments_page(emulator, status_filter, page_size, token.as_deref()).await;
        if let Some(cursor) = &next {
            assert!(
                is_model_token(cursor),
                "ListAssessments(status={status_filter:?}, maxResults={page_size:?}): nextToken {cursor:?} is outside the model's Token pattern"
            );
            assert!(
                !items.is_empty(),
                "ListAssessments(status={status_filter:?}, maxResults={page_size:?}): a page with a nextToken must carry items"
            );
        }
        collected.extend(items);
        match next {
            Some(cursor) => token = Some(cursor),
            None => return (collected, page),
        }
    }
    panic!("ListAssessments(status={status_filter:?}) never reached a last page");
}

/// The ids reported in a list, in order.
fn ids_of(items: &[Value]) -> Vec<String> {
    items.iter().map(|item| text(item, "id")).collect()
}

/// The first standard framework id the emulator publishes.
async fn a_framework_id(emulator: &Emulator) -> String {
    let frameworks = list_frameworks(emulator, state::FRAMEWORK_TYPE_STANDARD, None).await;
    let id = text(
        frameworks
            .first()
            .expect("the account publishes at least one standard framework"),
        "id",
    );
    assert!(
        is_model_uuid(&id),
        "the published framework id {id:?} is outside the model's UUID pattern"
    );
    id
}

/// Create one assessment from `case` and return the create response body.
async fn create(emulator: &Emulator, case: &CreateCase, framework_id: &str) -> Value {
    let (status, body) = emulator
        .send(
            "POST",
            "/assessments",
            Some(create_body(case, framework_id)),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {}: CreateAssessment expected 200, got {status} {body}",
        case.label
    );
    body
}

#[tokio::test]
async fn create_stores_the_assessment_and_get_returns_exactly_what_was_stored() {
    let emulator = Emulator::new();
    let framework_id = a_framework_id(&emulator).await;
    let frameworks = list_frameworks(&emulator, state::FRAMEWORK_TYPE_STANDARD, None).await;
    let framework = frameworks
        .iter()
        .find(|f| text(f, "id") == framework_id)
        .expect("the framework just listed is still published")
        .clone();

    let mut minted: Vec<String> = Vec::new();
    for case in CREATE_CASES {
        let created = create(&emulator, case, &framework_id).await;
        let assessment = at(&created, "assessment").clone();
        let id = text(&assessment, "metadata.id");

        assert!(
            is_model_uuid(&id),
            "case {}: minted id {id:?} is outside the model's UUID pattern",
            case.label
        );
        assert!(
            !minted.contains(&id),
            "case {}: CreateAssessment reused the id {id:?}, which it already minted",
            case.label
        );
        minted.push(id.clone());

        let expected_arn = format!("arn:aws:auditmanager:{REGION}:{ACCOUNT}:assessment/{id}");
        assert_eq!(
            text(&assessment, "arn"),
            expected_arn,
            "case {}: expected arn {expected_arn}, got {:?}",
            case.label,
            text(&assessment, "arn")
        );
        assert_eq!(
            text(&assessment, "metadata.status"),
            state::STATUS_ACTIVE,
            "case {}: a new assessment is ACTIVE, got {:?}",
            case.label,
            text(&assessment, "metadata.status")
        );
        assert_eq!(
            text(&assessment, "awsAccount.id"),
            ACCOUNT,
            "case {}: expected the emulator account {ACCOUNT}, got {:?}",
            case.label,
            text(&assessment, "awsAccount.id")
        );
        // The compliance standard comes from the framework, not the request.
        assert_eq!(
            text(&assessment, "metadata.complianceType"),
            text(&framework, "complianceType"),
            "case {}: expected the framework's complianceType {:?}, got {:?}",
            case.label,
            text(&framework, "complianceType"),
            text(&assessment, "metadata.complianceType")
        );
        assert_eq!(
            text(&assessment, "framework.id"),
            framework_id,
            "case {}: expected framework.id {framework_id}, got {:?}",
            case.label,
            text(&assessment, "framework.id")
        );
        assert!(
            instant(&assessment, "metadata.creationTime") > 0.0,
            "case {}: creationTime must be set, got {:?}",
            case.label,
            at(&assessment, "metadata.creationTime")
        );
        assert_eq!(
            instant(&assessment, "metadata.lastUpdated"),
            instant(&assessment, "metadata.creationTime"),
            "case {}: a new assessment's lastUpdated equals its creationTime",
            case.label
        );
        // `Scope$awsServices` is deprecated: "Audit Manager ignores this input.
        // Instead, the value for awsServices will show as empty."
        assert_eq!(
            at(&assessment, "metadata.scope.awsServices")
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {}: awsServices must be stored empty however many services the request named ({} named), got {:?}",
            case.label,
            case.services.len(),
            at(&assessment, "metadata.scope.awsServices")
        );

        // Read it back: the stored record, not a synthesized one.
        let (status, fetched) = emulator
            .send("GET", &format!("/assessments/{id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {}: GetAssessment expected 200, got {status} {fetched}",
            case.label
        );
        let read = at(&fetched, "assessment").clone();
        for member in [
            "arn",
            "tags",
            "awsAccount",
            "framework",
            "metadata.id",
            "metadata.name",
            "metadata.description",
            "metadata.status",
            "metadata.complianceType",
            "metadata.scope",
            "metadata.roles",
            "metadata.assessmentReportsDestination",
            "metadata.creationTime",
            "metadata.lastUpdated",
        ] {
            assert_eq!(
                at(&read, member),
                at(&assessment, member),
                "case {}: GetAssessment.{member} differs from what CreateAssessment returned: {:?} vs {:?}",
                case.label,
                at(&read, member),
                at(&assessment, member)
            );
        }
        assert_eq!(
            text(&read, "metadata.name"),
            case.name,
            "case {}: expected the stored name, got {:?}",
            case.label,
            text(&read, "metadata.name")
        );
        assert_eq!(
            at(&read, "metadata.description").as_str(),
            case.description,
            "case {}: expected description {:?}, got {:?}",
            case.label,
            case.description,
            at(&read, "metadata.description")
        );
        assert_eq!(
            at(&read, "metadata.scope.awsAccounts")
                .as_array()
                .map(Vec::len),
            Some(case.accounts.len()),
            "case {}: expected {} accounts in scope, got {:?}",
            case.label,
            case.accounts.len(),
            at(&read, "metadata.scope.awsAccounts")
        );
        assert_eq!(
            at(&read, "metadata.roles").as_array().map(Vec::len),
            Some(case.roles.len()),
            "case {}: expected {} roles, got {:?}",
            case.label,
            case.roles.len(),
            at(&read, "metadata.roles")
        );
        assert_eq!(
            at(&read, "tags").as_object().map(serde_json::Map::len),
            Some(case.tags.len()),
            "case {}: expected {} tags, got {:?}",
            case.label,
            case.tags.len(),
            at(&read, "tags")
        );
        assert_eq!(
            text(&fetched, "userRole.roleType"),
            "PROCESS_OWNER",
            "case {}: userRole reports the owning persona the assessment records, got {:?}",
            case.label,
            at(&fetched, "userRole")
        );
    }

    // Every created assessment is in the store, and only those.
    let (listed, _) = list_assessments_all(&emulator, None, None).await;
    let mut listed_ids = ids_of(&listed);
    listed_ids.sort();
    let mut expected = minted.clone();
    expected.sort();
    assert_eq!(
        listed_ids, expected,
        "ListAssessments must report exactly the created assessments, got {listed_ids:?} for {expected:?}"
    );
}

#[tokio::test]
async fn create_refuses_a_framework_the_account_does_not_hold() {
    let emulator = Emulator::new();
    // Well-formed `UUID`s that name no framework the account holds.
    let unknown = [
        "00000000-0000-4000-8000-000000000000",
        "ffffffff-ffff-4fff-8fff-ffffffffffff",
        "deadbeef-dead-4eef-8eef-deadbeefdead",
    ];
    for framework_id in unknown {
        let case = &CREATE_CASES[0];
        let (status, body) = emulator
            .send(
                "POST",
                "/assessments",
                Some(create_body(case, framework_id)),
            )
            .await;
        assert_eq!(
            status, 404,
            "case unknown framework {framework_id}: expected 404, got {status} {body}"
        );
        assert!(
            text(&body, "__type").ends_with("ResourceNotFoundException"),
            "case unknown framework {framework_id}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            text(&body, "resourceId"),
            framework_id,
            "case unknown framework {framework_id}: the error names the resource asked for, got {body}"
        );
    }

    // And nothing was stored by a refused create.
    let (listed, _) = list_assessments_all(&emulator, None, None).await;
    assert!(
        listed.is_empty(),
        "a refused CreateAssessment must store nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn create_refuses_a_role_type_other_than_process_owner() {
    let emulator = Emulator::new();
    let framework_id = a_framework_id(&emulator).await;
    // `Role$roleType`: "In CreateAssessment, roleType can only be PROCESS_OWNER."
    let cases = [
        vec![("RESOURCE_OWNER", "arn:aws:iam::111122223333:role/resource")],
        vec![
            ("PROCESS_OWNER", "arn:aws:iam::111122223333:role/owner"),
            ("RESOURCE_OWNER", "arn:aws:iam::111122223333:role/resource"),
        ],
    ];
    for roles in &cases {
        let mut body = create_body(&CREATE_CASES[0], &framework_id);
        body["roles"] = Value::Array(
            roles
                .iter()
                .map(|(role_type, role_arn)| json!({"roleType": role_type, "roleArn": role_arn}))
                .collect(),
        );
        let (status, response) = emulator.send("POST", "/assessments", Some(body)).await;
        assert_eq!(
            status, 400,
            "case roles {roles:?}: expected 400, got {status} {response}"
        );
        assert!(
            text(&response, "__type").ends_with("ValidationException"),
            "case roles {roles:?}: expected ValidationException, got {response}"
        );
    }

    // And nothing was stored by a refused create.
    let (listed, _) = list_assessments_all(&emulator, None, None).await;
    assert!(
        listed.is_empty(),
        "a refused CreateAssessment must store nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn list_pages_every_assessment_once_whatever_the_page_size() {
    let emulator = Emulator::new();
    let framework_id = a_framework_id(&emulator).await;

    let mut created: Vec<String> = Vec::new();
    for case in CREATE_CASES {
        let body = create(&emulator, case, &framework_id).await;
        created.push(text(at(&body, "assessment"), "metadata.id"));
    }
    created.sort();

    // The rule holds for every page size, including one that is larger than the
    // collection and one that divides it exactly.
    for page_size in [None, Some(1), Some(2), Some(3), Some(4), Some(1000)] {
        let (items, pages) = list_assessments_all(&emulator, None, page_size).await;
        let mut seen = ids_of(&items);
        let unique: std::collections::BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            seen.len(),
            unique.len(),
            "case maxResults={page_size:?}: paging returned a duplicate: {seen:?}"
        );
        seen.sort();
        assert_eq!(
            seen, created,
            "case maxResults={page_size:?}: expected every created assessment exactly once, got {seen:?}"
        );
        let expected_pages = match page_size {
            Some(size) => created.len().div_ceil(size as usize),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case maxResults={page_size:?}: expected {expected_pages} page(s) for {} assessments, got {pages}",
            created.len()
        );
    }

    // Paging is ordered and resumable: a first page of one, resumed, yields the
    // rest in the same stable order.
    let (first, token) = list_assessments_page(&emulator, None, Some(1), None).await;
    let token = token.expect("a page of 1 out of several assessments carries a nextToken");
    let mut rest: Vec<Value> = Vec::new();
    let mut cursor = Some(token);
    while let Some(current) = cursor {
        let (items, next) = list_assessments_page(&emulator, None, Some(1), Some(&current)).await;
        rest.extend(items);
        cursor = next;
    }
    let ordered: Vec<String> = ids_of(&first).into_iter().chain(ids_of(&rest)).collect();
    let (whole, _) = list_assessments_all(&emulator, None, Some(1000)).await;
    assert_eq!(
        ordered,
        ids_of(&whole),
        "resuming from a nextToken must continue the same stable order"
    );
}

#[tokio::test]
async fn status_update_moves_the_assessment_between_the_status_filters() {
    let emulator = Emulator::new();
    let framework_id = a_framework_id(&emulator).await;
    let created = create(&emulator, &CREATE_CASES[1], &framework_id).await;
    let id = text(at(&created, "assessment"), "metadata.id");
    let other = create(&emulator, &CREATE_CASES[0], &framework_id).await;
    let other_id = text(at(&other, "assessment"), "metadata.id");

    // Every row flips the status and asserts the stored record and both filters.
    for status in ["INACTIVE", "ACTIVE", "INACTIVE"] {
        let (code, body) = emulator
            .send(
                "PUT",
                &format!("/assessments/{id}/status"),
                Some(json!({"status": status})),
            )
            .await;
        assert_eq!(
            code, 200,
            "case status={status}: UpdateAssessmentStatus expected 200, got {code} {body}"
        );
        assert_eq!(
            text(at(&body, "assessment"), "metadata.status"),
            status,
            "case status={status}: the response reports the new status, got {body}"
        );

        let (code, fetched) = emulator
            .send("GET", &format!("/assessments/{id}"), None)
            .await;
        assert_eq!(
            code, 200,
            "case status={status}: GetAssessment expected 200, got {code} {fetched}"
        );
        assert_eq!(
            text(at(&fetched, "assessment"), "metadata.status"),
            status,
            "case status={status}: the stored status must be the one just set, got {fetched}"
        );

        for (filter, should_contain) in [
            ("ACTIVE", status == "ACTIVE"),
            ("INACTIVE", status == "INACTIVE"),
        ] {
            let (items, _) = list_assessments_all(&emulator, Some(filter), None).await;
            let ids = ids_of(&items);
            assert_eq!(
                ids.contains(&id),
                should_contain,
                "case status={status}: ListAssessments?status={filter} contains={} expected={should_contain}, got {ids:?}",
                ids.contains(&id)
            );
            // The filter reports only assessments in that status.
            for item in &items {
                assert_eq!(
                    text(item, "status"),
                    filter,
                    "case status={status}: ListAssessments?status={filter} returned {:?}",
                    item
                );
            }
        }
        // The untouched assessment keeps its own status.
        let (active, _) = list_assessments_all(&emulator, Some("ACTIVE"), None).await;
        assert!(
            ids_of(&active).contains(&other_id),
            "case status={status}: flipping one assessment must not move another, got {:?}",
            ids_of(&active)
        );
    }
}

#[tokio::test]
async fn update_replaces_the_members_the_request_names_and_leaves_the_rest() {
    let emulator = Emulator::new();
    let framework_id = a_framework_id(&emulator).await;
    let created = create(&emulator, &CREATE_CASES[1], &framework_id).await;
    let assessment = at(&created, "assessment").clone();
    let id = text(&assessment, "metadata.id");
    let created_at = instant(&assessment, "metadata.creationTime");

    // Each row names a different subset of the updatable members.
    let cases: Vec<(&str, Value)> = vec![
        (
            "name only",
            json!({
                "assessmentName": "renamed-assessment",
                "scope": at(&assessment, "metadata.scope").clone(),
            }),
        ),
        (
            "description only",
            json!({
                "assessmentDescription": "a second description",
                "scope": at(&assessment, "metadata.scope").clone(),
            }),
        ),
        (
            "scope: two accounts and services that are dropped",
            json!({
                "scope": {
                    "awsAccounts": [{"id": "111122223333"}, {"id": "444455556666"}],
                    "awsServices": [{"serviceName": "s3"}],
                },
            }),
        ),
        (
            "report destination and roles",
            json!({
                "scope": at(&assessment, "metadata.scope").clone(),
                "assessmentReportsDestination": {"destinationType": "S3", "destination": "s3://audit-reports/moved"},
                "roles": [{"roleType": "RESOURCE_OWNER", "roleArn": "arn:aws:iam::111122223333:role/resource"}],
            }),
        ),
        (
            "description cleared with the empty string",
            json!({
                "assessmentDescription": "",
                "scope": at(&assessment, "metadata.scope").clone(),
            }),
        ),
        (
            "name and description together",
            json!({
                "assessmentName": "final-name",
                "assessmentDescription": "final description",
                "scope": at(&assessment, "metadata.scope").clone(),
            }),
        ),
    ];

    let mut expected_name = text(&assessment, "metadata.name");
    let mut expected_description = at(&assessment, "metadata.description").clone();
    let mut expected_destination = at(&assessment, "metadata.assessmentReportsDestination").clone();
    let mut expected_roles = at(&assessment, "metadata.roles").clone();
    let mut expected_accounts = at(&assessment, "metadata.scope.awsAccounts").clone();
    let mut previous_update = instant(&assessment, "metadata.lastUpdated");

    for (label, request) in cases {
        // `lastUpdated` is read off the WALL clock (the service stores
        // `SystemTime::now`), so the only way to observe a strictly later value
        // is to let real time pass. This blocks the runtime worker deliberately:
        // the suite's `tokio` features are `macros, rt, rt-multi-thread` with no
        // `time`, so `tokio::time::sleep` is not available, and this test drives
        // one request at a time with no concurrent task to starve. Do not copy
        // the pattern into a test that does run tasks concurrently.
        std::thread::sleep(std::time::Duration::from_millis(5));

        if let Some(name) = request.get("assessmentName") {
            expected_name = name.as_str().unwrap_or_default().to_string();
        }
        if let Some(description) = request.get("assessmentDescription") {
            expected_description = description.clone();
        }
        if let Some(destination) = request.get("assessmentReportsDestination") {
            expected_destination = destination.clone();
        }
        if let Some(roles) = request.get("roles") {
            expected_roles = roles.clone();
        }
        expected_accounts = request
            .get("scope")
            .and_then(|scope| scope.get("awsAccounts"))
            .cloned()
            .unwrap_or(expected_accounts);

        let (code, body) = emulator
            .send("PUT", &format!("/assessments/{id}"), Some(request.clone()))
            .await;
        assert_eq!(
            code, 200,
            "case {label}: UpdateAssessment expected 200, got {code} {body}"
        );

        // Both the response and a following read show the update.
        for (source, document) in [
            ("UpdateAssessment", at(&body, "assessment").clone()),
            ("GetAssessment", {
                let (code, fetched) = emulator
                    .send("GET", &format!("/assessments/{id}"), None)
                    .await;
                assert_eq!(
                    code, 200,
                    "case {label}: GetAssessment expected 200, got {code} {fetched}"
                );
                at(&fetched, "assessment").clone()
            }),
        ] {
            assert_eq!(
                text(&document, "metadata.name"),
                expected_name,
                "case {label}: {source} expected name {expected_name:?}, got {:?}",
                text(&document, "metadata.name")
            );
            assert_eq!(
                at(&document, "metadata.description"),
                &expected_description,
                "case {label}: {source} expected description {expected_description:?}, got {:?}",
                at(&document, "metadata.description")
            );
            assert_eq!(
                at(&document, "metadata.assessmentReportsDestination"),
                &expected_destination,
                "case {label}: {source} expected destination {expected_destination:?}, got {:?}",
                at(&document, "metadata.assessmentReportsDestination")
            );
            assert_eq!(
                at(&document, "metadata.roles"),
                &expected_roles,
                "case {label}: {source} expected roles {expected_roles:?}, got {:?}",
                at(&document, "metadata.roles")
            );
            assert_eq!(
                at(&document, "metadata.scope.awsAccounts"),
                &expected_accounts,
                "case {label}: {source} expected accounts {expected_accounts:?}, got {:?}",
                at(&document, "metadata.scope.awsAccounts")
            );
            assert_eq!(
                at(&document, "metadata.scope.awsServices")
                    .as_array()
                    .map(Vec::len),
                Some(0),
                "case {label}: {source} must store an empty awsServices, got {:?}",
                at(&document, "metadata.scope.awsServices")
            );
            // Identity never changes under an update.
            assert_eq!(
                text(&document, "metadata.id"),
                id,
                "case {label}: {source} changed the id"
            );
            assert_eq!(
                instant(&document, "metadata.creationTime"),
                created_at,
                "case {label}: {source} changed creationTime"
            );
            let updated_at = instant(&document, "metadata.lastUpdated");
            assert!(
                updated_at > previous_update,
                "case {label}: {source} expected lastUpdated past {previous_update}, got {updated_at}"
            );
        }
        previous_update = instant(at(&body, "assessment"), "metadata.lastUpdated");
    }
}

#[tokio::test]
async fn delete_removes_the_assessment_and_a_later_read_is_not_found() {
    let emulator = Emulator::new();
    let framework_id = a_framework_id(&emulator).await;

    let mut ids = Vec::new();
    for case in CREATE_CASES {
        let body = create(&emulator, case, &framework_id).await;
        ids.push(text(at(&body, "assessment"), "metadata.id"));
    }

    // Deleting one leaves every other one listed and readable.
    for (index, id) in ids.iter().enumerate() {
        let (code, body) = emulator
            .send("DELETE", &format!("/assessments/{id}"), None)
            .await;
        assert_eq!(
            code, 200,
            "case delete #{index}: expected 200, got {code} {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case delete #{index}: expected an empty body, got {body}"
        );

        let (code, body) = emulator
            .send("GET", &format!("/assessments/{id}"), None)
            .await;
        assert_eq!(
            code, 404,
            "case delete #{index}: GetAssessment after delete expected 404, got {code} {body}"
        );
        assert!(
            text(&body, "__type").ends_with("ResourceNotFoundException"),
            "case delete #{index}: expected ResourceNotFoundException, got {body}"
        );

        // Deleting it again is a missing resource too.
        let (code, body) = emulator
            .send("DELETE", &format!("/assessments/{id}"), None)
            .await;
        assert_eq!(
            code, 404,
            "case delete #{index}: deleting twice expected 404, got {code} {body}"
        );

        let (listed, _) = list_assessments_all(&emulator, None, Some(2)).await;
        let listed_ids = ids_of(&listed);
        assert!(
            !listed_ids.contains(id),
            "case delete #{index}: ListAssessments still reports {id}, got {listed_ids:?}"
        );
        let mut expected: Vec<String> = ids[index + 1..].to_vec();
        expected.sort();
        let mut remaining = listed_ids.clone();
        remaining.sort();
        assert_eq!(
            remaining, expected,
            "case delete #{index}: expected the undeleted assessments {expected:?}, got {remaining:?}"
        );
    }
}

#[tokio::test]
async fn every_assessment_operation_reports_an_unknown_identifier_as_not_found() {
    let emulator = Emulator::new();
    let unknown = "00000000-0000-0000-0000-000000000000";
    let scope = json!({"awsAccounts": [{"id": "111122223333"}]});
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "GetAssessment",
            "GET",
            format!("/assessments/{unknown}"),
            None,
        ),
        (
            "UpdateAssessment",
            "PUT",
            format!("/assessments/{unknown}"),
            Some(json!({"scope": scope})),
        ),
        (
            "UpdateAssessmentStatus",
            "PUT",
            format!("/assessments/{unknown}/status"),
            Some(json!({"status": "INACTIVE"})),
        ),
        (
            "DeleteAssessment",
            "DELETE",
            format!("/assessments/{unknown}"),
            None,
        ),
    ];
    for (label, method, path, body) in cases {
        let (code, response) = emulator.send(method, &path, body).await;
        assert_eq!(
            code, 404,
            "case {label}: expected 404 for an unknown identifier, got {code} {response}"
        );
        assert!(
            text(&response, "__type").ends_with("ResourceNotFoundException"),
            "case {label}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            text(&response, "resourceId"),
            unknown,
            "case {label}: the error names the identifier asked for, got {response}"
        );
    }
}

#[tokio::test]
async fn a_page_token_this_listing_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let framework_id = a_framework_id(&emulator).await;
    for case in CREATE_CASES {
        create(&emulator, case, &framework_id).await;
    }

    // A token one listing issued is not a resume point for another, and a string
    // that merely decodes is not a resume point at all. Each row is a token the
    // model's `Token` pattern accepts, so it reaches the handler.
    let (_, assessments_token) = list_assessments_page(&emulator, None, Some(1), None).await;
    let assessments_token =
        assessments_token.expect("a page of 1 out of several assessments carries a nextToken");
    let frameworks_token = {
        let path = "/assessmentFrameworks?frameworkType=Standard&maxResults=1";
        let (status, body) = emulator.send("GET", path, None).await;
        assert_eq!(status, 200, "listing one framework: got {status} {body}");
        text(&body, "nextToken")
    };
    assert!(
        !frameworks_token.is_empty(),
        "a page of 1 out of several frameworks carries a nextToken"
    );

    let cases: Vec<(&str, String, String)> = vec![
        (
            "ListAssessments fed the framework listing's token",
            "/assessments?maxResults=1".to_string(),
            frameworks_token.clone(),
        ),
        (
            "ListAssessmentFrameworks fed the assessment listing's token",
            "/assessmentFrameworks?frameworkType=Standard&maxResults=1".to_string(),
            assessments_token.clone(),
        ),
        (
            "ListAssessments fed base64 that is not a cursor",
            "/assessments?maxResults=1".to_string(),
            "bm90LWEtY3Vyc29y".to_string(),
        ),
        (
            "ListAssessmentFrameworks fed base64 that is not a cursor",
            "/assessmentFrameworks?frameworkType=Standard&maxResults=1".to_string(),
            "bm90LWEtY3Vyc29y".to_string(),
        ),
        (
            "ListAssessments fed bytes that are not base64",
            "/assessments?maxResults=1".to_string(),
            "AAAAA".to_string(),
        ),
    ];
    for (label, path, token) in cases {
        let (status, body) = emulator
            .send(
                "GET",
                &format!("{path}&nextToken={}", encode_query_value(&token)),
                None,
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400 for a token this listing did not issue, got {status} {body}"
        );
        assert!(
            text(&body, "__type").ends_with("ValidationException"),
            "case {label}: expected ValidationException, got {body}"
        );
        assert_eq!(
            text(&body, "reason"),
            "cannotParse",
            "case {label}: expected reason cannotParse, got {body}"
        );
    }
}

#[tokio::test]
async fn the_store_is_bounded_and_a_delete_makes_room_again() {
    let emulator = Emulator::new();
    let framework_id = a_framework_id(&emulator).await;
    let case = &CREATE_CASES[0];

    // Fill the store to the bound the service documents.
    let mut last_id = String::new();
    for index in 0..state::ASSESSMENT_QUOTA {
        let (status, body) = emulator
            .send(
                "POST",
                "/assessments",
                Some(create_body(case, &framework_id)),
            )
            .await;
        assert_eq!(
            status, 200,
            "case create #{index} below the bound: expected 200, got {status} {body}"
        );
        last_id = text(at(&body, "assessment"), "metadata.id");
    }

    // One past it is the modelled quota failure, and nothing is stored.
    let (status, body) = emulator
        .send(
            "POST",
            "/assessments",
            Some(create_body(case, &framework_id)),
        )
        .await;
    assert_eq!(
        status, 402,
        "case create at the bound: expected 402, got {status} {body}"
    );
    assert!(
        text(&body, "__type").ends_with("ServiceQuotaExceededException"),
        "case create at the bound: expected ServiceQuotaExceededException, got {body}"
    );

    // Releasing one record makes room for exactly one more.
    let (status, body) = emulator
        .send("DELETE", &format!("/assessments/{last_id}"), None)
        .await;
    assert_eq!(
        status, 200,
        "case delete to make room: expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .send(
            "POST",
            "/assessments",
            Some(create_body(case, &framework_id)),
        )
        .await;
    assert_eq!(
        status, 200,
        "case create after a delete: expected 200, got {status} {body}"
    );
}

#[tokio::test]
async fn list_assessment_frameworks_publishes_the_catalog_create_resolves_against() {
    let emulator = Emulator::new();

    // Every page size reports the same catalog, which is the one the service
    // holds (`state::STANDARD_FRAMEWORKS`).
    let expected: Vec<(&str, &str)> = {
        let mut rows: Vec<(&str, &str)> = state::STANDARD_FRAMEWORKS
            .iter()
            .map(|framework| (framework.name, framework.id))
            .collect();
        rows.sort();
        rows
    };
    for page_size in [None, Some(1), Some(3), Some(expected.len() as i32), Some(1000)] {
        let listed = list_frameworks(&emulator, state::FRAMEWORK_TYPE_STANDARD, page_size).await;
        let rows: Vec<(String, String)> = listed
            .iter()
            .map(|framework| (text(framework, "name"), text(framework, "id")))
            .collect();
        let expected_rows: Vec<(String, String)> = expected
            .iter()
            .map(|(name, id)| ((*name).to_string(), (*id).to_string()))
            .collect();
        assert_eq!(
            rows, expected_rows,
            "case maxResults={page_size:?}: expected the standard catalog in name order, got {rows:?}"
        );
        for framework in &listed {
            assert_eq!(
                text(framework, "type"),
                state::FRAMEWORK_TYPE_STANDARD,
                "case maxResults={page_size:?}: a framework listed as Standard reports type {:?}",
                text(framework, "type")
            );
            let id = text(framework, "id");
            assert_eq!(
                text(framework, "arn"),
                format!("arn:aws:auditmanager:{REGION}:{ACCOUNT}:assessmentFramework/{id}"),
                "case maxResults={page_size:?}: unexpected framework arn {:?}",
                text(framework, "arn")
            );
        }
    }

    // The account holds no custom frameworks until one is created.
    let custom = list_frameworks(&emulator, "Custom", None).await;
    assert!(
        custom.is_empty(),
        "no custom framework has been created, got {custom:?}"
    );

    // Every published id is one CreateAssessment accepts, and the assessment
    // reports that framework's own compliance standard — checked here on a
    // framework the issue never cited.
    for framework in list_frameworks(&emulator, state::FRAMEWORK_TYPE_STANDARD, None).await {
        let id = text(&framework, "id");
        let created = create(&emulator, &CREATE_CASES[0], &id).await;
        let assessment = at(&created, "assessment");
        assert_eq!(
            text(assessment, "framework.id"),
            id,
            "case framework {id}: expected the assessment to record it, got {assessment}"
        );
        assert_eq!(
            text(assessment, "metadata.complianceType"),
            text(&framework, "complianceType"),
            "case framework {id}: expected complianceType {:?}, got {:?}",
            text(&framework, "complianceType"),
            text(assessment, "metadata.complianceType")
        );
        assert_eq!(
            text(assessment, "framework.metadata.name"),
            text(&framework, "name"),
            "case framework {id}: expected framework name {:?}, got {:?}",
            text(&framework, "name"),
            text(assessment, "framework.metadata.name")
        );
    }
}
