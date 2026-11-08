// SPDX-License-Identifier: CC0-1.0

//! The NOUN this service names a resource type by in the message of a refusal about
//! one id.
//!
//! The rule under test: a message about an identified resource of this service names
//! the resource TYPE by the one label this API words that type with — never by the
//! modelled shape or operation name — and names it the same way whichever refusal
//! answered. For a packaging configuration that label is the abbreviation
//! `PackagingConfig`, recorded both in the not-found of an id this service holds none
//! of (behavioral unit
//! `mediapackage-vod.delete-packaging-configuration-state-transition`) and in the
//! conflict a duplicate create is refused with (behavioral unit
//! `mediapackage-vod.concurrent-packaging-configuration-duplicate-id-contention`);
//! for a packaging group it is `PackagingGroup` and for an asset `Asset`.
//!
//! Every case drives real HTTP through the router the production binary builds, and
//! each test asserts the rule over a TABLE: the resource type, the form of the id,
//! the packaging format of the configuration and — for the conflict — the number of
//! callers racing for the one id all vary, so the recorded single-caller case is one
//! row of many.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; it only has to be a legal credential scope.
const REGION: &str = "us-west-2";

/// The wire error code a conflicting create is refused with, and the modelled
/// `httpError` of that shape
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")`: `errorKind
/// client`, `httpError 422`).
const CONFLICT: (&str, u16) = ("UnprocessableEntityException", 422);

/// The wire error code a missing addressed resource answers, and its modelled
/// `httpError` (`smithy_query(query="shape",shape="NotFoundException")`:
/// `httpError 404`).
const ABSENT: (&str, u16) = ("NotFoundException", 404);

/// One resource type of this service, as the row of every table below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Group,
    Configuration,
    Asset,
}

impl Kind {
    /// Every resource type a create of this service mints, so a table spans them all.
    const ALL: [Kind; 3] = [Kind::Group, Kind::Configuration, Kind::Asset];

    /// The REST collection the create posts to and a read addresses one member of
    /// (`smithy_query(query="digest")`: `POST /packaging_groups`,
    /// `POST /packaging_configurations`, `POST /assets`).
    fn collection(self) -> &'static str {
        match self {
            Kind::Group => "/packaging_groups",
            Kind::Configuration => "/packaging_configurations",
            Kind::Asset => "/assets",
        }
    }

    /// The noun every message of this service about this resource type must use.
    fn label(self) -> &'static str {
        match self {
            Kind::Group => "PackagingGroup",
            // The abbreviation, NOT the `PackagingConfiguration` of the shape and the
            // operation names.
            Kind::Configuration => "PackagingConfig",
            Kind::Asset => "Asset",
        }
    }

    /// The HTTP status a create of this type answers on success — the status this
    /// emulator's recordings show for each (the configuration's 202 included).
    fn created(self) -> u16 {
        match self {
            Kind::Configuration => 202,
            _ => 200,
        }
    }
}

/// What one answer says about itself: the HTTP status, the error shape named by
/// `x-amzn-errortype` (how restJson1 resolves an error shape), and the parsed body.
struct Answer {
    status: u16,
    errortype: String,
    body: Value,
}

impl Answer {
    /// The `message` member the modelled error shapes spell `Message` with
    /// `jsonName: message`, or `"<unset>"` when the body carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }

    /// The per-attribute `errors` map, or `Value::Null` when the body carries none —
    /// the distinction between a validation refusal and a resource-level one.
    fn errors(&self) -> Value {
        self.body.get("errors").cloned().unwrap_or(Value::Null)
    }
}

/// A router carrying exactly this service over one shared engine.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and report what it answered.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "<unset>".to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        errortype,
        body,
    }
}

