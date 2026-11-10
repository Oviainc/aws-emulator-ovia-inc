// SPDX-License-Identifier: CC0-1.0

//! Migration Hub and MigrationHub Config share one SigV4 signing name — each must still
//! be claimed by exactly one service in the assembled router.
//!
//! Both services declare `aws.auth#sigv4` name `mgh`
//! (`smithy_query(query="service_info")` on either model; their endpoint prefixes are
//! `mgh` and `migrationhub-config`), and the router resolves a request's routing key from
//! the SigV4 credential scope FIRST, so both see the same key. A service that claims on
//! the signing name alone therefore claims its peer's traffic too, and the router answers
//! a refusal naming two claimants rather than serving the request from an arbitrary one.
//!
//! Both speak `awsJson1_1`, so every request names its service out of band in
//! `X-Amz-Target` (`AWSMigrationHubMultiAccountService.<Operation>` for MigrationHub
//! Config, `AWSMigrationHub.<Operation>` for Migration Hub) — that prefix is the
//! discriminator each crate's `can_handle` reads, so the split is decided the same way on
//! both sides.
//!
//! The first suite pins it in the PRODUCTION router — the one `build_router` assembles —
//! across both services' own traffic, both routing tiers (credential scope and
//! `X-Amz-Target`), either endpoint host and a loopback endpoint, with each service's
//! writes read back so they are seen landing in its OWN store. The suites after it mount
//! the two services alone and ask which of them claims a request, over every prefix form
//! a client may send, and over an unsigned request, whose routing key is the target
//! prefix itself rather than the shared signing name.
//!
//! Assembling the whole router costs stack, so the first suite runs on an explicitly
//! sized thread for the same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The emulator endpoint a client configured with `endpoint_url` addresses: a loopback
/// host names no service at all, so only the target can decide.
const LOOPBACK_HOST: &str = "127.0.0.1:9297";

/// The progress update stream, and the migration task inside it, the Migration Hub write
/// cases create — a name only that service's store can report back.
const STREAM_NAME: &str = "routing-check-stream";
const TASK_NAME: &str = "routing-check-task";

/// The home region the MigrationHub Config write case sets, matching the modelled
/// `HomeRegion` pattern `^([a-z]+)-([a-z]+)-([0-9]+)$`.
const HOME_REGION: &str = "eu-central-1";

/// What must happen to a request both services could claim on their signing name: it is
/// served by the service the case names, with this status, and a body carrying every
/// `present` marker and none of the `absent` ones — strings only one service's model or
/// store can produce, so they identify which crate answered.
struct Case {
    name: &'static str,
    host: &'static str,
    target: &'static str,
    /// Whether to sign for `mgh`. A case with `false` carries no credential scope, so the
    /// router falls through to the `X-Amz-Target` routing tier.
    signed: bool,
    body: String,
    who: &'static str,
    status: u16,
    present: &'static [&'static str],
    absent: &'static [&'static str],
}

