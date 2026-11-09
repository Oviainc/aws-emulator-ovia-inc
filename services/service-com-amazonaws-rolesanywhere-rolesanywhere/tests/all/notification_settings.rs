// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for a trust anchor's notification settings and for the writes
//! that change a trust anchor: `PutNotificationSettings`, `ResetNotificationSettings`
//! and `UpdateTrustAnchor`.
//!
//! The rules under test:
//!
//! * A trust anchor's `notificationSettings` is a COMPLETE per-key set. From the moment
//!   it is created it carries one `NotificationSettingDetail` for every value of the
//!   modelled `NotificationEvent` enum: this service's own setting (enabled, at the
//!   service threshold, on every channel, `configuredBy` the service principal — "For
//!   default settings configured by IAM Roles Anywhere, the value is
//!   `rolesanywhere.amazonaws.com`") until a caller customizes that key, and the
//!   submitted values `configuredBy` the caller's account afterwards ("for customized
//!   notifications settings, it is the respective account ID"); a reset of a key puts it
//!   back to the service's own setting. Every path that projects a trust anchor reports
//!   the same set.
//! * A write to a trust anchor is persisted: what `UpdateTrustAnchor`,
//!   `PutNotificationSettings` and `ResetNotificationSettings` write is what every later
//!   read, list projection and delete echo reports, and the instant the anchor was last
//!   updated moves with it while the instant it was created does not.
//!
//! Each test therefore spans a TABLE of inputs — which keys are customized, which are
//! reset, which members an update carries — rather than one recorded request.
//!
//! The harness below is local to this suite: each sibling suite's client is private to
//! its own module, and the targets are compiled into one test binary only as separate
//! modules.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle. `SourceData$x509CertificateData` is `@length(1, 204800)`
/// with no `@pattern`, so any non-empty string is a legal certificate bundle on the wire.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// A second, different bundle, so an update that replaces the source is observable.
const REPLACEMENT_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\ncmVwbGFjZW1lbnQtY2VydA==\n-----END CERTIFICATE-----\n";

/// The Region the suite signs for.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`), and the
/// principal a CUSTOMIZED notification setting is reported under.
const ACCOUNT: &str = "000000000000";

/// The principal a setting IAM Roles Anywhere itself configured is reported under
/// (`smithy_query(query="shape", shape="NotificationSettingDetail")`, member
/// `configuredBy`).
const SERVICE_NOTIFICATION_PRINCIPAL: &str = "rolesanywhere.amazonaws.com";

/// Every value the modelled `NotificationEvent` enum accepts, in the order the model
/// lists them (`smithy_query(query="shape", shape="NotificationEvent")`).
const NOTIFICATION_EVENTS: [&str; 2] = ["CA_CERTIFICATE_EXPIRY", "END_ENTITY_CERTIFICATE_EXPIRY"];

/// The only value the modelled `NotificationChannel` enum accepts, and the channel a
/// setting that names none applies to ("In the absence of a specific channel, IAM Roles
/// Anywhere applies this setting to 'ALL' channels").
const ALL_CHANNELS: &str = "ALL";

/// The `threshold` a setting this service configured carries. Recorded behaviour: the
/// model publishes only `@range(1, 360)` for the member.
const SERVICE_THRESHOLD_DAYS: i64 = 45;

/// The published bounds of `NotificationSetting$threshold` (`@range(1, 360)`).
const THRESHOLD_BOUNDS: [i64; 2] = [1, 360];

/// How the service words a refusal when the trust anchor a request ADDRESSES by
/// `trustAnchorId` is not stored.
const TRUST_ANCHOR_NOT_FOUND_MESSAGE: &str = "TrustAnchor not found";

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

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
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
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/rolesanywhere/aws4_request, \
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

    async fn list_trust_anchors(&self) -> Answer {
        self.send("GET", "/trustanchors", None).await
    }

    async fn update_trust_anchor(&self, id: &str, body: Value) -> Answer {
        self.send("PATCH", &format!("/trustanchor/{id}"), Some(body))
            .await
    }

    async fn put_notification_settings(&self, id: &str, settings: Value) -> Answer {
        self.send(
            "PATCH",
            "/put-notifications-settings",
            Some(json!({"trustAnchorId": id, "notificationSettings": settings})),
        )
        .await
    }

    async fn reset_notification_settings(&self, id: &str, keys: Value) -> Answer {
        self.send(
            "PATCH",
            "/reset-notifications-settings",
            Some(json!({"trustAnchorId": id, "notificationSettingKeys": keys})),
        )
        .await
    }
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

