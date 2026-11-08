// SPDX-License-Identifier: CC0-1.0

//! The environment lifecycle, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what an SDK client sees: the status, the
//! JSON body, and the state the next call observes.
//!
//! The rules under test, each exercised over a SET of inputs rather than one request:
//!
//! * `CreateEnvironment` mints an `environmentId` and an `environmentArn` for the region
//!   the request was signed for, persists the record, and every later read projects it;
//! * an omitted `configuration` member keeps the documented default (`scanFrequency` P1D,
//!   `patchCreationPolicy` DISABLED) and no sources;
//! * `UpdateEnvironment` writes the members the request carries and preserves every
//!   member it omits, at every level of the configuration;
//! * `ListEnvironments` partitions the stored environments exactly for any `maxResults`,
//!   in `environmentId` order, handing back a `nextToken` only while entries remain;
//! * `DeleteEnvironment` removes the record, so a later read and a later delete answer the
//!   modelled 404 and the id is gone from the listing.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_continuum_continuumcontrolplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region of the caller's sigv4 credential scope, which every minted ARN carries.
/// Deliberately not the handler's default, so an ARN built from a default instead of from
/// the request fails the assertion.
const REGION: &str = "eu-west-1";

/// A second scope region, so the ARN is shown to come from the request.
const OTHER_REGION: &str = "ap-southeast-2";

/// The account segment of every ARN a generated emulator presents
/// (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// `maxResults`' modelled range for `ListEnvironments` (1..100), from
/// `smithy_query operation=ListEnvironments`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The recurring scan intervals `EnvironmentConfiguration$scanFrequency` documents:
/// "P1D (1 day), P2D (2 days), or P7D (7 days). PT0H disables recurring scans."
const SCAN_FREQUENCIES: &[&str] = &["P1D", "P2D", "P7D", "PT0H"];

/// The values `PatchCreationPolicy` admits (`smithy_query shape=PatchCreationPolicy`).
const PATCH_CREATION_POLICIES: &[&str] = &["ENABLED", "DISABLED"];

/// The documented defaults of a created environment's operating policy:
/// `scanFrequency` "Defaults to P1D when creating an environment", `patchCreationPolicy`
/// "Defaults to DISABLED".
const DEFAULT_SCAN_FREQUENCY: &str = "P1D";
const DEFAULT_PATCH_CREATION_POLICY: &str = "DISABLED";

/// A continuum emulator and the one wire path into it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serializable request body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://continuum.{region}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/continuum/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {path} answered {status} with a body that is not JSON ({e}): {:?}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        (status, parsed)
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call_in(REGION, method, path, body).await
    }

    async fn create(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/environments", Some(body)).await
    }

    /// Create an environment that must succeed, answering its `environmentId`.
    async fn create_ok(&self, case: &str, body: Value) -> String {
        let (status, response) = self.create(body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateEnvironment, got {status} ({response})"
        );
        text(&response, "environmentId")
    }

    async fn get(&self, id: &str) -> (u16, Value) {
        self.call("GET", &format!("/environments/{id}"), None).await
    }

    async fn get_ok(&self, case: &str, id: &str) -> Value {
        let (status, response) = self.get(id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetEnvironment, got {status} ({response})"
        );
        response
    }

    async fn update(&self, id: &str, body: Value) -> (u16, Value) {
        self.call("PATCH", &format!("/environments/{id}"), Some(body))
            .await
    }

    async fn delete(&self, id: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/environments/{id}"), None)
            .await
    }

    async fn list(&self, query: &str) -> (u16, Value) {
        self.call("GET", &format!("/environments{query}"), None).await
    }
}

/// A string member of a response, or the empty string when it carries none.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The error code `restJson1` puts in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The ARN an environment of this account carries in `region`.
fn expected_arn(region: &str, id: &str) -> String {
    format!("arn:aws:continuum:{region}:{ACCOUNT}:environments/{id}")
}

/// How every environment operation words an id no environment holds.
fn not_found_message(id: &str) -> String {
    format!(
        "Environment {id} was not found. Verify that the resource identifier is correct and that \
         the resource exists."
    )
}