/// The rows differ along which service owns the request, the routing tier its key comes
/// from (credential scope vs `X-Amz-Target`), the endpoint host, and whether the
/// operation reads or writes. The cases run in order against one engine, so a read after
/// a write shows the write landed in the writing service's own store.
fn cases() -> Vec<Case> {
    let account = engine::emulator_account_id();
    vec![
        Case {
            // The minimal repro the issue reports: the first call every MigrationHub
            // Config client makes, which never reached a handler because two services
            // claimed `mgh`.
            name: "Config DescribeHomeRegionControls on the Config endpoint host",
            host: CONFIG_HOST,
            target: "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
            signed: true,
            body: "{}".to_string(),
            who: "MigrationHub Config",
            status: 200,
            present: &["HomeRegionControls"],
            absent: &["MigrationTaskSummaryList"],
        },
        Case {
            // The same read against the emulator endpoint a client is pointed at: the
            // host does not decide, the target does.
            name: "Config DescribeHomeRegionControls on a loopback endpoint",
            host: LOOPBACK_HOST,
            target: "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
            signed: true,
            body: "{}".to_string(),
            who: "MigrationHub Config",
            status: 200,
            present: &["HomeRegionControls"],
            absent: &["MigrationTaskSummaryList"],
        },
        Case {
            // …and even on Migration Hub's endpoint host, for the same reason.
            name: "Config DescribeHomeRegionControls on the Migration Hub endpoint host",
            host: HUB_HOST,
            target: "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
            signed: true,
            body: "{}".to_string(),
            who: "MigrationHub Config",
            status: 200,
            present: &["HomeRegionControls"],
            absent: &["MigrationTaskSummaryList"],
        },
        Case {
            // An UNSIGNED Config request: with no credential scope the router's next
            // tier is `X-Amz-Target`, whose prefix resolves to this service's own key, so
            // the claim must still split the same way.
            name: "Config DescribeHomeRegionControls routed by X-Amz-Target alone",
            host: LOOPBACK_HOST,
            target: "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
            signed: false,
            body: "{}".to_string(),
            who: "MigrationHub Config",
            status: 200,
            present: &["HomeRegionControls"],
            absent: &["MigrationTaskSummaryList"],
        },
        Case {
            // A Config WRITE, so dispatch is shown reaching the handler that writes and
            // not only a reader. `ControlId` matches `^hrc-[a-z0-9]{12}$`.
            name: "Config CreateHomeRegionControl",
            host: CONFIG_HOST,
            target: "AWSMigrationHubMultiAccountService.CreateHomeRegionControl",
            signed: true,
            body: format!(
                r#"{{"HomeRegion":"{HOME_REGION}",
                     "Target":{{"Type":"ACCOUNT","Id":"{account}"}},
                     "DryRun":false}}"#
            ),
            who: "MigrationHub Config",
            status: 200,
            present: &["HomeRegionControl", "hrc-", HOME_REGION],
            absent: &[],
        },
        Case {
            // The region just written, read back through the other Config reader: the
            // write landed in the Config store the Config crate owns.
            name: "Config GetHomeRegion after the create",
            host: CONFIG_HOST,
            target: "AWSMigrationHubMultiAccountService.GetHomeRegion",
            signed: true,
            body: "{}".to_string(),
            who: "MigrationHub Config",
            status: 200,
            present: &["HomeRegion", HOME_REGION],
            absent: &[],
        },
        Case {
            // Migration Hub's own traffic, under the same signing name: it must still be
            // served by the Migration Hub crate.
            name: "Migration Hub ListMigrationTasks on the Migration Hub endpoint host",
            host: HUB_HOST,
            target: "AWSMigrationHub.ListMigrationTasks",
            signed: true,
            body: "{}".to_string(),
            who: "Migration Hub",
            status: 200,
            present: &["MigrationTaskSummaryList"],
            absent: &["HomeRegionControls"],
        },
        Case {
            name: "Migration Hub ListMigrationTasks on the Config endpoint host",
            host: CONFIG_HOST,
            target: "AWSMigrationHub.ListMigrationTasks",
            signed: true,
            body: "{}".to_string(),
            who: "Migration Hub",
            status: 200,
            present: &["MigrationTaskSummaryList"],
            absent: &["HomeRegionControls"],
        },
        Case {
            // The unsigned Migration Hub request, for the same reason as its Config
            // counterpart.
            name: "Migration Hub ListMigrationTasks routed by X-Amz-Target alone",
            host: LOOPBACK_HOST,
            target: "AWSMigrationHub.ListMigrationTasks",
            signed: false,
            body: "{}".to_string(),
            who: "Migration Hub",
            status: 200,
            present: &["MigrationTaskSummaryList"],
            absent: &["HomeRegionControls"],
        },
        Case {
            // A Migration Hub WRITE: the stream a task lives in.
            name: "Migration Hub CreateProgressUpdateStream",
            host: HUB_HOST,
            target: "AWSMigrationHub.CreateProgressUpdateStream",
            signed: true,
            body: format!(
                r#"{{"ProgressUpdateStreamName":"{STREAM_NAME}","DryRun":false}}"#
            ),
            who: "Migration Hub",
            status: 200,
            present: &[],
            absent: &["HomeRegionControls"],
        },
        Case {
            name: "Migration Hub ImportMigrationTask",
            host: HUB_HOST,
            target: "AWSMigrationHub.ImportMigrationTask",
            signed: true,
            body: format!(
                r#"{{"ProgressUpdateStream":"{STREAM_NAME}",
                     "MigrationTaskName":"{TASK_NAME}",
                     "DryRun":false}}"#
            ),
            who: "Migration Hub",
            status: 200,
            present: &[],
            absent: &["HomeRegionControls"],
        },
        Case {
            // The task just imported, read back: the Migration Hub writes landed in the
            // Migration Hub store, not in its peer's.
            name: "Migration Hub ListMigrationTasks reports the imported task",
            host: HUB_HOST,
            target: "AWSMigrationHub.ListMigrationTasks",
            signed: true,
            body: "{}".to_string(),
            who: "Migration Hub",
            status: 200,
            present: &["MigrationTaskSummaryList", TASK_NAME, STREAM_NAME],
            absent: &["HomeRegionControls", "hrc-"],
        },
        Case {
            // …and the Config read still reports only what Config stored, so neither
            // service answered for the other at any point.
            name: "Config DescribeHomeRegionControls still reports only its own control",
            host: CONFIG_HOST,
            target: "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
            signed: true,
            body: "{}".to_string(),
            who: "MigrationHub Config",
            status: 200,
            present: &["HomeRegionControls", "hrc-", HOME_REGION],
            absent: &[TASK_NAME, "MigrationTaskSummaryList"],
        },
        Case {
            // A target naming an operation its service does not declare: the service the
            // target NAMES answers the protocol's own routing failure (awsJson1_1 ->
            // 501), which a client can act on — not a refusal that two services claimed
            // the request.
            name: "Config target naming an operation the model does not declare",
            host: CONFIG_HOST,
            target: "AWSMigrationHubMultiAccountService.DescribeNothingAtAll",
            signed: true,
            body: "{}".to_string(),
            who: "MigrationHub Config",
            status: 501,
            present: &["NotImplemented"],
            absent: &[],
        },
        Case {
            name: "Migration Hub target naming an operation the model does not declare",
            host: HUB_HOST,
            target: "AWSMigrationHub.ListNothingAtAll",
            signed: true,
            body: "{}".to_string(),
            who: "Migration Hub",
            status: 501,
            present: &["NotImplemented"],
            absent: &[],
        },
    ]
}