/// The `trustAnchor` a successful call answered with.
fn anchor_of(case: &str, expected_status: u16, answer: &Answer) -> Value {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
    );
    answer
        .body
        .get("trustAnchor")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no trustAnchor in {answer:?}"))
}

/// What one notification setting is expected to say: `None` for a key this service
/// configures (its own default), `Some((enabled, threshold))` for a key the caller
/// customized with those submitted values.
type Expected = Option<(bool, Option<i64>)>;

/// Assert the per-key rule over a projected trust anchor: exactly one setting per
/// modelled event, in the model's order, each on every channel, each either this
/// service's own default or the submitted values stamped with the caller's account.
fn assert_settings(case: &str, anchor: &Value, expected: &[Expected; 2]) {
    let settings = anchor
        .get("notificationSettings")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: notificationSettings must be a list, got {anchor:?}")
        });
    assert_eq!(
        settings.len(),
        NOTIFICATION_EVENTS.len(),
        "case {case:?}: expected one setting per modelled event, got {anchor:?}"
    );
    for (index, event) in NOTIFICATION_EVENTS.iter().enumerate() {
        let setting = &settings[index];
        assert_eq!(
            setting.get("event").and_then(Value::as_str),
            Some(*event),
            "case {case:?}: setting {index} must be the one for {event}, got {setting:?}"
        );
        assert_eq!(
            setting.get("channel").and_then(Value::as_str),
            Some(ALL_CHANNELS),
            "case {case:?}: the setting for {event} must name a channel, got {setting:?}"
        );
        let (expected_enabled, expected_threshold, expected_by) = match expected[index] {
            None => (
                true,
                Some(SERVICE_THRESHOLD_DAYS),
                SERVICE_NOTIFICATION_PRINCIPAL,
            ),
            Some((enabled, threshold)) => (enabled, threshold, ACCOUNT),
        };
        assert_eq!(
            setting.get("enabled").and_then(Value::as_bool),
            Some(expected_enabled),
            "case {case:?}: the setting for {event} must report enabled \
             {expected_enabled}, got {setting:?}"
        );
        assert_eq!(
            setting.get("threshold").and_then(Value::as_i64),
            expected_threshold,
            "case {case:?}: the setting for {event} must report threshold \
             {expected_threshold:?}, got {setting:?}"
        );
        assert_eq!(
            setting.get("configuredBy").and_then(Value::as_str),
            Some(expected_by),
            "case {case:?}: the setting for {event} must be configuredBy {expected_by:?}, \
             got {setting:?}"
        );
    }
}

/// The element of a `ListTrustAnchors` page that carries this id.
fn listed(case: &str, page: &Value, id: &str) -> Value {
    page.get("trustAnchors")
        .and_then(Value::as_array)
        .and_then(|anchors| {
            anchors
                .iter()
                .find(|anchor| anchor.get("trustAnchorId").and_then(Value::as_str) == Some(id))
                .cloned()
        })
        .unwrap_or_else(|| panic!("case {case:?}: {id} must appear in the list, got {page:?}"))
}

/// The id of a trust anchor a call answered with.
fn id_of(case: &str, anchor: &Value) -> String {
    anchor
        .get("trustAnchorId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no trustAnchorId in {anchor:?}"))
        .to_string()
}

