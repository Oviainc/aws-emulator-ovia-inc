// SPDX-License-Identifier: CC0-1.0

//! `BatchGetRepositoryScanningConfiguration`, end to end over the wire.
//!
//! The rule under test: the operation answers one entry per requested repository name,
//! in the order asked for, computed from the stored repository — its ARN, its name, the
//! `scanOnPush` flag its `imageScanningConfiguration` carries (absent means `false`),
//! the frequency that follows from that flag (`SCAN_ON_PUSH` when it is on, `MANUAL`
//! when it is not), and the registry scanning rules' filters that apply to it (none,
//! while no operation configures a registry rule) — and reports a name the registry
//! does not hold in `failures` with the `REPOSITORY_NOT_FOUND` code instead of failing
//! the whole call.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::at;

/// A SigV4 credential scope naming this service. It is what routes a request to ECR,
/// and its region is what a repository ARN is built for.
const SIGNED_FOR_AP_SOUTH_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/ap-south-1/ecr/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// The operation under test.
const OPERATION: &str = "BatchGetRepositoryScanningConfiguration";

/// `ScanningConfigurationRepositoryNameList` is modelled `@length(min: 1, max: 25)`, so
/// 25 names is the largest batch this operation accepts and 1 the smallest.
const MAX_BATCH: usize = 25;

/// One row of the configuration table: the case name, the `CreateRepository` request,
/// and the `scanOnPush` flag and `scanFrequency` the stored repository must report.
type ConfigurationCase<'a> = (&'a str, Value, bool, &'a str);

/// One row of the failure table: the case name, the names asked for, the names a
/// configuration must come back for, and the names that must be reported as failures.
type FailureCase<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, Vec<&'a str>);

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

/// Call one operation of this service on the awsJson1_1 wire: `POST /` with
/// `X-Amz-Target: <prefix>.<operation>` and a JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "api.ecr.ap-south-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AP_SOUTH_1)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A successful call, or a panic naming the case and what came back instead.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The batch read of `names`, as a successful body.
async fn scanning_configurations(
    router: &MultiServiceRouter,
    case: &str,
    names: &[String],
) -> Value {
    ok(router, case, OPERATION, &json!({"repositoryNames": names})).await
}

