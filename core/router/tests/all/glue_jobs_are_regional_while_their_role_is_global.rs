// SPDX-License-Identifier: CC0-1.0

//! One IAM role behind Glue jobs in several Regions: the role is ONE account-global
//! entity every Region's endpoint reports identically, while each job definition exists
//! only in the Region it was created through.
//!
//! The rule under test has two halves, and only a suite holding BOTH services on one
//! engine can pin them together — which is why it lives here rather than in either
//! service's crate:
//!
//! * IAM is a global service, so a role is one entity for the whole account: its `Arn`
//!   names no Region and every Region's endpoint reports the same `Arn` and `RoleId`.
//! * Glue is a regional service, so a job definition is reachable only through the
//!   endpoint that created it: `GetJob` answers `EntityNotFoundException` for a name the
//!   endpoint's Region does not hold, `BatchGetJobs` reports such a name in `JobsNotFound`
//!   while returning the Region's own definitions in `Jobs`, `ListJobs` names only that
//!   Region's jobs, and the same name in two Regions names two independent jobs.
//!
//! A job stores the role ARN it was given verbatim, so the regional records all point at
//! the one global role.
//!
//! The rows span what the rule generalises over: four Regions across three geographic
//! prefixes, a name held by exactly one Region, a name held by two, a name held by none,
//! how many names a batch carries and in which order, and whether a name repeats. Every
//! expectation is derived from which Region each name was created through, so no row can
//! agree with a store that merges the Regions.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers are this test crate's own
// (`query_wire`), written once there.
use crate::query_wire::{element, form_encode};

/// The Regions these endpoints are addressed in. Four of them, spanning three geographic
/// prefixes, so nothing here depends on one Region name.
const REGIONS: [&str; 4] = ["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-1"];

/// The one role every job below names.
const ROLE_NAME: &str = "sim2real-gjwr-glue-job-role";

/// A trust policy letting the Glue service principal assume the role.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The job name the first two Regions each hold a definition of, so a name held TWICE is
/// covered beside the names held once.
const SHARED_NAME: &str = "gjwr-shared-name";

/// A name no Region holds a job of: a miss from every endpoint, and never an error.
const UNHELD_NAME: &str = "gjwr-never-created";

/// The access key id every request presents. The router here runs with request
/// authentication off, so only the credential scope is read — its service field routes
/// the request and its Region field is the Region the service serves it in.
const ACCESS_KEY: &str = "AKIDGLUEIAMREGIONS";

/// A SigV4 `Authorization` header whose credential scope names `region` and
/// `signing_name`. The signature is never verified.
fn authorization(region: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/20260101/{region}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The emulator with IAM and Glue mounted on ONE engine, as `core/router/src/lib.rs`
/// mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        engine,
    ));
    router
}