/// A created trust anchor carries one notification setting per modelled event — the
/// service's own where the request said nothing, the submitted values where it did — and
/// every path that projects that anchor reports the same set.
///
/// The rows differ along what the rule generalises over: how many keys the create
/// customized, which ones, whether a customized setting is enabled, and the threshold it
/// carries (both published `@range` bounds among them).
#[tokio::test]
async fn a_trust_anchor_reports_one_notification_setting_per_modelled_event() {
    let [low_threshold, high_threshold] = THRESHOLD_BOUNDS;
    let cases: Vec<(&str, Value, [Expected; 2])> = vec![
        // Nothing customized: both keys are this service's own.
        ("no-settings", minimal_trust_anchor("default settings"), [None, None]),
        (
            "ca-event-only",
            json!({
                "name": "ca only",
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
                "notificationSettings": [
                    {"enabled": true, "event": NOTIFICATION_EVENTS[0],
                     "threshold": low_threshold},
                ],
            }),
            [Some((true, Some(low_threshold))), None],
        ),
        (
            "end-entity-event-only-disabled",
            json!({
                "name": "end entity only",
                "source": {
                    "sourceType": "AWS_ACM_PCA",
                    "sourceData": {
                        "acmPcaArn": format!(
                            "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
                             11111111-2222-3333-4444-555555555555"
                        ),
                    },
                },
                "notificationSettings": [
                    {"enabled": false, "event": NOTIFICATION_EVENTS[1]},
                ],
            }),
            [None, Some((false, None))],
        ),
        (
            "both-events-at-the-published-bounds",
            json!({
                "name": "both events",
                "source": {
                    "sourceType": "SELF_SIGNED_REPOSITORY",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
                "notificationSettings": [
                    {"enabled": true, "event": NOTIFICATION_EVENTS[1],
                     "threshold": high_threshold, "channel": ALL_CHANNELS},
                    {"enabled": true, "event": NOTIFICATION_EVENTS[0],
                     "threshold": low_threshold, "channel": ALL_CHANNELS},
                ],
            }),
            [
                Some((true, Some(low_threshold))),
                Some((true, Some(high_threshold))),
            ],
        ),
    ];

    let client = Client::new();
    for (case, body, expected) in cases {
        let created = anchor_of(case, 201, &client.create_trust_anchor(body).await);
        let id = id_of(case, &created);
        assert_settings(&format!("{case}/CreateTrustAnchor"), &created, &expected);

        let fetched = anchor_of(case, 200, &client.get_trust_anchor(&id).await);
        assert_settings(&format!("{case}/GetTrustAnchor"), &fetched, &expected);

        let page = client.list_trust_anchors().await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected ListTrustAnchors 200, got {page:?}"
        );
        let element = listed(case, &page.body, &id);
        assert_settings(&format!("{case}/ListTrustAnchors"), &element, &expected);

        let deleted = anchor_of(case, 200, &client.delete_trust_anchor(&id).await);
        assert_settings(&format!("{case}/DeleteTrustAnchor"), &deleted, &expected);
    }
}

/// `crlDpRevocationEnabled` is reported on every projection of a trust anchor, from the
/// stored record: an anchor nothing can turn it on for reports it off, on the write path,
/// the read path, the list projection and the delete echo alike.
#[tokio::test]
async fn every_trust_anchor_projection_reports_crl_dp_revocation() {
    let client = Client::new();
    let created = anchor_of(
        "create",
        201,
        &client
            .create_trust_anchor(minimal_trust_anchor("crl dp anchor"))
            .await,
    );
    let id = id_of("create", &created);

    let page = client.list_trust_anchors().await;
    assert_eq!(
        page.status, 200,
        "expected ListTrustAnchors 200, got {page:?}"
    );
    let rows: Vec<(&str, Value)> = vec![
        ("CreateTrustAnchor", created.clone()),
        (
            "GetTrustAnchor",
            anchor_of("get", 200, &client.get_trust_anchor(&id).await),
        ),
        (
            "UpdateTrustAnchor",
            anchor_of(
                "update",
                200,
                &client
                    .update_trust_anchor(&id, json!({"name": "crl dp anchor renamed"}))
                    .await,
            ),
        ),
        (
            "PutNotificationSettings",
            anchor_of(
                "put",
                200,
                &client
                    .put_notification_settings(
                        &id,
                        json!([{"enabled": true, "event": NOTIFICATION_EVENTS[0],
                                "threshold": 10}]),
                    )
                    .await,
            ),
        ),
        (
            "ResetNotificationSettings",
            anchor_of(
                "reset",
                200,
                &client
                    .reset_notification_settings(&id, json!([{"event": NOTIFICATION_EVENTS[0]}]))
                    .await,
            ),
        ),
        ("ListTrustAnchors", listed("list", &page.body, &id)),
        (
            "DeleteTrustAnchor",
            anchor_of("delete", 200, &client.delete_trust_anchor(&id).await),
        ),
    ];

    for (case, anchor) in rows {
        assert_eq!(
            anchor.get("crlDpRevocationEnabled").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: crlDpRevocationEnabled must be reported, got {anchor:?}"
        );
    }
}

