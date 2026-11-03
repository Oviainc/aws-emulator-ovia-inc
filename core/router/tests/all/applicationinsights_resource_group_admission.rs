// SPDX-License-Identifier: CC0-1.0

//! Application Insights resolves an application's `ResourceGroupName` against Resource
//! Groups before it creates anything.
//!
//! The rule under test: a create whose input names a resource of ANOTHER service
//! resolves that resource against the owning service before it persists anything, and
//! answers its own modelled not-found error — here `ResourceNotFoundException`, which
//! `CreateApplication` declares (smithy_query: operation CreateApplication, errors
//! include ResourceNotFoundException) — when the owner reports it absent, leaving no
//! application behind. `CreateApplication` "Adds an application that is created from a
//! resource group", and a resource group is Resource Groups' resource, so its existence
//! is Resource Groups' to answer.
//!
//! This suite needs TWO services on one engine — Application Insights reaches Resource
//! Groups through the in-process callback, so a single-service router has nobody to ask
//! and the group a row names has to be creatable — which is why it lives here rather
//! than in either service's own crate, exactly as `s3files_create_file_system_bucket.rs`
//! does for S3 Files and S3. The applicationinsights crate's own suite pins the other
//! half: with no Resource Groups to ask, the create is admitted rather than refused on
//! an assumption.
//!
//! The rows span what the rule generalises over: whether the group was ever created, the
//! group name's length (the modelled `ResourceGroupName` is 1..256 characters) and form
//! (dots, hyphens, underscores, digits), the region the group was created in, and a name
//! that merely HAS an existing group's name as a prefix — so the resolution is
//! exact-name, not prefix, matching.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for, unless a row says otherwise.
const REGION: &str = "eu-west-2";

/// Another region, for the row whose group is created through a different endpoint: a
/// resource group is regional, so a group of another region answers no lookup here.
const OTHER_REGION: &str = "ap-south-1";

/// The `X-Amz-Target` prefix this service's awsJson1_1 operations are named by.
const TARGET_PREFIX: &str = "EC2WindowsBarleyService";

/// What one call answered: the HTTP status and the parsed JSON body (`Value::Null` for a
/// body that is empty or not JSON).
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The awsJson error code, without its namespace.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .rsplit(['#', ':'])
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// The error message, whichever casing the envelope carries it under.
    fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }
}

/// Dispatch one request addressed to `signing_name` in `region`, the way a signed SDK
/// call is: the credential scope is what names the service and the region.
fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    region: &str,
    path: &str,
    target: Option<&str>,
    body: Value,
) -> Answer {
    let host = format!("{signing_name}.{region}.amazonaws.com");
    let content_type = if target.is_some() {
        "application/x-amz-json-1.1"
    } else {
        "application/json"
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", content_type)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        );
    if let Some(target) = target {
        request = request.header("x-amz-target", format!("{TARGET_PREFIX}.{target}"));
    }
    let request = request
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("serialize body"),
        ))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// One Application Insights operation, over its awsJson1_1 wire.
fn application_insights(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: Value,
) -> Answer {
    send(
        router,
        runtime,
        "applicationinsights",
        REGION,
        "/",
        Some(operation),
        body,
    )
}

/// `POST /groups` — Resource Groups' own `CreateGroup`, in `region`.
fn create_group(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    region: &str,
    name: &str,
) {
    let answer = send(
        router,
        runtime,
        "resource-groups",
        region,
        "/groups",
        None,
        json!({ "Name": name }),
    );
    assert_eq!(
        answer.status, 200,
        "setup: CreateGroup({name:?}) in {region} must succeed, got {} / {:?}",
        answer.status, answer.body
    );
}

/// The refusal AWS answers a create over a group the account does not hold.
fn group_missing_message(name: &str) -> String {
    format!(
        "Resource Group with name {name} does not exist in {}",
        engine::emulator_account_id()
    )
}

