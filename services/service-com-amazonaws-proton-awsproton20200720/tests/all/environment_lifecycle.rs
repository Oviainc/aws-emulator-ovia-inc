// SPDX-License-Identifier: CC0-1.0

//! The environment-template + environment lifecycle, driven end to end over the wire.
//!
//! Every case here goes through the router the production emulator mounts, so it
//! exercises the real awsJson1_0 envelope (`POST /` with
//! `X-Amz-Target: AwsProton20200720.<Operation>`), the modelled-constraint validator and
//! the handlers' own state, not a Rust-level shortcut into them.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_proton_awsproton20200720::EmulatedService;

/// The region every request here is signed for, so the ARNs the service mints are
/// checked against a region that is NOT the fallback one.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id, the account segment of every minted ARN.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Comfortably past `state::DEPLOYMENT_SETTLE_DELAY`, so a clock-owed transition is
/// certainly due by the next call.
const PAST_THE_SETTLE_DELAY: Duration = Duration::from_millis(900);

/// One emulator with one Proton service mounted on it.
struct Proton {
    router: MultiServiceRouter,
}

impl Proton {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise operation behaviour, not signature verification.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return its HTTP status and JSON body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AwsProton20200720.{operation}"))
            // The SigV4 credential scope is what routes the request to `proton` and
            // what names the region the resource is created in.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/proton/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, body)
    }

    /// Call an operation that must succeed, returning its body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, body) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {status}: {body}");
        body
    }

    /// Create an environment template of `name` with no optional members.
    async fn template(&self, name: &str) -> Value {
        self.ok("CreateEnvironmentTemplate", json!({ "name": name })).await
    }
}

/// The modelled error shape a failure names, as the awsJson `__type` carries it (the
/// namespace prefix, when present, is dropped).
fn error_code(body: &Value) -> &str {
    let rendered = body.get("__type").and_then(Value::as_str).unwrap_or("");
    match rendered.rsplit_once('#') {
        Some((_, shape)) => shape,
        None => rendered,
    }
}

fn names_of(list: &Value) -> Vec<String> {
    list.as_array()
        .expect("a list of summaries")
        .iter()
        .map(|item| {
            item.get("name")
                .and_then(Value::as_str)
                .expect("every summary names its environment")
                .to_string()
        })
        .collect()
}

/// A create answers with the record it stored, and a later get answers with the SAME
/// record — for every combination of the optional members the input carries.
#[tokio::test]
async fn an_environment_template_round_trips_every_stored_member() {
    // One row per optional member of `CreateEnvironmentTemplateInput`
    // (`smithy_query(query="operation",operation="CreateEnvironmentTemplate")`:
    // displayName, description, encryptionKey, provisioning, tags), plus the minimal
    // request and one carrying all of them.
    let cases = [
        ("minimal", json!({ "name": "minimal" })),
        (
            "displayName only",
            json!({ "name": "displayOnly", "displayName": "Display Only" }),
        ),
        (
            "empty description (the modelled length minimum is 0)",
            json!({ "name": "emptyDescription", "description": "" }),
        ),
        (
            "encryptionKey only",
            json!({
                "name": "encrypted",
                "encryptionKey": "arn:aws:kms:eu-west-1:000000000000:key/abc",
            }),
        ),
        (
            "provisioning only",
            json!({ "name": "customerManaged", "provisioning": "CUSTOMER_MANAGED" }),
        ),
        (
            "tags only",
            json!({ "name": "tagged", "tags": [{ "key": "team", "value": "platform" }] }),
        ),
        (
            "every optional member",
            json!({
                "name": "everything_1-A",
                "displayName": "Everything",
                "description": "all of the optional members at once",
                "encryptionKey": "arn:aws:kms:eu-west-1:000000000000:key/def",
                "provisioning": "CUSTOMER_MANAGED",
                "tags": [{ "key": "a", "value": "1" }, { "key": "b", "value": "2" }],
            }),
        ),
    ];

    let proton = Proton::new();
    for (case, input) in cases {
        let name = input["name"].as_str().expect("the case names a template");
        let created = proton.ok("CreateEnvironmentTemplate", input.clone()).await;
        let created = &created["environmentTemplate"];

        assert_eq!(
            created["arn"].as_str(),
            Some(format!("arn:aws:proton:{REGION}:{ACCOUNT}:environment-template/{name}").as_str()),
            "case {case:?}: expected the template's regional ARN, got {:?}",
            created["arn"]
        );
        for required in ["name", "arn", "createdAt", "lastModifiedAt"] {
            assert!(
                created.get(required).is_some(),
                "case {case:?}: expected the required member {required}, got {created:?}"
            );
        }
        for (member, expected) in [
            ("displayName", input.get("displayName")),
            ("description", input.get("description")),
            ("encryptionKey", input.get("encryptionKey")),
            ("provisioning", input.get("provisioning")),
        ] {
            assert_eq!(
                created.get(member),
                expected,
                "case {case:?}: member {member} must answer what was stored, got {:?}",
                created.get(member)
            );
        }
        assert!(
            created.get("recommendedVersion").is_none(),
            "case {case:?}: a template with no published version has no recommended \
             version, got {:?}",
            created.get("recommendedVersion")
        );

        let fetched = proton
            .ok("GetEnvironmentTemplate", json!({ "name": name }))
            .await;
        assert_eq!(
            &fetched["environmentTemplate"], created,
            "case {case:?}: the get must answer the stored record, got {:?}",
            fetched["environmentTemplate"]
        );
    }
}