/// `PutNotificationSettings` is read-after-write and `ResetNotificationSettings` undoes
/// exactly the keys it names: the rows walk a trust anchor through a sequence of writes
/// and read the whole set back after each one, through the read, the list projection and
/// the write's own echo.
#[tokio::test]
async fn customizing_and_resetting_notification_settings_is_read_after_write() {
    let [ca, end_entity] = NOTIFICATION_EVENTS;
    let client = Client::new();
    let created = anchor_of(
        "setup",
        201,
        &client
            .create_trust_anchor(minimal_trust_anchor("customized anchor"))
            .await,
    );
    let id = id_of("setup", &created);
    assert_settings("setup", &created, &[None, None]);

    // (case, the write to perform, the set every projection must then report)
    enum Write {
        Put(Value),
        Reset(Value),
    }
    let steps: Vec<(&str, Write, [Expected; 2])> = vec![
        (
            "customize-one-key",
            Write::Put(json!([{"enabled": true, "event": ca, "threshold": 7}])),
            [Some((true, Some(7))), None],
        ),
        (
            "customize-the-other-key-too",
            Write::Put(json!([
                {"enabled": true, "event": end_entity, "threshold": 14, "channel": ALL_CHANNELS},
            ])),
            [Some((true, Some(7))), Some((true, Some(14)))],
        ),
        (
            "replace-a-customized-key",
            Write::Put(json!([{"enabled": false, "event": ca, "threshold": 90}])),
            [Some((false, Some(90))), Some((true, Some(14)))],
        ),
        (
            "write-both-keys-at-once",
            Write::Put(json!([
                {"enabled": true, "event": ca, "threshold": 30},
                {"enabled": true, "event": end_entity, "threshold": 60},
            ])),
            [Some((true, Some(30))), Some((true, Some(60)))],
        ),
        (
            "reset-one-key",
            Write::Reset(json!([{"event": ca}])),
            [None, Some((true, Some(60)))],
        ),
        (
            "reset-a-key-that-is-already-default",
            Write::Reset(json!([{"event": ca, "channel": ALL_CHANNELS}])),
            [None, Some((true, Some(60)))],
        ),
        (
            "an-empty-write-changes-nothing",
            Write::Put(json!([])),
            [None, Some((true, Some(60)))],
        ),
        (
            "reset-every-key",
            Write::Reset(json!([{"event": ca}, {"event": end_entity}])),
            [None, None],
        ),
    ];

    for (case, write, expected) in steps {
        let echoed = match write {
            Write::Put(settings) => anchor_of(
                case,
                200,
                &client.put_notification_settings(&id, settings).await,
            ),
            Write::Reset(keys) => anchor_of(
                case,
                200,
                &client.reset_notification_settings(&id, keys).await,
            ),
        };
        assert_settings(&format!("{case}/echo"), &echoed, &expected);

        let fetched = anchor_of(case, 200, &client.get_trust_anchor(&id).await);
        assert_settings(&format!("{case}/GetTrustAnchor"), &fetched, &expected);

        let page = client.list_trust_anchors().await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected ListTrustAnchors 200, got {page:?}"
        );
        assert_settings(
            &format!("{case}/ListTrustAnchors"),
            &listed(case, &page.body, &id),
            &expected,
        );
    }

    // And the delete echo reports the set that was in force when it ran.
    let deleted = anchor_of("teardown", 200, &client.delete_trust_anchor(&id).await);
    assert_settings("teardown", &deleted, &[None, None]);
}

