// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the MediaPackage channel / origin-endpoint lifecycle.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//!
//! The rules under test, one per suite below:
//!
//! 1. A create persists the resource and returns the stored record; every later read of
//!    it (`Describe*`, `List*`) renders the SAME record, and a create on an id already in
//!    use is refused with `UnprocessableEntityException` (422) rather than overwriting.
//! 2. A read of an id the service does not hold is `NotFoundException` (404), and a
//!    delete makes the resource unreadable that way.
//! 3. A `List*` returns `MaxResults` items in key order and a `NextToken` only while more
//!    remain; resuming with that token continues without a repeat or a gap.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region every request below is signed for. Deliberately NOT the emulator's
/// `us-east-1` default, so an ARN or hostname that ignored the request's credential
/// scope would show up.
///
/// Shared with the sibling suites in this test target, which drive the same wire.
pub(crate) const REGION: &str = "eu-west-1";

/// The emulator's canonical placeholder account id (`emulator_account_id`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// An account id that is NOT this emulator's own ([`ACCOUNT`]), for a value that has to name
/// a resource of another account.
pub(crate) const OTHER_ACCOUNT: &str = "210987654321";

/// A harvest destination role in ANOTHER account ([`OTHER_ACCOUNT`]).
///
/// `CreateHarvestJob` pre-flights the destination it would write the harvested manifest to,
/// and a role of THIS account that the account's IAM records hold no role for is one
/// MediaPackage could not act as, so such a submission is refused
/// (`state::harvest_role_refusal`). This emulator holds no record of ANOTHER account's
/// roles, so it establishes nothing about one and refuses nothing for it — which is what
/// lets the suites whose subject is not the destination (paging, filters, Region isolation,
/// the id rule, the refusal wording) submit a job without provisioning IAM. The destination
/// rule itself, with roles a real `CreateRole` created and buckets a real `CreateBucket`
/// created, is `tests/all/harvest_destination.rs`.
pub(crate) const CROSS_ACCOUNT_HARVEST_ROLE: &str = "arn:aws:iam::210987654321:role/HarvestRole";

/// A fresh emulator holding only this service, with request authentication off (these
/// tests exercise behaviour, not signing).
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// [`emulator`], plus the shared engine the service was built from.
///
/// A suite that has to read or write what the emulator itself stored — the instant a
/// transition the clock owes falls due, say — needs that handle; this is the one router
/// builder, so the two forms cannot drift apart.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A SigV4 `Authorization` whose credential scope routes the request to `mediapackage`
/// in `region`; the signature is never verified here.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/mediapackage/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One request/response round trip, answering the WHOLE response — status, headers and
/// body — as the router served it.
///
/// [`call`] is this call with the body parsed; a suite whose subject is a response HEADER
/// (how a bodyless answer is framed, say) needs the headers too, so both forms go through
/// this one dispatch and cannot drift apart.
pub(crate) async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Response<Bytes> {
    dispatch_in(router, REGION, method, uri, body).await
}

/// [`dispatch`] for a request served in `region` rather than in [`REGION`]: the host and
/// the SigV4 credential scope both name it.
///
/// Every suite but one signs for a single Region, so [`dispatch`] is the form they use;
/// this is the one whose SUBJECT is the Region a request is served in
/// (`tests/all/region_isolation.rs`), and both go through the one request builder.
pub(crate) async fn dispatch_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Response<Bytes> {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://mediapackage.{region}.amazonaws.com{uri}"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    router.dispatch(request).await
}

/// One request/response round trip: the HTTP status and the parsed JSON body (`Null` for
/// an empty body).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, uri, body).await
}

/// [`call`] for a request served in `region`; see [`dispatch_in`].
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = dispatch_in(router, region, method, uri, body).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code restJson1 carries in `__type`, stripped of any namespace prefix.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `true` when `text` is an ISO-8601 instant of the form `YYYY-MM-DDThh:mm:ssZ`
/// (optionally with a fractional part), which is what `CreatedAt` reports.
fn looks_iso8601(text: &str) -> bool {
    let (date, rest) = match text.split_once('T') {
        Some(split) => split,
        None => return false,
    };
    let date_ok = date.len() == 10
        && date.as_bytes()[4] == b'-'
        && date.as_bytes()[7] == b'-'
        && date
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    let time = rest.strip_suffix('Z').unwrap_or("");
    let time_ok = time.len() >= 8
        && time.as_bytes()[2] == b':'
        && time.as_bytes()[5] == b':'
        && time
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b':' || b == b'.');
    date_ok && time_ok
}

/// A deterministic, reproducible generator for the property tests in this target: a
/// SHA-256 counter stream (the `sha2` crate, already this crate's dependency) over a named
/// seed. A failing case reproduces from the seed printed in its assertion message, and
/// nothing is written outside the in-memory engine.
///
/// Shared with the sibling suites, so no suite writes a second generator.
pub(crate) struct SeededGenerator {
    seed: u64,
    counter: u64,
}

