// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a layer version's resource-based policy:
//! `AddLayerVersionPermission` grants layer usage permission on one published version,
//! `GetLayerVersionPolicy` reports the policy those grants make up, and
//! `RemoveLayerVersionPermission` takes one statement back out.
//!
//! The rule under test: `AddLayerVersionPermission` appends to the resource-based policy
//! of the layer VERSION the request addresses exactly one statement describing the grant
//! — its `Sid` is the `StatementId`, its `Effect` is `Allow`, its `Principal` is the
//! supplied principal in the form the policy language spells it, its `Action` is the
//! supplied action, its `Resource` is the ARN of that version, and an `OrganizationId`
//! becomes the IAM condition key that expresses it — answers that statement and the
//! revision the change minted, and `GetLayerVersionPolicy` reports the document of the
//! statements that version has accumulated at that same revision.
//!
//! Every row drives real HTTP requests through the router and reads the grant back
//! through `GetLayerVersionPolicy`, so a statement that was answered but never stored
//! fails here. The rows differ along what the rule generalises over: the form of the
//! principal (an account id, every account, every account in an organization, an IAM
//! root ARN), the version addressed, and whether the layer is named by its bare name or
//! by its ARN.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off, exactly as
/// the emulator runs unless started with it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The IAM policy language version a policy document carries, and the `Id` Lambda gives
/// the policy it maintains for a resource.
const POLICY_LANGUAGE_VERSION: &str = "2012-10-17";
const POLICY_ID: &str = "default";

/// The only action a layer-version grant can name: the model's
/// `LayerPermissionAllowedAction` is constrained to the pattern
/// `^lambda:GetLayerVersion$`
/// (`smithy_query(query="shape",shape="LayerPermissionAllowedAction")`).
const ACTION: &str = "lambda:GetLayerVersion";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body. Panics with the body text
    /// when it is neither, which is the failure a caller needs to see.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The unqualified ARN a layer of `name` carries.
fn layer_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:{name}")
}

/// The ARN version `version` of a layer of `name` carries.
fn layer_version_arn(name: &str, version: i64) -> String {
    format!("{}:{version}", layer_arn(name))
}

/// The `?RevisionId=` suffix of a call carrying the precondition, and nothing for one
/// that carries none.
fn revision_query(revision: Option<&str>) -> String {
    revision.map_or_else(String::new, |value| format!("?RevisionId={value}"))
}