#[test]
fn migration_hub_and_migrationhub_config_are_each_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("migrationhub-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check:
                // these cases are about which service claims a request.
                router.set_request_authentication(false);

                for case in cases() {
                    let mut builder = http::Request::builder()
                        .method("POST")
                        .uri(format!("https://{}/", case.host))
                        .header("host", case.host)
                        .header("content-type", "application/x-amz-json-1.1")
                        .header("x-amz-target", case.target);
                    if case.signed {
                        builder = builder.header("authorization", AUTHORIZATION);
                    }
                    let request = builder
                        .body(bytes::Bytes::from(case.body.clone()))
                        .expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {:?}: two services claimed one request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    assert!(
                        !body.contains("UnknownService"),
                        "case {:?}: no service claimed the request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    assert_eq!(
                        status, case.status,
                        "case {:?}: expected HTTP {} from {}, got {status} {body}",
                        case.name, case.status, case.who
                    );
                    for marker in case.present {
                        assert!(
                            body.contains(marker),
                            "case {:?}: expected a body from {} carrying {marker:?}, got \
                             {status} {body}",
                            case.name,
                            case.who
                        );
                    }
                    for marker in case.absent {
                        assert!(
                            !body.contains(marker),
                            "case {:?}: expected a body from {} carrying no {marker:?}, got \
                             {status} {body}",
                            case.name,
                            case.who
                        );
                    }
                }
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("the routing check must pass");
}