impl SeededGenerator {
    pub(crate) fn new(seed: u64) -> Self {
        Self { seed, counter: 0 }
    }

    /// The next 64 bits of the stream.
    pub(crate) fn next_u64(&mut self) -> u64 {
        use sha2::Digest;
        self.counter += 1;
        let digest = sha2::Sha256::digest(format!("{}|{}", self.seed, self.counter).as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        u64::from_be_bytes(bytes)
    }

    /// The next value of the stream in `1..=max`.
    pub(crate) fn next_in(&mut self, max: u64) -> usize {
        usize::try_from(self.next_u64() % max).unwrap_or(0) + 1
    }
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/// A channel case: the `CreateChannel` body and the id it creates. The rows differ along
/// what the rule generalises over — the id's form and length, whether a description is
/// sent, and how many tags come with it (none, one, several).
fn channel_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("demo-channel", json!({ "id": "demo-channel" })),
        (
            "with-description",
            json!({ "id": "with-description", "description": "bootstrap demo" }),
        ),
        (
            "one_tag_1",
            json!({ "id": "one_tag_1", "tags": { "team": "video" } }),
        ),
        (
            "a-much-longer-channel-id-0123456789",
            json!({
                "id": "a-much-longer-channel-id-0123456789",
                "description": "",
                "tags": { "a": "1", "b": "2", "c": "3" }
            }),
        ),
        ("Z", json!({ "id": "Z", "description": "single character id" })),
    ]
}