/// The create body for one resource of `kind` under `id`, in `group`.
///
/// `variant` is what one racer differs from another by: the packaging format and the
/// segment duration of a configuration, the tag value of a group or an asset. The id
/// is what the uniqueness rule is about, so it is NOT varied by `variant`.
fn create_body(kind: Kind, id: &str, group: &str, variant: usize) -> Value {
    let seconds = 4 + variant as i64;
    match kind {
        Kind::Group => json!({ "id": id, "tags": { "racer": variant.to_string() } }),
        Kind::Configuration => {
            // The four packaging formats `CreatePackagingConfiguration` takes exactly
            // one of (`smithy_query(query="operation",operation=
            // "CreatePackagingConfiguration")`: `CmafPackage`, `DashPackage`,
            // `HlsPackage`, `MssPackage`).
            let block = match variant % 4 {
                0 => json!({ "hlsManifests": [{ "manifestName": "index" }],
                             "segmentDurationSeconds": seconds }),
                1 => json!({ "dashManifests": [{ "manifestName": "index" }],
                             "segmentDurationSeconds": seconds }),
                2 => json!({ "hlsManifests": [{ "manifestName": "index" }],
                             "segmentDurationSeconds": seconds }),
                _ => json!({ "mssManifests": [{ "manifestName": "index" }],
                             "segmentDurationSeconds": seconds }),
            };
            let member = match variant % 4 {
                0 => "hlsPackage",
                1 => "dashPackage",
                2 => "cmafPackage",
                _ => "mssPackage",
            };
            json!({ "id": id, "packagingGroupId": group, member: block })
        }
        Kind::Asset => json!({
            "id": id,
            "packagingGroupId": group,
            "sourceArn": format!("arn:aws:s3:::test-bucket/{id}.smil"),
            "sourceRoleArn": format!(
                "arn:aws:iam::{}:role/MediaPackageVod",
                engine::emulator_account_id()
            ),
            "tags": { "racer": variant.to_string() },
        }),
    }
}

/// The path a read of one resource of `kind` under `id` addresses.
fn member_path(kind: Kind, id: &str) -> String {
    format!("{}/{id}", kind.collection())
}

/// Create a packaging group, asserting the setup succeeded.
async fn create_group(router: &MultiServiceRouter, id: &str) {
    let answer = call(router, "POST", "/packaging_groups", Some(json!({ "id": id }))).await;
    assert_eq!(
        answer.status,
        Kind::Group.created(),
        "setup: CreatePackagingGroup {id:?}: expected {}, got {} body {}",
        Kind::Group.created(),
        answer.status,
        answer.body
    );
}

/// Assert one refusal: its status, the `x-amzn-errortype` header a client resolves the
/// shape from, its message and that a resource-level refusal carries no `errors` map.
fn assert_refusal(answer: &Answer, case: &str, (code, status): (&str, u16), message: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.errortype, code,
        "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
        answer.errortype
    );
    assert_eq!(
        answer.message(),
        message,
        "case {case:?}: expected the message {message:?}, got {:?}",
        answer.message()
    );
    assert_eq!(
        answer.errors(),
        Value::Null,
        "case {case:?}: a refusal about one resource is not a member violation, got {}",
        answer.errors()
    );
}

/// A create that re-uses an id this service already holds is refused with
/// `<label> with id=<the requested id> already exists`, with the label of the
/// resource type — and the refusal leaves the stored resource alone.
#[tokio::test]
async fn a_duplicate_create_names_the_resource_type_by_this_services_label() {
    // Ids differing in length, separators, case and digits, plus one the issue did
    // not cite at all: the rule is about the type's noun, not the id's form.
    let ids = [
        "c",
        "config-1",
        "Mixed_Case-9",
        "sim2real-mpvod-race-d17de9b6a48a",
        &"z".repeat(80),
    ];
    for (variant, id) in ids.into_iter().enumerate() {
        let router = router();
        create_group(&router, id).await;
        for kind in Kind::ALL {
            let case = format!("{kind:?} {id:?} created twice");
            let body = create_body(kind, id, id, variant);
            if kind != Kind::Group {
                let created = call(&router, "POST", kind.collection(), Some(body.clone())).await;
                assert_eq!(
                    created.status,
                    kind.created(),
                    "setup: {case:?}: expected {}, got {} body {}",
                    kind.created(),
                    created.status,
                    created.body
                );
            }

            // The same id a second time: refused, and worded with the type's label.
            let answer = call(&router, "POST", kind.collection(), Some(body)).await;
            assert_refusal(
                &answer,
                &case,
                CONFLICT,
                &format!("{} with id={id} already exists", kind.label()),
            );

            // Positive control: the refused repeat left the stored resource readable.
            let stored = call(&router, "GET", &member_path(kind, id), None).await;
            assert_eq!(
                stored.status, 200,
                "case {case:?}: the stored resource must still read back, got {} body {}",
                stored.status, stored.body
            );
            assert_eq!(
                stored.body.get("id").and_then(Value::as_str),
                Some(id),
                "case {case:?}: the read must report the created id, body {}",
                stored.body
            );
        }
    }
}