/// The instant a rendered `date-time` member carries, as the (seconds, nanoseconds) pair
/// two instants can be ORDERED by.
///
/// The rendering trims the trailing zeros of the fractional second, so two renderings do
/// not sort as the instants they stand for; parsing is what makes the comparison an
/// ordering. Parsed the way the rest of this workspace parses a modelled `date-time`
/// member: `aws_smithy_types::DateTime::from_str` with `Format::DateTime`.
fn instant_of(case: &str, path: &str, member: &str, value: &Value) -> (i64, u32) {
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {path} must report {member} as a date-time string, got {value:?}")
    });
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: {path} must report {member} as a date-time, got {text:?}: {error}")
    });
    (parsed.secs(), parsed.subsec_nanos())
}

/// `UpdateTrustAnchor` persists what it was given: a later read, the list projection and
/// the delete echo all report the written name and source, a member the request omitted
/// keeps its stored value, and `updatedAt` moves while `createdAt` does not.
///
/// The rows differ along what the rule generalises over: which members the update carries
/// and the form of the name — both published `ResourceName` `@length` bounds and every
/// character class its `@pattern` allows.
#[tokio::test]
async fn update_trust_anchor_persists_the_members_it_carries() {
    let replacement_source = json!({
        "sourceType": "CERTIFICATE_BUNDLE",
        "sourceData": {"x509CertificateData": REPLACEMENT_BUNDLE_PEM},
    });
    // `ResourceName` is `@length(1, 255)` with `@pattern("^[ a-zA-Z0-9-_]*$")`.
    let cases: Vec<(&str, Value)> = vec![
        ("rename-only", json!({"name": "renamed anchor"})),
        ("shortest-name", json!({"name": "a"})),
        ("longest-name", json!({"name": "n".repeat(255)})),
        (
            "every-allowed-character-class",
            json!({"name": "Anchor-9_with spaces"}),
        ),
        ("source-only", json!({"source": replacement_source.clone()})),
        (
            "name-and-source",
            json!({"name": "both members", "source": replacement_source.clone()}),
        ),
        ("neither-member", json!({})),
    ];

    let client = Client::new();
    for (case, patch) in cases {
        let created = anchor_of(
            case,
            201,
            &client
                .create_trust_anchor(minimal_trust_anchor("original name"))
                .await,
        );
        let id = id_of(case, &created);
        let created_at = created.get("createdAt").cloned().unwrap_or(Value::Null);

        let expected_name = patch
            .get("name")
            .cloned()
            .unwrap_or_else(|| created.get("name").cloned().unwrap_or(Value::Null));
        let expected_source = patch
            .get("source")
            .cloned()
            .unwrap_or_else(|| created.get("source").cloned().unwrap_or(Value::Null));

        let updated = anchor_of(case, 200, &client.update_trust_anchor(&id, patch).await);
        let page = client.list_trust_anchors().await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected ListTrustAnchors 200, got {page:?}"
        );
        let projections: Vec<(&str, Value)> = vec![
            ("UpdateTrustAnchor", updated.clone()),
            (
                "GetTrustAnchor",
                anchor_of(case, 200, &client.get_trust_anchor(&id).await),
            ),
            ("ListTrustAnchors", listed(case, &page.body, &id)),
            (
                "DeleteTrustAnchor",
                anchor_of(case, 200, &client.delete_trust_anchor(&id).await),
            ),
        ];
        for (path, anchor) in projections {
            assert_eq!(
                anchor.get("name"),
                Some(&expected_name),
                "case {case:?}: {path} must report the name the update wrote, got {anchor:?}"
            );
            assert_eq!(
                anchor.get("source"),
                Some(&expected_source),
                "case {case:?}: {path} must report the source the update wrote, got {anchor:?}"
            );
            assert_eq!(
                anchor.get("createdAt").cloned().unwrap_or(Value::Null),
                created_at,
                "case {case:?}: {path} must not move createdAt, got {anchor:?}"
            );
            let updated_at = anchor.get("updatedAt").cloned().unwrap_or(Value::Null);
            // Both members are rendered `date-time` instants, whose fractional second is
            // written without its trailing zeros, so comparing the two RENDERINGS as
            // strings is not an ordering of the two instants ("…:01.5Z" sorts after
            // "…:01.55Z"). They are parsed and compared as instants instead.
            assert!(
                instant_of(case, path, "updatedAt", &updated_at)
                    >= instant_of(case, path, "createdAt", &created_at),
                "case {case:?}: {path} must report an updatedAt at or after createdAt \
                 {created_at:?}, got {updated_at:?}"
            );
        }
    }
}