/// Publish `count` versions of a layer, so the grants below address versions that exist.
async fn publish_versions(router: &MultiServiceRouter, name: &str, count: usize) {
    for index in 0..count {
        // The archive content is irrelevant to a permission; only its presence makes a
        // version. It differs per version so no two share a digest.
        let bytes: Vec<u8> = (0..64 + index)
            .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
            .collect();
        let answer = call(
            router,
            "POST",
            &format!("/2018-10-31/layers/{name}/versions"),
            &json!({
                "Content": {
                    "ZipFile": base64::engine::general_purpose::STANDARD.encode(&bytes),
                },
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {name:?}: expected 201 from PublishLayerVersion, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// `AddLayerVersionPermission` on a layer reference and a version number.
async fn add_permission(
    router: &MultiServiceRouter,
    reference: &str,
    version: i64,
    body: &Value,
    revision: Option<&str>,
) -> Answer {
    let uri = format!(
        "/2018-10-31/layers/{reference}/versions/{version}/policy{}",
        revision_query(revision)
    );
    call(router, "POST", &uri, body).await
}

/// `GetLayerVersionPolicy` on a layer reference and a version number.
async fn get_policy(router: &MultiServiceRouter, reference: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{reference}/versions/{version}/policy");
    call(router, "GET", &uri, &Value::Null).await
}

/// `RemoveLayerVersionPermission` on a layer reference, a version number and a statement
/// id.
async fn remove_permission(
    router: &MultiServiceRouter,
    reference: &str,
    version: i64,
    statement_id: &str,
    revision: Option<&str>,
) -> Answer {
    let uri = format!(
        "/2018-10-31/layers/{reference}/versions/{version}/policy/{statement_id}{}",
        revision_query(revision)
    );
    call(router, "DELETE", &uri, &Value::Null).await
}

/// The `Statement` member of an `AddLayerVersionPermission` answer, parsed.
fn statement_of(case: &str, answer: &Answer) -> Value {
    let statement = answer
        .string("Statement")
        .unwrap_or_else(|| panic!("case {case:?}: no Statement member in {}", answer.text()));
    serde_json::from_str(&statement).unwrap_or_else(|error| {
        panic!("case {case:?}: Statement is not JSON ({error}): {statement:?}")
    })
}

/// The `Policy` member of a `GetLayerVersionPolicy` answer, parsed.
fn policy_of(case: &str, answer: &Answer) -> Value {
    let policy = answer
        .string("Policy")
        .unwrap_or_else(|| panic!("case {case:?}: no Policy member in {}", answer.text()));
    serde_json::from_str(&policy)
        .unwrap_or_else(|error| panic!("case {case:?}: Policy is not JSON ({error}): {policy:?}"))
}

/// The document a policy of exactly `statements` must be.
fn document(statements: Vec<Value>) -> Value {
    json!({
        "Version": POLICY_LANGUAGE_VERSION,
        "Id": POLICY_ID,
        "Statement": statements,
    })
}

/// One row of the grant table: the layer version to grant on, how the request addresses
/// it, the request body, and the statement the policy must then hold.
struct GrantCase {
    name: &'static str,
    /// The bare layer name, which is also how many versions to publish under it.
    layer: &'static str,
    /// How many versions to publish, so the row can address a later one.
    versions: usize,
    /// How the request names the layer: its bare name or its ARN.
    reference: String,
    /// The version the grant addresses.
    version: i64,
    request: Value,
    expected_statement: Value,
}

/// The rows, each differing from the others in the form of its principal, the version it
/// addresses or the way it names the layer — the dimensions the rule generalises over.
/// The principal forms are the ones the model's `LayerPermissionAllowedPrincipal` pattern
/// admits, `^\d{12}|\*|arn:(aws[a-zA-Z-]*):iam::\d{12}:root$`
/// (`smithy_query(query="shape",shape="LayerPermissionAllowedPrincipal")`).
fn grant_cases() -> Vec<GrantCase> {
    vec![
        // "An account ID": that account's root principal, written as its ARN.
        GrantCase {
            name: "one account",
            layer: "account-grant",
            versions: 1,
            reference: "account-grant".to_string(),
            version: 1,
            request: json!({
                "StatementId": "one-account",
                "Action": ACTION,
                "Principal": "111122223333",
            }),
            expected_statement: json!({
                "Sid": "one-account",
                "Effect": "Allow",
                "Principal": { "AWS": "arn:aws:iam::111122223333:root" },
                "Action": ACTION,
                "Resource": layer_version_arn("account-grant", 1),
            }),
        },
        // "* to grant layer usage permission to all accounts": every principal, which
        // the policy language writes as the bare string, and nothing restricts it.
        GrantCase {
            name: "every account",
            layer: "public-layer",
            versions: 1,
            reference: "public-layer".to_string(),
            version: 1,
            request: json!({
                "StatementId": "everyone",
                "Action": ACTION,
                "Principal": "*",
            }),
            expected_statement: json!({
                "Sid": "everyone",
                "Effect": "Allow",
                "Principal": "*",
                "Action": ACTION,
                "Resource": layer_version_arn("public-layer", 1),
            }),
        },
        // "With the principal set to *, grant permission to all accounts in the
        // specified organization": the organization is the condition that narrows the
        // wildcard.
        GrantCase {
            name: "every account in one organization",
            layer: "org-layer",
            versions: 1,
            reference: "org-layer".to_string(),
            version: 1,
            request: json!({
                "StatementId": "whole-org",
                "Action": ACTION,
                "Principal": "*",
                "OrganizationId": "o-a1b2c3d4e5",
            }),
            expected_statement: json!({
                "Sid": "whole-org",
                "Effect": "Allow",
                "Principal": "*",
                "Action": ACTION,
                "Resource": layer_version_arn("org-layer", 1),
                "Condition": {
                    "StringEquals": { "aws:PrincipalOrgID": "o-a1b2c3d4e5" },
                },
            }),
        },
        // An IAM root ARN is already an ARN and is used as given.
        GrantCase {
            name: "an account root ARN",
            layer: "root-arn-layer",
            versions: 1,
            reference: "root-arn-layer".to_string(),
            version: 1,
            request: json!({
                "StatementId": "root_arn-1",
                "Action": ACTION,
                "Principal": "arn:aws:iam::444455556666:root",
            }),
            expected_statement: json!({
                "Sid": "root_arn-1",
                "Effect": "Allow",
                "Principal": { "AWS": "arn:aws:iam::444455556666:root" },
                "Action": ACTION,
                "Resource": layer_version_arn("root-arn-layer", 1),
            }),
        },
        // A later version, addressed by the layer's ARN rather than its name: both forms
        // name the same layer, and the statement is granted on the version addressed.
        GrantCase {
            name: "the third version named by the layer ARN",
            layer: "multi-version",
            versions: 3,
            reference: layer_arn("multi-version"),
            version: 3,
            request: json!({
                "StatementId": "late-version",
                "Action": ACTION,
                "Principal": "123456789012",
            }),
            expected_statement: json!({
                "Sid": "late-version",
                "Effect": "Allow",
                "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
                "Action": ACTION,
                "Resource": layer_version_arn("multi-version", 3),
            }),
        },
    ]
}

/// `AddLayerVersionPermission` answers the statement it adds, and
/// `GetLayerVersionPolicy` reports the policy it was added to.
#[tokio::test]
async fn add_layer_version_permission_writes_the_grant_the_request_describes() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in grant_cases() {
        publish_versions(&router, case.layer, case.versions).await;

        let answer =
            add_permission(&router, &case.reference, case.version, &case.request, None).await;
        assert_eq!(
            answer.status,
            201,
            "case {:?}: expected HTTP 201 from AddLayerVersionPermission, got {} {}",
            case.name,
            answer.status,
            answer.text()
        );
        let statement = statement_of(case.name, &answer);
        assert_eq!(
            statement, case.expected_statement,
            "case {:?}: expected the statement {}, got {statement}",
            case.name, case.expected_statement
        );
        let minted = answer.string("RevisionId").unwrap_or_default();
        assert!(
            !minted.is_empty(),
            "case {:?}: expected AddLayerVersionPermission to report a RevisionId, got {}",
            case.name,
            answer.text()
        );

        // The grant is readable back: the policy of that version holds exactly the one
        // statement, at the revision the call reported.
        let read = get_policy(&router, &case.reference, case.version).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected HTTP 200 from GetLayerVersionPolicy, got {} {}",
            case.name,
            read.status,
            read.text()
        );
        let policy = policy_of(case.name, &read);
        assert_eq!(
            policy,
            document(vec![case.expected_statement.clone()]),
            "case {:?}: expected the policy document to hold just that statement, got {policy}",
            case.name
        );
        assert_eq!(
            read.string("RevisionId"),
            Some(minted.clone()),
            "case {:?}: expected the reported revision {minted:?}, got {}",
            case.name,
            read.text()
        );

        // A report must not change what a later caller sees.
        let again = get_policy(&router, &case.reference, case.version).await;
        assert_eq!(
            (
                again.status,
                policy_of(case.name, &again),
                again.string("RevisionId")
            ),
            (read.status, policy.clone(), Some(minted)),
            "case {:?}: reading the policy changed it: {} {}",
            case.name,
            again.status,
            again.text()
        );
    }

    // A grant is attached to the single version it addressed, so the sibling versions of
    // the multi-version layer hold no policy at all.
    for version in [1, 2] {
        let sibling = get_policy(&router, "multi-version", version).await;
        assert_eq!(
            sibling.status,
            404,
            "case {:?}: expected no policy on a version nothing was granted on, got {} {}",
            format!("multi-version:{version}"),
            sibling.status,
            sibling.text()
        );
    }
}

/// Statements accumulate on the version they were granted on, and each change mints a
/// new revision.
#[tokio::test]
async fn statements_accumulate_on_the_version_and_each_change_mints_a_revision() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    publish_versions(&router, "fan-out", 2).await;

    // Three grants differing in the form of their principal, which is what accumulates.
    let grants = [
        ("first-account", json!("111122223333")),
        ("every-account", json!("*")),
        ("root-arn", json!("arn:aws:iam::444455556666:root")),
    ];
    let expected_principals = [
        json!({ "AWS": "arn:aws:iam::111122223333:root" }),
        json!("*"),
        json!({ "AWS": "arn:aws:iam::444455556666:root" }),
    ];

    let mut expected: Vec<Value> = Vec::new();
    let mut revisions: Vec<String> = Vec::new();
    for ((sid, principal), rendered) in grants.iter().zip(expected_principals.iter()) {
        let answer = add_permission(
            &router,
            "fan-out",
            1,
            &json!({
                "StatementId": sid,
                "Action": ACTION,
                "Principal": principal,
            }),
            None,
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {sid:?}: expected HTTP 201, got {} {}",
            answer.status,
            answer.text()
        );
        expected.push(json!({
            "Sid": sid,
            "Effect": "Allow",
            "Principal": rendered,
            "Action": ACTION,
            "Resource": layer_version_arn("fan-out", 1),
        }));

        let read = get_policy(&router, "fan-out", 1).await;
        let policy = policy_of(sid, &read);
        assert_eq!(
            policy,
            document(expected.clone()),
            "case {sid:?}: expected the policy to hold the {} statements added so far, got \
             {policy}",
            expected.len()
        );
        let revision = read.string("RevisionId").unwrap_or_default();
        assert!(
            !revisions.contains(&revision),
            "case {sid:?}: expected a new RevisionId after a change, got the earlier \
             {revision:?}"
        );
        revisions.push(revision);
    }

    // The statements live on the stored layer version, not in some per-call scratch
    // state, and version 2 holds none of them.
    let held = engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .layer(REGION, "fan-out")
        .map(|record| {
            record
                .versions
                .iter()
                .map(|(version, stored)| {
                    (
                        *version,
                        stored
                            .policy
                            .as_ref()
                            .map_or(0, |policy| policy.statements.len()),
                    )
                })
                .collect::<Vec<_>>()
        });
    assert_eq!(
        held,
        Some(vec![(1, 3), (2, 0)]),
        "expected the store to hold three statements on version 1 and none on version 2, got \
         {held:?}"
    );
}

/// The published quota on a resource-based policy, in bytes (Lambda quotas, "Function
/// resource-based policy": 20 KB), which is what `PolicyLengthExceededException` ("The
/// permissions policy for the resource is too large") refuses a call past.
const MAX_POLICY_SIZE_BYTES: usize = 20_480;

/// A grant is refused once the document it would make is past the published quota, and
/// the size is measured on the rendered document rather than on the request.
#[tokio::test]
async fn a_grant_past_the_policy_size_quota_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    publish_versions(&router, "padded", 1).await;

    // Statement ids at the model's `StatementId` maximum length of 100, so the document
    // reaches the quota in a bounded number of grants (each statement renders to a few
    // hundred bytes). The loop is capped well above that number and the assertion below
    // fails if the quota was never reached.
    let mut accepted = 0usize;
    let mut refused: Option<Answer> = None;
    for index in 0..200u32 {
        let sid = format!("padding-statement-{index:0>82}");
        assert_eq!(
            sid.len(),
            100,
            "case {sid:?}: the padded statement id must be at the modelled maximum of 100, \
             got {}",
            sid.len()
        );
        let answer = add_permission(
            &router,
            "padded",
            1,
            &json!({
                "StatementId": sid,
                "Action": ACTION,
                "Principal": "111122223333",
            }),
            None,
        )
        .await;
        if answer.status == 201 {
            accepted += 1;
            continue;
        }
        refused = Some(answer);
        break;
    }

    let answer = refused.unwrap_or_else(|| {
        panic!("expected the policy quota to be reached within 200 grants, {accepted} accepted")
    });
    assert_eq!(
        answer.status,
        400,
        "case {:?}: expected HTTP 400 once the document is past the quota, got {} {}",
        "a grant past the quota",
        answer.status,
        answer.text()
    );
    assert!(
        answer.error_code().contains("PolicyLengthExceededException"),
        "case {:?}: expected the error code PolicyLengthExceededException, got {}",
        "a grant past the quota",
        answer.error_code()
    );

    // The refused grant was not stored: the document holds exactly the accepted
    // statements, and it is within the quota, so the size was measured on the rendered
    // document.
    let read = get_policy(&router, "padded", 1).await;
    let policy = policy_of("a grant past the quota", &read);
    let held = policy
        .pointer("/Statement")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        held, accepted,
        "case {:?}: expected the {accepted} accepted statements to be held, got {held}",
        "a grant past the quota"
    );
    let rendered = read.string("Policy").unwrap_or_default().len();
    assert!(
        rendered <= MAX_POLICY_SIZE_BYTES,
        "case {:?}: expected the stored document to stay within {MAX_POLICY_SIZE_BYTES} bytes, \
         got {rendered}",
        "a grant past the quota"
    );
    assert!(
        accepted > 1,
        "case {:?}: expected more than one grant to fit inside the quota, got {accepted}",
        "a grant past the quota"
    );
}