#[tokio::test]
async fn a_created_channel_is_read_back_from_state_unchanged() {
    let router = emulator();

    for (id, body) in channel_cases() {
        let case = format!("CreateChannel {id}");
        let (status, created) = call(&router, "POST", "/channels", Some(&body)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");

        // The record the create returned is computed from what was stored: the id as
        // sent, the ARN for this region and account, an ISO-8601 CreatedAt, and the
        // redundant pair of system-generated ingest endpoints.
        assert_eq!(
            created.get("id").and_then(Value::as_str),
            Some(id),
            "case {case:?}: expected id {id:?}, got {created:?}"
        );
        let expected_arn = format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:channels/{id}");
        assert_eq!(
            created.get("arn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected arn {expected_arn:?}, got {created:?}"
        );
        let created_at = created
            .get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            looks_iso8601(created_at),
            "case {case:?}: expected an ISO-8601 createdAt, got {created_at:?}"
        );
        assert_eq!(
            created.get("description").and_then(Value::as_str),
            body.get("description").and_then(Value::as_str),
            "case {case:?}: description must be the one sent, got {created:?}"
        );
        assert_eq!(
            created.get("tags"),
            Some(body.get("tags").cloned().unwrap_or(json!({}))).as_ref(),
            "case {case:?}: tags must be the ones sent, got {created:?}"
        );

        let endpoints = created
            .pointer("/hlsIngest/ingestEndpoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            endpoints.len(),
            2,
            "case {case:?}: expected 2 ingest endpoints, got {created:?}"
        );
        for (index, endpoint) in endpoints.iter().enumerate() {
            for member in ["id", "url", "username", "password"] {
                let value = endpoint.get(member).and_then(Value::as_str).unwrap_or("");
                assert!(
                    !value.is_empty(),
                    "case {case:?}: ingest endpoint {index} {member} must be generated, \
                     got {endpoint:?}"
                );
            }
            let (endpoint_id, url) = (
                endpoint.get("id").and_then(Value::as_str).unwrap_or(""),
                endpoint.get("url").and_then(Value::as_str).unwrap_or(""),
            );
            assert!(
                url.starts_with("https://") && url.contains(endpoint_id) && url.contains(REGION),
                "case {case:?}: ingest endpoint {index} url must address this endpoint in \
                 {REGION}, got {url:?}"
            );
        }
        let distinct_ids: std::collections::BTreeSet<&str> = endpoints
            .iter()
            .filter_map(|e| e.get("id").and_then(Value::as_str))
            .collect();
        assert_eq!(
            distinct_ids.len(),
            2,
            "case {case:?}: the two ingest endpoints must have distinct ids, got {endpoints:?}"
        );

        // DescribeChannel renders the same stored record, including the credentials and
        // the CreatedAt minted at creation.
        let (status, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel expected 200, got {status}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: DescribeChannel must return exactly what was stored"
        );

        // And a second read is identical too: reporting operations do not change state.
        let (_, described_again) = call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            described_again, created,
            "case {case:?}: a repeated DescribeChannel must not change the record"
        );
    }

    // Every created channel is observable in a listing, rendered from the same records.
    let (status, listed) = call(&router, "GET", "/channels", None).await;
    assert_eq!(status, 200, "ListChannels expected 200, got {status}");
    let channels = listed
        .get("channels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let listed_ids: Vec<&str> = channels
        .iter()
        .filter_map(|c| c.get("id").and_then(Value::as_str))
        .collect();
    let mut expected_ids: Vec<&str> = channel_cases().iter().map(|(id, _)| *id).collect();
    expected_ids.sort_unstable();
    assert_eq!(
        listed_ids, expected_ids,
        "ListChannels must list every stored channel in key order, got {listed:?}"
    );
    assert_eq!(
        listed.get("nextToken"),
        None,
        "ListChannels must omit nextToken when no more remain, got {listed:?}"
    );
    for channel in &channels {
        let id = channel.get("id").and_then(Value::as_str).unwrap_or_default();
        let (_, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            channel.get("arn"),
            described.get("arn"),
            "case {id:?}: the listed channel must be the described one"
        );
        assert_eq!(
            channel.get("hlsIngest"),
            described.get("hlsIngest"),
            "case {id:?}: the listed ingest endpoints must be the stored ones"
        );
    }
}

#[tokio::test]
async fn a_duplicate_channel_id_is_refused_and_leaves_the_stored_channel_alone() {
    let router = emulator();

    for (id, body) in channel_cases() {
        let case = format!("duplicate {id}");
        let (status, first) = call(&router, "POST", "/channels", Some(&body)).await;
        assert_eq!(status, 200, "case {case:?}: first create expected 200");

        // A different Description on the same id must not become an update.
        let mut retry = body.clone();
        retry["description"] = json!("a different description");
        let (status, error) = call(&router, "POST", "/channels", Some(&retry)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422 for a duplicate id, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );

        let (_, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            described, first,
            "case {case:?}: the refused duplicate must not have changed the stored channel"
        );
    }
}

#[tokio::test]
async fn reading_or_deleting_a_channel_the_service_does_not_hold_is_not_found() {
    let router = emulator();
    let (_, _) = call(
        &router,
        "POST",
        "/channels",
        Some(&json!({ "id": "present" })),
    )
    .await;

    // Rows differ along the shape of the absent id: never created, a near-miss of a
    // stored id, a different case, and one carrying characters an id may hold.
    for id in ["absent", "presen", "PRESENT", "present-2", "a_b-1"] {
        for method in ["GET", "DELETE"] {
            let case = format!("{method} /channels/{id}");
            let (status, error) = call(&router, method, &format!("/channels/{id}"), None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {error:?}"
            );
            assert_eq!(
                error_code(&error),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {error:?}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_channel_removes_it_from_state() {
    let router = emulator();
    for (id, body) in channel_cases() {
        let (status, _) = call(&router, "POST", "/channels", Some(&body)).await;
        assert_eq!(status, 200, "case {id:?}: create expected 200");
    }

    for (id, _) in channel_cases() {
        let case = format!("DELETE /channels/{id}");
        let (status, body) = call(&router, "DELETE", &format!("/channels/{id}"), None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {body:?}"
        );

        let (status, error) = call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: after the delete DescribeChannel must answer 404, got {status}"
        );
        assert_eq!(
            error_code(&error),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {error:?}"
        );

        // Deleting it again is itself a 404: the record is gone from state.
        let (status, _) = call(&router, "DELETE", &format!("/channels/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a repeated delete must answer 404, got {status}"
        );
    }

    let (_, listed) = call(&router, "GET", "/channels", None).await;
    assert_eq!(
        listed.get("channels").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "every channel was deleted, so the listing must be empty, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Origin endpoints
// ---------------------------------------------------------------------------

/// The four packaging configurations `CreateOriginEndpoint` accepts, and the manifest
/// each one's playback URL ends in.
///
/// Source: the `CreateOriginEndpointRequest` members `HlsPackage`, `DashPackage`,
/// `CmafPackage` and `MssPackage` —
/// `smithy_query(query="operation",operation="CreateOriginEndpoint",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`.
/// Each row carries a DIFFERENT set of optional members, so what is asserted is that the
/// whole supplied configuration survives a round trip, not one member of it.
fn packaging_cases() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "hls",
            json!({
                "hlsPackage": {
                    "segmentDurationSeconds": 6,
                    "playlistType": "EVENT",
                    "playlistWindowSeconds": 60,
                    "adMarkers": "PASSTHROUGH",
                    "useAudioRenditionGroup": true,
                    "streamSelection": {
                        "minVideoBitsPerSecond": 100,
                        "maxVideoBitsPerSecond": 10000,
                        "streamOrder": "VIDEO_BITRATE_DESCENDING"
                    }
                }
            }),
            ".m3u8",
        ),
        (
            "dash",
            json!({
                "dashPackage": {
                    "segmentDurationSeconds": 2,
                    "manifestWindowSeconds": 30,
                    "minBufferTimeSeconds": 4,
                    "profile": "HBBTV_1_5",
                    "segmentTemplateFormat": "NUMBER_WITH_TIMELINE",
                    "periodTriggers": ["ADS"],
                    "utcTiming": "HTTP-ISO"
                }
            }),
            ".mpd",
        ),
        (
            "cmaf",
            json!({
                "cmafPackage": {
                    "segmentDurationSeconds": 4,
                    "hlsManifests": [
                        { "id": "primary", "manifestName": "master", "adMarkers": "NONE" },
                        { "id": "secondary" }
                    ]
                }
            }),
            "/index.m3u8",
        ),
        (
            "mss",
            json!({
                "mssPackage": {
                    "segmentDurationSeconds": 2,
                    "manifestWindowSeconds": 60,
                    "streamSelection": { "streamOrder": "ORIGINAL" }
                }
            }),
            ".ism/Manifest",
        ),
    ]
}

/// Create `channel_id`, failing the test if that does not succeed.
pub(crate) async fn create_channel(router: &MultiServiceRouter, channel_id: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/channels",
        Some(&json!({ "id": channel_id })),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the fixture channel {channel_id:?} expected 200, got {status} {body:?}"
    );
}

#[tokio::test]
async fn a_created_origin_endpoint_stores_every_supplied_setting_and_a_derived_url() {
    let router = emulator();
    create_channel(&router, "demo-channel").await;

    for (name, package, manifest_suffix) in packaging_cases() {
        for (variant, extras) in [
            ("bare", json!({})),
            (
                "full",
                json!({
                    "description": "packaged endpoint",
                    "manifestName": "stream",
                    "startoverWindowSeconds": 300,
                    "timeDelaySeconds": 10,
                    "origination": "DENY",
                    "whitelist": ["10.0.0.0/8", "192.168.0.0/16"],
                    "tags": { "tier": "gold" },
                    "authorization": {
                        "cdnIdentifierSecret": "arn:aws:secretsmanager:eu-west-1:000000000000:secret:cdn",
                        "secretsRoleArn": "arn:aws:iam::000000000000:role/mediapackage"
                    }
                }),
            ),
        ] {
            let id = format!("{name}-{variant}");
            let case = format!("CreateOriginEndpoint {id}");
            let mut body = json!({ "channelId": "demo-channel", "id": id });
            for (key, value) in package.as_object().expect("a package object") {
                body[key] = value.clone();
            }
            for (key, value) in extras.as_object().expect("an extras object") {
                body[key] = value.clone();
            }

            let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {created:?}"
            );

            let expected_arn =
                format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:origin_endpoints/{id}");
            assert_eq!(
                created.get("arn").and_then(Value::as_str),
                Some(expected_arn.as_str()),
                "case {case:?}: expected arn {expected_arn:?}, got {created:?}"
            );
            assert_eq!(
                created.get("channelId").and_then(Value::as_str),
                Some("demo-channel"),
                "case {case:?}: channelId must be the one created against, got {created:?}"
            );
            let created_at = created
                .get("createdAt")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                looks_iso8601(created_at),
                "case {case:?}: expected an ISO-8601 createdAt, got {created_at:?}"
            );

            // The playback URL is derived from the endpoint and the manifest its
            // packaging type serves, and it names the manifest the caller asked for
            // (`index` when none was sent).
            let expected_manifest = extras
                .get("manifestName")
                .and_then(Value::as_str)
                .unwrap_or("index");
            let url = created.get("url").and_then(Value::as_str).unwrap_or("");
            assert!(
                url.starts_with("https://")
                    && url.contains(REGION)
                    && url.contains(expected_manifest)
                    && url.ends_with(manifest_suffix),
                "case {case:?}: expected a {name} playback url naming {expected_manifest:?} and \
                 ending in {manifest_suffix:?}, got {url:?}"
            );
            assert_eq!(
                created.get("manifestName").and_then(Value::as_str),
                Some(expected_manifest),
                "case {case:?}: manifestName must be the effective one, got {created:?}"
            );

            // Every packaging setting supplied on input is stored, not dropped. The CMAF
            // parameter shape is the one that differs: the service adds the derived Url
            // to each child HLS manifest and defaults SegmentPrefix to the ChannelId.
            for (key, sent) in package.as_object().expect("a package object") {
                let stored = created.get(key).unwrap_or(&Value::Null);
                if key == "cmafPackage" {
                    assert_eq!(
                        stored.get("segmentPrefix").and_then(Value::as_str),
                        Some("demo-channel"),
                        "case {case:?}: an absent segmentPrefix defaults to the ChannelId, \
                         got {stored:?}"
                    );
                    let manifests = stored
                        .get("hlsManifests")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let sent_manifests = sent
                        .get("hlsManifests")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    assert_eq!(
                        manifests.len(),
                        sent_manifests.len(),
                        "case {case:?}: every supplied CMAF manifest must be stored, \
                         got {stored:?}"
                    );
                    for (child, sent_child) in manifests.iter().zip(&sent_manifests) {
                        assert_eq!(
                            child.get("id"),
                            sent_child.get("id"),
                            "case {case:?}: a stored CMAF manifest must keep its id, \
                             got {child:?}"
                        );
                        // A child manifest sent without a name of its own inherits the
                        // ENDPOINT's effective manifestName, so the name it reports and the
                        // name its url carries are both that one.
                        let child_url = child.get("url").and_then(Value::as_str).unwrap_or("");
                        let child_name = sent_child
                            .get("manifestName")
                            .and_then(Value::as_str)
                            .unwrap_or(expected_manifest);
                        assert_eq!(
                            child.get("manifestName").and_then(Value::as_str),
                            Some(child_name),
                            "case {case:?}: CMAF manifest {child:?} must report manifestName \
                             {child_name:?}"
                        );
                        assert!(
                            child_url.contains(&format!("/{child_name}/"))
                                && child_url.ends_with("/index.m3u8"),
                            "case {case:?}: CMAF manifest {child:?} must carry a url naming \
                             {child_name:?}, got {child_url:?}"
                        );
                    }
                } else {
                    // Member-wise, not whole-block: the service ALSO server-fills the
                    // packaging members this request left unset (see
                    // `state::apply_origin_endpoint_defaults`, covered by
                    // `origin_endpoint_defaults.rs`), so the stored block is a SUPERSET of
                    // the sent one. What this asserts is that nothing sent was dropped or
                    // rewritten.
                    // A nested block (streamSelection) is compared the same way, member by
                    // member, because the service completes it too.
                    for (member, value) in sent.as_object().expect("a packaging object") {
                        let stored_member = stored.get(member).unwrap_or(&Value::Null);
                        match value.as_object() {
                            Some(nested) => {
                                for (inner, inner_value) in nested {
                                    assert_eq!(
                                        stored_member.get(inner).unwrap_or(&Value::Null),
                                        inner_value,
                                        "case {case:?}: {key}.{member}.{inner} must be stored \
                                         as sent, got {stored_member:?}"
                                    );
                                }
                            }
                            None => assert_eq!(
                                stored_member,
                                value,
                                "case {case:?}: {key}.{member} must be stored as sent, \
                                 got {stored:?}"
                            ),
                        }
                    }
                }
            }
            for (key, sent) in extras.as_object().expect("an extras object") {
                if key == "manifestName" {
                    continue;
                }
                assert_eq!(
                    created.get(key).unwrap_or(&Value::Null),
                    sent,
                    "case {case:?}: {key} must be stored as sent, got {created:?}"
                );
            }

            // Read back through DescribeOriginEndpoint: the same record, byte for byte.
            let (status, described) =
                call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described, created,
                "case {case:?}: DescribeOriginEndpoint must return exactly what was stored"
            );
        }
    }
}

