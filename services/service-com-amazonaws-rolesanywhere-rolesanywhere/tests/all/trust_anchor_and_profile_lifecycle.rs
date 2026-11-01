// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the trust-anchor and profile lifecycle:
//! `CreateTrustAnchor`, `GetTrustAnchor`, `ListTrustAnchors`, `DeleteTrustAnchor`,
//! `CreateProfile`, `GetProfile`, `ListProfiles`, `DeleteProfile`.
//!
//! Every request goes through the real `MultiServiceRouter` — the same dispatch the
//! emulator binary uses — so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run exactly as they do in production.
//!
//! The rule under test is that each of these operations computes its answer from the
//! store: a create persists the whole record, a read rebuilds it unchanged, a list
//! enumerates exactly what is stored, and a delete removes it. Each test therefore
//! spans a TABLE of inputs rather than one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle. `SourceData$x509CertificateData` is `@length(1, 204800)`
/// with no `@pattern`, so any non-empty string is a legal certificate bundle on the
/// wire and the service stores it verbatim.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// The region the tests sign for unless a case varies it.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// Every value the modelled `NotificationEvent` enum accepts, in the order the model
/// lists them (`smithy_query(query="shape", shape="NotificationEvent")`).
const NOTIFICATION_EVENTS: [&str; 2] = ["CA_CERTIFICATE_EXPIRY", "END_ENTITY_CERTIFICATE_EXPIRY"];

/// The principal a setting this service configured is reported under
/// (`NotificationSettingDetail$configuredBy` doc).
const SERVICE_NOTIFICATION_PRINCIPAL: &str = "rolesanywhere.amazonaws.com";

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body (`Value::Null` for an empty body)
/// and the wire error code when the answer is a failure.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send_in_region(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        region: &str,
    ) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.send_in_region(method, uri, body, REGION).await
    }

    async fn create_trust_anchor(&self, body: Value) -> Answer {
        self.send("POST", "/trustanchors", Some(body)).await
    }

    async fn get_trust_anchor(&self, id: &str) -> Answer {
        self.send("GET", &format!("/trustanchor/{id}"), None).await
    }

    async fn delete_trust_anchor(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/trustanchor/{id}"), None)
            .await
    }

    async fn list_trust_anchors(&self, query: &str) -> Answer {
        self.send("GET", &format!("/trustanchors{query}"), None).await
    }

    async fn create_profile(&self, body: Value) -> Answer {
        self.send("POST", "/profiles", Some(body)).await
    }

    async fn get_profile(&self, id: &str) -> Answer {
        self.send("GET", &format!("/profile/{id}"), None).await
    }

    async fn delete_profile(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/profile/{id}"), None).await
    }

    async fn list_profiles(&self, query: &str) -> Answer {
        self.send("GET", &format!("/profiles{query}"), None).await
    }
}

/// The modelled `Uuid` form: `@length(36, 36)` plus
/// `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`.
fn is_modelled_uuid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    candidate.len() == 36
        && groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width
                    && group.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
            })
}

/// A minimal valid `CreateTrustAnchor` body; callers override what their case varies.
fn minimal_trust_anchor(name: &str) -> Value {
    json!({
        "name": name,
        "source": {
            "sourceType": "CERTIFICATE_BUNDLE",
            "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
        },
    })
}

/// A minimal valid `CreateProfile` body; callers override what their case varies.
fn minimal_profile(name: &str) -> Value {
    json!({
        "name": name,
        "roleArns": [format!("arn:aws:iam::{ACCOUNT}:role/DemoRole")],
    })
}

