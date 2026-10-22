// SPDX-License-Identifier: CC0-1.0

//! The environment lifecycle, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what an SDK client sees: the status, the
//! JSON body, and the state the next call observes.
//!
//! The rules under test, each exercised over a SET of inputs rather than one recorded
//! request:
//!
//! * a created environment is PERSISTED, and every later read projects the stored
//!   record (ids, ARNs, activation codes and timestamps are minted once);
//! * `desktopType` is the service segment of `desktopArn`, constrained to the model's
//!   `DesktopType` catalog;
//! * `clientToken` makes `CreateEnvironment` idempotent;
//! * `ListEnvironments` pages the environments in state in creation order, partitioning
//!   them exactly for any `maxResults`;
//! * `UpdateEnvironment` writes the members the request carries and leaves the rest;
//! * `DeleteEnvironment` removes the record;
//! * tags are stored against the resource ARN and reported verbatim.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesthinclient_thinclient::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region of the caller's sigv4 credential scope, which every minted ARN carries.
/// Deliberately not `us-east-1`, so an ARN built from a default instead of from the
/// request fails the assertion.
const REGION: &str = "eu-west-1";

/// The account segment of every ARN a generated emulator presents
/// (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The three desktop services an environment can stream from, i.e. the model's
/// `DesktopType` enum (`smithy_query` query=shape shape=DesktopType), each with a
/// resource of the form that service's ARNs use.
const DESKTOP_CATALOG: &[(&str, &str, &str)] = &[
    ("workspaces", "workspace/ws-0a1b2c3d4", "workspaces"),
    ("workspaces-web", "portal/8f2b1c3d", "workspaces-web"),
    ("appstream", "stack/analyst-stack", "appstream"),
];

/// A thin client emulator and the one wire path into it.
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

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serializable request body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://thinclient.{REGION}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/thinclient/aws4_request, \
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

    async fn create(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/environments", Some(body)).await
    }

    /// Create an environment that must succeed, returning its `EnvironmentSummary`.
    async fn create_ok(&self, body: Value) -> Value {
        let (status, response) = self.create(body).await;
        assert_eq!(status, 201, "CreateEnvironment answered {status}: {response}");
        response["environment"].clone()
    }

    async fn get(&self, id: &str) -> (u16, Value) {
        self.call("GET", &format!("/environments/{id}"), None).await
    }

    async fn list(&self, query: &str) -> (u16, Value) {
        self.call("GET", &format!("/environments{query}"), None).await
    }

    async fn update(&self, id: &str, body: Value) -> (u16, Value) {
        self.call("PATCH", &format!("/environments/{id}"), Some(body))
            .await
    }

    async fn delete(&self, id: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/environments/{id}"), None)
            .await
    }

    async fn list_tags(&self, arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/tags/{}", encode_segment(arn)), None)
            .await
    }

    async fn tag(&self, arn: &str, tags: Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/tags/{}", encode_segment(arn)),
            Some(json!({ "tags": tags })),
        )
        .await
    }
}

/// Percent-encode a value that occupies one non-greedy `@httpLabel` path segment, the
/// way an SDK does: everything outside RFC 3986's unreserved set is escaped, so the
/// `:` and `/` of an ARN do not split the segment.
fn encode_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn desktop_arn(service: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{REGION}:{ACCOUNT}:{resource}")
}

fn text(value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("member '{member}' is not a string in {value}"))
        .to_string()
}

fn error_code(response: &Value) -> String {
    text(response, "__type")
}