/// One row of the refusal table: the call to make, and the status and wire error code it
/// must answer.
struct RefusalCase {
    name: &'static str,
    reference: String,
    version: i64,
    body: Value,
    /// The `RevisionId` precondition the call carries, when any.
    revision: Option<String>,
    status: u16,
    code: &'static str,
}

/// Every refusal the grant makes, each from the state the store holds.
#[tokio::test]
async fn a_grant_is_refused_from_the_state_the_store_holds() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    publish_versions(&router, "guarded", 2).await;

    let granted = add_permission(
        &router,
        "guarded",
        1,
        &json!({
            "StatementId": "held",
            "Action": ACTION,
            "Principal": "111122223333",
        }),
        None,
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "expected the first grant to be accepted, got {} {}",
        granted.status,
        granted.text()
    );
    let revision = granted.string("RevisionId").unwrap_or_default();
    let expected_document = document(vec![json!({
        "Sid": "held",
        "Effect": "Allow",
        "Principal": { "AWS": "arn:aws:iam::111122223333:root" },
        "Action": ACTION,
        "Resource": layer_version_arn("guarded", 1),
    })]);

    let fresh_grant = |sid: &str| {
        json!({
            "StatementId": sid,
            "Action": ACTION,
            "Principal": "*",
        })
    };

    let cases = vec![
        // "An identifier that distinguishes the policy from others on the same layer
        // version": one the version already carries names an existing grant.
        RefusalCase {
            name: "a statement id the version already holds",
            reference: "guarded".to_string(),
            version: 1,
            body: fresh_grant("held"),
            revision: None,
            status: 409,
            code: "ResourceConflictException",
        },
        // "Only update the policy if the revision ID matches the ID specified."
        RefusalCase {
            name: "a revision that is not the policy's",
            reference: "guarded".to_string(),
            version: 1,
            body: fresh_grant("stale-revision"),
            revision: Some("00000000-0000-4000-8000-000000000000".to_string()),
            status: 412,
            code: "PreconditionFailedException",
        },
        // A version with no policy is at no revision, so any expectation fails against
        // it.
        RefusalCase {
            name: "a revision expected of a version with no policy",
            reference: "guarded".to_string(),
            version: 2,
            body: fresh_grant("no-policy-yet"),
            revision: Some(revision.clone()),
            status: 412,
            code: "PreconditionFailedException",
        },
        RefusalCase {
            name: "a layer this account never published",
            reference: "absent-layer".to_string(),
            version: 1,
            body: fresh_grant("unknown-layer"),
            revision: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "a version this layer never had",
            reference: layer_arn("guarded"),
            version: 99,
            body: fresh_grant("unknown-version"),
            revision: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        // "With the principal set to *, grant permission to all accounts in the
        // specified organization": an organization narrows the wildcard principal, so it
        // cannot be named beside a single account.
        RefusalCase {
            name: "an organization beside a single account principal",
            reference: "guarded".to_string(),
            version: 1,
            body: json!({
                "StatementId": "org-and-account",
                "Action": ACTION,
                "Principal": "111122223333",
                "OrganizationId": "o-a1b2c3d4e5",
            }),
            revision: None,
            status: 400,
            code: "InvalidParameterValueException",
        },
    ];

    for case in &cases {
        let answer = add_permission(
            &router,
            &case.reference,
            case.version,
            &case.body,
            case.revision.as_deref(),
        )
        .await;
        assert_eq!(
            answer.status,
            case.status,
            "case {:?}: expected HTTP {}, got {} {}",
            case.name,
            case.status,
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(case.code),
            "case {:?}: expected the error code {}, got {}",
            case.name,
            case.code,
            answer.error_code()
        );

        // A refused call changes nothing: the one statement held is still the only one,
        // at the revision the accepted grant minted.
        let read = get_policy(&router, "guarded", 1).await;
        assert_eq!(
            (
                read.status,
                policy_of(case.name, &read),
                read.string("RevisionId")
            ),
            (200, expected_document.clone(), Some(revision.clone())),
            "case {:?}: a refused grant changed the stored policy: {} {}",
            case.name,
            read.status,
            read.text()
        );
        // And nothing was created on the version that holds no policy.
        let empty = get_policy(&router, "guarded", 2).await;
        assert_eq!(
            empty.status,
            404,
            "case {:?}: a refused grant created a policy on version 2: {} {}",
            case.name,
            empty.status,
            empty.text()
        );
    }
}