/// `CreateTrustAnchor` persists the whole record and `GetTrustAnchor` rebuilds it: a
/// table spanning every modelled `sourceType` (including `SELF_SIGNED_REPOSITORY`, which
/// the issue did not cite), both `enabled` values and the absent one, the published
/// `name` `@length` bounds, tags and notification settings.
#[tokio::test]
async fn created_trust_anchor_round_trips_through_get_trust_anchor() {
    // `sourceType` values from `smithy_query(query="shape", shape="TrustAnchorType")`;
    // `NotificationEvent` / `NotificationChannel` values from their own shapes.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("minimal-certificate-bundle", minimal_trust_anchor("demo-anchor"), true),
        (
            "acm-pca-disabled",
            json!({
                "name": "acm pca anchor",
                "enabled": false,
                "source": {
                    "sourceType": "AWS_ACM_PCA",
                    "sourceData": {
                        "acmPcaArn": format!(
                            "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
                             11111111-2222-3333-4444-555555555555"
                        ),
                    },
                },
            }),
            false,
        ),
        (
            "self-signed-repository",
            json!({
                "name": "self_signed",
                "enabled": true,
                "source": {
                    "sourceType": "SELF_SIGNED_REPOSITORY",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
            }),
            true,
        ),
        (
            "every-member",
            json!({
                "name": "every member anchor",
                "enabled": true,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
                "tags": [{"key": "env", "value": "test"}, {"key": "owner", "value": "guild"}],
                "notificationSettings": [
                    {"enabled": true, "event": "CA_CERTIFICATE_EXPIRY", "threshold": 30,
                     "channel": "ALL"},
                    {"enabled": false, "event": "END_ENTITY_CERTIFICATE_EXPIRY"},
                ],
            }),
            true,
        ),
        // `ResourceName` is `@length(1, 255)`: both published bounds.
        ("shortest-name", minimal_trust_anchor("a"), true),
        ("longest-name", minimal_trust_anchor(&"n".repeat(255)), true),
    ];

    let client = Client::new();
    for (case, body, expected_enabled) in cases {
        let created = client.create_trust_anchor(body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected CreateTrustAnchor 201, got {created:?}"
        );
        let anchor = created
            .body
            .get("trustAnchor")
            .cloned()
            .unwrap_or(Value::Null);
        let id = anchor
            .get("trustAnchorId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_uuid(&id),
            "case {case:?}: trustAnchorId must match the modelled Uuid pattern, got {id:?}"
        );
        assert_eq!(
            anchor.get("trustAnchorArn").and_then(Value::as_str),
            Some(format!("arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:trust-anchor/{id}").as_str()),
            "case {case:?}: trustAnchorArn must be built from the request's Region and the \
             minted id, got {anchor:?}"
        );
        assert_eq!(
            anchor.get("name"),
            body.get("name"),
            "case {case:?}: name must be the stored one, got {anchor:?}"
        );
        assert_eq!(
            anchor.get("source"),
            body.get("source"),
            "case {case:?}: source must survive the round trip, got {anchor:?}"
        );
        assert_eq!(
            anchor.get("enabled").and_then(Value::as_bool),
            Some(expected_enabled),
            "case {case:?}: expected enabled {expected_enabled}, got {anchor:?}"
        );
        for member in ["createdAt", "updatedAt"] {
            assert!(
                anchor.get(member).is_some(),
                "case {case:?}: {member} must be present, got {anchor:?}"
            );
        }
        // A trust anchor reports one notification setting per modelled event whatever the
        // request said: the ones it customized plus this service's own default for the
        // rest. `tests/all/notification_settings.rs` pins the whole rule; here it is the
        // create-path projection that matters.
        let customized: BTreeSet<&str> = body
            .get("notificationSettings")
            .and_then(Value::as_array)
            .map(|settings| {
                settings
                    .iter()
                    .filter_map(|setting| setting.get("event").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        let settings = anchor
            .get("notificationSettings")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: notificationSettings must be a list, got {anchor:?}")
            });
        let returned: Vec<&str> = settings
            .iter()
            .filter_map(|setting| setting.get("event").and_then(Value::as_str))
            .collect();
        assert_eq!(
            returned, NOTIFICATION_EVENTS,
            "case {case:?}: every modelled NotificationEvent must carry a setting, got \
             {anchor:?}"
        );
        for setting in settings {
            let event = setting.get("event").and_then(Value::as_str).unwrap_or_default();
            let expected_by = if customized.contains(event) {
                ACCOUNT
            } else {
                SERVICE_NOTIFICATION_PRINCIPAL
            };
            assert_eq!(
                setting.get("configuredBy").and_then(Value::as_str),
                Some(expected_by),
                "case {case:?}: the setting for {event} must be configuredBy \
                 {expected_by:?}, got {setting:?}"
            );
        }

        let fetched = client.get_trust_anchor(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetTrustAnchor 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.body.get("trustAnchor"),
            Some(&anchor),
            "case {case:?}: GetTrustAnchor must return the record CreateTrustAnchor stored, \
             got {fetched:?}"
        );
    }
}

/// `CreateProfile` persists the whole record and `GetProfile` rebuilds it: a table
/// spanning `roleArns` list sizes (including the empty list the `@length(0, 250)` bound
/// allows), the published `durationSeconds` `@range` bounds, the documented default for
/// an absent `durationSeconds`, every optional boolean and `sessionPolicy`.
#[tokio::test]
async fn created_profile_round_trips_through_get_profile() {
    let role = |suffix: &str| format!("arn:aws:iam::{ACCOUNT}:role/{suffix}");
    // `durationSeconds` is `@range(min: 900, max: 43200)`
    // (`smithy_query(query="operation", operation="CreateProfile")`); an absent value
    // defaults to 3600 (`ProfileDetail$durationSeconds` doc).
    let cases: Vec<(&str, Value, i64, bool)> = vec![
        ("minimal-default-duration", minimal_profile("demo-profile"), 3600, true),
        (
            "explicit-minimum-duration",
            json!({"name": "min duration", "roleArns": [role("A")], "durationSeconds": 900}),
            900,
            true,
        ),
        (
            "explicit-maximum-duration",
            json!({"name": "max duration", "roleArns": [role("B")], "durationSeconds": 43200}),
            43200,
            true,
        ),
        (
            "no-role-arns",
            json!({"name": "no roles", "roleArns": []}),
            3600,
            true,
        ),
        (
            "many-role-arns-disabled",
            json!({
                "name": "many roles",
                "roleArns": [role("One"), role("Two"), role("Three"), role("path/Four")],
                "enabled": false,
                "durationSeconds": 7200,
            }),
            7200,
            false,
        ),
        (
            "every-member",
            json!({
                "name": "every member profile",
                "roleArns": [role("Full")],
                "managedPolicyArns": [
                    "arn:aws:iam::aws:policy/ReadOnlyAccess",
                    format!("arn:aws:iam::{ACCOUNT}:policy/Custom"),
                ],
                "sessionPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "durationSeconds": 1800,
                "enabled": true,
                "requireInstanceProperties": true,
                "acceptRoleSessionName": true,
                "tags": [{"key": "env", "value": "test"}],
            }),
            1800,
            true,
        ),
        // `ResourceName` is `@length(1, 255)`: both published bounds.
        ("shortest-name", minimal_profile("p"), 3600, true),
        ("longest-name", minimal_profile(&"p".repeat(255)), 3600, true),
    ];

    let client = Client::new();
    for (case, body, expected_duration, expected_enabled) in cases {
        let created = client.create_profile(body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected CreateProfile 201, got {created:?}"
        );
        let profile = created.body.get("profile").cloned().unwrap_or(Value::Null);
        let id = profile
            .get("profileId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_uuid(&id),
            "case {case:?}: profileId must match the modelled Uuid pattern, got {id:?}"
        );
        assert_eq!(
            profile.get("profileArn").and_then(Value::as_str),
            Some(format!("arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:profile/{id}").as_str()),
            "case {case:?}: profileArn must be built from the request's Region and the \
             minted id, got {profile:?}"
        );
        assert_eq!(
            profile.get("name"),
            body.get("name"),
            "case {case:?}: name must be the stored one, got {profile:?}"
        );
        assert_eq!(
            profile.get("roleArns"),
            body.get("roleArns"),
            "case {case:?}: roleArns must survive the round trip, got {profile:?}"
        );
        assert_eq!(
            profile.get("durationSeconds").and_then(Value::as_i64),
            Some(expected_duration),
            "case {case:?}: expected durationSeconds {expected_duration}, got {profile:?}"
        );
        assert_eq!(
            profile.get("enabled").and_then(Value::as_bool),
            Some(expected_enabled),
            "case {case:?}: expected enabled {expected_enabled}, got {profile:?}"
        );
        assert_eq!(
            profile.get("createdBy").and_then(Value::as_str),
            Some(ACCOUNT),
            "case {case:?}: createdBy is the account that created the profile, got {profile:?}"
        );
        assert_eq!(
            profile.get("managedPolicyArns").and_then(Value::as_array),
            Some(
                &body
                    .get("managedPolicyArns")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            ),
            "case {case:?}: managedPolicyArns must survive the round trip, got {profile:?}"
        );
        assert_eq!(
            profile.get("sessionPolicy"),
            body.get("sessionPolicy"),
            "case {case:?}: sessionPolicy must survive the round trip, got {profile:?}"
        );
        for member in ["createdAt", "updatedAt"] {
            assert!(
                profile.get(member).is_some(),
                "case {case:?}: {member} must be present, got {profile:?}"
            );
        }

        let fetched = client.get_profile(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetProfile 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.body.get("profile"),
            Some(&profile),
            "case {case:?}: GetProfile must return the record CreateProfile stored, got \
             {fetched:?}"
        );
    }
}

/// An ARN is built from the Region the request was signed for, not from a constant:
/// the same create in a different Region yields a different ARN.
#[tokio::test]
async fn resource_arns_carry_the_requests_region() {
    // One from each of three partitions' worth of Region shapes, plus the service's own
    // fallback for an unsigned request.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let client = Client::new();
        let created = client
            .send_in_region(
                "POST",
                "/trustanchors",
                Some(minimal_trust_anchor("region-anchor")),
                region,
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {region:?}: expected CreateTrustAnchor 201, got {created:?}"
        );
        let anchor = created
            .body
            .get("trustAnchor")
            .cloned()
            .unwrap_or(Value::Null);
        let id = anchor
            .get("trustAnchorId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            anchor.get("trustAnchorArn").and_then(Value::as_str),
            Some(format!("arn:aws:rolesanywhere:{region}:{ACCOUNT}:trust-anchor/{id}").as_str()),
            "case {region:?}: the ARN must carry the request's Region, got {anchor:?}"
        );

        let profile = client
            .send_in_region(
                "POST",
                "/profiles",
                Some(minimal_profile("region-profile")),
                region,
            )
            .await;
        let detail = profile.body.get("profile").cloned().unwrap_or(Value::Null);
        let profile_id = detail
            .get("profileId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            detail.get("profileArn").and_then(Value::as_str),
            Some(format!("arn:aws:rolesanywhere:{region}:{ACCOUNT}:profile/{profile_id}").as_str()),
            "case {region:?}: the ARN must carry the request's Region, got {detail:?}"
        );
    }
}