/// Does `candidate` match `^[a-z0-9]{9}$` — the model's `EnvironmentId`?
fn is_environment_id(candidate: &str) -> bool {
    candidate.len() == 9
        && candidate
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Does `candidate` match `^[a-z]{2}[a-z0-9]{6}$` — the model's `ActivationCode`?
fn is_activation_code(candidate: &str) -> bool {
    let bytes = candidate.as_bytes();
    bytes.len() == 8
        && bytes[..2].iter().all(u8::is_ascii_lowercase)
        && bytes[2..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn tag_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected a tag map, got {value}"))
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                v.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn expected_tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[tokio::test]
async fn created_environments_are_minted_and_persisted_for_every_desktop_type() {
    let emulator = Emulator::new();

    // Each row of the catalog, plus two ARNs that name no streaming desktop: the rule
    // is "desktopType is the ARN's service segment, drawn from the DesktopType
    // catalog", so anything outside the catalog has no desktop type and is refused.
    for (service, resource, expected_type) in DESKTOP_CATALOG {
        let case = format!("desktopArn service '{service}'");
        let arn = desktop_arn(service, resource);
        let summary = emulator
            .create_ok(json!({ "desktopArn": arn, "name": format!("env for {service}") }))
            .await;

        let id = text(&summary, "id");
        assert!(
            is_environment_id(&id),
            "case {case:?}: expected an id matching ^[a-z0-9]{{9}}$, got {id:?}"
        );
        let activation_code = text(&summary, "activationCode");
        assert!(
            is_activation_code(&activation_code),
            "case {case:?}: expected an activationCode matching ^[a-z]{{2}}[a-z0-9]{{6}}$, \
             got {activation_code:?}"
        );
        assert_eq!(
            text(&summary, "desktopType"),
            *expected_type,
            "case {case:?}: wrong desktopType in {summary}"
        );
        assert_eq!(
            text(&summary, "arn"),
            format!("arn:aws:thinclient:{REGION}:{ACCOUNT}:environment/{id}"),
            "case {case:?}: wrong environment ARN in {summary}"
        );

        // The record is in state: a read projects exactly what create minted.
        let (status, response) = emulator.get(&id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEnvironment answered {status}: {response}"
        );
        let stored = &response["environment"];
        assert_eq!(
            text(stored, "activationCode"),
            activation_code,
            "case {case:?}: the activation code was regenerated on read, in {stored}"
        );
        assert_eq!(
            text(stored, "desktopArn"),
            arn,
            "case {case:?}: wrong desktopArn in {stored}"
        );
        assert_eq!(
            stored["registeredDevicesCount"].as_i64(),
            Some(0),
            "case {case:?}: a new environment has no registered devices, in {stored}"
        );
        assert_eq!(
            text(stored, "softwareSetComplianceStatus"),
            "NO_REGISTERED_DEVICES",
            "case {case:?}: wrong compliance status for a device-less environment, in {stored}"
        );
    }

    for service in ["ec2", "workspacesweb"] {
        let case = format!("desktopArn service '{service}'");
        let (status, response) = emulator
            .create(json!({ "desktopArn": desktop_arn(service, "thing/abc") }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an ARN outside the DesktopType catalog must be refused, got {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: wrong error code in {response}"
        );
    }
}

#[tokio::test]
async fn get_environment_returns_every_member_as_stored() {
    let emulator = Emulator::new();
    let arn = desktop_arn("appstream", "stack/analyst-stack");
    let kms_key_arn = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9");
    let summary = emulator
        .create_ok(json!({
            "desktopArn": arn,
            "name": "analyst desk",
            "desktopEndpoint": "https://login.example.com/start",
            "softwareSetUpdateSchedule": "USE_MAINTENANCE_WINDOW",
            "softwareSetUpdateMode": "USE_DESIRED",
            "desiredSoftwareSetId": "412",
            "kmsKeyArn": kms_key_arn,
            "maintenanceWindow": {
                "type": "CUSTOM",
                "startTimeHour": 2,
                "startTimeMinute": 30,
                "endTimeHour": 4,
                "endTimeMinute": 0,
                "daysOfTheWeek": ["MONDAY", "THURSDAY"],
                "applyTimeOf": "UTC"
            },
            "deviceCreationTags": { "Fleet": "analysts" }
        }))
        .await;

    let id = text(&summary, "id");
    let (status, response) = emulator.get(&id).await;
    assert_eq!(status, 200, "GetEnvironment answered {status}: {response}");
    let stored = &response["environment"];

    let expectations: &[(&str, Value)] = &[
        ("id", json!(id.clone())),
        ("name", json!("analyst desk")),
        ("desktopArn", json!(arn)),
        ("desktopEndpoint", json!("https://login.example.com/start")),
        ("desktopType", json!("appstream")),
        ("softwareSetUpdateSchedule", json!("USE_MAINTENANCE_WINDOW")),
        ("softwareSetUpdateMode", json!("USE_DESIRED")),
        ("desiredSoftwareSetId", json!("412")),
        ("kmsKeyArn", json!(kms_key_arn)),
        ("registeredDevicesCount", json!(0)),
        ("deviceCreationTags", json!({ "Fleet": "analysts" })),
        (
            "maintenanceWindow",
            json!({
                "type": "CUSTOM",
                "startTimeHour": 2,
                "startTimeMinute": 30,
                "endTimeHour": 4,
                "endTimeMinute": 0,
                "daysOfTheWeek": ["MONDAY", "THURSDAY"],
                "applyTimeOf": "UTC"
            }),
        ),
        (
            "arn",
            json!(format!("arn:aws:thinclient:{REGION}:{ACCOUNT}:environment/{id}")),
        ),
    ];
    for (member, expected) in expectations {
        assert_eq!(
            &stored[*member], expected,
            "case {member:?}: expected {expected}, got {} in {stored}",
            stored[*member]
        );
    }

    let created_at = stored["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("createdAt is not a timestamp in {stored}"));
    assert!(
        created_at > 1_600_000_000.0,
        "case \"createdAt\": expected a wall-clock timestamp, got {created_at}"
    );
    assert_eq!(
        stored["updatedAt"].as_f64(),
        Some(created_at),
        "case \"updatedAt\": an untouched environment was never updated, in {stored}"
    );

    // An id of the modelled shape that names nothing is a 404, not an empty success.
    let (status, response) = emulator.get("zzz999zzz").await;
    assert_eq!(
        status, 404,
        "case \"unknown id\": expected 404, got {status}: {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case \"unknown id\": wrong error code in {response}"
    );
}

#[tokio::test]
async fn client_token_makes_create_environment_idempotent() {
    let emulator = Emulator::new();
    let base = json!({
        "desktopArn": desktop_arn("workspaces", "workspace/ws-0a1b2c3d4"),
        "name": "shared desk",
        "clientToken": "a2d4c6e8-1111-2222-3333-444455556666"
    });

    let first = emulator.create_ok(base.clone()).await;
    let first_id = text(&first, "id");

    // Same token, same parameters: the SAME environment, not a second one.
    for attempt in 1..=2 {
        let case = format!("replay {attempt}");
        let replay = emulator.create_ok(base.clone()).await;
        assert_eq!(
            text(&replay, "id"),
            first_id,
            "case {case:?}: a replayed clientToken must return the original environment"
        );
        assert_eq!(
            text(&replay, "activationCode"),
            text(&first, "activationCode"),
            "case {case:?}: a replay must not mint a new activation code"
        );
    }

    // Same token, DIFFERENT parameters: a conflict, per the documented
    // idempotent-parameter-mismatch behaviour.
    let mut mismatched = base.clone();
    mismatched["name"] = json!("other desk");
    let (status, response) = emulator.create(mismatched).await;
    assert_eq!(
        status, 409,
        "case \"token reused with different parameters\": expected 409, got {status}: {response}"
    );
    assert_eq!(
        error_code(&response),
        "ConflictException",
        "case \"token reused with different parameters\": wrong error code in {response}"
    );

    // A different token mints a different environment.
    let mut other_token = base.clone();
    other_token["clientToken"] = json!("ffffffff-1111-2222-3333-444455556666");
    let second = emulator.create_ok(other_token).await;
    assert_ne!(
        text(&second, "id"),
        first_id,
        "case \"fresh token\": a new token must mint a new environment"
    );

    // And exactly two environments exist, not four.
    let (status, response) = emulator.list("").await;
    assert_eq!(status, 200, "ListEnvironments answered {status}: {response}");
    assert_eq!(
        response["environments"].as_array().map(Vec::len),
        Some(2),
        "two creates with distinct tokens and two replays must leave two environments: {response}"
    );
}

#[tokio::test]
async fn list_environments_pages_the_environments_in_state() {
    // Five environments, created in a known order.
    let emulator = Emulator::new();
    let mut ids = Vec::new();
    for index in 0..5 {
        let summary = emulator
            .create_ok(json!({
                "desktopArn": desktop_arn("workspaces", &format!("workspace/ws-00000000{index}")),
                "name": format!("env {index}")
            }))
            .await;
        ids.push(text(&summary, "id"));
    }

    // Every page size, including the bounds of the model's `maxResults` range
    // (1..50) and one in between: the pages must partition the set, in creation
    // order, with `nextToken` present on every page but the last.
    for page_size in [1, 2, 3, 5, 50] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            let query = match &token {
                None => format!("?maxResults={page_size}"),
                Some(t) => format!("?maxResults={page_size}&nextToken={t}"),
            };
            let (status, response) = emulator.list(&query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListEnvironments answered {status}: {response}"
            );
            let page = response["environments"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no environments array in {response}"));
            assert!(
                page.len() <= page_size as usize,
                "case {case:?}: a page of {} exceeds maxResults, in {response}",
                page.len()
            );
            seen.extend(page.iter().map(|env| text(env, "id")));
            match response["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, ids,
            "case {case:?}: the pages must enumerate every environment exactly once, \
             in creation order"
        );
    }

    // A token this service did not mint names no position, so it is refused.
    for bad in ["not-a-token", "tc-env-zzzz"] {
        let (status, response) = emulator.list(&format!("?nextToken={bad}")).await;
        assert_eq!(
            status, 400,
            "case {bad:?}: an unrecognised nextToken must be refused, got {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {bad:?}: wrong error code in {response}"
        );
    }

    // A deleted environment leaves the listing, and the remaining pages still
    // partition what is left.
    let removed = ids[2].clone();
    let (status, _) = emulator.delete(&removed).await;
    assert_eq!(
        status, 204,
        "case \"delete the middle environment\": got {status}"
    );
    let mut remaining: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            None => "?maxResults=2".to_string(),
            Some(t) => format!("?maxResults=2&nextToken={t}"),
        };
        let (status, response) = emulator.list(&query).await;
        assert_eq!(
            status, 200,
            "after delete: ListEnvironments answered {status}: {response}"
        );
        remaining.extend(
            response["environments"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|env| text(env, "id")),
        );
        match response["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    let expected: Vec<String> = ids.iter().filter(|id| **id != removed).cloned().collect();
    assert_eq!(
        remaining, expected,
        "case \"after delete\": the deleted environment must not appear"
    );
}

#[tokio::test]
async fn update_environment_writes_only_the_members_the_request_carries() {
    // One row per updatable member: the request carries exactly that member, so
    // every other member must come back unchanged. The flag says whether the member
    // is also part of `EnvironmentSummary` — the shape this operation returns — since
    // `deviceCreationTags` belongs to `Environment` only (`smithy_query` query=shape
    // shape=EnvironmentSummary,Environment).
    let cases: &[(&str, Value, bool)] = &[
        ("name", json!("renamed desk"), true),
        (
            "desktopArn",
            json!(desktop_arn("workspaces-web", "portal/8f2b1c3d")),
            true,
        ),
        (
            "desktopEndpoint",
            json!("https://sso.example.org/login"),
            true,
        ),
        ("softwareSetUpdateSchedule", json!("APPLY_IMMEDIATELY"), true),
        ("softwareSetUpdateMode", json!("USE_LATEST"), true),
        ("desiredSoftwareSetId", json!("7"), true),
        ("deviceCreationTags", json!({ "Fleet": "lab" }), false),
        (
            "maintenanceWindow",
            json!({ "type": "SYSTEM", "applyTimeOf": "DEVICE" }),
            true,
        ),
    ];

    for (member, value, in_summary) in cases {
        let emulator = Emulator::new();
        let created = emulator
            .create_ok(json!({
                "desktopArn": desktop_arn("appstream", "stack/analyst-stack"),
                "name": "analyst desk",
                "desktopEndpoint": "https://login.example.com/start",
                "softwareSetUpdateSchedule": "USE_MAINTENANCE_WINDOW",
                "softwareSetUpdateMode": "USE_DESIRED",
                "desiredSoftwareSetId": "412",
                "deviceCreationTags": { "Fleet": "analysts" },
                "maintenanceWindow": { "type": "CUSTOM", "startTimeHour": 2, "applyTimeOf": "UTC" }
            }))
            .await;
        let id = text(&created, "id");
        let before = emulator.get(&id).await.1["environment"].clone();

        let (status, response) = emulator.update(&id, json!({ *member: value.clone() })).await;
        assert_eq!(
            status, 200,
            "case {member:?}: UpdateEnvironment answered {status}: {response}"
        );

        // The persisted record carries the new value...
        let after = emulator.get(&id).await.1["environment"].clone();
        assert_eq!(
            &after[*member], value,
            "case {member:?}: the update was not persisted; got {} in {after}",
            after[*member]
        );
        if *in_summary {
            assert_eq!(
                response["environment"][*member], *value,
                "case {member:?}: the response must project the updated record"
            );
        }

        // ...and nothing else moved, except `updatedAt` and, for `desktopArn`, the
        // `desktopType` derived from it.
        let derived = if *member == "desktopArn" {
            vec!["desktopArn", "desktopType"]
        } else {
            vec![*member]
        };
        for key in before.as_object().expect("an object").keys() {
            if key == "updatedAt" || derived.contains(&key.as_str()) {
                continue;
            }
            assert_eq!(
                after[key], before[key],
                "case {member:?}: member {key:?} changed from {} to {}",
                before[key], after[key]
            );
        }
        if *member == "desktopArn" {
            assert_eq!(
                text(&after, "desktopType"),
                "workspaces-web",
                "case {member:?}: desktopType must follow the new desktopArn, in {after}"
            );
        }

        let created_at = before["createdAt"].as_f64().expect("a createdAt timestamp");
        let updated_at = after["updatedAt"].as_f64().expect("an updatedAt timestamp");
        assert!(
            updated_at > created_at,
            "case {member:?}: expected updatedAt later than createdAt, got {updated_at} vs {created_at}"
        );
    }

    // An unknown id is a 404 whatever the body asks for.
    let emulator = Emulator::new();
    let (status, response) = emulator.update("zzz999zzz", json!({ "name": "nobody" })).await;
    assert_eq!(
        status, 404,
        "case \"unknown id\": expected 404, got {status}: {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case \"unknown id\": wrong error code in {response}"
    );
}

#[tokio::test]
async fn delete_environment_removes_the_record_from_state() {
    let emulator = Emulator::new();
    let mut ids = Vec::new();
    for index in 0..3 {
        let summary = emulator
            .create_ok(json!({
                "desktopArn": desktop_arn("workspaces", &format!("workspace/ws-00000000{index}")),
                "name": format!("env {index}")
            }))
            .await;
        ids.push(text(&summary, "id"));
    }

    for (index, id) in ids.iter().enumerate() {
        let case = format!("delete {id}");
        let (status, response) = emulator.delete(id).await;
        assert_eq!(status, 204, "case {case:?}: expected 204, got {status}: {response}");
        assert!(
            response.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: a 204 carries no members, got {response}"
        );

        let (status, response) = emulator.get(id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the record must be gone, got {status}: {response}"
        );

        // Deleting it again is a 404: there is no record to remove.
        let (status, response) = emulator.delete(id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a second delete must answer 404, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {response}"
        );

        let (status, response) = emulator.list("").await;
        assert_eq!(status, 200, "case {case:?}: ListEnvironments answered {status}");
        let listed: Vec<String> = response["environments"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no environments array in {response}"))
            .iter()
            .map(|env| text(env, "id"))
            .collect();
        assert_eq!(
            listed,
            ids[index + 1..].to_vec(),
            "case {case:?}: the listing must hold exactly the environments that remain"
        );
    }
}

#[tokio::test]
async fn delete_environment_replayed_with_the_same_client_token_reports_the_original_success() {
    let emulator = Emulator::new();
    let summary = emulator
        .create_ok(json!({
            "desktopArn": desktop_arn("workspaces", "workspace/ws-0a1b2c3d4")
        }))
        .await;
    let id = text(&summary, "id");
    let token = "7c9e6679-7425-40de-944b-e07fc1f90ae7";

    for attempt in 1..=3 {
        let (status, response) = emulator
            .call(
                "DELETE",
                &format!("/environments/{id}?clientToken={token}"),
                None,
            )
            .await;
        assert_eq!(
            status, 204,
            "case \"attempt {attempt}\": a retry with the same clientToken must report the \
             original success, got {status}: {response}"
        );
    }
    let (status, _) = emulator.get(&id).await;
    assert_eq!(status, 404, "the environment must still be gone after the retries");
}

#[tokio::test]
async fn the_store_is_bounded_and_shrinks_again_when_environments_are_deleted() {
    let emulator = Emulator::new();
    let quota = service_com_amazonaws_workspacesthinclient_thinclient::state::ENVIRONMENT_QUOTA;

    // Fill the store to its bound. Every create succeeds up to the quota.
    let mut ids = Vec::with_capacity(quota);
    for index in 0..quota {
        let summary = emulator
            .create_ok(json!({
                "desktopArn": desktop_arn("workspaces", "workspace/ws-0a1b2c3d4"),
                "name": format!("env {index}")
            }))
            .await;
        ids.push(text(&summary, "id"));
    }

    // Past it, a create is refused with the modelled quota error rather than
    // growing the store further.
    for attempt in 1..=2 {
        let (status, response) = emulator
            .create(json!({
                "desktopArn": desktop_arn("workspaces", "workspace/ws-0a1b2c3d4")
            }))
            .await;
        assert_eq!(
            status, 402,
            "case \"attempt {attempt} past the quota\": expected the modelled quota refusal, \
             got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ServiceQuotaExceededException",
            "case \"attempt {attempt} past the quota\": wrong error code in {response}"
        );
    }

    // Releasing a record makes room again: the store shrinks, so the next create
    // succeeds and the one after it is refused once more.
    let (status, _) = emulator.delete(&ids[0]).await;
    assert_eq!(status, 204, "case \"release one record\": got {status}");
    let replacement = emulator
        .create_ok(json!({
            "desktopArn": desktop_arn("appstream", "stack/analyst-stack")
        }))
        .await;
    assert!(
        is_environment_id(&text(&replacement, "id")),
        "case \"create after a release\": expected a minted environment, got {replacement}"
    );
    let (status, response) = emulator
        .create(json!({
            "desktopArn": desktop_arn("workspaces", "workspace/ws-0a1b2c3d4")
        }))
        .await;
    assert_eq!(
        status, 402,
        "case \"full again\": expected the quota refusal, got {status}: {response}"
    );
}

/// One tagging case: a label, the tags passed to `CreateEnvironment`, and the tags
/// `TagResource` adds afterwards.
struct TagCase<'a> {
    label: &'a str,
    initial: &'a [(&'a str, &'a str)],
    added: &'a [(&'a str, &'a str)],
}

#[tokio::test]
async fn tags_are_stored_against_the_resource_arn_and_reported_verbatim() {
    let emulator = Emulator::new();

    // Tag sets of different sizes and shapes: the rule is "what was stored comes back",
    // so each row is checked whole.
    let cases = [
        TagCase {
            label: "no tags at create",
            initial: &[],
            added: &[("Env", "test")],
        },
        TagCase {
            label: "one tag at create",
            initial: &[("Owner", "demo")],
            added: &[("Env", "test")],
        },
        TagCase {
            label: "several tags at create",
            initial: &[("Owner", "demo"), ("Team", "fleet ops"), ("Cost.Center", "42")],
            added: &[("Env", "test"), ("Owner", "replaced")],
        },
    ];

    for TagCase {
        label: case,
        initial,
        added,
    } in cases
    {
        let mut body = json!({
            "desktopArn": desktop_arn("workspaces", "workspace/ws-0a1b2c3d4"),
            "name": "tagged desk"
        });
        if !initial.is_empty() {
            body["tags"] = Value::Object(
                initial
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), json!(*v)))
                    .collect(),
            );
        }
        let summary = emulator.create_ok(body).await;
        let arn = text(&summary, "arn");

        let (status, response) = emulator.list_tags(&arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource answered {status}: {response}"
        );
        assert_eq!(
            tag_map(&response["tags"]),
            expected_tags(initial),
            "case {case:?}: the tags passed to CreateEnvironment must come back verbatim"
        );

        let (status, response) = emulator
            .tag(
                &arn,
                Value::Object(
                    added
                        .iter()
                        .map(|(k, v)| ((*k).to_string(), json!(*v)))
                        .collect(),
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource answered {status}: {response}"
        );

        // TagResource merges: a new key is added, an existing key is overwritten.
        let mut expected = expected_tags(initial);
        expected.extend(expected_tags(added));
        let (_, response) = emulator.list_tags(&arn).await;
        assert_eq!(
            tag_map(&response["tags"]),
            expected,
            "case {case:?}: ListTagsForResource must report create's tags plus TagResource's"
        );
    }

    // An ARN naming no resource this service holds is a 404 on both paths.
    let unknown = format!("arn:aws:thinclient:{REGION}:{ACCOUNT}:environment/zzz999zzz");
    for method in ["ListTagsForResource", "TagResource"] {
        let (status, response) = match method {
            "ListTagsForResource" => emulator.list_tags(&unknown).await,
            _ => emulator.tag(&unknown, json!({ "Env": "test" })).await,
        };
        assert_eq!(
            status, 404,
            "case {method:?}: an unknown ARN must answer 404, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {method:?}: wrong error code in {response}"
        );
    }

    // Deleting the environment takes its tags with it.
    let summary = emulator
        .create_ok(json!({
            "desktopArn": desktop_arn("workspaces", "workspace/ws-0a1b2c3d4"),
            "tags": { "Owner": "demo" }
        }))
        .await;
    let arn = text(&summary, "arn");
    let (status, _) = emulator.delete(&text(&summary, "id")).await;
    assert_eq!(status, 204, "the environment must delete");
    let (status, response) = emulator.list_tags(&arn).await;
    assert_eq!(
        status, 404,
        "case \"tags of a deleted environment\": expected 404, got {status}: {response}"
    );
}

/// Build a tag map of `count` distinct keys, each with a one-character value.
fn tags_of_size(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("Key{index}"), json!("v")))
            .collect(),
    )
}

#[tokio::test]
async fn tag_writes_past_the_tagging_limits_are_refused_and_store_nothing() {
    let emulator = Emulator::new();
    let max = service_com_amazonaws_workspacesthinclient_thinclient::state::MAX_TAGS;
    let max_key = service_com_amazonaws_workspacesthinclient_thinclient::state::MAX_TAG_KEY_CHARS;
    let max_value =
        service_com_amazonaws_workspacesthinclient_thinclient::state::MAX_TAG_VALUE_CHARS;

    // At the published bound a create is accepted; one tag past it is refused.
    let at_bound = emulator
        .create_ok(json!({
            "desktopArn": desktop_arn("workspaces", "workspace/ws-0a1b2c3d4"),
            "tags": tags_of_size(max)
        }))
        .await;
    let (_, response) = emulator.list_tags(&text(&at_bound, "arn")).await;
    assert_eq!(
        tag_map(&response["tags"]).len(),
        max,
        "case \"create at the tag bound\": every tag must be stored, got {response}"
    );

    // One row per limit, each tried on CreateEnvironment: the request is refused
    // and no environment is minted for it.
    let refused: &[(&str, Value)] = &[
        ("one tag past the limit", tags_of_size(max + 1)),
        (
            "a tag key one character too long",
            json!({ "k".repeat(max_key + 1): "v" }),
        ),
        (
            "a tag value one character too long",
            json!({ "Key": "v".repeat(max_value + 1) }),
        ),
    ];
    let (_, before) = emulator.list("").await;
    let count_before = before["environments"].as_array().map(Vec::len);
    for (case, tags) in refused {
        let (status, response) = emulator
            .create(json!({
                "desktopArn": desktop_arn("workspaces", "workspace/ws-0a1b2c3d4"),
                "tags": tags.clone()
            }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the tag write to be refused, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: wrong error code in {response}"
        );
    }
    let (_, after) = emulator.list("").await;
    assert_eq!(
        after["environments"].as_array().map(Vec::len),
        count_before,
        "a refused create must not mint an environment"
    );

    // And on TagResource: the merge is judged on the result, so re-writing a key
    // the resource already holds stays within the bound while a new key past it
    // is refused — and nothing is stored on refusal.
    let arn = text(&at_bound, "arn");
    let (status, response) = emulator.tag(&arn, json!({ "Key0": "rewritten" })).await;
    assert_eq!(
        status, 200,
        "case \"overwrite an existing key at the bound\": expected success, got {status}: {response}"
    );
    let (status, response) = emulator.tag(&arn, json!({ "Fresh": "v" })).await;
    assert_eq!(
        status, 400,
        "case \"a new key past the bound\": expected a refusal, got {status}: {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case \"a new key past the bound\": wrong error code in {response}"
    );
    let (_, response) = emulator.list_tags(&arn).await;
    let stored = tag_map(&response["tags"]);
    assert_eq!(
        stored.len(),
        max,
        "case \"a new key past the bound\": the refused write must store nothing, got {response}"
    );
    assert_eq!(
        stored.get("Key0").map(String::as_str),
        Some("rewritten"),
        "case \"overwrite an existing key at the bound\": the accepted write must persist, \
         got {response}"
    );
    assert!(
        !stored.contains_key("Fresh"),
        "case \"a new key past the bound\": the refused key must not be stored, got {response}"
    );
}