/// A template's name is its identity: the second create of a name conflicts, whatever
/// form the name takes.
#[tokio::test]
async fn creating_an_environment_template_twice_conflicts() {
    // Rows differ along the shape of the name the modelled `ResourceName` pattern
    // `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$` admits: one character, digits, underscores and
    // hyphens, and the 100-character maximum.
    let cases = [
        ("single character", "a".to_string()),
        ("leading digit", "9lives".to_string()),
        ("underscore and hyphen", "my_env-tmpl".to_string()),
        ("the 100-character maximum", "x".repeat(100)),
    ];

    let proton = Proton::new();
    for (case, name) in cases {
        proton.template(&name).await;
        let (status, body) = proton
            .call("CreateEnvironmentTemplate", json!({ "name": name }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a duplicate template name must be refused 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    }
}

/// An environment can only be deployed from a template that is stored, so a
/// `templateName` no template carries is an absent resource.
#[tokio::test]
async fn an_environment_requires_a_stored_template() {
    let proton = Proton::new();
    proton.template("stored").await;

    // Rows differ along HOW the named template is missing: never created, a near-miss
    // of a stored name, and a name whose case differs from a stored one.
    let cases = [
        ("never created", "absent"),
        ("near miss", "store"),
        ("different case", "Stored"),
    ];
    for (case, template_name) in cases {
        let (status, body) = proton
            .call(
                "CreateEnvironment",
                json!({
                    "name": "env",
                    "templateName": template_name,
                    "templateMajorVersion": "1",
                    "spec": "proton: EnvironmentSpec\n",
                }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: an absent template must be refused 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // And the stored one is accepted, so the rows above fail for the stated reason.
    proton
        .ok(
            "CreateEnvironment",
            json!({
                "name": "env",
                "templateName": "stored",
                "templateMajorVersion": "1",
                "spec": "proton: EnvironmentSpec\n",
            }),
        )
        .await;
}

/// CreateEnvironment answers with the record it stored, and GetEnvironment answers with
/// the same record — including the `spec` and `description` the caller submitted.
#[tokio::test]
async fn an_environment_round_trips_every_stored_member() {
    // Rows differ along the inputs the rule generalises over: the optional members
    // present, the template version parts (including the `0` the modelled pattern
    // `^(0|([1-9]{1}\d*))$` admits), and the size of the `spec` (1 character up to the
    // modelled 51200 maximum).
    let cases = [
        (
            "minimal, no minor version",
            json!({
                "name": "minimal",
                "templateName": "tmpl",
                "templateMajorVersion": "1",
                "spec": "x",
            }),
        ),
        (
            "major version zero",
            json!({
                "name": "majorZero",
                "templateName": "tmpl",
                "templateMajorVersion": "0",
                "spec": "proton: EnvironmentSpec\n",
            }),
        ),
        (
            "explicit minor version",
            json!({
                "name": "minor_7",
                "templateName": "tmpl",
                "templateMajorVersion": "12",
                "templateMinorVersion": "7",
                "spec": "proton: EnvironmentSpec\n",
            }),
        ),
        (
            "description and role ARNs",
            json!({
                "name": "withRoles",
                "templateName": "tmpl",
                "templateMajorVersion": "1",
                "templateMinorVersion": "0",
                "description": "dev env",
                "spec": "proton: EnvironmentSpec\nspec:\n  key: value\n",
                "protonServiceRoleArn": "arn:aws:iam::000000000000:role/proton",
                "componentRoleArn": "arn:aws:iam::000000000000:role/component",
                "codebuildRoleArn": "arn:aws:iam::000000000000:role/codebuild",
                "tags": [{ "key": "team", "value": "platform" }],
            }),
        ),
        (
            "the 51200-character spec maximum",
            json!({
                "name": "bigSpec",
                "templateName": "tmpl",
                "templateMajorVersion": "1",
                "spec": "y".repeat(51_200),
            }),
        ),
    ];

    let proton = Proton::new();
    proton
        .ok(
            "CreateEnvironmentTemplate",
            json!({ "name": "tmpl", "provisioning": "CUSTOMER_MANAGED" }),
        )
        .await;

    for (case, input) in cases {
        let name = input["name"].as_str().expect("the case names an environment");
        let created = proton.ok("CreateEnvironment", input.clone()).await;
        let created = &created["environment"];

        assert_eq!(
            created["arn"].as_str(),
            Some(format!("arn:aws:proton:{REGION}:{ACCOUNT}:environment/{name}").as_str()),
            "case {case:?}: expected the environment's regional ARN, got {:?}",
            created["arn"]
        );
        for required in [
            "name",
            "arn",
            "createdAt",
            "lastDeploymentAttemptedAt",
            "lastDeploymentSucceededAt",
            "templateName",
            "templateMajorVersion",
            "templateMinorVersion",
            "deploymentStatus",
        ] {
            assert!(
                created.get(required).is_some(),
                "case {case:?}: expected the required member {required}, got {created:?}"
            );
        }
        for member in ["templateName", "templateMajorVersion", "spec", "description"] {
            assert_eq!(
                created.get(member),
                input.get(member),
                "case {case:?}: member {member} must answer what was stored, got {:?}",
                created.get(member)
            );
        }
        // An absent `templateMinorVersion` resolves to the first minor version of the
        // major version, and a given one is kept.
        let expected_minor = input
            .get("templateMinorVersion")
            .and_then(Value::as_str)
            .unwrap_or("0");
        assert_eq!(
            created["templateMinorVersion"].as_str(),
            Some(expected_minor),
            "case {case:?}: expected minor version {expected_minor}, got {:?}",
            created["templateMinorVersion"]
        );
        // The environment is customer-managed exactly when its template is.
        assert_eq!(
            created["provisioning"].as_str(),
            Some("CUSTOMER_MANAGED"),
            "case {case:?}: expected the template's provisioning, got {:?}",
            created["provisioning"]
        );

        let fetched = proton.ok("GetEnvironment", json!({ "name": name })).await;
        assert_eq!(
            &fetched["environment"], created,
            "case {case:?}: the get must answer the stored record, got {:?}",
            fetched["environment"]
        );
    }
}

/// An environment's name is its identity: a second create of the same name conflicts.
#[tokio::test]
async fn creating_an_environment_twice_conflicts() {
    let proton = Proton::new();
    proton.template("tmpl").await;

    // Rows differ along what the retry changes BESIDES the name: nothing, the spec, the
    // template version. None of them makes the name free.
    let cases = [
        ("identical request", json!({})),
        ("a different spec", json!({ "spec": "changed\n" })),
        ("a different major version", json!({ "templateMajorVersion": "2" })),
    ];
    for (case, overrides) in cases {
        let name = format!("env{}", case.len());
        let mut request = json!({
            "name": name,
            "templateName": "tmpl",
            "templateMajorVersion": "1",
            "spec": "proton: EnvironmentSpec\n",
        });
        proton.ok("CreateEnvironment", request.clone()).await;
        for (member, value) in overrides.as_object().expect("an object of overrides") {
            request[member] = value.clone();
        }

        let (status, body) = proton.call("CreateEnvironment", request).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a duplicate environment name must be refused 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    }
}

/// Proton answers CreateEnvironment with `IN_PROGRESS`, so the deployment's completion
/// is owed by the clock: a caller acting straight after the create still sees the
/// transient status, and once the deployment falls due every read reports `SUCCEEDED`
/// with the instant it succeeded.
#[tokio::test]
async fn an_environment_deployment_settles_from_in_progress_to_succeeded() {
    let proton = Proton::new();
    proton.template("tmpl").await;
    let created = proton
        .ok(
            "CreateEnvironment",
            json!({
                "name": "dev",
                "templateName": "tmpl",
                "templateMajorVersion": "1",
                "spec": "proton: EnvironmentSpec\n",
            }),
        )
        .await;
    let created = &created["environment"];
    assert_eq!(
        created["deploymentStatus"].as_str(),
        Some("IN_PROGRESS"),
        "a deployment that has just started is IN_PROGRESS, got {:?}",
        created["deploymentStatus"]
    );

    // Straight after the call, before the span elapses, every read still reports the
    // transient status.
    let immediately = proton.ok("GetEnvironment", json!({ "name": "dev" })).await;
    assert_eq!(
        immediately["environment"]["deploymentStatus"].as_str(),
        Some("IN_PROGRESS"),
        "the transient status must be observable by a caller acting immediately, got {:?}",
        immediately["environment"]["deploymentStatus"]
    );
    let listed = proton.ok("ListEnvironments", json!({})).await;
    assert_eq!(
        listed["environments"][0]["deploymentStatus"].as_str(),
        Some("IN_PROGRESS"),
        "the summary must report the same status as the detail, got {:?}",
        listed["environments"][0]["deploymentStatus"]
    );

    std::thread::sleep(PAST_THE_SETTLE_DELAY);

    let settled = proton.ok("GetEnvironment", json!({ "name": "dev" })).await;
    let settled = &settled["environment"];
    assert_eq!(
        settled["deploymentStatus"].as_str(),
        Some("SUCCEEDED"),
        "a deployment the clock has made due reports SUCCEEDED, got {:?}",
        settled["deploymentStatus"]
    );
    let created_at = created["createdAt"].as_f64().expect("createdAt is a timestamp");
    let succeeded_at = settled["lastDeploymentSucceededAt"]
        .as_f64()
        .expect("lastDeploymentSucceededAt is a timestamp");
    assert!(
        succeeded_at > created_at,
        "the deployment must succeed after it was created: createdAt {created_at}, \
         lastDeploymentSucceededAt {succeeded_at}"
    );
    // And the rest of the record is untouched by the transition.
    assert_eq!(
        settled["arn"], created["arn"],
        "settling must not change the environment's identity, got {:?}",
        settled["arn"]
    );

    let listed = proton.ok("ListEnvironments", json!({})).await;
    assert_eq!(
        listed["environments"][0]["lastDeploymentSucceededAt"].as_f64(),
        Some(succeeded_at),
        "the summary must report the same success instant as the detail, got {:?}",
        listed["environments"][0]["lastDeploymentSucceededAt"]
    );
}

/// `maxResults` is a real page size and `nextToken` really resumes: paging a listing to
/// its end yields every stored environment exactly once, in name order, for every
/// combination of store size and page size.
///
/// The sweep below is EXHAUSTIVE over the interesting range rather than sampled, so no
/// generator (and no seed) is involved: 0..=7 environments against page sizes 1..=8,
/// plus the absent-`maxResults` case.
#[tokio::test]
async fn listing_environments_pages_the_whole_set_exactly_once() {
    for count in 0_usize..=7 {
        for page_size in [None, Some(1), Some(2), Some(3), Some(5), Some(8), Some(100)] {
            let case = format!("{count} environments, maxResults {page_size:?}");
            let proton = Proton::new();
            proton.template("tmpl").await;
            let mut expected: Vec<String> = Vec::new();
            for index in 0..count {
                let name = format!("env{index}");
                proton
                    .ok(
                        "CreateEnvironment",
                        json!({
                            "name": name,
                            "templateName": "tmpl",
                            "templateMajorVersion": "1",
                            "spec": "proton: EnvironmentSpec\n",
                        }),
                    )
                    .await;
                expected.push(name);
            }
            expected.sort();

            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({});
                if let Some(max) = page_size {
                    request["maxResults"] = json!(max);
                }
                if let Some(token) = &next_token {
                    request["nextToken"] = json!(token);
                }
                let page = proton.ok("ListEnvironments", request).await;
                pages += 1;
                assert!(
                    pages <= count + 1,
                    "case {case:?}: the listing did not terminate after {pages} pages"
                );

                let names = names_of(&page["environments"]);
                if let Some(max) = page_size {
                    assert!(
                        names.len() <= max,
                        "case {case:?}: a page must hold at most maxResults items, got {names:?}"
                    );
                }
                seen.extend(names);
                next_token = page
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                // A token is handed back only while items remain.
                assert_eq!(
                    next_token.is_some(),
                    seen.len() < expected.len(),
                    "case {case:?}: nextToken {next_token:?} disagrees with {} of {} items seen",
                    seen.len(),
                    expected.len()
                );
                if next_token.is_none() {
                    break;
                }
            }

            assert_eq!(
                seen, expected,
                "case {case:?}: paging must yield every environment exactly once, in name order"
            );
        }
    }
}

/// `environmentTemplates` narrows a listing to the environments deployed from the
/// template/major-version pairs it names; an absent or empty filter narrows nothing.
#[tokio::test]
async fn listing_environments_filters_by_template_and_major_version() {
    let proton = Proton::new();
    for template in ["alpha", "beta"] {
        proton.template(template).await;
    }
    // name -> (templateName, templateMajorVersion)
    let environments = [
        ("alpha1", "alpha", "1"),
        ("alpha1b", "alpha", "1"),
        ("alpha2", "alpha", "2"),
        ("beta1", "beta", "1"),
    ];
    for (name, template_name, major) in environments {
        proton
            .ok(
                "CreateEnvironment",
                json!({
                    "name": name,
                    "templateName": template_name,
                    "templateMajorVersion": major,
                    "spec": "proton: EnvironmentSpec\n",
                }),
            )
            .await;
    }

    let cases: [(&str, Value, Vec<&str>); 6] = [
        (
            "no filter",
            json!({}),
            vec!["alpha1", "alpha1b", "alpha2", "beta1"],
        ),
        (
            "empty filter list",
            json!({ "environmentTemplates": [] }),
            vec!["alpha1", "alpha1b", "alpha2", "beta1"],
        ),
        (
            "one template at one major version",
            json!({ "environmentTemplates": [{ "templateName": "alpha", "majorVersion": "1" }] }),
            vec!["alpha1", "alpha1b"],
        ),
        (
            "the other major version of the same template",
            json!({ "environmentTemplates": [{ "templateName": "alpha", "majorVersion": "2" }] }),
            vec!["alpha2"],
        ),
        (
            "two filters union",
            json!({ "environmentTemplates": [
                { "templateName": "alpha", "majorVersion": "2" },
                { "templateName": "beta", "majorVersion": "1" },
            ] }),
            vec!["alpha2", "beta1"],
        ),
        (
            "a pair nothing was deployed from",
            json!({ "environmentTemplates": [{ "templateName": "beta", "majorVersion": "9" }] }),
            vec![],
        ),
    ];

    for (case, request, expected) in cases {
        let listed = proton.ok("ListEnvironments", request).await;
        assert_eq!(
            names_of(&listed["environments"]),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed["environments"]
        );
    }

    // The filter composes with paging rather than replacing it.
    let first = proton
        .ok(
            "ListEnvironments",
            json!({
                "maxResults": 1,
                "environmentTemplates": [{ "templateName": "alpha", "majorVersion": "1" }],
            }),
        )
        .await;
    assert_eq!(
        names_of(&first["environments"]),
        vec!["alpha1".to_string()],
        "a filtered first page honours maxResults, got {:?}",
        first["environments"]
    );
    let token = first["nextToken"]
        .as_str()
        .expect("a filtered listing with more items hands back a token");
    let second = proton
        .ok(
            "ListEnvironments",
            json!({
                "maxResults": 1,
                "nextToken": token,
                "environmentTemplates": [{ "templateName": "alpha", "majorVersion": "1" }],
            }),
        )
        .await;
    assert_eq!(
        names_of(&second["environments"]),
        vec!["alpha1b".to_string()],
        "a filtered second page resumes after the first, got {:?}",
        second["environments"]
    );
}

/// Delete starts the deletion and the clock finishes it: the call answers with the
/// record as `DELETE_IN_PROGRESS`, a retry is accepted unchanged, and once the deletion
/// falls due the environment is gone from every read.
#[tokio::test]
async fn deleting_an_environment_takes_it_out_of_every_read() {
    let proton = Proton::new();
    proton.template("tmpl").await;
    for name in ["dev", "prod"] {
        proton
            .ok(
                "CreateEnvironment",
                json!({
                    "name": name,
                    "templateName": "tmpl",
                    "templateMajorVersion": "1",
                    "spec": format!("proton: {name}\n"),
                }),
            )
            .await;
    }

    let deleted = proton.ok("DeleteEnvironment", json!({ "name": "dev" })).await;
    assert_eq!(
        deleted["environment"]["name"].as_str(),
        Some("dev"),
        "delete must answer the environment it is removing, got {:?}",
        deleted["environment"]
    );
    assert_eq!(
        deleted["environment"]["deploymentStatus"].as_str(),
        Some("DELETE_IN_PROGRESS"),
        "a deletion that has just started is DELETE_IN_PROGRESS, got {:?}",
        deleted["environment"]["deploymentStatus"]
    );

    // The retry of this `@idempotent` call is accepted and reports the same record.
    let retried = proton.ok("DeleteEnvironment", json!({ "name": "dev" })).await;
    assert_eq!(
        retried["environment"], deleted["environment"],
        "a retried delete must not restart the deletion, got {:?}",
        retried["environment"]
    );

    std::thread::sleep(PAST_THE_SETTLE_DELAY);

    let (status, body) = proton.call("GetEnvironment", json!({ "name": "dev" })).await;
    assert_eq!(
        status, 404,
        "a deleted environment must be gone, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {body}"
    );

    let listed = proton.ok("ListEnvironments", json!({})).await;
    assert_eq!(
        names_of(&listed["environments"]),
        vec!["prod".to_string()],
        "a deleted environment must not be listed, got {:?}",
        listed["environments"]
    );

    // And a name no environment carries is an absent resource, not a server fault.
    for case in ["already deleted", "never created"] {
        let name = if case == "already deleted" { "dev" } else { "nope" };
        let (status, body) = proton.call("DeleteEnvironment", json!({ "name": name })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: deleting an absent environment must answer 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // The surviving environment is untouched by its neighbour's deletion.
    let survivor = proton.ok("GetEnvironment", json!({ "name": "prod" })).await;
    assert_eq!(
        survivor["environment"]["spec"].as_str(),
        Some("proton: prod\n"),
        "the surviving environment must keep its own spec, got {:?}",
        survivor["environment"]["spec"]
    );
}

/// A request outside a modelled constraint — or carrying a pagination token this
/// service never issued — is refused 400 `ValidationException`, and nothing is stored.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    // Each row names the member and the modelled constraint it breaks, taken from
    // `smithy_query(query="operation",...)`: the `ResourceName` pattern and 1..100
    // length, the `TemplateVersionPart` pattern `^(0|([1-9]{1}\d*))$`, the `SpecContents`
    // 1..51200 length, the `MaxPageResults` 1..100 range, and the `NextToken` pattern.
    let cases: [(&str, &str, Value); 11] = [
        (
            "template name outside the pattern",
            "CreateEnvironmentTemplate",
            json!({ "name": "bad name!" }),
        ),
        (
            "template name below the length minimum",
            "CreateEnvironmentTemplate",
            json!({ "name": "" }),
        ),
        (
            "template name past the length maximum",
            "CreateEnvironmentTemplate",
            json!({ "name": "x".repeat(101) }),
        ),
        (
            "environment name outside the pattern",
            "CreateEnvironment",
            json!({
                "name": "bad name!",
                "templateName": "tmpl",
                "templateMajorVersion": "1",
                "spec": "x",
            }),
        ),
        (
            "major version with a leading zero",
            "CreateEnvironment",
            json!({
                "name": "env",
                "templateName": "tmpl",
                "templateMajorVersion": "01",
                "spec": "x",
            }),
        ),
        (
            "minor version that is not a number",
            "CreateEnvironment",
            json!({
                "name": "env",
                "templateName": "tmpl",
                "templateMajorVersion": "1",
                "templateMinorVersion": "beta",
                "spec": "x",
            }),
        ),
        (
            "spec below the length minimum",
            "CreateEnvironment",
            json!({
                "name": "env",
                "templateName": "tmpl",
                "templateMajorVersion": "1",
                "spec": "",
            }),
        ),
        (
            "spec past the length maximum",
            "CreateEnvironment",
            json!({
                "name": "env",
                "templateName": "tmpl",
                "templateMajorVersion": "1",
                "spec": "z".repeat(51_201),
            }),
        ),
        (
            "maxResults below the range minimum",
            "ListEnvironments",
            json!({ "maxResults": 0 }),
        ),
        (
            "maxResults past the range maximum",
            "ListEnvironments",
            json!({ "maxResults": 101 }),
        ),
        (
            "a nextToken this service never issued",
            "ListEnvironments",
            json!({ "nextToken": "notAToken" }),
        ),
    ];

    let proton = Proton::new();
    proton.template("tmpl").await;
    for (case, operation, request) in cases {
        let (status, body) = proton.call(operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: must be refused 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // Nothing above was stored: only the one template this case created exists, and no
    // environment does.
    let listed = proton.ok("ListEnvironments", json!({})).await;
    assert_eq!(
        names_of(&listed["environments"]),
        Vec::<String>::new(),
        "a refused request must store nothing, got {:?}",
        listed["environments"]
    );
}

/// Both stores are quotaed per account, as Proton quotas these resources: a create
/// past the quota is refused with the modelled `ServiceQuotaExceededException` (402)
/// and stores nothing, so neither store grows without bound.
#[tokio::test]
async fn a_create_past_the_resource_quota_is_refused() {
    // Rows: the two quotaed resources, their cap, and the create that fills the store.
    let proton = Proton::new();

    for index in 0..100 {
        proton.template(&format!("tmpl{index}")).await;
    }
    let (status, body) = proton
        .call("CreateEnvironmentTemplate", json!({ "name": "oneTooMany" }))
        .await;
    assert_eq!(
        status, 402,
        "case \"environment template past the quota\": must be refused 402, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"environment template past the quota\": expected \
         ServiceQuotaExceededException, got {body}"
    );
    // The refused create stored nothing: the store still holds exactly the quota.
    let (_, refused) = proton
        .call("GetEnvironmentTemplate", json!({ "name": "oneTooMany" }))
        .await;
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case \"environment template past the quota\": the refused template must not be \
         stored, got {refused}"
    );

    for index in 0..100 {
        proton
            .ok(
                "CreateEnvironment",
                json!({
                    "name": format!("env{index}"),
                    "templateName": "tmpl0",
                    "templateMajorVersion": "1",
                    "spec": "proton: EnvironmentSpec\n",
                }),
            )
            .await;
    }
    let (status, body) = proton
        .call(
            "CreateEnvironment",
            json!({
                "name": "oneTooMany",
                "templateName": "tmpl0",
                "templateMajorVersion": "1",
                "spec": "proton: EnvironmentSpec\n",
            }),
        )
        .await;
    assert_eq!(
        status, 402,
        "case \"environment past the quota\": must be refused 402, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"environment past the quota\": expected ServiceQuotaExceededException, got {body}"
    );
    let (_, refused) = proton
        .call("GetEnvironment", json!({ "name": "oneTooMany" }))
        .await;
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case \"environment past the quota\": the refused environment must not be stored, \
         got {refused}"
    );

    // Releasing one makes room again, so the bound is a live quota and not a wedge.
    proton.ok("DeleteEnvironment", json!({ "name": "env0" })).await;
    std::thread::sleep(PAST_THE_SETTLE_DELAY);
    proton
        .ok(
            "CreateEnvironment",
            json!({
                "name": "roomAgain",
                "templateName": "tmpl0",
                "templateMajorVersion": "1",
                "spec": "proton: EnvironmentSpec\n",
            }),
        )
        .await;
}

/// An environment account connection and a linked provisioning repository are created by
/// operations this emulator has not implemented yet, so no such resource can exist:
/// naming one names an absent resource rather than creating it implicitly.
#[tokio::test]
async fn an_environment_naming_an_unregistered_resource_is_refused() {
    let proton = Proton::new();
    proton.template("tmpl").await;

    let cases = [
        (
            "an environment account connection",
            json!({ "environmentAccountConnectionId": "0bf3b1ba-1a2b-3c4d-5e6f-0123456789ab" }),
        ),
        (
            "a provisioning repository",
            json!({ "provisioningRepository": {
                "provider": "GITHUB",
                "name": "org/repo",
                "branch": "main",
            } }),
        ),
    ];
    for (case, extra) in cases {
        let mut request = json!({
            "name": "env",
            "templateName": "tmpl",
            "templateMajorVersion": "1",
            "spec": "proton: EnvironmentSpec\n",
        });
        for (member, value) in extra.as_object().expect("an object of members") {
            request[member] = value.clone();
        }
        let (status, body) = proton.call("CreateEnvironment", request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: naming an unregistered resource must answer 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    let listed = proton.ok("ListEnvironments", json!({})).await;
    assert_eq!(
        names_of(&listed["environments"]),
        Vec::<String>::new(),
        "a refused create must store nothing, got {:?}",
        listed["environments"]
    );
}