/// Assert an instant is on the wire in the form its model declares: every timestamp
/// member of this model carries `@timestampFormat("date-time")`, so each one is an
/// RFC 3339 STRING and not the protocol's default epoch-seconds number.
fn assert_date_time(case: &str, body: &Value, member: &str) -> String {
    let rendered = body.get(member).unwrap_or(&Value::Null).clone();
    let text = rendered.as_str().unwrap_or_default().to_string();
    assert!(
        text.contains('T') && text.ends_with('Z') && text.len() >= "2024-01-01T00:00:00Z".len(),
        "case {case:?}: expected {member} as an RFC 3339 date-time string, got {rendered}"
    );
    text
}

fn assert_not_found(case: &str, status: u16, body: &Value, id: &str) {
    assert_eq!(
        (status, error_code(body), text(body, "message")),
        (404, "ResourceNotFoundException".to_string(), not_found_message(id)),
        "case {case:?}: expected the modelled 404 ResourceNotFoundException, got {status} ({body})"
    );
}

/// `configuration.codeConfiguration` of a `GetEnvironment`/`UpdateEnvironment` answer.
fn code_configuration(case: &str, body: &Value) -> Value {
    body.get("configuration")
        .and_then(|configuration| configuration.get("codeConfiguration"))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected configuration.codeConfiguration, got {body}")
        })
}

/// The stored operating policy (`environmentConfiguration`) of an answer.
fn operating_policy(case: &str, body: &Value) -> Value {
    code_configuration(case, body)
        .get("environmentConfiguration")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The stored sources (`sourceConfiguration`) of an answer.
fn sources(case: &str, body: &Value) -> Value {
    code_configuration(case, body)
        .get("sourceConfiguration")
        .cloned()
        .unwrap_or(Value::Null)
}

/// One S3 source, built from values the model's own patterns admit
/// (`integrationId` `^[a-z0-9]+$`, prefixes `^.*/$`).
fn s3_source(integration_id: &str, code_prefix: &str) -> Value {
    json!({"integrationId": integration_id, "codePrefix": code_prefix})
}

/// One repository source (`repository` `^[a-zA-Z0-9_\-\.]+/[a-zA-Z0-9_\-\.]+$`).
fn repository_source(integration_id: &str, repository: &str) -> Value {
    json!({"integrationId": integration_id, "repository": repository})
}

#[tokio::test]
async fn created_environments_are_persisted_and_read_back() {
    let emulator = Emulator::new();

    // The class `EnvironmentName` spans: one character, the punctuation the pattern
    // admits, a space, and the 255-character ceiling (len 1..255, pattern
    // `^[a-zA-Z0-9][a-zA-Z0-9 _.\-]{0,254}$`).
    let names = [
        ("one character", "a".to_string()),
        ("hyphenated", "demo-env".to_string()),
        ("spaces and dots", "Prod 1.2 _ env".to_string()),
        ("255 characters", format!("x{}", "y".repeat(254))),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, name) in &names {
        let (status, created) = emulator.create(json!({"name": name})).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateEnvironment, got {status} ({created})"
        );

        let id = text(&created, "environmentId");
        assert!(
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
            "case {case:?}: expected an environmentId matching ^[a-z0-9]+$ of length 1..64, got \
             {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: expected a fresh environmentId, got one already minted: {id:?}"
        );

        assert_eq!(
            text(&created, "environmentArn"),
            expected_arn(REGION, &id),
            "case {case:?}: expected the ARN of {id:?} in {REGION}, got {created}"
        );
        assert_eq!(
            text(&created, "status"),
            "ACTIVE",
            "case {case:?}: expected status ACTIVE on creation, got {created}"
        );
        let created_at = assert_date_time(case, &created, "createdAt");

        // The record is readable, and reports what was stored rather than what this
        // response carried.
        let read = emulator.get_ok(case, &id).await;
        for member in ["createdAt", "updatedAt", "statusUpdatedAt"] {
            assert_date_time(case, &read, member);
        }
        assert_eq!(
            (
                text(&read, "environmentId"),
                text(&read, "environmentArn"),
                text(&read, "name"),
                text(&read, "status"),
                text(&read, "createdAt"),
                text(&read, "updatedAt"),
                text(&read, "statusUpdatedAt"),
            ),
            (
                id.clone(),
                expected_arn(REGION, &id),
                name.clone(),
                "ACTIVE".to_string(),
                created_at.clone(),
                created_at.clone(),
                created_at.clone(),
            ),
            "case {case:?}: expected GetEnvironment to project the stored record, got {read}"
        );
    }

    // Every environment created above is still retrievable: a minted id that was not
    // kept would fail here.
    for id in &minted {
        let (status, _) = emulator.get(id).await;
        assert_eq!(status, 200, "expected {id:?} to still be retrievable, got {status}");
    }
}