// Cross-service routing for the `mgh` SigV4 signing name, from the service side.
//
// Migration Hub (`AWSMigrationHub`, endpointPrefix `mgh`) and MigrationHub Config
// (`AWSMigrationHubMultiAccountService`, endpointPrefix `migrationhub-config`) SHARE one
// SigV4 signing name, `mgh`, and one `arnNamespace`, while serving two different
// endpoints. The router resolves a request's routing key from the SigV4 credential scope
// first, so both services see the key `mgh`: a service that claims on the signing name
// alone claims the other's traffic too, and the router — which refuses rather than
// guesses — answers `500` with an error code no AWS model declares and a 5xx an SDK
// retries to exhaustion.
//
// The rule the cases below pin: a request signed for a signing name several emulated
// services share is claimed by exactly ONE of them — on an RPC protocol, the service the
// request's own wire form NAMES. Both models are `awsJson1_1`, so every operation posts
// to `/` and names its target out of band in
// `X-Amz-Target: <service shape>.<OperationName>`; the service-shape prefix is therefore
// the only discriminator either model publishes, and no path can separate the pair.
//
// Rows span what that rule generalises over: which service owns the operation, the
// prefix form a client may send (the shape's local name, the same name with an
// API-version suffix, the fully qualified Coral name), an operation name one service owns
// and the other does not, a prefix neither model declares, and a request carrying no
// target at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// services share. The signature is never checked: the routers here run with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTMIGRATIONHUB/20260101/us-west-2/mgh/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000";

/// How a row names the service that must claim it. `Both` and `Neither` are the two
/// routing failures: `Both` is the `500 AmbiguousService` this issue is about, `Neither`
/// the router's `404 UnknownService`.
const OWNER_HUB: &str = "mgh (Migration Hub)";
const OWNER_CONFIG: &str = "migrationhub-config (MigrationHub Config)";
const OWNER_BOTH: &str = "both (the router answers 500 AmbiguousService)";
const OWNER_NEITHER: &str = "neither (the router answers 404 UnknownService)";

/// The host that names Migration Hub's endpoint, the one that names MigrationHub Config's,
/// and a bare emulator endpoint that names neither.
const HUB_HOST: &str = "mgh.us-west-2.amazonaws.com";
const CONFIG_HOST: &str = "migrationhub-config.us-west-2.amazonaws.com";
const BARE_HOST: &str = "localhost:9297";

type Hub =
    service_com_amazonaws_migrationhub_awsmigrationhub::EmulatedService<engine::InMemoryEngine>;
type Config = service_com_amazonaws_migrationhubconfig_awsmigrationhubmultiaccountservice::EmulatedService<
    engine::InMemoryEngine,
>;

/// The two services that share the `mgh` signing name, over one shared engine, exactly as
/// `router/src/lib.rs` mounts them.
fn both_services() -> (Hub, Config) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (Hub::new(Arc::clone(&engine)), Config::new(engine))
}

/// A router with both of them mounted, so a request that two services claim really is
/// refused rather than served.
fn router_with_both_services() -> MultiServiceRouter {
    let (hub, config) = both_services();
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(hub);
    router.register_service(config);
    router
}

/// The request parts the router hands to `Service::can_handle`. `target` is the
/// `X-Amz-Target` value, or `None` for a request that carries none.
fn parts(host: &str, target: Option<&str>) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("authorization", AUTHORIZATION);
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    builder
        .body(Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

/// The service that claims a request, as one of the four `OWNER_*` outcomes.
fn claimant(hub: &Hub, config: &Config, parts: &http::request::Parts) -> &'static str {
    match (hub.can_handle(parts), config.can_handle(parts)) {
        (true, false) => OWNER_HUB,
        (false, true) => OWNER_CONFIG,
        (true, true) => OWNER_BOTH,
        (false, false) => OWNER_NEITHER,
    }
}

/// One answer from the router: the HTTP status, the awsJson error code (the empty string
/// on a response that carries none) and the parsed body.
async fn call(router: &MultiServiceRouter, target: &str, body: Value) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", BARE_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string();
    (status, code, parsed)
}

/// A well-formed `CreateHomeRegionControl` request body.
fn create_home_region_control_body(region: &str) -> Value {
    json!({
        "HomeRegion": region,
        "Target": { "Type": "ACCOUNT", "Id": engine::emulator_account_id() },
        "DryRun": false,
    })
}

