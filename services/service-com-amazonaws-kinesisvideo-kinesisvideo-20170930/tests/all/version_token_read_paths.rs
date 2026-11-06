// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the `Version` token every read path of this service
//! projects: `DescribeStream` and `ListStreams` for a stream, `DescribeSignalingChannel`
//! and `ListSignalingChannels` for a signaling channel.
//!
//! RULE: a record's `Version` is one opaque per-record token (`@length` 1..64,
//! `@pattern` `^[a-zA-Z0-9]+$`, from
//! `smithy_query(query="shape", shape="Version")`), minted for that record alone: every
//! read path reports the SAME token for the same record, two records of one store carry
//! unrelated tokens whether they were created together or apart, and the token is what
//! the record's own mutation takes back as `CurrentVersion` — which re-mints it, on
//! every path at once. Nothing a caller can read is encoded in it, so no token orders
//! the store's mutations and no page of records shares a prefix or a suffix.
//!
//! The round trip is driven through the implemented readers rather than through the
//! state, so a token that reached only one projection would fail here.
//!
//! The harness mirrors the one in the sibling suites of this crate (each test target
//! module owns its own, since none of them is importable from another).

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over its own engine, with request
/// authentication off so a case can send an unsigned-but-scoped request.
fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(kvs::EmulatedService::new(engine));
    router
}

/// The emulator endpoint every case addresses.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// POST one restJson1 call to `path`.
async fn call(
    router: &MultiServiceRouter,
    path: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{ENDPOINT_HOST}{path}"))
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service and
        // names its region; the signature itself is not checked here.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// The string member `name` of `value`.
fn text(case: &str, value: &serde_json::Value, name: &str) -> String {
    value
        .get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no string {name:?} in {value:?}"))
        .to_string()
}

/// Wait out the activation spans so the clock owes every freshly created stream and
/// channel its move to `ACTIVE` — the status a mutation requires.
fn wait_for_activation() {
    let span = kvs::state::STREAM_ACTIVATION_SPAN.max(kvs::state::CHANNEL_ACTIVATION_SPAN);
    std::thread::sleep(span + Duration::from_millis(150));
}

/// The `Version` `DescribeStream` reports for `name`.
async fn described_stream_version(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "/describeStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: DescribeStream expected 200, got {status} with {body:?}"
    );
    let info = body
        .get("StreamInfo")
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: no StreamInfo in {body:?}"));
    text(name, &info, "Version")
}

/// The `Version` `ListStreams` reports for `name`, over the whole (unfiltered) page, so
/// the list projection is exercised as a client enumerating the account sees it.
async fn listed_stream_version(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "/listStreams", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "case {name:?}: ListStreams expected 200, got {status} with {body:?}"
    );
    let listed = body
        .get("StreamInfoList")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: no StreamInfoList in {body:?}"));
    let found = listed
        .iter()
        .find(|info| info.get("StreamName").and_then(serde_json::Value::as_str) == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: not listed among {listed:?}"));
    text(name, &found, "Version")
}