#[tokio::test]
async fn the_arn_carries_the_region_the_request_was_signed_for() {
    let emulator = Emulator::new();

    for (case, region) in [("scope region", REGION), ("other scope region", OTHER_REGION)] {
        let (status, created) = emulator
            .call_in(region, "POST", "/environments", Some(json!({"name": "regional"})))
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateEnvironment, got {status} ({created})"
        );
        let id = text(&created, "environmentId");
        assert_eq!(
            text(&created, "environmentArn"),
            expected_arn(region, &id),
            "case {case:?}: expected the ARN to carry {region}, got {created}"
        );
    }
}

#[tokio::test]
async fn a_created_configuration_keeps_the_documented_defaults_for_what_it_omits() {
    let emulator = Emulator::new();

    // Each row: what the create carried, and the operating policy the stored
    // environment must report for it.
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "no configuration at all",
            json!({"name": "defaults"}),
            json!({
                "scanFrequency": DEFAULT_SCAN_FREQUENCY,
                "patchCreationPolicy": DEFAULT_PATCH_CREATION_POLICY,
            }),
            Value::Null,
        ),
        (
            "an empty code configuration",
            json!({"name": "empty code", "configuration": {"codeConfiguration": {}}}),
            json!({
                "scanFrequency": DEFAULT_SCAN_FREQUENCY,
                "patchCreationPolicy": DEFAULT_PATCH_CREATION_POLICY,
            }),
            Value::Null,
        ),
        (
            "only a scan frequency",
            json!({
                "name": "scan only",
                "configuration": {"codeConfiguration": {
                    "environmentConfiguration": {"scanFrequency": "P7D"},
                }},
            }),
            json!({"scanFrequency": "P7D", "patchCreationPolicy": DEFAULT_PATCH_CREATION_POLICY}),
            Value::Null,
        ),
        (
            "only a patch creation policy",
            json!({
                "name": "patch only",
                "configuration": {"codeConfiguration": {
                    "environmentConfiguration": {"patchCreationPolicy": "ENABLED"},
                }},
            }),
            json!({"scanFrequency": DEFAULT_SCAN_FREQUENCY, "patchCreationPolicy": "ENABLED"}),
            Value::Null,
        ),
        (
            "a whole policy and both source kinds",
            json!({
                "name": "fully configured",
                "configuration": {"codeConfiguration": {
                    "sourceConfiguration": {
                        "s3Sources": [s3_source("int1", "code/")],
                        "repositorySources": [repository_source("int2", "acme/service")],
                    },
                    "environmentConfiguration": {
                        "network": {"allowedDomains": ["example.com", "crates.io"]},
                        "scanFrequency": "PT0H",
                        "patchCreationPolicy": "ENABLED",
                    },
                }},
            }),
            json!({
                "network": {"allowedDomains": ["example.com", "crates.io"]},
                "scanFrequency": "PT0H",
                "patchCreationPolicy": "ENABLED",
            }),
            json!({
                "s3Sources": [s3_source("int1", "code/")],
                "repositorySources": [repository_source("int2", "acme/service")],
            }),
        ),
    ];

    for (case, request, expected_policy, expected_sources) in cases {
        let id = emulator.create_ok(case, request).await;
        let read = emulator.get_ok(case, &id).await;
        assert_eq!(
            operating_policy(case, &read),
            expected_policy,
            "case {case:?}: expected the stored operating policy {expected_policy}, got {read}"
        );
        assert_eq!(
            sources(case, &read),
            expected_sources,
            "case {case:?}: expected the stored sources {expected_sources}, got {read}"
        );
    }
}