#[tokio::test]
async fn an_origin_endpoint_the_service_cannot_package_is_refused() {
    let router = emulator();
    create_channel(&router, "demo-channel").await;
    let (status, _) = call(
        &router,
        "POST",
        "/origin_endpoints",
        Some(&json!({
            "channelId": "demo-channel",
            "id": "taken",
            "hlsPackage": {}
        })),
    )
    .await;
    assert_eq!(status, 200, "the fixture endpoint expected 200");

    // A ChannelId naming no stored channel: nothing to package. It is a value in the
    // REQUEST that cannot be resolved, so it is refused as 422 keyed by the member (see
    // `tests/all/error_envelope.rs`, which pins that rule against the 404 a request
    // ADDRESSING a missing resource gets). The rows differ along the shape of the absent
    // channel id.
    for channel_id in ["no-such-channel", "demo-channe", "DEMO-CHANNEL"] {
        let case = format!("absent channel {channel_id}");
        let (status, error) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&json!({ "channelId": channel_id, "id": "x", "hlsPackage": {} })),
        )
        .await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
        let (status, _) = call(&router, "GET", "/origin_endpoints/x", None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused create must not have stored anything"
        );
    }

    // An id already in use is refused, whatever packaging the retry carries.
    for (name, package, _) in packaging_cases() {
        let case = format!("duplicate id with {name}");
        let mut body = json!({ "channelId": "demo-channel", "id": "taken" });
        for (key, value) in package.as_object().expect("a package object") {
            body[key] = value.clone();
        }
        let (status, error) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
    }

    // Exactly one packaging configuration: neither none nor two can be packaged, and the
    // endpoint's playback URL has no manifest to end in.
    let none = json!({ "channelId": "demo-channel", "id": "no-package" });
    let two = json!({
        "channelId": "demo-channel",
        "id": "two-packages",
        "hlsPackage": {},
        "dashPackage": {}
    });
    let four = json!({
        "channelId": "demo-channel",
        "id": "four-packages",
        "hlsPackage": {},
        "dashPackage": {},
        "cmafPackage": { "hlsManifests": [] },
        "mssPackage": {}
    });
    for body in [none, two, four] {
        let case = body
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (status, error) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
        let (status, _) = call(&router, "GET", &format!("/origin_endpoints/{case}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused create must not have stored anything"
        );
    }
}