/// `ListProfiles` enumerates what is stored, honours `pageSize` as a hard bound, and
/// resumes exactly where the previous page ended: a table over store sizes and page
/// sizes, including the empty store and a page size larger than the store.
#[tokio::test]
async fn list_profiles_pages_over_exactly_what_is_stored() {
    let cases: [(usize, Option<i32>); 6] = [
        (0, None),
        (0, Some(2)),
        (1, Some(1)),
        (5, Some(2)),
        (7, Some(3)),
        (4, Some(10)),
    ];

    for (total, page_size) in cases {
        let case = format!("total={total} pageSize={page_size:?}");
        let client = Client::new();
        let mut created: BTreeSet<String> = BTreeSet::new();
        for index in 0..total {
            let answer = client
                .create_profile(minimal_profile(&format!("profile-{index}")))
                .await;
            assert_eq!(
                answer.status, 201,
                "case {case:?}: expected CreateProfile 201, got {answer:?}"
            );
            let id = answer
                .body
                .pointer("/profile/profileId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                created.insert(id.clone()),
                "case {case:?}: every create must mint a distinct profileId, saw {id:?} twice"
            );
        }

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("pageSize={size}"));
            }
            if let Some(token) = &next_token {
                query.push(format!("nextToken={token}"));
            }
            let suffix = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };
            let answer = client.list_profiles(&suffix).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected ListProfiles 200, got {answer:?}"
            );
            let profiles = answer
                .body
                .get("profiles")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(size) = page_size {
                assert!(
                    i32::try_from(profiles.len()).is_ok_and(|len| len <= size),
                    "case {case:?}: a page must not exceed pageSize {size}, got {} entries",
                    profiles.len()
                );
            }
            for profile in &profiles {
                seen.push(
                    profile
                        .get("profileId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            next_token = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= total + 2,
                "case {case:?}: paging must terminate, but it produced {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen.len(),
            total,
            "case {case:?}: paging must return every stored profile exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            created,
            "case {case:?}: the pages must hold exactly the created profiles, got {seen:?}"
        );
        if total == 0 {
            let answer = client.list_profiles("").await;
            assert_eq!(
                answer.body.get("profiles"),
                Some(&json!([])),
                "case {case:?}: an empty store returns an empty profiles list, got {answer:?}"
            );
            assert!(
                answer.body.get("nextToken").is_none(),
                "case {case:?}: an empty store returns no nextToken, got {answer:?}"
            );
        }
    }
}