/// One resource type reads as ONE noun: the conflict of a held id and the not-found of
/// an unheld one name the type the same way.
#[tokio::test]
async fn the_conflict_and_the_not_found_name_the_same_noun_per_resource_type() {
    let router = router();
    create_group(&router, "g").await;

    for kind in Kind::ALL {
        let case = format!("{kind:?}: one noun in both refusals");
        // An id nothing was created under: the ADDRESSED resource is missing.
        let absent = call(&router, "GET", &member_path(kind, "absent-id"), None).await;
        assert_refusal(
            &absent,
            &format!("{case}, not found"),
            ABSENT,
            &format!("{} with id=absent-id not found", kind.label()),
        );

        // An id it holds: the create is a conflict, worded with that same noun.
        let held = format!("held-{}", kind.label().to_lowercase());
        if kind == Kind::Group {
            create_group(&router, &held).await;
        } else {
            let created = call(
                &router,
                "POST",
                kind.collection(),
                Some(create_body(kind, &held, "g", 0)),
            )
            .await;
            assert_eq!(
                created.status,
                kind.created(),
                "setup: {case:?}: expected {}, got {} body {}",
                kind.created(),
                created.status,
                created.body
            );
        }
        let conflict = call(
            &router,
            "POST",
            kind.collection(),
            Some(create_body(kind, &held, "g", 1)),
        )
        .await;
        assert_refusal(
            &conflict,
            &format!("{case}, already exists"),
            CONFLICT,
            &format!("{} with id={held} already exists", kind.label()),
        );
    }
}

/// Of N callers racing to create the SAME id, exactly one wins and every loser gets
/// that one conflict message — the atomicity and the wording together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_loser_of_a_race_for_one_id_gets_the_same_conflict_message() {
    // Rows differ along how many callers contend for the one id, including the
    // degenerate row where nobody loses and the recorded row of eight.
    for racers in [1usize, 2, 3, 8] {
        for kind in Kind::ALL {
            let case = format!("{racers} concurrent creates of one {kind:?}");
            let router = Arc::new(router());
            create_group(&router, "g").await;
            let id = format!("race-{racers}-{}", kind.label().to_lowercase());

            let mut running = Vec::with_capacity(racers);
            for variant in 0..racers {
                let router = Arc::clone(&router);
                let body = create_body(kind, &id, "g", variant);
                running.push(tokio::spawn(async move {
                    let answer = call(&router, "POST", kind.collection(), Some(body)).await;
                    (variant, answer)
                }));
            }
            let mut outcomes = Vec::with_capacity(racers);
            for handle in running {
                outcomes.push(
                    handle
                        .await
                        .unwrap_or_else(|err| panic!("case {case:?}: a caller panicked: {err}")),
                );
            }

            let winners: Vec<usize> = outcomes
                .iter()
                .filter(|(_, answer)| answer.status == kind.created())
                .map(|(variant, _)| *variant)
                .collect();
            assert_eq!(
                winners.len(),
                1,
                "case {case:?}: exactly one caller may take the id, got statuses {:?}",
                outcomes
                    .iter()
                    .map(|(variant, answer)| (*variant, answer.status))
                    .collect::<Vec<_>>()
            );
            let expected = format!("{} with id={id} already exists", kind.label());
            let mut loser_messages = BTreeSet::new();
            for (variant, answer) in outcomes
                .iter()
                .filter(|(_, answer)| answer.status != kind.created())
            {
                assert_refusal(
                    answer,
                    &format!("{case}, loser {variant}"),
                    CONFLICT,
                    &expected,
                );
                loser_messages.insert(answer.message());
            }
            assert!(
                loser_messages.len() <= 1,
                "case {case:?}: every loser must get one message, got {loser_messages:?}"
            );

            // Positive control: the id the race converged on reads back, twice the
            // same, so the losers stored nothing.
            for read in 0..2 {
                let stored = call(&router, "GET", &member_path(kind, &id), None).await;
                assert_eq!(
                    stored.status, 200,
                    "case {case:?}: read {read} of the winner: got {} body {}",
                    stored.status, stored.body
                );
                assert_eq!(
                    stored.body.get("id").and_then(Value::as_str),
                    Some(id.as_str()),
                    "case {case:?}: read {read} must report the contended id, body {}",
                    stored.body
                );
            }
        }
    }
}