#[tokio::test]
async fn an_update_writes_what_it_carries_and_preserves_what_it_omits() {
    // The environment every row starts from: a name, both source kinds, and a policy
    // with every member set, so a preserved member is distinguishable from a default.
    let stored = || {
        json!({
            "name": "baseline",
            "configuration": {"codeConfiguration": {
                "sourceConfiguration": {
                    "s3Sources": [s3_source("base1", "base/")],
                    "repositorySources": [repository_source("base2", "acme/baseline")],
                },
                "environmentConfiguration": {
                    "network": {"allowedDomains": ["baseline.example.com"]},
                    "scanFrequency": "P2D",
                    "patchCreationPolicy": "ENABLED",
                },
            }},
        })
    };

    // Each row: the update body, then the name, policy and sources the environment must
    // report afterwards.
    let cases: Vec<(&str, Value, &str, Value, Value)> = vec![
        (
            "nothing at all",
            json!({}),
            "baseline",
            json!({
                "network": {"allowedDomains": ["baseline.example.com"]},
                "scanFrequency": "P2D",
                "patchCreationPolicy": "ENABLED",
            }),
            json!({
                "s3Sources": [s3_source("base1", "base/")],
                "repositorySources": [repository_source("base2", "acme/baseline")],
            }),
        ),
        (
            "only the name",
            json!({"name": "renamed env"}),
            "renamed env",
            json!({
                "network": {"allowedDomains": ["baseline.example.com"]},
                "scanFrequency": "P2D",
                "patchCreationPolicy": "ENABLED",
            }),
            json!({
                "s3Sources": [s3_source("base1", "base/")],
                "repositorySources": [repository_source("base2", "acme/baseline")],
            }),
        ),
        (
            "only the scan frequency",
            json!({"configuration": {"codeConfiguration": {
                "environmentConfiguration": {"scanFrequency": "P7D"},
            }}}),
            "baseline",
            json!({
                "network": {"allowedDomains": ["baseline.example.com"]},
                "scanFrequency": "P7D",
                "patchCreationPolicy": "ENABLED",
            }),
            json!({
                "s3Sources": [s3_source("base1", "base/")],
                "repositorySources": [repository_source("base2", "acme/baseline")],
            }),
        ),
        (
            "only the patch creation policy",
            json!({"configuration": {"codeConfiguration": {
                "environmentConfiguration": {"patchCreationPolicy": "DISABLED"},
            }}}),
            "baseline",
            json!({
                "network": {"allowedDomains": ["baseline.example.com"]},
                "scanFrequency": "P2D",
                "patchCreationPolicy": "DISABLED",
            }),
            json!({
                "s3Sources": [s3_source("base1", "base/")],
                "repositorySources": [repository_source("base2", "acme/baseline")],
            }),
        ),
        (
            "only the network policy",
            json!({"configuration": {"codeConfiguration": {
                "environmentConfiguration": {"network": {"allowedDomains": ["updated.example.com"]}},
            }}}),
            "baseline",
            json!({
                "network": {"allowedDomains": ["updated.example.com"]},
                "scanFrequency": "P2D",
                "patchCreationPolicy": "ENABLED",
            }),
            json!({
                "s3Sources": [s3_source("base1", "base/")],
                "repositorySources": [repository_source("base2", "acme/baseline")],
            }),
        ),
        (
            "only the S3 source",
            json!({"configuration": {"codeConfiguration": {
                "sourceConfiguration": {"s3Sources": [s3_source("new1", "next/")]},
            }}}),
            "baseline",
            json!({
                "network": {"allowedDomains": ["baseline.example.com"]},
                "scanFrequency": "P2D",
                "patchCreationPolicy": "ENABLED",
            }),
            json!({
                "s3Sources": [s3_source("new1", "next/")],
                "repositorySources": [repository_source("base2", "acme/baseline")],
            }),
        ),
        (
            "only the repository sources",
            json!({"configuration": {"codeConfiguration": {
                "sourceConfiguration": {"repositorySources": [
                    repository_source("new2", "acme/one"),
                    repository_source("new3", "acme/two"),
                ]},
            }}}),
            "baseline",
            json!({
                "network": {"allowedDomains": ["baseline.example.com"]},
                "scanFrequency": "P2D",
                "patchCreationPolicy": "ENABLED",
            }),
            json!({
                "s3Sources": [s3_source("base1", "base/")],
                "repositorySources": [
                    repository_source("new2", "acme/one"),
                    repository_source("new3", "acme/two"),
                ],
            }),
        ),
        (
            "the name and the whole policy",
            json!({
                "name": "fully updated",
                "configuration": {"codeConfiguration": {
                    "environmentConfiguration": {
                        "network": {"allowedDomains": ["a.example.com", "b.example.com"]},
                        "scanFrequency": "P1D",
                        "patchCreationPolicy": "DISABLED",
                    },
                }},
            }),
            "fully updated",
            json!({
                "network": {"allowedDomains": ["a.example.com", "b.example.com"]},
                "scanFrequency": "P1D",
                "patchCreationPolicy": "DISABLED",
            }),
            json!({
                "s3Sources": [s3_source("base1", "base/")],
                "repositorySources": [repository_source("base2", "acme/baseline")],
            }),
        ),
    ];

    for (case, update, expected_name, expected_policy, expected_sources) in cases {
        // One environment per row, so a row observes only its own update.
        let emulator = Emulator::new();
        let id = emulator.create_ok(case, stored()).await;
        let created = emulator.get_ok(case, &id).await;
        let created_at = assert_date_time(case, &created, "createdAt");

        let (status, updated) = emulator.update(&id, update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateEnvironment, got {status} ({updated})"
        );
        assert_eq!(
            (
                text(&updated, "environmentId"),
                text(&updated, "environmentArn"),
                text(&updated, "name"),
                text(&updated, "status"),
                text(&updated, "createdAt"),
            ),
            (
                id.clone(),
                expected_arn(REGION, &id),
                expected_name.to_string(),
                "ACTIVE".to_string(),
                created_at.clone(),
            ),
            "case {case:?}: expected the merged record with its original createdAt, got {updated}"
        );
        assert_eq!(
            operating_policy(case, &updated),
            expected_policy,
            "case {case:?}: expected the merged operating policy {expected_policy}, got {updated}"
        );
        assert_eq!(
            sources(case, &updated),
            expected_sources,
            "case {case:?}: expected the merged sources {expected_sources}, got {updated}"
        );

        // The merge was written, not only reported: a later read shows the same record.
        let read = emulator.get_ok(case, &id).await;
        assert_eq!(
            (
                text(&read, "name"),
                operating_policy(case, &read),
                sources(case, &read),
                text(&read, "createdAt"),
            ),
            (
                expected_name.to_string(),
                expected_policy,
                expected_sources,
                created_at
            ),
            "case {case:?}: expected GetEnvironment to show the merged record, got {read}"
        );
    }
}