#[tokio::test]
async fn a_caller_sized_collection_or_string_past_its_bound_is_refused() {
    let router = emulator();
    create_channel(&router, "demo-channel").await;

    // The bounds the service stores within, by what they bound: tag count, tag key and
    // value length, free-text length, `Whitelist` entry count, and the CMAF child
    // manifest count. One row per bound, each one entry/character past it.
    let long = "x".repeat(2_000);
    let many_tags: serde_json::Map<String, Value> = (0..51)
        .map(|index| (format!("key-{index}"), json!("v")))
        .collect();
    let long_key_tags: serde_json::Map<String, Value> =
        [(long.clone(), json!("v"))].into_iter().collect();
    let long_value_tags: serde_json::Map<String, Value> =
        [("k".to_string(), json!(long.clone()))].into_iter().collect();
    let channel_rows: Vec<(&str, Value)> = vec![
        (
            "too many tags",
            json!({ "id": "bounded", "tags": many_tags }),
        ),
        (
            "tag key too long",
            json!({ "id": "bounded", "tags": long_key_tags }),
        ),
        (
            "tag value too long",
            json!({ "id": "bounded", "tags": long_value_tags }),
        ),
        (
            "description too long",
            json!({ "id": "bounded", "description": long.clone() }),
        ),
    ];
    for (case, body) in channel_rows {
        let (status, error) = call(&router, "POST", "/channels", Some(&body)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
        let (status, _) = call(&router, "GET", "/channels/bounded", None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused create must not have stored anything"
        );
    }

    let endpoint_rows: Vec<(&str, Value)> = vec![
        (
            "whitelist too long",
            json!({
                "channelId": "demo-channel",
                "id": "bounded-endpoint",
                "hlsPackage": {},
                "whitelist": (0..101).map(|i| json!(format!("10.0.0.{i}/32"))).collect::<Vec<_>>()
            }),
        ),
        (
            "manifest name too long",
            json!({
                "channelId": "demo-channel",
                "id": "bounded-endpoint",
                "hlsPackage": {},
                "manifestName": long.clone()
            }),
        ),
        (
            "too many cmaf manifests",
            json!({
                "channelId": "demo-channel",
                "id": "bounded-endpoint",
                "cmafPackage": {
                    "hlsManifests": (0..51)
                        .map(|i| json!({ "id": format!("m-{i}") }))
                        .collect::<Vec<_>>()
                }
            }),
        ),
    ];
    for (case, body) in endpoint_rows {
        let (status, error) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
        let (status, _) = call(&router, "GET", "/origin_endpoints/bounded-endpoint", None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the refused create must not have stored anything"
        );
    }

    // And the values just INSIDE each bound are accepted and stored, so the bound refuses
    // only what is past it.
    let at_bound = json!({
        "channelId": "demo-channel",
        "id": "at-bound",
        "hlsPackage": {},
        "manifestName": "m".repeat(1_024),
        "whitelist": (0..100).map(|i| json!(format!("10.0.0.{i}/32"))).collect::<Vec<_>>(),
        "tags": (0..50)
            .map(|index| (format!("key-{index}"), json!("v")))
            .collect::<serde_json::Map<String, Value>>()
    });
    let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&at_bound)).await;
    assert_eq!(
        status, 200,
        "case \"at bound\": values at the bound must be accepted, got {status} {created:?}"
    );
    assert_eq!(
        created
            .get("whitelist")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(100),
        "case \"at bound\": the whole whitelist must be stored, got {created:?}"
    );
}