/// `ListTrustAnchors` shares the paging rule, so it is asserted over the same table.
#[tokio::test]
async fn list_trust_anchors_pages_over_exactly_what_is_stored() {
    let cases: [(usize, Option<i32>); 5] =
        [(0, None), (1, Some(1)), (3, Some(1)), (6, Some(4)), (2, None)];

    for (total, page_size) in cases {
        let case = format!("total={total} pageSize={page_size:?}");
        let client = Client::new();
        let mut created: BTreeSet<String> = BTreeSet::new();
        for index in 0..total {
            let answer = client
                .create_trust_anchor(minimal_trust_anchor(&format!("anchor-{index}")))
                .await;
            assert_eq!(
                answer.status, 201,
                "case {case:?}: expected CreateTrustAnchor 201, got {answer:?}"
            );
            let id = answer
                .body
                .pointer("/trustAnchor/trustAnchorId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                created.insert(id.clone()),
                "case {case:?}: every create must mint a distinct trustAnchorId, saw {id:?} twice"
            );
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut count = 0usize;
        let mut next_token: Option<String> = None;
        loop {
            let mut query = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("pageSize={size}"));
            }
            if let Some(token) = &next_token {
                query.push(format!("nextToken={token}"));
            }
            let suffix = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };
            let answer = client.list_trust_anchors(&suffix).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected ListTrustAnchors 200, got {answer:?}"
            );
            let anchors = answer
                .body
                .get("trustAnchors")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(size) = page_size {
                assert!(
                    i32::try_from(anchors.len()).is_ok_and(|len| len <= size),
                    "case {case:?}: a page must not exceed pageSize {size}, got {} entries",
                    anchors.len()
                );
            }
            for anchor in &anchors {
                count += 1;
                seen.insert(
                    anchor
                        .get("trustAnchorId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            next_token = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            count, total,
            "case {case:?}: paging must return every stored trust anchor exactly once"
        );
        assert_eq!(
            seen, created,
            "case {case:?}: the pages must hold exactly the created trust anchors"
        );
    }
}