/// The `Version` `DescribeSignalingChannel` reports for `name`.
async fn described_channel_version(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "/describeSignalingChannel",
        serde_json::json!({ "ChannelName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: DescribeSignalingChannel expected 200, got {status} with {body:?}"
    );
    let info = body
        .get("ChannelInfo")
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: no ChannelInfo in {body:?}"));
    text(name, &info, "Version")
}

/// The `Version` `ListSignalingChannels` reports for `name`.
async fn listed_channel_version(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "/listSignalingChannels", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "case {name:?}: ListSignalingChannels expected 200, got {status} with {body:?}"
    );
    let listed = body
        .get("ChannelInfoList")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: no ChannelInfoList in {body:?}"));
    let found = listed
        .iter()
        .find(|info| info.get("ChannelName").and_then(serde_json::Value::as_str) == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: not listed among {listed:?}"));
    text(name, &found, "Version")
}

/// `Version`'s modeled `@length` bounds, from
/// `smithy_query(query="shape", shape="Version")`.
const MIN_LENGTH: usize = 1;
/// See [`MIN_LENGTH`].
const MAX_LENGTH: usize = 64;

/// Assert one token satisfies the modeled `Version` shape.
fn assert_modeled_token(case: &str, token: &str) {
    assert!(
        token.chars().all(|symbol| symbol.is_ascii_alphanumeric()),
        "case {case:?}: a token must satisfy ^[a-zA-Z0-9]+$, got {token:?}"
    );
    let length = token.chars().count();
    assert!(
        (MIN_LENGTH..=MAX_LENGTH).contains(&length),
        "case {case:?}: expected a length within {MIN_LENGTH}..={MAX_LENGTH}, got {length} in \
         {token:?}"
    );
}

/// RULE: both stream read paths project one record's one token, and a mutation re-mints
/// it on both at once.
///
/// The rows differ along how many streams the store holds when the page is read (so a
/// page of one, of two and of several is covered), and along which mutation re-mints the
/// token.
#[tokio::test]
async fn both_stream_read_paths_project_the_record_s_own_token() {
    let router = harness();

    // (case, how many streams exist by the time this one is read)
    let rows: &[(&str, usize)] = &[
        ("the only stream of the store", 1),
        ("the second of two in one page", 2),
        ("one of five in one page", 5),
    ];

    let mut collected: Vec<(String, String)> = Vec::new();
    let mut created = 0_usize;
    for (case, population) in rows {
        while created < *population {
            let name = format!("version-stream-{created}");
            let (status, body) = call(
                &router,
                "/createStream",
                serde_json::json!({ "StreamName": name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateStream {name:?} expected 200, got {status} with {body:?}"
            );
            created += 1;
        }
        wait_for_activation();

        let name = format!("version-stream-{}", population - 1);
        let described = described_stream_version(&router, &name).await;
        let listed = listed_stream_version(&router, &name).await;
        assert_modeled_token(case, &described);
        assert_eq!(
            described, listed,
            "case {case:?}: DescribeStream reported {described:?} and ListStreams {listed:?} for \
             one record"
        );
        collected.push((format!("{case} at creation"), described.clone()));

        // The token the reads report is the one the record's own mutation takes back,
        // and that mutation re-mints it on both paths at once.
        let (status, body) = call(
            &router,
            "/updateDataRetention",
            serde_json::json!({
                "StreamName": name.clone(),
                "CurrentVersion": described.clone(),
                "Operation": "INCREASE_DATA_RETENTION",
                "DataRetentionChangeInHours": 1
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reported token must work as CurrentVersion, got {status} with \
             {body:?}"
        );
        let reminted = described_stream_version(&router, &name).await;
        let relisted = listed_stream_version(&router, &name).await;
        assert_modeled_token(case, &reminted);
        assert_ne!(
            reminted, described,
            "case {case:?}: a mutation must re-mint the token, still {reminted:?}"
        );
        assert_eq!(
            reminted, relisted,
            "case {case:?}: after the update DescribeStream reported {reminted:?} and ListStreams \
             {relisted:?}"
        );
        collected.push((format!("{case} after its update"), reminted));
    }

    assert_unrelated(&collected);
}

/// RULE: the signaling-channel read paths carry the same rule, and a channel's token is
/// unrelated to a stream's minted in the same store — nothing in either reveals how many
/// records the other index holds.
///
/// The rows differ along how many channels the store holds when the page is read, and the
/// mutation that re-mints the token is the channel's own
/// (`UpdateSignalingChannel`), whose `MessageTtlSeconds` varies within its modeled
/// `@range` 5..120.
#[tokio::test]
async fn both_signaling_channel_read_paths_project_the_record_s_own_token() {
    let router = harness();

    // (case, how many channels exist by the time this one is read, the TTL the update
    // sets — inside `MessageTtlSeconds`' modeled range)
    let rows: &[(&str, usize, i64)] = &[
        ("the only channel of the store", 1, 5),
        ("the second of two in one page", 2, 60),
        ("one of four in one page", 4, 120),
    ];

    // A stream created in the same store, so the two indexes' tokens are compared.
    let (status, body) = call(
        &router,
        "/createStream",
        serde_json::json!({ "StreamName": "version-neighbour-stream" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateStream expected 200, got {status} with {body:?}"
    );

    let mut collected: Vec<(String, String)> = Vec::new();
    let mut created = 0_usize;
    let mut arns: Vec<String> = Vec::new();
    for (case, population, ttl) in rows {
        while created < *population {
            let name = format!("version-channel-{created}");
            let (status, body) = call(
                &router,
                "/createSignalingChannel",
                serde_json::json!({ "ChannelName": name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateSignalingChannel {name:?} expected 200, got {status} with \
                 {body:?}"
            );
            arns.push(text(case, &body, "ChannelARN"));
            created += 1;
        }
        wait_for_activation();

        let index = population - 1;
        let name = format!("version-channel-{index}");
        let described = described_channel_version(&router, &name).await;
        let listed = listed_channel_version(&router, &name).await;
        assert_modeled_token(case, &described);
        assert_eq!(
            described, listed,
            "case {case:?}: DescribeSignalingChannel reported {described:?} and \
             ListSignalingChannels {listed:?} for one record"
        );
        collected.push((format!("{case} at creation"), described.clone()));

        let (status, body) = call(
            &router,
            "/updateSignalingChannel",
            serde_json::json!({
                "ChannelARN": arns[index].clone(),
                "CurrentVersion": described.clone(),
                "SingleMasterConfiguration": { "MessageTtlSeconds": ttl }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reported token must work as CurrentVersion, got {status} with \
             {body:?}"
        );
        wait_for_activation();
        let reminted = described_channel_version(&router, &name).await;
        let relisted = listed_channel_version(&router, &name).await;
        assert_modeled_token(case, &reminted);
        assert_ne!(
            reminted, described,
            "case {case:?}: a mutation must re-mint the token, still {reminted:?}"
        );
        assert_eq!(
            reminted, relisted,
            "case {case:?}: after the update DescribeSignalingChannel reported {reminted:?} and \
             ListSignalingChannels {relisted:?}"
        );
        collected.push((format!("{case} after its update"), reminted));
    }

    // The stream's token joins the set, so a token shared between the two indexes — one
    // drawn from a counter both of them bump, say — shows up as a near-match here.
    collected.push((
        "the stream created beside the channels".to_string(),
        described_stream_version(&router, "version-neighbour-stream").await,
    ));
    assert_unrelated(&collected);
}

/// Assert the tokens of `collected` are pairwise unrelated: distinct, of one width, and
/// differing in most positions.
///
/// Half the width is a floor no composed token clears — a shared mint-time clock or a
/// shared counter prefix leaves two tokens agreeing almost everywhere — while two drawn
/// tokens of this width agree in about one position out of sixty.
fn assert_unrelated(collected: &[(String, String)]) {
    let width = collected
        .first()
        .map(|(_, token)| token.chars().count())
        .unwrap_or_default();
    for (left_index, (left_case, left)) in collected.iter().enumerate() {
        assert_eq!(
            left.chars().count(),
            width,
            "case {left_case:?}: every token carries one fixed width, expected {width}, got \
             {left:?}"
        );
        for (right_case, right) in collected.iter().skip(left_index + 1) {
            assert_ne!(
                left, right,
                "case {left_case:?} vs {right_case:?}: tokens must be distinct, both {left:?}"
            );
            let shared = left
                .chars()
                .zip(right.chars())
                .filter(|(one, other)| one == other)
                .count();
            assert!(
                shared <= width / 2,
                "case {left_case:?} vs {right_case:?}: two tokens must differ across most of \
                 their width, but {left:?} and {right:?} agree in {shared} of {width} positions"
            );
        }
    }
}