/// The repository names, in order, that a body reports a configuration for.
fn configured_names(body: &Value) -> Vec<String> {
    at(body, "/scanningConfigurations")
        .as_array()
        .unwrap_or_else(|| panic!("{OPERATION} must return a scanningConfigurations list: {body:?}"))
        .iter()
        .map(|entry| entry["repositoryName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every stored repository reports the scanning configuration its own record implies,
/// and the batch answers in the order it was asked.
///
/// The rows differ along what the rule generalises over: whether the create named an
/// `imageScanningConfiguration` at all, which way its flag was set, and the shape of the
/// repository name the configuration is keyed by.
#[tokio::test]
async fn a_stored_repository_reports_the_scanning_configuration_it_implies() {
    let cases: Vec<ConfigurationCase> = vec![
        (
            "no imageScanningConfiguration at all",
            json!({"repositoryName": "plain-app"}),
            false,
            "MANUAL",
        ),
        (
            "scan on push switched off explicitly",
            json!({
                "repositoryName": "ab",
                "imageScanningConfiguration": {"scanOnPush": false},
            }),
            false,
            "MANUAL",
        ),
        (
            "scan on push switched on",
            json!({
                "repositoryName": "scanned-app",
                "imageScanningConfiguration": {"scanOnPush": true},
            }),
            true,
            "SCAN_ON_PUSH",
        ),
        (
            "scan on push on, under a namespaced name",
            json!({
                "repositoryName": "project-a/nginx-web-app",
                "imageScanningConfiguration": {"scanOnPush": true},
            }),
            true,
            "SCAN_ON_PUSH",
        ),
        (
            "scan on push off, with every separator the name pattern allows",
            json!({
                "repositoryName": "team.a_b__c--d/inner-1",
                "imageScanningConfiguration": {"scanOnPush": false},
                "imageTagMutability": "IMMUTABLE",
                "tags": [{"Key": "env", "Value": "test"}],
            }),
            false,
            "MANUAL",
        ),
    ];

    let router = emulator();
    let mut names = Vec::new();
    let mut arns = Vec::new();

    for (case, request, scan_on_push, frequency) in &cases {
        let name = request["repositoryName"].as_str().unwrap().to_string();
        let created = ok(&router, case, "CreateRepository", request).await;
        let arn = at(&created, "/repository/repositoryArn");

        // Read back one name at a time: the configuration is the stored repository's.
        let single = scanning_configurations(&router, case, std::slice::from_ref(&name)).await;
        let entry = at(&single, "/scanningConfigurations/0");
        assert_eq!(
            at(&single, "/failures"),
            json!([]),
            "case {case:?}: a name the registry holds is no failure, got {:?}",
            at(&single, "/failures")
        );
        assert_eq!(
            at(&entry, "/repositoryName"),
            json!(name),
            "case {case:?}: expected the configuration of {name:?}, got {entry:?}"
        );
        assert_eq!(
            at(&entry, "/repositoryArn"),
            arn,
            "case {case:?}: expected the ARN the create reported ({arn:?}), got {:?}",
            at(&entry, "/repositoryArn")
        );
        assert_eq!(
            at(&entry, "/scanOnPush"),
            json!(scan_on_push),
            "case {case:?}: expected scanOnPush {scan_on_push}, got {:?}",
            at(&entry, "/scanOnPush")
        );
        assert_eq!(
            at(&entry, "/scanFrequency"),
            json!(frequency),
            "case {case:?}: expected scanFrequency {frequency:?}, got {:?}",
            at(&entry, "/scanFrequency")
        );
        // No registry scanning rule exists to be applied, so no filter is.
        assert_eq!(
            at(&entry, "/appliedScanFilters"),
            json!([]),
            "case {case:?}: expected no applied scan filters, got {:?}",
            at(&entry, "/appliedScanFilters")
        );

        names.push(name);
        arns.push(arn);
    }

    // One batch over every name: one entry each, in the order asked for.
    let batch = scanning_configurations(&router, "every name at once", &names).await;
    assert_eq!(
        configured_names(&batch),
        names,
        "the batch answers one configuration per requested name, in request order, got {:?}",
        at(&batch, "/scanningConfigurations")
    );
    for (index, arn) in arns.iter().enumerate() {
        assert_eq!(
            at(&batch, &format!("/scanningConfigurations/{index}/repositoryArn")),
            *arn,
            "the batch entry at {index} must carry the ARN of the name at {index}, got {:?}",
            at(&batch, &format!("/scanningConfigurations/{index}"))
        );
    }

    // Asked in the reverse order, the answer is reversed too: the order is the request's,
    // not the store's.
    let reversed_names: Vec<String> = names.iter().rev().cloned().collect();
    let reversed = scanning_configurations(&router, "reverse order", &reversed_names).await;
    assert_eq!(
        configured_names(&reversed),
        reversed_names,
        "the batch follows the request order, got {:?}",
        at(&reversed, "/scanningConfigurations")
    );

    // The read changed nothing: asking again answers exactly the same thing.
    let again = scanning_configurations(&router, "read twice", &names).await;
    assert_eq!(
        again, batch,
        "reading the scanning configuration must not change what the next caller sees"
    );
}

/// A requested name the registry does not hold is a per-entry failure, not a refusal of
/// the whole call — and the names that do resolve are still answered.
#[tokio::test]
async fn a_name_the_registry_does_not_hold_is_reported_as_a_failure() {
    let router = emulator();
    for name in ["present-one", "present-two"] {
        ok(
            &router,
            name,
            "CreateRepository",
            &json!({
                "repositoryName": name,
                "imageScanningConfiguration": {"scanOnPush": true},
            }),
        )
        .await;
    }

    // Rows differ in how many names are missing, where they sit in the request, and the
    // shape of the missing name.
    let cases: Vec<FailureCase> = vec![
        (
            "a single missing name",
            vec!["absent-app"],
            vec![],
            vec!["absent-app"],
        ),
        (
            "missing first, present second",
            vec!["absent-app", "present-one"],
            vec!["present-one"],
            vec!["absent-app"],
        ),
        (
            "present, missing, present",
            vec!["present-one", "team-x/absent", "present-two"],
            vec!["present-one", "present-two"],
            vec!["team-x/absent"],
        ),
        (
            "every name missing",
            vec!["ax", "absent.two", "absent__three"],
            vec![],
            vec!["ax", "absent.two", "absent__three"],
        ),
    ];

    for (case, asked, expected_configured, expected_failed) in cases {
        let asked: Vec<String> = asked.into_iter().map(ToString::to_string).collect();
        let body = scanning_configurations(&router, case, &asked).await;

        assert_eq!(
            configured_names(&body),
            expected_configured,
            "case {case:?}: expected configurations for {expected_configured:?}, got {:?}",
            at(&body, "/scanningConfigurations")
        );

        let failures = at(&body, "/failures");
        let failures = failures
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a failures list, got {body:?}"));
        assert_eq!(
            failures.len(),
            expected_failed.len(),
            "case {case:?}: expected {} failures, got {failures:?}",
            expected_failed.len()
        );
        for (failure, name) in failures.iter().zip(&expected_failed) {
            assert_eq!(
                at(failure, "/repositoryName"),
                json!(name),
                "case {case:?}: expected the failure to name {name:?}, got {failure:?}"
            );
            // `ScanningConfigurationFailureCode` carries exactly one value.
            assert_eq!(
                at(failure, "/failureCode"),
                json!("REPOSITORY_NOT_FOUND"),
                "case {case:?}: expected the REPOSITORY_NOT_FOUND code, got {failure:?}"
            );
            assert_eq!(
                at(failure, "/failureReason"),
                json!("REPOSITORY_NOT_FOUND"),
                "case {case:?}: expected a failure reason, got {failure:?}"
            );
        }

        // Nothing was created by being asked for: the missing names are still missing.
        for name in &expected_failed {
            let described = call(
                &router,
                "DescribeRepositories",
                &json!({"repositoryNames": [name]}),
            )
            .await;
            assert_eq!(
                described.error_code(),
                "RepositoryNotFoundException",
                "case {case:?}: reading a scanning configuration must not create {name:?}, \
                 got {:?}",
                described.body
            );
        }
    }
}

/// The batch is bounded by the model: `repositoryNames` holds between 1 and 25 names,
/// and a request outside that is refused at the boundary rather than served or 501'd.
#[tokio::test]
async fn the_batch_size_bounds_are_enforced() {
    let router = emulator();
    let names: Vec<String> = (0..MAX_BATCH + 1)
        .map(|index| format!("bound-{index:02}"))
        .collect();
    for name in &names {
        ok(
            &router,
            name,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;
    }

    // Each published bound, and one past it.
    for size in [1usize, MAX_BATCH] {
        let asked = &names[..size];
        let body = scanning_configurations(&router, &format!("{size} names"), asked).await;
        assert_eq!(
            configured_names(&body),
            asked.to_vec(),
            "case {size} names: expected one configuration per name, got {:?}",
            at(&body, "/scanningConfigurations")
        );
    }

    for (case, asked) in [
        ("no names at all", &names[..0]),
        ("one name past the maximum", &names[..MAX_BATCH + 1]),
    ] {
        let answer = call(&router, OPERATION, &json!({"repositoryNames": asked})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        // A refusal against a constraint the model declares is stated with the
        // client-input error this service models — `InvalidParameterException`, which
        // this operation declares — naming the member and the bound it failed. See
        // `override_refusal` in `src/service.rs`.
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.body
        );
        assert!(
            answer.body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("at 'repositoryNames' failed to satisfy constraint:"),
            "case {case:?}: the message must name the member and the bound, got {:?}",
            answer.body
        );
    }
}