#[test]
fn exactly_one_service_sharing_the_mgh_signing_name_claims_each_request() {
    let (hub, config) = both_services();

    // Operation names come from each model's own surface, as each crate's `OPERATIONS.md`
    // lists them (`smithy_query(query="digest", …)` on each model). Rows are
    // (case, host, X-Amz-Target, the service that must claim it).
    let rows: Vec<(&str, &str, Option<&str>, &str)> = vec![
        // MigrationHub Config's four operations, on hosts that name it, name its sibling,
        // and name nothing at all: on a SIGNED request the credential scope resolves the
        // key, so the host cannot change the answer.
        (
            "config CreateHomeRegionControl",
            CONFIG_HOST,
            Some("AWSMigrationHubMultiAccountService.CreateHomeRegionControl"),
            OWNER_CONFIG,
        ),
        (
            "config GetHomeRegion on the sibling's host",
            HUB_HOST,
            Some("AWSMigrationHubMultiAccountService.GetHomeRegion"),
            OWNER_CONFIG,
        ),
        (
            "config DescribeHomeRegionControls on a bare emulator endpoint",
            BARE_HOST,
            Some("AWSMigrationHubMultiAccountService.DescribeHomeRegionControls"),
            OWNER_CONFIG,
        ),
        (
            "config DeleteHomeRegionControl",
            BARE_HOST,
            Some("AWSMigrationHubMultiAccountService.DeleteHomeRegionControl"),
            OWNER_CONFIG,
        ),
        // Migration Hub's own operations, including one whose name the sibling does not
        // publish at all.
        (
            "hub CreateProgressUpdateStream",
            HUB_HOST,
            Some("AWSMigrationHub.CreateProgressUpdateStream"),
            OWNER_HUB,
        ),
        (
            "hub ListMigrationTasks on the sibling's host",
            CONFIG_HOST,
            Some("AWSMigrationHub.ListMigrationTasks"),
            OWNER_HUB,
        ),
        (
            "hub DescribeApplicationState on a bare emulator endpoint",
            BARE_HOST,
            Some("AWSMigrationHub.DescribeApplicationState"),
            OWNER_HUB,
        ),
        // The prefix forms a client may send for the same service: the shape's local name
        // with an API-version suffix, and the fully qualified Coral name. Neither changes
        // which service the target names.
        (
            "config target with an API-version suffix",
            BARE_HOST,
            Some("AWSMigrationHubMultiAccountService_20190630.GetHomeRegion"),
            OWNER_CONFIG,
        ),
        (
            "hub target with an API-version suffix",
            BARE_HOST,
            Some("AWSMigrationHub_20170531.ListProgressUpdateStreams"),
            OWNER_HUB,
        ),
        (
            "config target as a fully qualified name",
            BARE_HOST,
            Some("com.amazonaws.migrationhubconfig.AWSMigrationHubMultiAccountService.GetHomeRegion"),
            OWNER_CONFIG,
        ),
        (
            "hub target as a fully qualified name",
            BARE_HOST,
            Some("com.amazonaws.migrationhub.AWSMigrationHub.ListProgressUpdateStreams"),
            OWNER_HUB,
        ),
        // An operation name of ONE service under the OTHER service's prefix: the prefix
        // decides, so the named service claims it and then refuses it as a routing error
        // of its own rather than two services claiming it.
        (
            "config's operation name under the hub's prefix",
            BARE_HOST,
            Some("AWSMigrationHub.GetHomeRegion"),
            OWNER_HUB,
        ),
        (
            "the hub's operation name under config's prefix",
            BARE_HOST,
            Some("AWSMigrationHubMultiAccountService.ListMigrationTasks"),
            OWNER_CONFIG,
        ),
        // A prefix neither model declares, and a request with no target at all: nobody may
        // claim either, which is the router's honest `404` rather than a 500.
        (
            "a prefix no model declares",
            BARE_HOST,
            Some("AWSMigrationHubSomethingElse.GetHomeRegion"),
            OWNER_NEITHER,
        ),
        ("no target at all", BARE_HOST, None, OWNER_NEITHER),
    ];

    for (case, host, target, expected) in rows {
        let claimed = claimant(&hub, &config, &parts(host, target));
        assert_eq!(
            claimed, expected,
            "case {case:?} (target {target:?} on {host}): exactly one service must claim \
             it, got {claimed:?}"
        );
    }
}