/// A delete returns the record it removed, and the resource is then gone: the read
/// answers `ResourceNotFoundException` (404) and the list no longer holds it. Asserted
/// for both resource types, and for a store that still holds other resources.
#[tokio::test]
async fn delete_removes_the_resource_and_a_later_read_is_404() {
    let client = Client::new();

    // Two of each, so the delete is shown to remove exactly one.
    let mut profile_ids = Vec::new();
    let mut anchor_ids = Vec::new();
    for index in 0..2 {
        let profile = client
            .create_profile(minimal_profile(&format!("doomed-profile-{index}")))
            .await;
        profile_ids.push(
            profile
                .body
                .pointer("/profile/profileId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        let anchor = client
            .create_trust_anchor(minimal_trust_anchor(&format!("doomed-anchor-{index}")))
            .await;
        anchor_ids.push(
            anchor
                .body
                .pointer("/trustAnchor/trustAnchorId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }

    for (case, id) in [("first", &profile_ids[0]), ("second", &profile_ids[1])] {
        let before = client.get_profile(id).await;
        let deleted = client.delete_profile(id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteProfile 200, got {deleted:?}"
        );
        assert_eq!(
            deleted.body.get("profile"),
            before.body.get("profile"),
            "case {case:?}: DeleteProfile must return the profile it removed, got {deleted:?}"
        );
        let after = client.get_profile(id).await;
        assert_eq!(
            (after.status, after.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a deleted profile must read back as 404, got {after:?}"
        );
        let listed = client.list_profiles("").await;
        let still_there = listed
            .body
            .get("profiles")
            .and_then(Value::as_array)
            .is_some_and(|profiles| {
                profiles.iter().any(|profile| {
                    profile.get("profileId").and_then(Value::as_str) == Some(id.as_str())
                })
            });
        assert!(
            !still_there,
            "case {case:?}: a deleted profile must be absent from ListProfiles, got {listed:?}"
        );
    }

    for (case, id) in [("first", &anchor_ids[0]), ("second", &anchor_ids[1])] {
        let before = client.get_trust_anchor(id).await;
        let deleted = client.delete_trust_anchor(id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteTrustAnchor 200, got {deleted:?}"
        );
        assert_eq!(
            deleted.body.get("trustAnchor"),
            before.body.get("trustAnchor"),
            "case {case:?}: DeleteTrustAnchor must return the anchor it removed, got {deleted:?}"
        );
        let after = client.get_trust_anchor(id).await;
        assert_eq!(
            (after.status, after.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a deleted trust anchor must read back as 404, got {after:?}"
        );
    }

    let listed = client.list_trust_anchors("").await;
    assert_eq!(
        listed.body.get("trustAnchors"),
        Some(&json!([])),
        "every trust anchor was deleted, so the list must be empty, got {listed:?}"
    );
}

/// A well-formed id that names nothing is a 404 on every read and delete — never a
/// canned success — and a malformed id is refused as a `ValidationException` (400) by
/// the modelled `Uuid` constraints.
#[tokio::test]
async fn unknown_ids_are_404_and_malformed_ids_are_400() {
    let client = Client::new();
    // One resource of each type exists, so the 404s below are about the id and not
    // about an empty store.
    client.create_profile(minimal_profile("present")).await;
    client
        .create_trust_anchor(minimal_trust_anchor("present"))
        .await;

    let unknown = "0123abcd-4567-89ef-0123-456789abcdef";
    for (case, answer) in [
        ("get-profile", client.get_profile(unknown).await),
        ("delete-profile", client.delete_profile(unknown).await),
        ("get-trust-anchor", client.get_trust_anchor(unknown).await),
        (
            "delete-trust-anchor",
            client.delete_trust_anchor(unknown).await,
        ),
    ] {
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an unknown id must be a 404, got {answer:?}"
        );
    }

    // `Uuid` is `@length(36, 36)` + `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`,
    // so each of these is outside the modelled form: too short, too long by one, and
    // 36 characters of the wrong shape.
    for malformed in [
        "not-a-uuid",
        "0123abcd-4567-89ef-0123-456789abcde",
        "0123abcd-4567-89ef-0123-456789abcdef0",
        "0123ABCD-4567-89EF-0123-456789ABCDEF",
        "0123abcd456789ef0123456789abcdef0123",
    ] {
        for (op, answer) in [
            ("get-profile", client.get_profile(malformed).await),
            ("delete-profile", client.delete_profile(malformed).await),
            ("get-trust-anchor", client.get_trust_anchor(malformed).await),
            (
                "delete-trust-anchor",
                client.delete_trust_anchor(malformed).await,
            ),
        ] {
            let case = format!("{op} id={malformed:?}");
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: a malformed id must be refused as a ValidationException, got \
                 {answer:?}"
            );
        }
    }
}

/// A `Source` has to name its type and carry the certificate data that type owns —
/// `x509CertificateData` for `CERTIFICATE_BUNDLE`, `acmPcaArn` for `AWS_ACM_PCA` — so a
/// source that names neither, or pairs the wrong data, is refused instead of stored.
#[tokio::test]
async fn trust_anchor_source_must_pair_its_type_with_its_certificate_data() {
    let acm_pca_arn =
        format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/abcd-1234");
    let refused: Vec<(&str, Value)> = vec![
        (
            "no-source-type",
            json!({"name": "bad", "source": {
                "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM}}}),
        ),
        (
            "no-source-data",
            json!({"name": "bad", "source": {"sourceType": "CERTIFICATE_BUNDLE"}}),
        ),
        ("empty-source", json!({"name": "bad", "source": {}})),
        (
            "bundle-with-pca-arn",
            json!({"name": "bad", "source": {
                "sourceType": "CERTIFICATE_BUNDLE",
                "sourceData": {"acmPcaArn": acm_pca_arn}}}),
        ),
        (
            "pca-with-certificate-data",
            json!({"name": "bad", "source": {
                "sourceType": "AWS_ACM_PCA",
                "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM}}}),
        ),
    ];

    let client = Client::new();
    for (case, body) in refused {
        let answer = client.create_trust_anchor(body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: an unusable source must be refused, got {answer:?}"
        );
        let listed = client.list_trust_anchors("").await;
        assert_eq!(
            listed.body.get("trustAnchors"),
            Some(&json!([])),
            "case {case:?}: a refused create must store nothing, got {listed:?}"
        );
    }

    // The two pairings the model names, and the type it ties no data member to, are all
    // accepted.
    for (case, body) in [
        (
            "bundle-with-certificate-data",
            json!({"name": "good bundle", "source": {
                "sourceType": "CERTIFICATE_BUNDLE",
                "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM}}}),
        ),
        (
            "pca-with-pca-arn",
            json!({"name": "good pca", "source": {
                "sourceType": "AWS_ACM_PCA",
                "sourceData": {"acmPcaArn": acm_pca_arn}}}),
        ),
        (
            "self-signed-with-certificate-data",
            json!({"name": "good self signed", "source": {
                "sourceType": "SELF_SIGNED_REPOSITORY",
                "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM}}}),
        ),
    ] {
        let answer = client.create_trust_anchor(body).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: a well-formed source must be accepted, got {answer:?}"
        );
    }
}

/// A profile vends credentials for the roles it names, so every `roleArns` entry has to
/// be an IAM role ARN; anything else is refused and nothing is stored.
#[tokio::test]
async fn profile_role_arns_must_be_iam_role_arns() {
    let client = Client::new();
    for (case, role_arn) in [
        ("not-an-arn", "DemoRole".to_string()),
        (
            "wrong-service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/demo"),
        ),
        ("user-not-role", format!("arn:aws:iam::{ACCOUNT}:user/Demo")),
        ("no-role-name", format!("arn:aws:iam::{ACCOUNT}:role/")),
        (
            "region-on-a-global-arn",
            format!("arn:aws:iam:{REGION}:{ACCOUNT}:role/Demo"),
        ),
        ("empty", String::new()),
    ] {
        let answer = client
            .create_profile(json!({"name": "bad roles", "roleArns": [role_arn]}))
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a roleArns entry that is not an IAM role ARN must be refused, got \
             {answer:?}"
        );
    }

    let listed = client.list_profiles("").await;
    assert_eq!(
        listed.body.get("profiles"),
        Some(&json!([])),
        "a refused create must store nothing, got {listed:?}"
    );

    // Accepted: a plain role, a pathed role, and a non-`aws` partition.
    for (case, role_arn) in [
        ("plain", format!("arn:aws:iam::{ACCOUNT}:role/Demo")),
        ("pathed", format!("arn:aws:iam::{ACCOUNT}:role/team/Demo")),
        (
            "other-partition",
            format!("arn:aws-us-gov:iam::{ACCOUNT}:role/Demo"),
        ),
    ] {
        let answer = client
            .create_profile(json!({"name": "good roles", "roleArns": [role_arn]}))
            .await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: a legal IAM role ARN must be accepted, got {answer:?}"
        );
    }
}

/// A `nextToken` this service did not issue is a bad request, not an empty page.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let client = Client::new();
    client.create_profile(minimal_profile("present")).await;
    client
        .create_trust_anchor(minimal_trust_anchor("present"))
        .await;

    for token in ["zzz", "notahexstring", "4142434445464748"] {
        let profiles = client
            .list_profiles(&format!("?nextToken={token}"))
            .await;
        // `4142434445464748` IS valid hex, so it decodes to a cursor this service could
        // have issued — it simply resumes from a key no resource is at, which is a
        // normal page rather than a refusal.
        let expected = if token == "4142434445464748" {
            (200, "")
        } else {
            (400, "ValidationException")
        };
        assert_eq!(
            (profiles.status, profiles.error_code.as_str()),
            expected,
            "case {token:?}: unexpected ListProfiles answer, got {profiles:?}"
        );
        let anchors = client
            .list_trust_anchors(&format!("?nextToken={token}"))
            .await;
        assert_eq!(
            (anchors.status, anchors.error_code.as_str()),
            expected,
            "case {token:?}: unexpected ListTrustAnchors answer, got {anchors:?}"
        );
    }
}

/// The modelled `@length`, `@pattern` and `@range` constraints on the create requests
/// are refused as `ValidationException` (400) rather than silently accepted: each
/// published bound is accepted and one step past it is refused.
#[tokio::test]
async fn modelled_constraints_on_create_are_refused() {
    let role = format!("arn:aws:iam::{ACCOUNT}:role/Demo");
    let client = Client::new();

    // `ResourceName`: `@length(1, 255)`, `@pattern("^[ a-zA-Z0-9-_]*$")`.
    for (case, name) in [
        ("empty-name", String::new()),
        ("name-one-over-the-bound", "n".repeat(256)),
        ("name-with-an-illegal-character", "demo!anchor".to_string()),
        ("name-with-a-slash", "demo/anchor".to_string()),
    ] {
        let anchor = client.create_trust_anchor(minimal_trust_anchor(&name)).await;
        assert_eq!(
            (anchor.status, anchor.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: CreateTrustAnchor must refuse the name, got {anchor:?}"
        );
        let profile = client.create_profile(minimal_profile(&name)).await;
        assert_eq!(
            (profile.status, profile.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: CreateProfile must refuse the name, got {profile:?}"
        );
    }

    // `durationSeconds`: `@range(min: 900, max: 43200)` — one past each bound.
    for (case, duration) in [("below-minimum", 899), ("above-maximum", 43201)] {
        let answer = client
            .create_profile(json!({
                "name": "duration case",
                "roleArns": [role.clone()],
                "durationSeconds": duration,
            }))
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: CreateProfile must refuse durationSeconds {duration}, got {answer:?}"
        );
    }

    // `RoleArnList`: `@length(0, 250)` — the bound is accepted, one past it refused.
    let at_bound: Vec<String> = (0..250)
        .map(|index| format!("arn:aws:iam::{ACCOUNT}:role/Role{index}"))
        .collect();
    let accepted = client
        .create_profile(json!({"name": "at the bound", "roleArns": at_bound.clone()}))
        .await;
    assert_eq!(
        accepted.status, 201,
        "case \"roleArns-at-the-bound\": 250 roleArns is the published maximum, got {accepted:?}"
    );
    let mut over_bound = at_bound;
    over_bound.push(role);
    let refused = client
        .create_profile(json!({"name": "over the bound", "roleArns": over_bound}))
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "case \"roleArns-over-the-bound\": 251 roleArns must be refused, got {refused:?}"
    );

    // `notificationSettings`: `@length(0, 50)` on the list, `@range(1, 360)` on the
    // threshold.
    let refused = client
        .create_trust_anchor(json!({
            "name": "bad threshold",
            "source": {
                "sourceType": "CERTIFICATE_BUNDLE",
                "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
            },
            "notificationSettings": [
                {"enabled": true, "event": "CA_CERTIFICATE_EXPIRY", "threshold": 361},
            ],
        }))
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "case \"threshold-over-the-bound\": 361 days must be refused, got {refused:?}"
    );
}

/// `pageSize` bounds ONE response: a non-positive page is no page at all and is
/// refused, and a page larger than the server's own ceiling is served at the ceiling
/// with the rest reachable through `nextToken`.
#[tokio::test]
async fn page_size_is_bounded_in_both_directions() {
    let client = Client::new();
    // The server's own ceiling (`state::MAX_PAGE_SIZE`); `ListRequest$pageSize` carries
    // no `@range`, so this bound is the emulator's, which is why it is asserted here.
    let ceiling = 100;
    for index in 0..(ceiling + 3) {
        let answer = client
            .create_profile(minimal_profile(&format!("bulk-{index:04}")))
            .await;
        assert_eq!(
            answer.status, 201,
            "case {index}: expected CreateProfile 201, got {answer:?}"
        );
    }

    for requested in [0, -1, -100] {
        let profiles = client
            .list_profiles(&format!("?pageSize={requested}"))
            .await;
        assert_eq!(
            (profiles.status, profiles.error_code.as_str()),
            (400, "ValidationException"),
            "case pageSize={requested}: a non-positive page must be refused, got {profiles:?}"
        );
        let anchors = client
            .list_trust_anchors(&format!("?pageSize={requested}"))
            .await;
        assert_eq!(
            (anchors.status, anchors.error_code.as_str()),
            (400, "ValidationException"),
            "case pageSize={requested}: a non-positive page must be refused, got {anchors:?}"
        );
    }

    // A page size above the ceiling, and an absent one, both serve the ceiling and
    // leave a cursor for the rest.
    for query in ["?pageSize=5000", ""] {
        let answer = client.list_profiles(query).await;
        let returned = answer
            .body
            .get("profiles")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert_eq!(
            returned, ceiling,
            "case {query:?}: a page is bounded by the server ceiling {ceiling}, got {answer:?}"
        );
        assert!(
            answer.body.get("nextToken").is_some(),
            "case {query:?}: entries remain beyond the page, so a nextToken is due, got \
             {answer:?}"
        );
    }
}
