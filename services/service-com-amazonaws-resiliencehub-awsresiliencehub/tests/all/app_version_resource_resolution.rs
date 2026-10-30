// SPDX-License-Identifier: CC0-1.0

//! Resolving the resources of an app version, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: `ResolveAppVersionResources` records a FRESH,
//! server-generated resolution of the app version it addresses — any version the
//! application has, whatever that version holds — reports it with the
//! non-terminal `Pending` status, and makes it that version's CURRENT resolution;
//! the resolution finishes one `RESOLUTION_SPAN` after the call that started it,
//! so `DescribeAppVersionResourcesResolutionStatus` reports that same resolution
//! — addressed by its `resolutionId` or by none at all — as `Pending` until its
//! completion instant and as the terminal `Success` from then on; and a path that
//! does not resolve (an unknown application, an app version the application does
//! not have, a `resolutionId` that version has no resolution for, or no
//! resolution at all) is refused with the modelled 404.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::ops::types::RESOURCE_RESOLUTION_STATUS_TYPE_VALUES;
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    ComAmazonawsResiliencehubAwsresiliencehubState, DRAFT_APP_VERSION,
    MAX_RESOLUTIONS_PER_APP_VERSION, RESOLUTION_SPAN, RESOLUTION_STATUS_PENDING,
    RESOLUTION_STATUS_SUCCESS,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-east-1";

/// The account id `engine::mock_account_id` mints every ARN under.
const ACCOUNT: &str = "000000000000";

/// The modelled `@length` of `String255`, the shape `resolutionId` targets (from
/// `smithy_query query="operation" operation="ResolveAppVersionResources"`:
/// `resolutionId` targets `String255`, length 1..255).
const RESOLUTION_ID_LENGTH: std::ops::RangeInclusive<usize> = 1..=255;

/// An emulator holding only this service, over one shared engine — and that
/// engine, so a case can move the CLOCK a resolution is waiting on instead of
/// sleeping through its span.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: this suite tests
    // the operations, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
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

/// An application to resolve versions of, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// Add one physical resource to `app_arn`'s draft version, so the version being
/// resolved holds something.
async fn add_a_resource(router: &MultiServiceRouter, app_arn: &str, identifier: &str) {
    let reply = post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": identifier,
            "logicalResourceId": {"identifier": identifier},
            "physicalResourceId": identifier,
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["queue-tier"],
        }),
    )
    .await;
    reply.ok(identifier);
}

/// Publish `app_arn`'s draft and answer the `appVersion` the publish minted.
async fn publish(router: &MultiServiceRouter, app_arn: &str) -> String {
    let reply = post(router, "/publish-app-version", &json!({"appArn": app_arn})).await;
    reply.ok("publish")["appVersion"]
        .as_str()
        .expect("a published appVersion")
        .to_string()
}

/// Start a resolution of `app_version` of `app_arn`.
async fn resolve(router: &MultiServiceRouter, app_arn: &str, app_version: &str) -> Reply {
    post(
        router,
        "/resolve-app-version-resources",
        &json!({"appArn": app_arn, "appVersion": app_version}),
    )
    .await
}

/// Read a resolution's status: `resolution_id` when the case addresses one,
/// nothing when it asks for the version's current resolution.
async fn describe(
    router: &MultiServiceRouter,
    app_arn: &str,
    app_version: &str,
    resolution_id: Option<&str>,
) -> Reply {
    let mut body = json!({"appArn": app_arn, "appVersion": app_version});
    if let Some(resolution_id) = resolution_id {
        body["resolutionId"] = json!(resolution_id);
    }
    post(
        router,
        "/describe-app-version-resources-resolution-status",
        &body,
    )
    .await
}

/// When the stored resolution `resolution_id` finishes — the instant the service
/// recorded for it, which is what its status is derived from.
fn completion_instant(
    engine: &Arc<engine::InMemoryEngine>,
    app_arn: &str,
    app_version: &str,
    resolution_id: &str,
) -> SystemTime {
    engine
        .state::<ComAmazonawsResiliencehubAwsresiliencehubState>()
        .lock()
        .resolution_mut(app_arn, app_version, resolution_id)
        .expect("the resolution the call recorded")
        .completes_at
}

/// Move the clock a resolution is waiting on into the PAST, so a case can read
/// what the finished resolution reports without sleeping through its span.
fn finish_the_resolution(
    engine: &Arc<engine::InMemoryEngine>,
    app_arn: &str,
    app_version: &str,
    resolution_id: &str,
) {
    engine
        .state::<ComAmazonawsResiliencehubAwsresiliencehubState>()
        .lock()
        .resolution_mut(app_arn, app_version, resolution_id)
        .expect("the resolution the call recorded")
        .completes_at = SystemTime::now() - Duration::from_secs(1);
}