#[tokio::test]
async fn a_channel_with_origin_endpoints_cannot_be_deleted() {
    let router = emulator();
    create_channel(&router, "demo-channel").await;

    for (name, package, _) in packaging_cases() {
        let mut body = json!({ "channelId": "demo-channel", "id": name });
        for (key, value) in package.as_object().expect("a package object") {
            body[key] = value.clone();
        }
        let (status, _) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
        assert_eq!(status, 200, "case {name:?}: create expected 200");
    }

    // One row per remaining endpoint count: the refusal holds while ANY endpoint remains,
    // and only the last removal lets the delete through.
    let mut remaining: Vec<&str> = packaging_cases().iter().map(|(name, _, _)| *name).collect();
    while let Some(next) = remaining.pop() {
        let case = format!("{} endpoint(s) remaining", remaining.len() + 1);
        let (status, error) = call(&router, "DELETE", "/channels/demo-channel", None).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {error:?}"
        );
        assert_eq!(
            error_code(&error),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {error:?}"
        );
        let (status, _) = call(&router, "GET", "/channels/demo-channel", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused delete must leave the channel readable"
        );

        let (status, _) = call(&router, "DELETE", &format!("/origin_endpoints/{next}"), None).await;
        assert_eq!(status, 202, "case {case:?}: deleting {next:?} expected 202");
    }

    // With the last endpoint gone the delete goes through.
    let (status, body) = call(&router, "DELETE", "/channels/demo-channel", None).await;
    assert_eq!(
        status, 202,
        "case \"no endpoints remaining\": expected 202, got {status} {body:?}"
    );
    let (status, _) = call(&router, "GET", "/channels/demo-channel", None).await;
    assert_eq!(
        status, 404,
        "case \"no endpoints remaining\": the deleted channel must answer 404, got {status}"
    );
}