/// One row: the group the row names, whether (and where) that group is created first,
/// the `ResourceGroupName` the create carries, and whether the create must be admitted.
struct Row {
    case: &'static str,
    /// The group to create through Resource Groups before the call, and the region to
    /// create it in, when the row has one.
    create: Option<(String, &'static str)>,
    resource_group_name: String,
    admitted: bool,
}

/// A 256-character group name — the longest the modelled `ResourceGroupName` (length
/// 1..256) admits.
fn longest_group_name() -> String {
    let name = format!("ai-gate-{}", "a".repeat(248));
    assert_eq!(
        name.chars().count(),
        256,
        "the longest-name row must be 256 characters"
    );
    name
}

fn rows() -> Vec<Row> {
    let existing = "ai-gate-plain";
    vec![
        Row {
            case: "a group that exists",
            create: Some((existing.to_string(), REGION)),
            resource_group_name: existing.to_string(),
            admitted: true,
        },
        Row {
            case: "a group that was never created",
            create: None,
            resource_group_name: "ai-gate-ghost".to_string(),
            admitted: false,
        },
        Row {
            // The shortest name the modelled length admits.
            case: "the shortest group name, created",
            create: Some(("a".to_string(), REGION)),
            resource_group_name: "a".to_string(),
            admitted: true,
        },
        Row {
            case: "the shortest group name, never created",
            create: None,
            resource_group_name: "b".to_string(),
            admitted: false,
        },
        Row {
            case: "the longest group name, created",
            create: Some((longest_group_name(), REGION)),
            resource_group_name: longest_group_name(),
            admitted: true,
        },
        Row {
            case: "a dotted, underscored, digit-bearing name, created",
            create: Some(("ai.gate_7-group".to_string(), REGION)),
            resource_group_name: "ai.gate_7-group".to_string(),
            admitted: true,
        },
        Row {
            case: "a dotted, underscored name that was never created",
            create: None,
            resource_group_name: "ai.gate_9-group".to_string(),
            admitted: false,
        },
        Row {
            // Resolution is exact-name: a name that merely STARTS WITH an existing
            // group's name is a different group.
            case: "a name that only has an existing group's name as its prefix",
            create: None,
            resource_group_name: format!("{existing}-extra"),
            admitted: false,
        },
        Row {
            // A resource group lives in the region whose endpoint created it, so the
            // same name in another region names no group here.
            case: "a group created in another region",
            create: Some(("ai-gate-elsewhere".to_string(), OTHER_REGION)),
            resource_group_name: "ai-gate-elsewhere".to_string(),
            admitted: false,
        },
    ]
}

/// Whether `DescribeApplication` reports an application for `name` — what decides
/// whether the create persisted anything.
fn application_exists(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
) -> bool {
    let read = application_insights(
        router,
        runtime,
        "DescribeApplication",
        json!({ "ResourceGroupName": name }),
    );
    read.status == 200
}

#[test]
fn an_application_is_created_only_for_a_group_resource_groups_holds() {
    std::thread::Builder::new()
        .name("applicationinsights-group-gate".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            for row in rows() {
                if let Some((name, region)) = &row.create {
                    create_group(&router, &runtime, region, name);
                }

                let created = application_insights(
                    &router,
                    &runtime,
                    "CreateApplication",
                    json!({ "ResourceGroupName": row.resource_group_name }),
                );

                if row.admitted {
                    assert_eq!(
                        created.status, 200,
                        "case {:?}: a group Resource Groups holds must be accepted, got {} / {:?}",
                        row.case, created.status, created.body
                    );
                    assert_eq!(
                        created.body["ApplicationInfo"]["ResourceGroupName"],
                        json!(row.resource_group_name),
                        "case {:?}: the create must report the group it stored, got {:?}",
                        row.case,
                        created.body
                    );
                    // Round trip: the create is readable back through
                    // DescribeApplication, which answers from the same stored record.
                    assert!(
                        application_exists(&router, &runtime, &row.resource_group_name),
                        "case {:?}: the application must be persisted",
                        row.case
                    );
                } else {
                    assert_eq!(
                        (created.status, created.error_code().as_str()),
                        (400, "ResourceNotFoundException"),
                        "case {:?}: a group Resource Groups does not hold must be refused \
                         400 ResourceNotFoundException, got {} / {:?}",
                        row.case,
                        created.status,
                        created.body
                    );
                    assert_eq!(
                        created.error_message(),
                        group_missing_message(&row.resource_group_name),
                        "case {:?}: expected the absent-group refusal, got {:?}",
                        row.case,
                        created.body
                    );
                    assert!(
                        !application_exists(&router, &runtime, &row.resource_group_name),
                        "case {:?}: a refused create must persist nothing",
                        row.case
                    );
                }
            }

            // The identical request, sent twice around one group's creation: refused
            // first, accepted after — so what decides is the group's existence and
            // nothing about the request itself.
            let name = "ai-gate-created-later";

            let before = application_insights(
                &router,
                &runtime,
                "CreateApplication",
                json!({ "ResourceGroupName": name }),
            );
            assert_eq!(
                (before.status, before.error_code().as_str()),
                (400, "ResourceNotFoundException"),
                "before the group exists the create must be refused, got {} / {:?}",
                before.status,
                before.body
            );
            assert!(
                !application_exists(&router, &runtime, name),
                "a refused create must leave the group with no application"
            );

            create_group(&router, &runtime, REGION, name);

            let after = application_insights(
                &router,
                &runtime,
                "CreateApplication",
                json!({ "ResourceGroupName": name }),
            );
            assert_eq!(
                after.status, 200,
                "once the group exists the identical create must succeed, got {} / {:?}",
                after.status, after.body
            );
            assert!(
                application_exists(&router, &runtime, name),
                "the accepted create must be readable back"
            );
        })
        .expect("spawn the group-gate thread")
        .join()
        .expect("every application's resource group must be resolved against Resource Groups");
}