#[tokio::test]
async fn every_scan_frequency_and_patch_policy_round_trips() {
    let emulator = Emulator::new();

    // The catalogs the model publishes, each entry round-tripped through a create and a
    // read — including the entries the defaults do not cover.
    for frequency in SCAN_FREQUENCIES {
        for policy in PATCH_CREATION_POLICIES {
            let case = format!("{frequency} + {policy}");
            let id = emulator
                .create_ok(
                    &case,
                    json!({
                        "name": format!("env {frequency} {policy}"),
                        "configuration": {"codeConfiguration": {"environmentConfiguration": {
                            "scanFrequency": frequency,
                            "patchCreationPolicy": policy,
                        }}},
                    }),
                )
                .await;
            let read = emulator.get_ok(&case, &id).await;
            assert_eq!(
                operating_policy(&case, &read),
                json!({"scanFrequency": frequency, "patchCreationPolicy": policy}),
                "case {case:?}: expected the stored policy to round-trip, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn listing_partitions_the_stored_environments_for_any_page_size() {
    let emulator = Emulator::new();

    // Seven environments, so a page size of 1, 2, 3 and the modelled maximum each
    // partition the store differently.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        expected.push(
            emulator
                .create_ok("seed", json!({"name": format!("env-{index}")}))
                .await,
        );
    }
    // The listing reports `environmentId` order, which is the order the store keys in.
    expected.sort();

    for page_size in [MAX_RESULTS_MIN, 2, 3, MAX_RESULTS_MAX] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, page) = emulator.list(&query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListEnvironments, got {status} ({page})"
            );
            let items = page
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() as i64 <= page_size,
                "case {case:?}: expected at most {page_size} items in a page, got {}",
                items.len()
            );
            for entry in &items {
                let id = text(entry, "environmentId");
                assert_eq!(
                    text(entry, "environmentArn"),
                    expected_arn(REGION, &id),
                    "case {case:?}: expected each entry to carry the stored ARN, got {entry}"
                );
                assert_eq!(
                    text(entry, "status"),
                    "ACTIVE",
                    "case {case:?}: expected each entry to carry the stored status, got {entry}"
                );
                assert!(
                    !text(entry, "name").is_empty(),
                    "case {case:?}: expected each entry to carry the stored name, got {entry}"
                );
                assert_date_time(&case, entry, "createdAt");
                seen.push(id);
            }
            pages += 1;
            assert!(
                pages <= 16,
                "case {case:?}: expected the listing to terminate, got {pages} pages"
            );
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                !items.is_empty(),
                "case {case:?}: expected a nextToken only beside items, got {page}"
            );
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every stored environment exactly once, in id order"
        );
        let expected_pages = (expected.len() as i64 + page_size - 1) / page_size;
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages for {} environments",
            expected.len()
        );
    }
}