#[tokio::test]
async fn every_migrationhub_config_operation_is_served_rather_than_refused_as_ambiguous() {
    let router = router_with_both_services();

    // A pristine account: the two reads report "nothing configured" as successes.
    let (status, code, body) = call(
        &router,
        "AWSMigrationHubMultiAccountService.GetHomeRegion",
        json!({}),
    )
    .await;
    assert_eq!(
        (status, code.as_str(), body.clone()),
        (200, "", json!({})),
        "case \"GetHomeRegion unset\": expected 200 with no HomeRegion, got {status} {code:?} {body}"
    );

    let (status, code, body) = call(
        &router,
        "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
        json!({}),
    )
    .await;
    assert_eq!(
        (status, code.as_str(), body["HomeRegionControls"].clone()),
        (200, "", json!([])),
        "case \"DescribeHomeRegionControls on a pristine account\": expected 200 with an \
         empty list, got {status} {code:?} {body}"
    );

    // A malformed region is this API's own 400, not an unmodelled 5xx.
    let (status, code, body) = call(
        &router,
        "AWSMigrationHubMultiAccountService.CreateHomeRegionControl",
        create_home_region_control_body("Not_A_Region!"),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInputException"),
        "case \"CreateHomeRegionControl with a malformed region\": got {status} {code:?} {body}"
    );

    // A well-formed one is served, and read back through both readers.
    let (status, code, created) = call(
        &router,
        "AWSMigrationHubMultiAccountService.CreateHomeRegionControl",
        create_home_region_control_body("ap-northeast-2"),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"CreateHomeRegionControl\": got {status} {code:?} {created}"
    );
    let control_id = created["HomeRegionControl"]["ControlId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        created["HomeRegionControl"]["Target"],
        json!({ "Type": "ACCOUNT", "Id": engine::emulator_account_id() }),
        "case \"CreateHomeRegionControl\": the control targets the calling account, got {created}"
    );

    let (status, _, body) = call(
        &router,
        "AWSMigrationHubMultiAccountService.GetHomeRegion",
        json!({}),
    )
    .await;
    assert_eq!(
        (status, body["HomeRegion"].as_str()),
        (200, Some("ap-northeast-2")),
        "case \"GetHomeRegion after create\": got {status} {body}"
    );

    let (status, _, body) = call(
        &router,
        "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
        json!({}),
    )
    .await;
    assert_eq!(
        (status, body["HomeRegionControls"][0]["ControlId"].as_str()),
        (200, Some(control_id.as_str())),
        "case \"DescribeHomeRegionControls after create\": got {status} {body}"
    );

    // An unknown control id is the modelled 400 naming the id.
    let (status, code, body) = call(
        &router,
        "AWSMigrationHubMultiAccountService.DeleteHomeRegionControl",
        json!({ "ControlId": "hrc-000000000000" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInputException"),
        "case \"DeleteHomeRegionControl on an unknown id\": got {status} {code:?} {body}"
    );

    let (status, code, body) = call(
        &router,
        "AWSMigrationHubMultiAccountService.DeleteHomeRegionControl",
        json!({ "ControlId": control_id.clone() }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"DeleteHomeRegionControl\": got {status} {code:?} {body}"
    );
}

#[tokio::test]
async fn no_mgh_request_is_answered_by_the_routers_refusals() {
    let router = router_with_both_services();

    // The two routing failures no row may provoke: `AmbiguousService` (500, several
    // claimants) and `UnknownService` (404, none claimed) on an operation a model DOES
    // declare. Every row must be answered by one service's own awsJson envelope instead.
    //
    // `served` is false for an operation this emulator still SCAFFOLDS: its own 501
    // stub channel is the claiming service answering, which is what the routing rule
    // requires, and whether that operation is implemented is its own issue.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "AWSMigrationHubMultiAccountService.GetHomeRegion",
            json!({}),
            true,
        ),
        (
            "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
            json!({}),
            true,
        ),
        (
            "AWSMigrationHubMultiAccountService.CreateHomeRegionControl",
            create_home_region_control_body("il-central-1"),
            true,
        ),
        (
            "AWSMigrationHubMultiAccountService.DeleteHomeRegionControl",
            json!({ "ControlId": "hrc-000000000000" }),
            true,
        ),
        ("AWSMigrationHub.ListProgressUpdateStreams", json!({}), false),
        (
            "AWSMigrationHub.CreateProgressUpdateStream",
            json!({ "ProgressUpdateStreamName": "routing-probe", "DryRun": false }),
            false,
        ),
    ];

    for (target, body, served) in rows {
        let (status, code, parsed) = call(&router, target, body).await;
        assert_ne!(
            code, "AmbiguousService",
            "case {target:?}: a request two services claim is refused 500 AmbiguousService; \
             exactly one must claim it, got {status} {code:?} {parsed}"
        );
        assert_ne!(
            code, "UnknownService",
            "case {target:?}: an operation a model declares must be claimed, got {status} \
             {code:?} {parsed}"
        );
        if served {
            assert!(
                status < 500,
                "case {target:?}: no implemented operation answers a 5xx, got {status} \
                 {code:?} {parsed}"
            );
        }
    }

    // And a prefix NEITHER model declares is the router's honest `404 UnknownService` —
    // not the 500 two claimants produce.
    let (status, code, parsed) =
        call(&router, "AWSMigrationHubSomethingElse.GetHomeRegion", json!({})).await;
    assert_eq!(
        (status, code.as_str()),
        (404, "UnknownService"),
        "case \"a prefix no model declares\": nobody claims it, got {status} {code:?} {parsed}"
    );
}

/// An UNSIGNED request names no service in a credential scope, so the router resolves its
/// routing key from the `X-Amz-Target` prefix instead — a key that is neither service's
/// signing name. Each service claims the key its OWN prefix resolves to, so such a
/// request still reaches exactly the service it names (and, on a router that verifies
/// signatures, is refused there as AWS refuses it) rather than reaching none.
#[tokio::test]
async fn an_unsigned_request_is_claimed_by_the_service_its_target_names() {
    let (hub, config) = both_services();
    let router = router_with_both_services();

    // Rows are (case, X-Amz-Target, the service that must claim it) on a bare emulator
    // endpoint, which names no service, so only the target can decide.
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "config DescribeHomeRegionControls",
            "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
            OWNER_CONFIG,
        ),
        (
            "config GetHomeRegion",
            "AWSMigrationHubMultiAccountService.GetHomeRegion",
            OWNER_CONFIG,
        ),
        (
            "hub ListMigrationTasks",
            "AWSMigrationHub.ListMigrationTasks",
            OWNER_HUB,
        ),
        (
            "hub CreateProgressUpdateStream",
            "AWSMigrationHub.CreateProgressUpdateStream",
            OWNER_HUB,
        ),
        (
            "a prefix no model declares",
            "AWSMigrationHubSomethingElse.GetHomeRegion",
            OWNER_NEITHER,
        ),
    ];

    for (case, target, expected) in rows {
        let claimed = claimant(&hub, &config, &unsigned_parts(BARE_HOST, Some(target)));
        assert_eq!(
            claimed, expected,
            "case {case:?} (unsigned, target {target:?}): exactly one service must claim \
             it, got {claimed:?}"
        );
    }

    // …and the claimed request is answered by that service, not by the router's refusals.
    // This router verifies no signature, so the answer is the operation's own: the
    // emulator's authenticating front door refuses an unsigned call on its own terms,
    // which is a separate seam from which service claims it.
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", BARE_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls",
        )
        .body(Bytes::from(json!({}).to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();
    assert_eq!(
        status, 200,
        "case \"unsigned DescribeHomeRegionControls\": expected the claiming service's own \
         answer, got {status} {body}"
    );
    assert!(
        body.contains("HomeRegionControls"),
        "case \"unsigned DescribeHomeRegionControls\": expected MigrationHub Config's own \
         answer, got {status} {body}"
    );
}

/// The request parts of an UNSIGNED call: [`parts`] without the `Authorization` header,
/// so the router's first routing signal is absent and the `X-Amz-Target` prefix is what
/// it resolves the key from.
fn unsigned_parts(host: &str, target: Option<&str>) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1");
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    builder
        .body(Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}
