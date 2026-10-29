// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE rule that says what a Region's Quick Setup
//! service settings read as, however that Region's stored record came to be — the rule
//! `crate::state`'s `service_settings_in` now holds for every operation that reports a
//! Region's settings, rather than each side of the crate stating it for itself.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: settings are not a creatable resource (`GetServiceSettings`
//! binds no not-found and no validation error and its only output member is the
//! structure itself — `smithy_query(query="operation",
//! operation="GetServiceSettings")`), so every account and Region HAS settings and a
//! read answers 200 carrying the `ServiceSettings` structure, with exactly the members
//! an accepted update wrote for that Region and every other member unset. A Region no
//! update configured is the structure with no member set, never a body without the
//! structure.
//!
//! The rows below differ along how the Region's record came to be — never written,
//! written by an update naming no member, written by an accepted update, overwritten,
//! left alone by a refused update, written only in a DIFFERENT Region — and along the
//! Region string and how many reads precede the assertion, because none of that
//! changes the shape of the answer. Both operations that reach the shared rule take
//! part: `UpdateServiceSettings` decides whether the Region has a record, and
//! `GetServiceSettings` turns that record (or its absence) into the answer.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in `region` — the signal the router routes on
/// and the handlers read the Region from (the SigV4 credential scope).
fn request(
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{region}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, region, body)).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

async fn get_settings(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call(router, "GET", "/serviceSettings", region, None).await
}

async fn update_settings(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call(router, "PUT", "/serviceSettings", region, Some(body)).await
}

/// The `ServiceSettings` structure a read answered, asserting it is there at all: the
/// operation models no way for settings to be absent, so the structure is part of every
/// answer.
fn reported_settings(case: &str, answer: &Value) -> Value {
    let settings = answer.get("ServiceSettings").cloned().unwrap_or(Value::Null);
    assert!(
        settings.is_object(),
        "case {case:?}: every answer carries the ServiceSettings structure, got {answer:?}"
    );
    settings
}

/// What the update history of one Region is, before the read under test.
#[derive(Debug, Clone, Copy)]
enum History {
    /// No update was ever sent for this Region.
    NeverWritten,
    /// Updates sent for this Region, in order, each expected to be accepted; the LAST
    /// one is what the read must report.
    Accepted(&'static [&'static str]),
    /// An update naming no member at all, which writes nothing.
    NoMemberNamed,
    /// An accepted update, then an update naming no member: the record stays as the
    /// accepted one left it.
    AcceptedThenNoMemberNamed(&'static str),
    /// An update this service refuses (a value the `IAMRoleArn` reference rules out),
    /// which must leave the Region exactly as unconfigured as it was.
    Refused(&'static str),
    /// Another Region was configured; this one never was.
    OnlyAnotherRegionWritten,
}

/// One accepted-update role ARN, re-used where the case is not about the ARN's form.
const SOME_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/QuickSetupExplorer";

/// Drive `history` against `region`, so the read that follows sees exactly that store.
async fn apply(router: &MultiServiceRouter, case: &str, region: &str, history: History) {
    let accepted: Vec<&str> = match history {
        History::NeverWritten => Vec::new(),
        History::Accepted(arns) => arns.to_vec(),
        History::NoMemberNamed => Vec::new(),
        History::AcceptedThenNoMemberNamed(arn) => vec![arn],
        History::Refused(_) => Vec::new(),
        History::OnlyAnotherRegionWritten => Vec::new(),
    };
    for arn in accepted {
        let (status, answer) =
            update_settings(router, region, &json!({ "ExplorerEnablingRoleArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update naming {arn:?} must be accepted, got {status} / {answer:?}"
        );
    }
    match history {
        History::NoMemberNamed | History::AcceptedThenNoMemberNamed(_) => {
            let (status, answer) = update_settings(router, region, &json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: an update naming no member must be accepted, got {status} / \
                 {answer:?}"
            );
        }
        History::Refused(bad_arn) => {
            let (status, answer) =
                update_settings(router, region, &json!({ "ExplorerEnablingRoleArn": bad_arn }))
                    .await;
            assert_eq!(
                status, 400,
                "case {case:?}: the update naming {bad_arn:?} must be refused, got {status} / \
                 {answer:?}"
            );
        }
        History::OnlyAnotherRegionWritten => {
            let elsewhere = "ca-central-1";
            assert_ne!(
                elsewhere, region,
                "case {case:?}: the other Region must not be the one under test"
            );
            let (status, answer) = update_settings(
                router,
                elsewhere,
                &json!({ "ExplorerEnablingRoleArn": "arn:aws:iam::000000000000:role/Elsewhere" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the update for {elsewhere} must be accepted, got {status} / \
                 {answer:?}"
            );
        }
        History::NeverWritten | History::Accepted(_) => {}
    }
}

/// Whatever a Region's update history, a read answers 200 carrying the
/// `ServiceSettings` structure with exactly the members an accepted update wrote — and
/// reading does not change what a later caller sees.
#[tokio::test]
async fn every_region_reads_as_the_service_settings_structure_its_updates_left() {
    // case, Region, history, how many reads to take, the structure every read must
    // answer.
    let cases: [(&str, &str, History, usize, Value); 8] = [
        (
            "never written",
            "us-west-2",
            History::NeverWritten,
            1,
            json!({}),
        ),
        (
            "never written, read three times",
            "eu-west-1",
            History::NeverWritten,
            3,
            json!({}),
        ),
        (
            "an update naming no member writes nothing",
            "ap-southeast-2",
            History::NoMemberNamed,
            2,
            json!({}),
        ),
        (
            "a refused update leaves the Region unconfigured",
            "us-east-1",
            // Not an IAM role ARN: the `IAMRoleArn` reference fixes the `iam` service
            // and the `role` resource, and this names neither.
            History::Refused("arn:aws:ssm-quicksetup:us-east-1:000000000000:user/Explorer"),
            1,
            json!({}),
        ),
        (
            "only another Region was configured",
            "eu-central-1",
            History::OnlyAnotherRegionWritten,
            1,
            json!({}),
        ),
        (
            "configured here",
            "sa-east-1",
            History::Accepted(&[SOME_ROLE_ARN]),
            1,
            json!({ "ExplorerEnablingRoleArn": SOME_ROLE_ARN }),
        ),
        (
            "overwritten: the last accepted update is what reads back",
            "ap-south-1",
            History::Accepted(&[
                "arn:aws:iam::123456789012:role/first/Explorer",
                "arn:aws-cn:iam::123456789012:role/Second",
            ]),
            2,
            json!({ "ExplorerEnablingRoleArn": "arn:aws-cn:iam::123456789012:role/Second" }),
        ),
        (
            "configured, then an update naming no member",
            "me-south-1",
            History::AcceptedThenNoMemberNamed(SOME_ROLE_ARN),
            1,
            json!({ "ExplorerEnablingRoleArn": SOME_ROLE_ARN }),
        ),
    ];

    for (case, region, history, reads, expected) in cases {
        let router = emulator();
        apply(&router, case, region, history).await;

        for read in 1..=reads {
            let (status, answer) = get_settings(&router, region).await;
            assert_eq!(
                status, 200,
                "case {case:?}: read {read} must answer 200, got {status} / {answer:?}"
            );
            let reported = reported_settings(case, &answer);
            assert_eq!(
                reported, expected,
                "case {case:?}: read {read} must report {expected}, got {reported}"
            );
        }
    }
}