#[tokio::test]
async fn deleting_an_origin_endpoint_removes_it_from_state() {
    let router = emulator();
    create_channel(&router, "demo-channel").await;

    for (name, package, _) in packaging_cases() {
        let mut body = json!({ "channelId": "demo-channel", "id": name });
        for (key, value) in package.as_object().expect("a package object") {
            body[key] = value.clone();
        }
        let (status, _) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
        assert_eq!(status, 200, "case {name:?}: create expected 200");
    }

    for (name, _, _) in packaging_cases() {
        let case = format!("DELETE /origin_endpoints/{name}");
        let (status, body) = call(&router, "DELETE", &format!("/origin_endpoints/{name}"), None)
            .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {body:?}"
        );
        let (status, error) = call(&router, "GET", &format!("/origin_endpoints/{name}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: after the delete the read must answer 404, got {status}"
        );
        assert_eq!(
            error_code(&error),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {error:?}"
        );
    }

    // The channel itself is untouched by its endpoints' deletion.
    let (status, _) = call(&router, "GET", "/channels/demo-channel", None).await;
    assert_eq!(status, 200, "the channel must still be readable, got {status}");
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// The page-size bound MediaPackage ENFORCES: `maxResults` must be an integer between 1
/// and 50. Narrower than the modelled `MaxResults` `@range` (1..1000 —
/// `smithy_query(query="shape",shape="MaxResults",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
/// which is what a client-side validator checks; this is what the service applies.
const MAX_RESULTS_RANGE: (i64, i64) = (1, 50);

#[tokio::test]
async fn max_results_is_honoured_inside_the_enforced_bound_and_refused_outside_it() {
    let router = emulator();
    for index in 0..5 {
        create_channel(&router, &format!("channel-{index}")).await;
    }

    let (low, high) = MAX_RESULTS_RANGE;
    for max_results in [low, 2, 4, high] {
        let case = format!("maxResults={max_results}");
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/channels?maxResults={max_results}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a maxResults inside {low}..{high} must be accepted, got \
             {status} {listed:?}"
        );
        let returned = listed
            .get("channels")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        let expected = usize::try_from(max_results).unwrap_or(5).min(5);
        assert_eq!(
            returned, expected,
            "case {case:?}: expected {expected} channels, got {returned}"
        );
        let more_remain = expected < 5;
        assert_eq!(
            listed.get("nextToken").is_some(),
            more_remain,
            "case {case:?}: nextToken must be present only while more remain, got {listed:?}"
        );
    }

    // One past each enforced bound is refused, not clamped — and so is a value the
    // MODELLED range would admit but the service does not, and one it would not.
    for max_results in [low - 1, high + 1, 1_000, 10_000] {
        let case = format!("maxResults={max_results}");
        let (status, body) = call(
            &router,
            "GET",
            &format!("/channels?maxResults={max_results}"),
            None,
        )
        .await;
        assert_eq!(
            status, 422,
            "case {case:?}: a maxResults outside {low}..{high} must be refused as 422, \
             got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {body:?}"
        );
        assert_eq!(
            body.get("channels"),
            None,
            "case {case:?}: a refused request must return no items, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_pagination_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    create_channel(&router, "demo-channel").await;

    // Rows differ along how the token is wrong: not hex at all, odd-length hex, and a
    // token-looking string from nowhere.
    for token in ["not-a-token", "abc", "zz", "%%%"] {
        for path in ["/channels", "/origin_endpoints"] {
            let case = format!("{path}?nextToken={token}");
            let (status, error) =
                call(&router, "GET", &format!("{path}?nextToken={token}"), None).await;
            assert_eq!(
                status, 422,
                "case {case:?}: expected 422, got {status} {error:?}"
            );
            assert_eq!(
                error_code(&error),
                "UnprocessableEntityException",
                "case {case:?}: expected UnprocessableEntityException, got {error:?}"
            );
        }
    }
}

/// Walk every page of `path`, `page_size` at a time, and return the ids seen in order.
async fn walk_pages(
    router: &MultiServiceRouter,
    path: &str,
    items_member: &str,
    page_size: usize,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    // Bounded: one page per item plus one, so a token that failed to advance cannot spin.
    for _ in 0..=(seen.capacity() + 4096) {
        let separator = if path.contains('?') { '&' } else { '?' };
        let mut uri = format!("{path}{separator}maxResults={page_size}");
        if let Some(token) = &token {
            uri.push_str(&format!("&nextToken={token}"));
        }
        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(status, 200, "paging {uri:?} expected 200, got {body:?}");
        let items = body
            .get(items_member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            items.len() <= page_size,
            "paging {uri:?}: a page must hold at most {page_size} items, got {}",
            items.len()
        );
        seen.extend(
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(str::to_string),
        );
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => {
                assert_eq!(
                    items.len(),
                    page_size,
                    "paging {uri:?}: a short page must not carry a nextToken, got {body:?}"
                );
                token = Some(next.to_string());
            }
            None => return seen,
        }
    }
    panic!("paging {path:?} did not terminate");
}

/// PROPERTY: for any number of endpoints spread over any number of channels, and any page
/// size, walking the pages of `ListOriginEndpoints` yields every matching endpoint
/// exactly once, in ascending id order, whatever the page size — and the `channelId`
/// filter selects exactly the endpoints of that channel.
///
/// Seeds are fixed, so a failure is reproducible from the case message alone; nothing is
/// written outside the in-memory engine.
#[tokio::test]
async fn paging_a_listing_yields_every_matching_item_exactly_once() {
    // Bounded case count: 6 generated worlds.
    for seed in [1u64, 7, 23, 101, 1_009, 65_537] {
        let mut rng = SeededGenerator::new(seed);
        let router = emulator();

        let channel_count = rng.next_in(4);
        let channels: Vec<String> = (0..channel_count)
            .map(|index| format!("ch-{:04x}-{index}", rng.next_u64() & 0xffff))
            .collect();
        for channel in &channels {
            create_channel(&router, channel).await;
        }

        let endpoint_count = rng.next_in(12);
        let mut expected: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for index in 0..endpoint_count {
            let channel = channels[rng.next_in(channels.len() as u64) - 1].clone();
            let id = format!("ep-{:08x}-{index}", rng.next_u64() as u32);
            let (status, body) = call(
                &router,
                "POST",
                "/origin_endpoints",
                Some(&json!({
                    "channelId": channel,
                    "id": id,
                    "hlsPackage": { "segmentDurationSeconds": 1 + (index as i64 % 6) }
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "seed {seed}: creating {id:?} on {channel:?} expected 200, got {body:?}"
            );
            expected.insert(id, channel);
        }

        let page_size = rng.next_in(5);

        // Unfiltered: every endpoint, once, in ascending id order.
        let all_ids: Vec<String> = expected.keys().cloned().collect();
        let walked = walk_pages(&router, "/origin_endpoints", "originEndpoints", page_size).await;
        assert_eq!(
            walked, all_ids,
            "seed {seed}, page size {page_size}: paging must yield every endpoint exactly \
             once in key order, got {walked:?}"
        );

        // Filtered by channelId: exactly that channel's endpoints, same invariant.
        for channel in &channels {
            let mut expected_for_channel: Vec<String> = expected
                .iter()
                .filter(|(_, owner)| *owner == channel)
                .map(|(id, _)| id.clone())
                .collect();
            expected_for_channel.sort();
            let walked = walk_pages(
                &router,
                &format!("/origin_endpoints?channelId={channel}"),
                "originEndpoints",
                page_size,
            )
            .await;
            assert_eq!(
                walked, expected_for_channel,
                "seed {seed}, page size {page_size}: the channelId filter must select exactly \
                 {channel:?}'s endpoints, got {walked:?}"
            );
        }

        // A channel that holds none lists none, with no token.
        let (status, body) = call(
            &router,
            "GET",
            "/origin_endpoints?channelId=holds-nothing",
            None,
        )
        .await;
        assert_eq!(status, 200, "seed {seed}: expected 200, got {body:?}");
        assert_eq!(
            body.get("originEndpoints")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0),
            "seed {seed}: an unmatched channelId must list nothing, got {body:?}"
        );
        assert_eq!(
            body.get("nextToken"),
            None,
            "seed {seed}: an empty page must carry no nextToken, got {body:?}"
        );

        // The same invariant for channels.
        let walked = walk_pages(&router, "/channels", "channels", page_size).await;
        let mut expected_channels = channels.clone();
        expected_channels.sort();
        assert_eq!(
            walked, expected_channels,
            "seed {seed}, page size {page_size}: paging channels must yield each exactly once \
             in key order, got {walked:?}"
        );
    }
}