/// One row of the removal table: the statement to remove, the precondition to carry, and
/// what the policy must hold afterwards.
struct RemovalCase {
    name: &'static str,
    reference: String,
    version: i64,
    statement_id: &'static str,
    /// Whether the call carries the policy's current revision as its precondition.
    with_current_revision: bool,
    /// A precondition that is not the policy's revision, when the row carries one.
    stale_revision: Option<&'static str>,
    status: u16,
    /// The statement ids the version's policy must hold afterwards, in order; empty
    /// means the version must hold no policy at all.
    remaining: Vec<&'static str>,
}

/// `RemoveLayerVersionPermission` takes out exactly the statement it names, and refuses
/// from the state the store holds.
#[tokio::test]
async fn remove_layer_version_permission_takes_out_exactly_the_named_statement() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    publish_versions(&router, "pruned", 2).await;

    for sid in ["first", "second", "third"] {
        let answer = add_permission(
            &router,
            "pruned",
            1,
            &json!({
                "StatementId": sid,
                "Action": ACTION,
                "Principal": "*",
            }),
            None,
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {sid:?}: expected the grant to be accepted, got {} {}",
            answer.status,
            answer.text()
        );
    }

    let cases = vec![
        // A precondition that is not the policy's revision refuses the removal, and the
        // policy keeps every statement.
        RemovalCase {
            name: "a revision that is not the policy's",
            reference: "pruned".to_string(),
            version: 1,
            statement_id: "second",
            with_current_revision: false,
            stale_revision: Some("00000000-0000-4000-8000-000000000000"),
            status: 412,
            remaining: vec!["first", "second", "third"],
        },
        // "The identifier that was specified when the statement was added": one that was
        // never added names no statement.
        RemovalCase {
            name: "a statement id the policy does not hold",
            reference: "pruned".to_string(),
            version: 1,
            statement_id: "never-added",
            with_current_revision: false,
            stale_revision: None,
            status: 404,
            remaining: vec!["first", "second", "third"],
        },
        RemovalCase {
            name: "a version that holds no policy",
            reference: "pruned".to_string(),
            version: 2,
            statement_id: "first",
            with_current_revision: false,
            stale_revision: None,
            status: 404,
            remaining: vec!["first", "second", "third"],
        },
        // The middle statement, with no precondition: exactly it goes, and the order of
        // the rest is kept.
        RemovalCase {
            name: "the middle statement",
            reference: "pruned".to_string(),
            version: 1,
            statement_id: "second",
            with_current_revision: false,
            stale_revision: None,
            status: 204,
            remaining: vec!["first", "third"],
        },
        // The first statement, with the policy's current revision as its precondition,
        // and the layer named by its ARN.
        RemovalCase {
            name: "the first statement at the current revision, by layer ARN",
            reference: layer_arn("pruned"),
            version: 1,
            statement_id: "first",
            with_current_revision: true,
            stale_revision: None,
            status: 204,
            remaining: vec!["third"],
        },
        // The last statement left: a policy with no statements is no policy at all.
        RemovalCase {
            name: "the last statement left",
            reference: "pruned".to_string(),
            version: 1,
            statement_id: "third",
            with_current_revision: true,
            stale_revision: None,
            status: 204,
            remaining: vec![],
        },
    ];

    for case in &cases {
        let before = get_policy(&router, "pruned", 1).await;
        let current = before.string("RevisionId");
        let revision = if case.with_current_revision {
            current.clone()
        } else {
            case.stale_revision.map(str::to_string)
        };

        let answer = remove_permission(
            &router,
            &case.reference,
            case.version,
            case.statement_id,
            revision.as_deref(),
        )
        .await;
        assert_eq!(
            answer.status,
            case.status,
            "case {:?}: expected HTTP {}, got {} {}",
            case.name,
            case.status,
            answer.status,
            answer.text()
        );

        let read = get_policy(&router, "pruned", 1).await;
        if case.remaining.is_empty() {
            assert_eq!(
                read.status,
                404,
                "case {:?}: expected the version to hold no policy, got {} {}",
                case.name,
                read.status,
                read.text()
            );
            continue;
        }
        let policy = policy_of(case.name, &read);
        let held: Vec<String> = policy
            .pointer("/Statement")
            .and_then(Value::as_array)
            .map(|statements| {
                statements
                    .iter()
                    .filter_map(|statement| statement.get("Sid"))
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            held,
            case.remaining
                .iter()
                .map(|sid| (*sid).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the policy to hold {:?}, got {policy}",
            case.name,
            case.remaining
        );
        let after = read.string("RevisionId");
        if case.status == 204 {
            assert_ne!(
                after, current,
                "case {:?}: expected a new RevisionId after a change, got {after:?}",
                case.name
            );
        } else {
            assert_eq!(
                after, current,
                "case {:?}: a refused removal changed the policy's revision, got {after:?}",
                case.name
            );
        }
    }

    // Removing every permission leaves the layer version itself exactly where it was.
    let version = call(
        &router,
        "GET",
        "/2018-10-31/layers/pruned/versions/1",
        &Value::Null,
    )
    .await;
    assert_eq!(
        version.status,
        200,
        "expected the layer version to survive the removal of its permissions, got {} {}",
        version.status,
        version.text()
    );
    assert_eq!(
        version.string("LayerVersionArn"),
        Some(layer_version_arn("pruned", 1)),
        "expected the layer version to still report its own ARN, got {}",
        version.text()
    );
}