/// One IAM action over the awsQuery wire, through `region`'s endpoint.
async fn iam(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(region, "iam"))
        .body(Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Glue operation over the awsJson1_1 wire, through `region`'s endpoint.
async fn glue(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("glue.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSGlue.{operation}"))
        .header("authorization", authorization(region, "glue"))
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The awsJson error code of a failure body, which may carry a namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Where the job `name` of Region `region` keeps its script. The location names the
/// Region, so a definition a read reports can be traced to the record ONE Region stored.
fn script_of(region: &str, name: &str) -> String {
    format!("s3://sim2real-etl-scripts/{region}/{name}.py")
}

/// Create the job `name` through `region`'s endpoint, backed by the account's one role.
async fn create_job(router: &MultiServiceRouter, region: &str, name: &str, role_arn: &str) {
    let body = json!({
        "Name": name,
        "Role": role_arn,
        "Command": { "Name": "glueetl", "ScriptLocation": script_of(region, name) },
    });
    let (status, answer) = glue(router, region, "CreateJob", &body).await;
    assert_eq!(
        status, 200,
        "case {region} / {name}: CreateJob must answer 200, got {status} with {answer}"
    );
    assert_eq!(
        answer["Name"].as_str(),
        Some(name),
        "case {region} / {name}: CreateJob must answer with the name it stored, got {answer}"
    );
}

/// The `(Jobs, JobsNotFound)` `BatchGetJobs` reports in `region` for `names`, each found
/// job as its `(Name, Role, Command.ScriptLocation)` triple — the triple says WHICH record
/// answered and which role backs it.
async fn batch_get_in(
    router: &MultiServiceRouter,
    region: &str,
    names: &[String],
) -> (Vec<(String, String, String)>, Vec<String>) {
    let (status, body) = glue(router, region, "BatchGetJobs", &json!({ "JobNames": names })).await;
    assert_eq!(
        status, 200,
        "case {region}: BatchGetJobs must answer 200, got {status} with {body}"
    );
    let found = body["Jobs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {region}: Jobs must be a list, got {body}"))
        .iter()
        .map(|job| {
            (
                job["Name"].as_str().unwrap_or_default().to_string(),
                job["Role"].as_str().unwrap_or_default().to_string(),
                job["Command"]["ScriptLocation"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    let missing = body["JobsNotFound"]
        .as_array()
        .unwrap_or_else(|| panic!("case {region}: JobsNotFound must be a list, got {body}"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    (found, missing)
}

/// The names `ListJobs` reports in `region`, sorted so the comparison is about membership.
async fn listed_names_in(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let (status, body) = glue(router, region, "ListJobs", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {region}: ListJobs must answer 200, got {status} with {body}"
    );
    let mut names: Vec<String> = body["JobNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case {region}: JobNames must be a list, got {body}"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

/// Whether `region` holds a job under `name`.
fn holds(holders: &BTreeMap<String, Vec<&str>>, region: &str, name: &str) -> bool {
    holders
        .get(name)
        .is_some_and(|regions| regions.contains(&region))
}

#[tokio::test]
async fn a_glue_job_is_served_only_in_its_region_while_the_role_behind_it_is_account_global() {
    let router = build_router();

    // The role: created once, through one Region's endpoint.
    let (status, created) = iam(
        &router,
        REGIONS[0],
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME.to_string()),
            ("AssumeRolePolicyDocument", TRUST_POLICY.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole must answer 200, got {status} with {created}"
    );
    let role_arn = element(&created, "Arn");
    let role_id = element(&created, "RoleId");

    // IAM is a global service, so field 4 of its ARN — the Region — is empty.
    let fields: Vec<&str> = role_arn.split(':').collect();
    assert_eq!(
        fields.get(3).copied(),
        Some(""),
        "a global service's ARN names no Region, got {role_arn:?}"
    );

    // And every Region's endpoint reports that one entity: the same ARN and the same id.
    for region in REGIONS {
        let (status, read) = iam(
            &router,
            region,
            "GetRole",
            &[("RoleName", ROLE_NAME.to_string())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: GetRole must answer 200 for an account-global role, got {status} \
             with {read}"
        );
        assert_eq!(
            element(&read, "Arn"),
            role_arn,
            "case {region}: every Region must report the one role ARN, expected {role_arn:?}, \
             got {read}"
        );
        assert_eq!(
            element(&read, "RoleId"),
            role_id,
            "case {region}: every Region must report the one role id, expected {role_id:?}, \
             got {read}"
        );
    }

    // The jobs: one named after each Region, plus one name the first two Regions each hold
    // a definition of. Which Region holds which name is what every expectation below is
    // derived from.
    let mut holders: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut every_name: Vec<String> = Vec::new();
    for region in REGIONS {
        let name = format!("gjwr-{region}");
        create_job(&router, region, &name, &role_arn).await;
        holders.entry(name.clone()).or_default().push(region);
        every_name.push(name);
    }
    for region in REGIONS.iter().take(2) {
        create_job(&router, region, SHARED_NAME, &role_arn).await;
        holders.entry(SHARED_NAME.to_string()).or_default().push(region);
    }
    every_name.push(SHARED_NAME.to_string());
    let unheld = UNHELD_NAME.to_string();

    // A single read resolves a name in the endpoint's Region alone: the Region that
    // created it serves its own definition, carrying the global role ARN verbatim, and
    // every other Region reports no such entity.
    for name in every_name.iter().chain(std::iter::once(&unheld)) {
        for region in REGIONS {
            let case = format!("{region} / {name}");
            let (status, body) = glue(&router, region, "GetJob", &json!({ "JobName": name })).await;
            if holds(&holders, region, name) {
                assert_eq!(
                    status, 200,
                    "case {case}: the Region that created the job must serve it, got {status} \
                     with {body}"
                );
                assert_eq!(
                    body["Job"]["Command"]["ScriptLocation"].as_str(),
                    Some(script_of(region, name).as_str()),
                    "case {case}: the Region must report the definition it stored, got {body}"
                );
                assert_eq!(
                    body["Job"]["Role"].as_str(),
                    Some(role_arn.as_str()),
                    "case {case}: the job must carry the account-global role ARN verbatim, \
                     got {body}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case}: a job of another Region must not be served, got {status} \
                     with {body}"
                );
                assert!(
                    error_code(&body).contains("EntityNotFoundException"),
                    "case {case}: expected EntityNotFoundException, got {}",
                    error_code(&body)
                );
            }
        }
    }

    // A batch read splits EVERY requested name between `Jobs` and `JobsNotFound` by the
    // endpoint's Region, in the order the request listed them, so two endpoints asked the
    // same batch answer the mirrored split.
    for region in REGIONS {
        let own: Vec<String> = every_name
            .iter()
            .filter(|name| holds(&holders, region, name))
            .cloned()
            .collect();
        let foreign: Vec<String> = every_name
            .iter()
            .filter(|name| !holds(&holders, region, name))
            .cloned()
            .collect();
        let mut reversed = every_name.clone();
        reversed.reverse();
        let mut with_unheld = every_name.clone();
        with_unheld.push(unheld.clone());

        let cases: Vec<(&str, Vec<String>)> = vec![
            ("every name, in create order", every_name.clone()),
            ("every name, reversed", reversed),
            ("only the names this Region holds", own.clone()),
            ("only names of other Regions", foreign.clone()),
            (
                "one of its own names repeated around a foreign one",
                vec![own[0].clone(), foreign[0].clone(), own[0].clone()],
            ),
            ("a name no Region holds", vec![unheld.clone()]),
            ("every name plus one no Region holds", with_unheld),
        ];

        for (label, requested) in cases {
            let case = format!("{region} / {label}");
            let expected_found: Vec<(String, String, String)> = requested
                .iter()
                .filter(|name| holds(&holders, region, name))
                .map(|name| (name.clone(), role_arn.clone(), script_of(region, name)))
                .collect();
            let expected_missing: Vec<String> = requested
                .iter()
                .filter(|name| !holds(&holders, region, name))
                .cloned()
                .collect();

            let (found, missing) = batch_get_in(&router, region, &requested).await;
            assert_eq!(
                found, expected_found,
                "case {case}: expected Jobs {expected_found:?}, got {found:?}"
            );
            assert_eq!(
                missing, expected_missing,
                "case {case}: expected JobsNotFound {expected_missing:?}, got {missing:?}"
            );
            assert_eq!(
                found.len() + missing.len(),
                requested.len(),
                "case {case}: every requested name must be reported exactly once, got {found:?} \
                 and {missing:?}"
            );
        }

        // And the listing of that endpoint names its own jobs and no other Region's, so a
        // client enumerating "the jobs in this Region" sees no phantom record.
        let mut expected: Vec<String> = own;
        expected.sort();
        let listed = listed_names_in(&router, region).await;
        assert_eq!(
            listed, expected,
            "case {region}: ListJobs must name this Region's jobs alone, expected \
             {expected:?}, got {listed:?}"
        );
    }
}