/// A write against a trust anchor id nothing is stored under is refused with the
/// operation's modelled `ResourceNotFoundException`, never a silent success: an id that
/// was never created and one whose anchor was deleted, on each of the three writes.
#[tokio::test]
async fn a_write_against_an_unknown_trust_anchor_is_refused() {
    let client = Client::new();
    let created = anchor_of(
        "setup",
        201,
        &client
            .create_trust_anchor(minimal_trust_anchor("deleted anchor"))
            .await,
    );
    let deleted_id = id_of("setup", &created);
    anchor_of("setup", 200, &client.delete_trust_anchor(&deleted_id).await);

    // Both ids satisfy the modelled `Uuid` @length/@pattern, so each request reaches the
    // handler rather than the boundary validator.
    let never_created = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string();
    for (case, id) in [("never-created", never_created), ("deleted", deleted_id)] {
        let answers = vec![
            (
                "UpdateTrustAnchor",
                client.update_trust_anchor(&id, json!({"name": "new name"})).await,
            ),
            (
                "PutNotificationSettings",
                client
                    .put_notification_settings(
                        &id,
                        json!([{"enabled": true, "event": NOTIFICATION_EVENTS[0],
                                "threshold": 20}]),
                    )
                    .await,
            ),
            (
                "ResetNotificationSettings",
                client
                    .reset_notification_settings(&id, json!([{"event": NOTIFICATION_EVENTS[0]}]))
                    .await,
            ),
        ];
        for (operation, answer) in answers {
            assert_eq!(
                answer.status, 404,
                "case {case:?}: {operation} must answer 404 for an unstored id, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}: {operation} must answer its modelled error, got {answer:?}"
            );
            let message = answer
                .body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                message, TRUST_ANCHOR_NOT_FOUND_MESSAGE,
                "case {case:?}: {operation}'s refusal must be worded the way the service \
                 words an addressed trust anchor that is not stored, got {message:?}"
            );
        }
    }
}

/// A `threshold` outside the published `@range(1, 360)` is refused at the boundary, so a
/// setting no real service would accept never reaches the store: one past each bound,
/// against both the create path and `PutNotificationSettings`.
#[tokio::test]
async fn a_threshold_outside_the_published_range_is_refused() {
    let [low, high] = THRESHOLD_BOUNDS;
    let client = Client::new();
    let created = anchor_of(
        "setup",
        201,
        &client
            .create_trust_anchor(minimal_trust_anchor("threshold anchor"))
            .await,
    );
    let id = id_of("setup", &created);

    for (case, threshold) in [("below-the-minimum", low - 1), ("above-the-maximum", high + 1)] {
        let setting =
            json!({"enabled": true, "event": NOTIFICATION_EVENTS[0], "threshold": threshold});
        let put = client
            .put_notification_settings(&id, json!([setting.clone()]))
            .await;
        assert_eq!(
            put.status, 400,
            "case {case:?}: PutNotificationSettings must refuse threshold {threshold}, got \
             {put:?}"
        );
        let mut body = minimal_trust_anchor("threshold on create");
        body["notificationSettings"] = json!([setting]);
        let create = client.create_trust_anchor(body).await;
        assert_eq!(
            create.status, 400,
            "case {case:?}: CreateTrustAnchor must refuse threshold {threshold}, got {create:?}"
        );
    }

    // The anchor still reports the settings it had: a refused write stored nothing.
    let fetched = anchor_of("after-refusals", 200, &client.get_trust_anchor(&id).await);
    assert_settings("after-refusals", &fetched, &[None, None]);
}
