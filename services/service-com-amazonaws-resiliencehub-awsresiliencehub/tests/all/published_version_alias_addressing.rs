// SPDX-License-Identifier: CC0-1.0

//! How a PUBLISHED app version is LABELLED and ADDRESSED, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: an application version's LABEL (`appVersion`, an
//! `EntityVersion`) and its publish SEQUENCE NUMBER (`identifier`, a
//! `LongOptional`) are distinct values, so the newest published snapshot is
//! labelled by the fixed release alias — never by its sequence number — through
//! EVERY operation that takes or reports an `appVersion`: the publish answers that
//! alias with the sequence number beside it, each operation that reads a version
//! resolves the current release by the alias and a snapshot the alias has moved off
//! by that snapshot's own sequence number, the number of the current release
//! resolves to nothing, and the listing reports every version's label together with
//! its sequence number (the draft's being the lowest).
//!
//! The readers are tabulated rather than tested one by one: the keying is one rule,
//! and a per-operation copy of it is exactly the defect this pins.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    DRAFT_APP_VERSION, DRAFT_APP_VERSION_IDENTIFIER, RELEASE_APP_VERSION,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-east-1";

/// The Application Component every published snapshot below holds: it is added to
/// the draft before the first publish, so every snapshot froze a copy of it.
const FROZEN_COMPONENT: &str = "compute-tier";

/// The `logicalResourceId$identifier` of the physical resource every published
/// snapshot below holds, added to the draft beside [`FROZEN_COMPONENT`].
const FROZEN_RESOURCE: &str = "orders-queue";

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: this suite tests
    // the operations, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype`; empty on success.
    error_code: String,
    body: Value,
}

impl Reply {
    /// The response body, asserting the call succeeded.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?} {:?}",
            self.status, self.error_code, self.body
        );
        &self.body
    }

    /// Assert the call was refused with `status` and `code`.
    fn refused(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {:?} {:?}",
            self.status,
            self.error_code,
            self.body
        );
    }
}

/// POST one request through the router, exactly as the server front door does.
async fn post(router: &MultiServiceRouter, target: &str, body: &Value) -> Reply {
    let payload = serde_json::to_vec(body).expect("serializable body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://resiliencehub.{REGION}.amazonaws.com{target}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/resiliencehub/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&raw)}))
    };
    Reply {
        status,
        error_code,
        body,
    }
}

/// An application to publish versions of, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// Add one Application Component with `id` to `app_arn`'s draft version.
async fn add_a_component(router: &MultiServiceRouter, app_arn: &str, id: &str) {
    let reply = post(
        router,
        "/create-app-version-app-component",
        &json!({
            "appArn": app_arn,
            "id": id,
            "name": id,
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
    )
    .await;
    reply.ok(id);
}

/// Map one physical resource into `app_arn`'s draft version, so the snapshots
/// published below hold a resource as well as a component.
async fn add_a_resource(router: &MultiServiceRouter, app_arn: &str, identifier: &str) {
    let reply = post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "logicalResourceId": {"identifier": identifier},
            "physicalResourceId": identifier,
            "resourceType": "AWS::SQS::Queue",
            "appComponents": [FROZEN_COMPONENT],
        }),
    )
    .await;
    reply.ok(identifier);
}