/// A row of the resolve table: what the draft gains before this resolve, and
/// which version the resolve addresses.
///
/// The rows differ along what the rule generalises over: how much the resolved
/// version holds (nothing, one resource, several) and which version it is (the
/// draft, or a frozen published one).
struct Resolve {
    label: &'static str,
    /// The resource identifiers added to the draft before this resolve.
    adds: &'static [&'static str],
    /// Whether the draft is published first and the PUBLISHED version resolved
    /// instead of the draft.
    publish_first: bool,
}

/// The table: one row per resolve, in order, against ONE application.
fn resolve_table() -> Vec<Resolve> {
    vec![
        Resolve {
            label: "the draft of an application holding nothing",
            adds: &[],
            publish_first: false,
        },
        Resolve {
            label: "the draft holding one resource",
            adds: &["orders-queue"],
            publish_first: false,
        },
        Resolve {
            label: "the first published version",
            adds: &[],
            publish_first: true,
        },
        Resolve {
            label: "the draft holding three resources",
            adds: &["orders-function", "orders-table"],
            publish_first: false,
        },
        Resolve {
            label: "the second published version",
            adds: &["orders-topic"],
            publish_first: true,
        },
        Resolve {
            label: "the draft again, unchanged since the last resolve",
            adds: &[],
            publish_first: false,
        },
    ]
}

/// The whole table: each resolve records a fresh resolution of the version it
/// addresses, reports it as `Pending`, and is read back by the status reader both
/// by `resolutionId` and by none — `Pending` until its completion instant and
/// `Success` from then on.
#[tokio::test]
async fn each_resolve_records_a_resolution_read_back_by_the_status_reader() {
    // The statuses asserted below are members of the modelled
    // `ResourceResolutionStatusType` (`smithy_query query="shape"
    // shape="ResourceResolutionStatusType"`, transcribed into
    // `RESOURCE_RESOLUTION_STATUS_TYPE_VALUES`).
    for status in [RESOLUTION_STATUS_PENDING, RESOLUTION_STATUS_SUCCESS] {
        assert!(
            RESOURCE_RESOLUTION_STATUS_TYPE_VALUES.contains(&status),
            "case {status:?}: the status reported must be a modelled \
             ResourceResolutionStatusType, which lists {RESOURCE_RESOLUTION_STATUS_TYPE_VALUES:?}"
        );
    }

    let (router, engine) = emulator();
    let app_arn = create_an_app(&router, "resolve-table").await;
    let mut minted: Vec<String> = Vec::new();

    for row in resolve_table() {
        let case = row.label;
        for identifier in row.adds {
            add_a_resource(&router, &app_arn, identifier).await;
        }
        let app_version = if row.publish_first {
            publish(&router, &app_arn).await
        } else {
            DRAFT_APP_VERSION.to_string()
        };

        let before = SystemTime::now();
        let resolved = resolve(&router, &app_arn, &app_version).await;
        let after = SystemTime::now();
        let resolved = resolved.ok(case).clone();

        let resolution_id = resolved["resolutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a minted resolutionId, got {resolved:?}"))
            .to_string();
        assert_eq!(
            resolved,
            json!({
                "appArn": app_arn,
                "appVersion": app_version,
                "resolutionId": resolution_id,
                "status": RESOLUTION_STATUS_PENDING,
            }),
            "case {case:?}: the resolve must echo the addressed version and report the \
             resolution it started as {RESOLUTION_STATUS_PENDING:?}, got {resolved:?}"
        );
        assert!(
            RESOLUTION_ID_LENGTH.contains(&resolution_id.chars().count()),
            "case {case:?}: resolutionId must satisfy String255's modelled length \
             {RESOLUTION_ID_LENGTH:?}, got {} characters ({resolution_id:?})",
            resolution_id.chars().count()
        );
        assert!(
            !minted.contains(&resolution_id),
            "case {case:?}: every resolve records a FRESH resolution, but \
             {resolution_id:?} was already minted ({minted:?})"
        );
        minted.push(resolution_id.clone());

        // The resolution finishes one span after the call that started it — never
        // in the call itself, so a caller reading straight after it still sees the
        // non-terminal status asserted above.
        let completes_at = completion_instant(&engine, &app_arn, &app_version, &resolution_id);
        assert!(
            completes_at >= before + RESOLUTION_SPAN && completes_at <= after + RESOLUTION_SPAN,
            "case {case:?}: the resolution must complete one {RESOLUTION_SPAN:?} after the \
             call, which ran between {before:?} and {after:?}, got {completes_at:?}"
        );

        // The round trip: the recorded resolution is what the status reader
        // returns, addressed by its own id or as the version's current one.
        for (selector, addressed_by) in [
            ("by its resolutionId", Some(resolution_id.as_str())),
            ("as the version's current resolution", None),
        ] {
            let described = describe(&router, &app_arn, &app_version, addressed_by).await;
            assert_eq!(
                described.ok(selector),
                &json!({
                    "appArn": app_arn,
                    "appVersion": app_version,
                    "resolutionId": resolution_id,
                    "status": RESOLUTION_STATUS_PENDING,
                }),
                "case {case:?} {selector:?}: the reader must report the recorded \
                 resolution, got {:?}",
                described.body
            );
        }

        // And once its completion instant has passed, the same read reports the
        // terminal status — the clock moved it, not the read.
        finish_the_resolution(&engine, &app_arn, &app_version, &resolution_id);
        for (selector, addressed_by) in [
            ("by its resolutionId", Some(resolution_id.as_str())),
            ("as the version's current resolution", None),
        ] {
            let described = describe(&router, &app_arn, &app_version, addressed_by).await;
            assert_eq!(
                described.ok(selector)["status"],
                json!(RESOLUTION_STATUS_SUCCESS),
                "case {case:?} {selector:?}: a resolution past its completion instant \
                 reports {RESOLUTION_STATUS_SUCCESS:?}, got {:?}",
                described.body
            );
            // Reading it again answers the same thing: the reader reports, it does
            // not consume.
            let again = describe(&router, &app_arn, &app_version, addressed_by).await;
            assert_eq!(
                again.ok(selector),
                described.ok(selector),
                "case {case:?} {selector:?}: two reads of one resolution must agree"
            );
        }
    }
}