#[tokio::test]
async fn a_page_size_past_the_modelled_maximum_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create_ok("seed", json!({"name": "bounds"}))
        .await;

    // The published bounds, and one past each.
    for (case, max_results, expected_status) in [
        ("the minimum", MAX_RESULTS_MIN, 200),
        ("the maximum", MAX_RESULTS_MAX, 200),
        ("below the minimum", MAX_RESULTS_MIN - 1, 400),
        ("above the maximum", MAX_RESULTS_MAX + 1, 400),
    ] {
        let (status, body) = emulator.list(&format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for maxResults={max_results}, got \
             {status} ({body})"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    emulator.create_ok("seed", json!({"name": "token"})).await;

    for (case, token) in [
        ("not hex at all", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of bytes that are not text", "ffffffffffffffffffffffffffffffff".to_string()),
        // Hex of a perfectly good cursor with no authenticator in front of it.
        ("hex of a bare environment id", hex::encode("aa".as_bytes())),
        // A value with the SHAPE of a minted token — an authenticator-width prefix
        // and a cursor — which only the store's secret can refuse.
        (
            "a forged authenticator",
            hex::encode(
                [
                    vec![0_u8; service_common::page_tokens::AUTHENTICATOR_BYTES].as_slice(),
                    "aa".as_bytes(),
                ]
                .concat(),
            ),
        ),
    ] {
        let (status, body) = emulator.list(&format!("?nextToken={token}")).await;
        assert_eq!(
            (status, error_code(&body), text(&body, "message")),
            (
                400,
                "ValidationException".to_string(),
                "The nextToken provided is invalid.".to_string()
            ),
            "case {case:?}: expected the modelled 400 ValidationException, got {status} ({body})"
        );
    }
}

#[tokio::test]
async fn a_page_resumes_even_when_the_environment_it_ended_on_was_deleted() {
    let emulator = Emulator::new();

    let mut ids: Vec<String> = Vec::new();
    for index in 0..3 {
        ids.push(
            emulator
                .create_ok("seed", json!({"name": format!("page-{index}")}))
                .await,
        );
    }
    ids.sort();

    // Take the first page, then delete the environment that page ended on: the token
    // names the key to resume AFTER, which is a place in the order rather than a
    // record that must still exist.
    let (status, first) = emulator.list("?maxResults=1").await;
    assert_eq!(status, 200, "expected HTTP 200 from ListEnvironments, got {status} ({first})");
    let token = text(&first, "nextToken");
    assert!(!token.is_empty(), "expected a nextToken beside the first page, got {first}");

    let (status, _) = emulator.delete(&ids[0]).await;
    assert_eq!(status, 204, "expected HTTP 204 from DeleteEnvironment, got {status}");

    let (status, second) = emulator
        .list(&format!("?maxResults=2&nextToken={token}"))
        .await;
    assert_eq!(status, 200, "expected HTTP 200 from ListEnvironments, got {status} ({second})");
    let listed: Vec<String> = second["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| text(entry, "environmentId"))
        .collect();
    assert_eq!(
        listed,
        ids[1..].to_vec(),
        "expected the page after the deleted key to carry the remaining environments, got {second}"
    );
}

#[tokio::test]
async fn creating_past_the_quota_is_refused_rather_than_growing_the_store() {
    let emulator = Emulator::new();

    // Fill the account to its quota, then ask for one more.
    let quota = service_com_amazonaws_continuum_continuumcontrolplane::state::MAX_ENVIRONMENTS;
    for index in 0..quota {
        emulator
            .create_ok("at the quota", json!({"name": format!("quota-{index}")}))
            .await;
    }

    let (status, body) = emulator.create(json!({"name": "one too many"})).await;
    assert_eq!(
        (status, error_code(&body)),
        (402, "ServiceQuotaExceededException".to_string()),
        "expected the modelled 402 ServiceQuotaExceededException past the quota, got {status} \
         ({body})"
    );
    assert_eq!(
        text(&body, "message"),
        format!(
            "The request would exceed the quota of {quota} environments for this account. Review \
             your quota and request an increase if needed."
        ),
        "expected the refusal to name the quota it would exceed, got {body}"
    );

    // The refused create stored nothing: the store still holds exactly the quota.
    let (status, listing) = emulator.list(&format!("?maxResults={MAX_RESULTS_MAX}")).await;
    assert_eq!(status, 200, "expected HTTP 200 from ListEnvironments, got {status} ({listing})");
    assert_eq!(
        listing["items"].as_array().map(Vec::len),
        Some(quota),
        "expected the store to still hold exactly {quota} environments, got {listing}"
    );
}

#[tokio::test]
async fn deleting_an_environment_removes_it_from_every_read() {
    let emulator = Emulator::new();

    let kept = emulator
        .create_ok("kept", json!({"name": "second-env"}))
        .await;
    let removed = emulator
        .create_ok("removed", json!({"name": "demo-env"}))
        .await;

    let (status, body) = emulator.delete(&removed).await;
    assert_eq!(
        (status, &body),
        (204, &Value::Null),
        "expected an empty HTTP 204 from DeleteEnvironment, got {status} ({body})"
    );

    let (status, body) = emulator.get(&removed).await;
    assert_not_found("read after delete", status, &body, &removed);

    let (status, body) = emulator.update(&removed, json!({"name": "revived"})).await;
    assert_not_found("update after delete", status, &body, &removed);

    let (status, body) = emulator.delete(&removed).await;
    assert_not_found("second delete", status, &body, &removed);

    let (status, listing) = emulator.list("").await;
    assert_eq!(status, 200, "expected HTTP 200 from ListEnvironments, got {status} ({listing})");
    let listed: Vec<String> = listing["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| text(entry, "environmentId"))
        .collect();
    assert_eq!(
        listed,
        vec![kept.clone()],
        "expected only the environment that was not deleted to remain, got {listing}"
    );

    // The environment that was not deleted is untouched.
    let read = emulator.get_ok("kept", &kept).await;
    assert_eq!(
        text(&read, "name"),
        "second-env",
        "expected the surviving environment to be unchanged, got {read}"
    );
}

#[tokio::test]
async fn an_unknown_environment_id_is_refused_by_every_operation_that_addresses_one() {
    let emulator = Emulator::new();

    // An id of the modelled form that no create minted: the store, not the shape of the
    // id, is what makes it a miss.
    let unknown = "0123456789abcdef0123456789abcdef";
    for (case, method, body) in [
        ("GetEnvironment", "GET", None),
        ("UpdateEnvironment", "PATCH", Some(json!({"name": "ghost"}))),
        ("DeleteEnvironment", "DELETE", None),
    ] {
        let (status, answered) = emulator
            .call(method, &format!("/environments/{unknown}"), body)
            .await;
        assert_not_found(case, status, &answered, unknown);
    }

    // An empty store lists nothing rather than failing.
    let (status, listing) = emulator.list("").await;
    assert_eq!(
        (status, listing["items"].as_array().map(Vec::len), listing.get("nextToken")),
        (200, Some(0), None),
        "expected an empty listing with no nextToken, got {status} ({listing})"
    );
}