/// Publish `app_arn`'s draft and return the `(appVersion, identifier)` pair the
/// publish reported.
async fn publish(router: &MultiServiceRouter, app_arn: &str, case: &str) -> (String, i64) {
    let reply = post(router, "/publish-app-version", &json!({"appArn": app_arn})).await;
    let body = reply.ok(case).clone();
    (
        body["appVersion"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no appVersion in {body:?}"))
            .to_string(),
        body["identifier"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: no identifier in {body:?}")),
    )
}

/// One operation that READS an app version: its wire target and the request members
/// beyond the `appArn`/`appVersion` pair that addresses the version.
///
/// Source: the `appVersion` member each of these operations' inputs carries, and
/// the `appVersion` each of their outputs echoes (`smithy_query query="operation"
/// operation="DescribeAppVersionTemplate,DescribeAppVersionAppComponent,DescribeAppVersionResource,ResolveAppVersionResources"`).
struct Reader {
    label: &'static str,
    target: &'static str,
    /// The request members this reader needs beyond `appArn` and `appVersion`.
    extra: fn() -> Value,
}

/// The table: every implemented operation that addresses an app version, which is
/// what the keying rule has to hold for. The rows differ along what the reader
/// addresses INSIDE the version — the whole version, one Application Component, one
/// physical resource — and along whether reading it records anything (the resolve
/// does, the describes do not).
fn reader_table() -> Vec<Reader> {
    vec![
        Reader {
            label: "DescribeAppVersionTemplate",
            target: "/describe-app-version-template",
            extra: || json!({}),
        },
        Reader {
            label: "DescribeAppVersionAppComponent",
            target: "/describe-app-version-app-component",
            extra: || json!({"id": FROZEN_COMPONENT}),
        },
        Reader {
            label: "DescribeAppVersionResource",
            target: "/describe-app-version-resource",
            extra: || json!({"logicalResourceId": {"identifier": FROZEN_RESOURCE}}),
        },
        Reader {
            label: "ResolveAppVersionResources",
            target: "/resolve-app-version-resources",
            extra: || json!({}),
        },
    ]
}

/// Drive one reader at one `appVersion` of one application.
async fn read_version(
    router: &MultiServiceRouter,
    reader: &Reader,
    app_arn: &str,
    app_version: &str,
) -> Reply {
    let mut request = (reader.extra)();
    request["appArn"] = json!(app_arn);
    request["appVersion"] = json!(app_version);
    post(router, reader.target, &request).await
}

/// `(appVersion, identifier)` for every version `ListAppVersions` reports, in the
/// order it reports them.
async fn listed_versions(router: &MultiServiceRouter, app_arn: &str) -> Vec<(String, Value)> {
    let reply = post(router, "/list-app-versions", &json!({"appArn": app_arn})).await;
    let body = reply.ok("list the versions").clone();
    body["appVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("no appVersions list in {body:?}"))
        .iter()
        .map(|summary| {
            (
                summary["appVersion"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a summary with no appVersion: {summary:?}"))
                    .to_string(),
                // `Value::Null` when the member is ABSENT, which is itself a
                // divergence: every listed version carries its identifier.
                summary["identifier"].clone(),
            )
        })
        .collect()
}

/// How many publishes the table below drives against one application: enough that
/// the alias has moved off more than one snapshot, so "the number addresses the
/// snapshot the alias moved off" is tested on more than the single superseded case.
const PUBLISHES: i64 = 3;

/// Every reader addresses the current release by the alias and each superseded
/// snapshot by its own sequence number, while the current release's number
/// addresses nothing — re-checked after each publish, so the alias is seen moving.
#[tokio::test]
async fn every_reader_addresses_the_current_release_by_the_alias() {
    let router = emulator();
    let app_arn = create_an_app(&router, "release-alias-addressing").await;
    add_a_component(&router, &app_arn, FROZEN_COMPONENT).await;
    add_a_resource(&router, &app_arn, FROZEN_RESOURCE).await;

    for sequence in 1..=PUBLISHES {
        // Something new in the draft before every publish past the first, so the
        // snapshots differ in content as well as in label.
        if sequence > 1 {
            add_a_component(&router, &app_arn, &format!("extra-tier-{sequence}")).await;
        }

        let case = format!("publish {sequence}");
        let published = publish(&router, &app_arn, &case).await;
        assert_eq!(
            published,
            (RELEASE_APP_VERSION.to_string(), sequence),
            "case {case:?}: a publish reports the release alias as its label and \
             {sequence} as its separate sequence number, got {published:?}"
        );

        for reader in reader_table() {
            // The alias names the version just published, and the reader echoes the
            // label it was addressed by.
            let case = format!("{} at the alias after publish {sequence}", reader.label);
            let answered = read_version(&router, &reader, &app_arn, RELEASE_APP_VERSION).await;
            let echoed = answered.ok(&case)["appVersion"].clone();
            assert_eq!(
                echoed,
                json!(RELEASE_APP_VERSION),
                "case {case:?}: expected the echoed appVersion {RELEASE_APP_VERSION:?}, \
                 got {echoed:?}"
            );

            // Every snapshot the alias has moved off is addressed by the sequence
            // number it was published with.
            for superseded in 1..sequence {
                let app_version = superseded.to_string();
                let case = format!(
                    "{} at superseded snapshot {app_version} after publish {sequence}",
                    reader.label
                );
                let answered = read_version(&router, &reader, &app_arn, &app_version).await;
                let echoed = answered.ok(&case)["appVersion"].clone();
                assert_eq!(
                    echoed,
                    json!(app_version),
                    "case {case:?}: expected the echoed appVersion {app_version:?}, \
                     got {echoed:?}"
                );
            }

            // The CURRENT release's own number is not a label: while the alias names
            // the snapshot, its number resolves to no version — and neither does a
            // number no publish has reached yet.
            for app_version in [sequence.to_string(), (sequence + 1).to_string()] {
                let case = format!(
                    "{} at number {app_version} after publish {sequence}",
                    reader.label
                );
                let answered = read_version(&router, &reader, &app_arn, &app_version).await;
                answered.refused(&case, 404, "ResourceNotFoundException");
                assert_eq!(
                    answered.body["resourceType"],
                    json!("AppVersion"),
                    "case {case:?}: the miss is the app VERSION, got {:?}",
                    answered.body
                );
            }

            // Publishing never moves the draft: it keeps its own label whatever the
            // alias resolves to.
            let case = format!("{} at the draft after publish {sequence}", reader.label);
            let answered = read_version(&router, &reader, &app_arn, DRAFT_APP_VERSION).await;
            let echoed = answered.ok(&case)["appVersion"].clone();
            assert_eq!(
                echoed,
                json!(DRAFT_APP_VERSION),
                "case {case:?}: expected the echoed appVersion {DRAFT_APP_VERSION:?}, \
                 got {echoed:?}"
            );
        }

        // The listing agrees with the readers, label for label, and reports every
        // version's sequence number beside its label: the draft's lowest, each
        // superseded snapshot's its own, and the current release's the number the
        // publish answered.
        let mut expected: Vec<(String, Value)> = vec![(
            DRAFT_APP_VERSION.to_string(),
            json!(DRAFT_APP_VERSION_IDENTIFIER),
        )];
        expected.extend((1..sequence).map(|superseded| (superseded.to_string(), json!(superseded))));
        expected.push((RELEASE_APP_VERSION.to_string(), json!(sequence)));

        let listed = listed_versions(&router, &app_arn).await;
        let case = format!("the listing after publish {sequence}");
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

/// Labels and sequence numbers are per application: two applications each address
/// their own current release by the SAME alias, and the alias of one never resolves
/// into the other.
#[tokio::test]
async fn the_alias_is_per_application() {
    let router = emulator();

    // Two applications whose publish counts differ, so neither alias can be
    // standing in for one shared snapshot or one shared sequence.
    let mut published: Vec<(String, i64)> = Vec::new();
    for (name, publishes) in [("alias-app-a", 1_i64), ("alias-app-b", 2)] {
        let app_arn = create_an_app(&router, name).await;
        add_a_component(&router, &app_arn, FROZEN_COMPONENT).await;
        add_a_resource(&router, &app_arn, FROZEN_RESOURCE).await;
        for sequence in 1..=publishes {
            let case = format!("{name} publish {sequence}");
            let answered = publish(&router, &app_arn, &case).await;
            assert_eq!(
                answered,
                (RELEASE_APP_VERSION.to_string(), sequence),
                "case {case:?}: expected the release alias with sequence number \
                 {sequence}, got {answered:?}"
            );
        }
        published.push((app_arn, publishes));
    }

    for (app_arn, publishes) in &published {
        for reader in reader_table() {
            let case = format!("{} on {app_arn} at the alias", reader.label);
            let answered = read_version(&router, &reader, app_arn, RELEASE_APP_VERSION).await;
            let echoed = answered.ok(&case)["appVersion"].clone();
            assert_eq!(
                echoed,
                json!(RELEASE_APP_VERSION),
                "case {case:?}: expected the echoed appVersion {RELEASE_APP_VERSION:?}, \
                 got {echoed:?}"
            );
        }

        // The last number this application published is the current release's, so
        // it addresses nothing here either, however many times the OTHER published.
        let case = format!("{app_arn} at its newest number {publishes}");
        let answered = post(
            &router,
            "/describe-app-version-template",
            &json!({"appArn": app_arn, "appVersion": publishes.to_string()}),
        )
        .await;
        answered.refused(&case, 404, "ResourceNotFoundException");
    }
}