/// A resolution belongs to ONE app version of ONE application: the id another
/// version or another application recorded addresses nothing here, and a version
/// nobody resolved has no current resolution.
#[tokio::test]
async fn a_resolution_is_scoped_to_its_app_version() {
    let (router, _engine) = emulator();
    let app_arn = create_an_app(&router, "resolve-scope").await;
    let other_arn = create_an_app(&router, "resolve-scope-other").await;
    add_a_resource(&router, &app_arn, "scoped-queue").await;
    add_a_resource(&router, &other_arn, "other-queue").await;

    let draft_resolution = resolve(&router, &app_arn, DRAFT_APP_VERSION).await;
    let draft_resolution = draft_resolution.ok("the draft's resolution")["resolutionId"]
        .as_str()
        .expect("a minted resolutionId")
        .to_string();

    let other_resolution = resolve(&router, &other_arn, DRAFT_APP_VERSION).await;
    let other_resolution = other_resolution.ok("another application's resolution")["resolutionId"]
        .as_str()
        .expect("a minted resolutionId")
        .to_string();
    assert_ne!(
        draft_resolution, other_resolution,
        "two applications' resolutions must not share an id"
    );

    // A published version is a version of its own: it has no resolution until it
    // is resolved, whatever the draft it was frozen from has.
    let published = publish(&router, &app_arn).await;
    for (case, app_version, resolution_id) in [
        (
            "the published version has no current resolution",
            published.as_str(),
            None,
        ),
        (
            "the draft's resolution id against the published version",
            published.as_str(),
            Some(draft_resolution.as_str()),
        ),
        (
            "another application's resolution id against this draft",
            DRAFT_APP_VERSION,
            Some(other_resolution.as_str()),
        ),
    ] {
        describe(&router, &app_arn, app_version, resolution_id)
            .await
            .refused(case, 404, "ResourceNotFoundException");
    }

    // Resolving the published version gives it its own resolution, and leaves the
    // draft's current one as it was.
    let published_resolution = resolve(&router, &app_arn, &published).await;
    let published_resolution = published_resolution.ok("the published version's resolution")
        ["resolutionId"]
        .as_str()
        .expect("a minted resolutionId")
        .to_string();
    assert_ne!(
        published_resolution, draft_resolution,
        "each app version records its own resolution"
    );
    let draft_current = describe(&router, &app_arn, DRAFT_APP_VERSION, None).await;
    assert_eq!(
        draft_current.ok("the draft's current resolution")["resolutionId"],
        json!(draft_resolution),
        "resolving another version must not change the draft's current resolution, got {:?}",
        draft_current.body
    );
}

/// Each link of a resolution's path has to resolve: an unknown application, an app
/// version the application does not have and an unknown `resolutionId` are all
/// refused with the modelled 404, by the resolve and by the reader alike — and a
/// deleted application's resolutions are gone with it.
#[tokio::test]
async fn an_unresolvable_path_is_refused_with_the_modeled_404() {
    let (router, _engine) = emulator();
    let app_arn = create_an_app(&router, "resolve-not-found").await;
    add_a_resource(&router, &app_arn, "present-queue").await;
    let published = publish(&router, &app_arn).await;
    let unknown_arn =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/00000000-0000-0000-0000-000000000000");

    // The versions this application does NOT have: one past its last publish, a
    // name no publish minted, and a 50-character version at the top of
    // `EntityVersion`'s modelled pattern `^\S{1,50}$`.
    let past_the_last_publish = format!("{}", published.parse::<i64>().unwrap_or(1) + 1);
    let longest_version = "v".repeat(50);
    let unknown_versions = [
        past_the_last_publish.as_str(),
        "no-such-version",
        longest_version.as_str(),
    ];

    for app_version in unknown_versions {
        let case = format!("resolving version {app_version:?}");
        resolve(&router, &app_arn, app_version)
            .await
            .refused(&case, 404, "ResourceNotFoundException");
        let case = format!("reading the status of version {app_version:?}");
        describe(&router, &app_arn, app_version, None)
            .await
            .refused(&case, 404, "ResourceNotFoundException");
    }

    for app_version in [DRAFT_APP_VERSION, published.as_str()] {
        let case = format!("an unknown application, version {app_version:?}");
        resolve(&router, &unknown_arn, app_version)
            .await
            .refused(&case, 404, "ResourceNotFoundException");
        describe(&router, &unknown_arn, app_version, None)
            .await
            .refused(&case, 404, "ResourceNotFoundException");
    }

    // A version that exists but was never resolved, and ids it has no resolution
    // for: a `String255` at each end of the modelled length, and one that looks
    // like a minted id.
    for resolution_id in [
        None,
        Some("r".to_string()),
        Some("r".repeat(255)),
        Some("00000000-0000-0000-0000-000000000000".to_string()),
    ] {
        let case = format!("a resolution the draft does not have: {resolution_id:?}");
        describe(&router, &app_arn, DRAFT_APP_VERSION, resolution_id.as_deref())
            .await
            .refused(&case, 404, "ResourceNotFoundException");
    }

    // And nothing of a deleted application's resolutions outlives it: the path is
    // refused at its first link.
    let resolved = resolve(&router, &app_arn, DRAFT_APP_VERSION).await;
    let resolution_id = resolved.ok("the draft's resolution")["resolutionId"]
        .as_str()
        .expect("a minted resolutionId")
        .to_string();
    post(&router, "/delete-app", &json!({"appArn": app_arn}))
        .await
        .ok("delete the application");
    describe(
        &router,
        &app_arn,
        DRAFT_APP_VERSION,
        Some(resolution_id.as_str()),
    )
    .await
    .refused("a deleted application's resolution", 404, "ResourceNotFoundException");
}

/// The remembered resolution history of one app version is bounded: past the
/// ceiling the OLDEST resolution is forgotten, the newest are still readable, and
/// the current one is always the last recorded.
#[tokio::test]
async fn the_remembered_resolution_history_is_bounded() {
    let (router, _engine) = emulator();
    let app_arn = create_an_app(&router, "resolve-ceiling").await;

    let mut minted: Vec<String> = Vec::new();
    for call in 0..MAX_RESOLUTIONS_PER_APP_VERSION + 2 {
        let case = format!("resolve number {}", call + 1);
        let resolved = resolve(&router, &app_arn, DRAFT_APP_VERSION).await;
        minted.push(
            resolved.ok(&case)["resolutionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a minted resolutionId"))
                .to_string(),
        );
    }

    let forgotten = &minted[..minted.len() - MAX_RESOLUTIONS_PER_APP_VERSION];
    let remembered = &minted[minted.len() - MAX_RESOLUTIONS_PER_APP_VERSION..];
    assert_eq!(
        forgotten.len(),
        2,
        "two resolutions past the ceiling must be the forgotten ones, got {forgotten:?}"
    );

    for resolution_id in forgotten {
        let case = format!("the forgotten resolution {resolution_id:?}");
        describe(&router, &app_arn, DRAFT_APP_VERSION, Some(resolution_id))
            .await
            .refused(&case, 404, "ResourceNotFoundException");
    }
    for resolution_id in remembered {
        let case = format!("the remembered resolution {resolution_id:?}");
        let described = describe(&router, &app_arn, DRAFT_APP_VERSION, Some(resolution_id)).await;
        assert_eq!(
            described.ok(&case)["resolutionId"],
            json!(resolution_id),
            "case {case:?}: a resolution within the ceiling must still be readable, got {:?}",
            described.body
        );
    }

    let current = describe(&router, &app_arn, DRAFT_APP_VERSION, None).await;
    assert_eq!(
        current.ok("the current resolution")["resolutionId"],
        json!(minted.last().expect("a last resolution")),
        "the current resolution is the last one recorded, got {:?}",
        current.body
    );
}
