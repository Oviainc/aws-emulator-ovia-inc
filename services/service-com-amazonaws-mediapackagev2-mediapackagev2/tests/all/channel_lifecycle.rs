// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the MediaPackageV2 channel-group -> channel -> origin-endpoint
//! lifecycle, driven over HTTP through the same router the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. A create persists the resource and every value the service derives for it (ARN,
//!    egress domain, ingest URLs, manifest URLs, timestamps, ETag), so the matching
//!    read reports those values unchanged.
//! 2. A create whose name is already taken at that level is refused with
//!    `ConflictException` (409) and leaves the existing resource untouched.
//! 3. An operation addressing a resource whose parent chain does not resolve answers
//!    `ResourceNotFoundException` (404).
//! 4. A list pages in name order over exactly the children of the addressed parent:
//!    every child appears once, no page is longer than `maxResults`, and a token is
//!    issued only while something is left.
//! 5. A delete removes the resource (so the read then answers 404), refuses while
//!    children remain (409), and succeeds again on an already-absent resource.
//! 6. Every store a create grows is bounded: past its ceiling the create is refused and
//!    stores nothing — with `ServiceQuotaExceededException` (402) for a resource-count
//!    quota, and with `ValidationException` (400) / `NUM_MANIFESTS_HIGH` for an
//!    over-sized manifest set, which the model declares as an input failure.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. It travels in the SigV4
/// credential scope, which is also what the service reads its region from.
///
/// `pub(crate)` because a suite that reads this emulator's persisted store back has to
/// address the same Region the client signs for (see
/// `tests/all/sub_resource_content_scope.rs`).
pub(crate) const REGION: &str = "us-west-2";

/// The S3 bucket a harvest job's destination names in this crate's suites, and the second
/// one an idempotency row switches to.
///
/// `CreateHarvestJob` refuses a destination the object store does not hold (see
/// `state::harvest_destination`), so an emulator a suite submits a harvest job to has to
/// HOLD these buckets. `pub(crate)` with [`hold_harvest_destinations`]: the one place the
/// crate's suites name a destination, so the body `rejection_rule_and_negative_paths`
/// builds and the bucket seeded here cannot drift apart.
pub(crate) const HARVEST_BUCKET: &str = "harvest-bucket";
/// See [`HARVEST_BUCKET`].
pub(crate) const ALTERNATE_HARVEST_BUCKET: &str = "another-harvest-bucket";

/// Record the destination buckets of [`HARVEST_BUCKET`] and [`ALTERNATE_HARVEST_BUCKET`] in
/// `region`, in Amazon S3's OWN store in this engine — the store `CreateHarvestJob` resolves
/// a harvest destination against, written through the object store's own `create_bucket`, so
/// what a suite provisions is what a real `CreateBucket` would have left there.
pub(crate) fn hold_harvest_destinations(engine: &engine::InMemoryEngine, region: &str) {
    let buckets = service_com_amazonaws_s3_amazons3::state::service_state(engine)
        .expect("the shared engine holds Amazon S3's store");
    let mut store = buckets.lock();
    for bucket in [HARVEST_BUCKET, ALTERNATE_HARVEST_BUCKET] {
        store.create_bucket(bucket, region);
    }
}

/// A MediaPackageV2 emulator plus a tiny restJson1 client for it.
///
/// `pub(crate)` so the crate's other suites drive the emulator through this one client
/// rather than each growing its own copy of it (see `tests/all/channel_reset.rs`,
/// `tests/all/control_plane_validation.rs`, `tests/all/origin_endpoint_reset.rs`,
/// `tests/all/channel_listing.rs` and `tests/all/channel_update.rs`).
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The one shared engine the registered service holds, kept so a suite can read this
    /// service's PERSISTED store back — what no response projects.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // This suite signs nothing: it exercises handler behaviour, not authentication.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediapackagev2_mediapackagev2::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        // A harvest job exports to a bucket Amazon S3 owns, and a create whose destination
        // the object store does not hold is refused, so the destinations this crate's
        // harvest-job bodies name are held before any suite submits one.
        hold_harvest_destinations(&engine, REGION);
        Self { router, engine }
    }

    /// This service's store in the shared engine — the same handle every handler reaches
    /// through `crate::state::service_state`.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_mediapackagev2_mediapackagev2::ComAmazonawsMediapackagev2Mediapackagev2State,
    > {
        service_com_amazonaws_mediapackagev2_mediapackagev2::state::service_state(&*self.engine)
            .expect("the shared engine holds this service's store")
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null` for an
    /// empty body).
    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call_with_headers(method, path, body, &[]).await
    }

    /// As [`Emulator::call`], with extra request headers — a request-header-bound member
    /// such as `ClientToken` (`x-amzn-client-token`) or the `ETag` precondition
    /// (`x-amzn-update-if-match`) travels through here.
    pub(crate) async fn call_with_headers(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        self.send_in_region(
            method,
            path,
            body,
            &format!("mediapackagev2.{REGION}.amazonaws.com"),
            REGION,
            headers,
        )
        .await
    }

    /// As [`Emulator::call_with_headers`], addressing `host` and signing for `region`.
    ///
    /// Every MediaPackage hostname is regional — the control-plane endpoint names its
    /// Region, and so does every data-plane hostname the service publishes — and a signed
    /// request names the same Region in its SigV4 credential scope, which is where the
    /// service reads it from. A suite that exercises more than one Region sends through
    /// here (see `tests/all/regional_partitioning.rs`).
    pub(crate) async fn send_in_region(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        host: &str,
        region: &str,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        let response = self
            .dispatch_in_region(method, path, body, host, region, headers)
            .await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "{method} {path} answered {status} with a body that is not JSON: {e}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// As [`Emulator::send_in_region`], with the WHOLE response kept — its status, its
    /// headers and its body bytes.
    ///
    /// `pub(crate)` because how an answer is FRAMED is a property of the response rather
    /// than of the body it carries, and the parsing clients above drop the headers that
    /// state it (see `tests/all/empty_response_framing.rs`).
    pub(crate) async fn dispatch_in_region(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        host: &str,
        region: &str,
        headers: &[(&str, &str)],
    ) -> http::Response<bytes::Bytes> {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let mut builder = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/mediapackagev2/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .header("content-type", "application/json");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed test request");
        self.router.dispatch(request).await
    }

    /// [`Emulator::dispatch_in_region`] against this suite's control-plane host and Region,
    /// which is what every control-plane case addresses.
    pub(crate) async fn dispatch(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> http::Response<bytes::Bytes> {
        self.dispatch_in_region(
            method,
            path,
            body,
            &format!("mediapackagev2.{REGION}.amazonaws.com"),
            REGION,
            &[],
        )
        .await
    }

    pub(crate) async fn create_group(&self, name: &str, body: Value) -> (u16, Value) {
        let mut body = body;
        body["ChannelGroupName"] = json!(name);
        self.call("POST", "/channelGroup", Some(body)).await
    }

    pub(crate) async fn get_group(&self, name: &str) -> (u16, Value) {
        self.call("GET", &format!("/channelGroup/{name}"), None).await
    }

    pub(crate) async fn delete_group(&self, name: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/channelGroup/{name}"), None)
            .await
    }

    pub(crate) async fn create_channel(&self, group: &str, name: &str, body: Value) -> (u16, Value) {
        let mut body = body;
        body["ChannelName"] = json!(name);
        self.call(
            "POST",
            &format!("/channelGroup/{group}/channel"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn get_channel(&self, group: &str, name: &str) -> (u16, Value) {
        self.call("GET", &format!("/channelGroup/{group}/channel/{name}/"), None)
            .await
    }

    pub(crate) async fn delete_channel(&self, group: &str, name: &str) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("/channelGroup/{group}/channel/{name}/"),
            None,
        )
        .await
    }

    pub(crate) async fn create_endpoint(
        &self,
        group: &str,
        channel: &str,
        name: &str,
        body: Value,
    ) -> (u16, Value) {
        let mut body = body;
        body["OriginEndpointName"] = json!(name);
        self.call(
            "POST",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn get_endpoint(&self, group: &str, channel: &str, name: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{name}"),
            None,
        )
        .await
    }

    async fn delete_endpoint(&self, group: &str, channel: &str, name: &str) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{name}"),
            None,
        )
        .await
    }

    pub(crate) async fn list_endpoints(
        &self,
        group: &str,
        channel: &str,
        query: &str,
    ) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint{query}"),
            None,
        )
        .await
    }

    /// `UpdateChannelGroup` — `PUT` on the group's own path. `headers` carries the
    /// conditional `x-amzn-update-if-match` when a case sets a precondition.
    pub(crate) async fn update_group(
        &self,
        name: &str,
        body: Value,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        self.call_with_headers("PUT", &format!("/channelGroup/{name}"), Some(body), headers)
            .await
    }

    /// `UpdateChannel` — `PUT` on the channel's own path.
    pub(crate) async fn update_channel(
        &self,
        group: &str,
        name: &str,
        body: Value,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        self.call_with_headers(
            "PUT",
            &format!("/channelGroup/{group}/channel/{name}/"),
            Some(body),
            headers,
        )
        .await
    }

    /// `UpdateOriginEndpoint` — `PUT` on the endpoint's own path.
    pub(crate) async fn update_endpoint(
        &self,
        group: &str,
        channel: &str,
        name: &str,
        body: Value,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        self.call_with_headers(
            "PUT",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{name}"),
            Some(body),
            headers,
        )
        .await
    }

    /// `ListChannels` — `GET /channelGroup/{group}/channel`, with `query` carrying
    /// `maxResults` / `nextToken` when the case needs them. The channel list projection
    /// (`ChannelListConfiguration`) is a different shape from `GetChannelResponse`, so a
    /// suite that checks what the list reports sends through here (see
    /// `tests/all/channel_members_by_input_type.rs`).
    pub(crate) async fn list_channels(&self, group: &str, query: &str) -> (u16, Value) {
        self.call("GET", &format!("/channelGroup/{group}/channel{query}"), None)
            .await
    }

    /// `ListChannelGroups` — `GET /channelGroup`, with `query` carrying `maxResults` /
    /// `nextToken` when the case needs them.
    pub(crate) async fn list_groups(&self, query: &str) -> (u16, Value) {
        self.call("GET", &format!("/channelGroup{query}"), None).await
    }
}

/// The value at a JSON pointer in a response body, or `Value::Null` when the member is
/// absent.
///
/// `pub(crate)` on the shared harness because an ABSENT member is half of what several
/// suites assert, and a missing one has to compare unequal to a reported value in all of
/// them (see `tests/all/channel_members_by_input_type.rs`,
/// `tests/all/server_materialised_members.rs` and
/// `tests/all/default_child_manifest_name.rs`).
pub(crate) fn at(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The `__type` discriminator of an error envelope.
pub(crate) fn error_type(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// The smallest ACCEPTABLE `CreateOriginEndpoint` body for a container type: an origin
/// endpoint needs at least one manifest (`NUM_MANIFESTS_LOW`), in a family the container
/// serves — ISM serves MSS, the others the HLS families.
pub(crate) fn minimal_endpoint(container: &str) -> Value {
    let manifest = json!([{"ManifestName": "index"}]);
    match container {
        "ISM" => json!({"ContainerType": "ISM", "MssManifests": manifest}),
        _ => json!({"ContainerType": container, "HlsManifests": manifest}),
    }
}

/// One row of the create-and-read-back table: a whole hierarchy, differing from the
/// others in name shape, description, tag count, input type, container type and which
/// manifest families (and how many of each) the endpoint carries.
struct LifecycleCase {
    label: &'static str,
    group: &'static str,
    channel: &'static str,
    endpoint: &'static str,
    group_body: fn() -> Value,
    channel_body: fn() -> Value,
    endpoint_body: fn() -> Value,
    /// `(json member, manifest name, expected URL suffix)` for every manifest the
    /// endpoint is created with. The suffix is the container format of the manifest
    /// family, not of the endpoint.
    manifests: &'static [(&'static str, &'static str, &'static str)],
}

/// The rows. `InputType` values come from the model's `InputType` enum (CMAF, HLS,
/// MULTIVIEW) and `ContainerType` from its `ContainerType` enum (CMAF, ISM, TS).
fn lifecycle_cases() -> Vec<LifecycleCase> {
    vec![
        LifecycleCase {
            label: "minimal: no description, no tags, one HLS manifest",
            group: "g1",
            channel: "c1",
            endpoint: "e1",
            group_body: || json!({}),
            channel_body: || json!({}),
            endpoint_body: || minimal_endpoint("TS"),
            manifests: &[("HlsManifests", "index", ".m3u8")],
        },
        LifecycleCase {
            label: "described and tagged, one HLS manifest",
            group: "demo-group",
            channel: "demo-channel",
            endpoint: "demo-ep",
            group_body: || json!({"Description": "bootstrap", "tags": {"Key1": "Value1"}}),
            channel_body: || {
                json!({"Description": "a channel", "InputType": "HLS", "tags": {"k": "v"}})
            },
            endpoint_body: || {
                json!({
                    "ContainerType": "TS",
                    "Description": "an endpoint",
                    "StartoverWindowSeconds": 300,
                    "HlsManifests": [{"ManifestName": "index"}],
                    "Tags": {"team": "video"},
                })
            },
            manifests: &[("HlsManifests", "index", ".m3u8")],
        },
        LifecycleCase {
            label: "long underscored names, CMAF input, every manifest family",
            group: "A_group_with_9_and-dashes",
            channel: "A_channel_0",
            endpoint: "An_endpoint_0",
            group_body: || {
                json!({"Description": "", "tags": {"a": "1", "b": "2", "c": "3"}})
            },
            channel_body: || json!({"InputType": "CMAF"}),
            endpoint_body: || {
                json!({
                    "ContainerType": "CMAF",
                    "Segment": {"SegmentDurationSeconds": 6, "SegmentName": "seg"},
                    "HlsManifests": [{"ManifestName": "hls-a"}, {"ManifestName": "hls-b"}],
                    "LowLatencyHlsManifests": [{"ManifestName": "ll-hls"}],
                    "DashManifests": [{"ManifestName": "dash"}],
                })
            },
            manifests: &[
                ("HlsManifests", "hls-a", ".m3u8"),
                ("HlsManifests", "hls-b", ".m3u8"),
                ("LowLatencyHlsManifests", "ll-hls", ".m3u8"),
                ("DashManifests", "dash", ".mpd"),
            ],
        },
        LifecycleCase {
            label: "ISM container with an MSS manifest",
            group: "g-ism",
            channel: "c-ism",
            endpoint: "e-ism",
            group_body: || json!({}),
            channel_body: || json!({"InputType": "CMAF"}),
            endpoint_body: || {
                json!({
                    "ContainerType": "ISM",
                    "MssManifests": [{"ManifestName": "smooth"}],
                })
            },
            manifests: &[("MssManifests", "smooth", ".ism/Manifest")],
        },
    ]
}

/// RULE 1 — every value a create derives is persisted, so the read reports it unchanged.
#[tokio::test]
async fn a_created_hierarchy_reads_back_exactly_as_it_was_stored() {
    let emulator = Emulator::new();

    for case in lifecycle_cases() {
        let label = case.label;

        let (status, created_group) = emulator
            .create_group(case.group, (case.group_body)())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannelGroup expected 200, got {status} {created_group}"
        );
        let group_arn = created_group["Arn"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            group_arn,
            format!(
                "arn:aws:mediapackagev2:{REGION}:{}:channelGroup/{}",
                engine::EMULATOR_ACCOUNT_ID, case.group
            ),
            "case {label:?}: the group ARN must name the service, region, account and group"
        );
        let egress_domain = created_group["EgressDomain"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            egress_domain.contains(".egress.") && egress_domain.ends_with(".amazonaws.com"),
            "case {label:?}: expected an egress domain, got {egress_domain:?}"
        );
        for member in ["CreatedAt", "ModifiedAt", "ETag"] {
            assert!(
                !created_group[member].is_null(),
                "case {label:?}: CreateChannelGroup must report {member}, got {created_group}"
            );
        }

        let (status, read_group) = emulator.get_group(case.group).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannelGroup expected 200, got {status} {read_group}"
        );
        for member in ["Arn", "EgressDomain", "CreatedAt", "ModifiedAt", "ETag", "Description"] {
            assert_eq!(
                read_group[member], created_group[member],
                "case {label:?}: GetChannelGroup {member} must equal what the create stored"
            );
        }
        // `CreateChannelGroupResponse` renders the map as `Tags`; `GetChannelGroupResponse`
        // carries `@jsonName("tags")` on the same member, so the two reads differ in the
        // member NAME and must agree on the value.
        assert_eq!(
            read_group["tags"], created_group["Tags"],
            "case {label:?}: the group's tags must survive the round trip"
        );

        let (status, created_channel) = emulator
            .create_channel(case.group, case.channel, (case.channel_body)())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel expected 200, got {status} {created_channel}"
        );
        assert_eq!(
            created_channel["Arn"].as_str().unwrap_or_default(),
            format!("{group_arn}/channel/{}", case.channel),
            "case {label:?}: a channel ARN nests under its group's"
        );
        let ingest = created_channel["IngestEndpoints"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !ingest.is_empty(),
            "case {label:?}: a channel must report at least one ingest endpoint, got \
             {created_channel}"
        );
        let dns_suffix = egress_domain
            .split_once(".egress.")
            .map(|(_, suffix)| suffix.to_string())
            .unwrap_or_default();
        for endpoint in &ingest {
            let url = endpoint["Url"].as_str().unwrap_or_default();
            assert!(
                url.contains(&dns_suffix) && url.contains(case.channel),
                "case {label:?}: ingest url {url:?} must be derived from the group's DNS \
                 ({dns_suffix}) and the channel name ({})",
                case.channel
            );
        }

        let (status, read_channel) = emulator.get_channel(case.group, case.channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {read_channel}"
        );
        for member in [
            "Arn",
            "IngestEndpoints",
            "Description",
            "InputType",
            "CreatedAt",
            "ModifiedAt",
            "ETag",
            "Tags",
        ] {
            assert_eq!(
                read_channel[member], created_channel[member],
                "case {label:?}: GetChannel {member} must equal what the create stored"
            );
        }

        let (status, created_endpoint) = emulator
            .create_endpoint(case.group, case.channel, case.endpoint, (case.endpoint_body)())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created_endpoint}"
        );
        assert!(
            created_endpoint["Segment"].is_object(),
            "case {label:?}: Segment is required in the response, got {created_endpoint}"
        );
        for (member, manifest_name, suffix) in case.manifests {
            let manifest = created_endpoint[member]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .find(|m| m["ManifestName"] == json!(manifest_name));
            let manifest = manifest.unwrap_or_else(|| {
                panic!(
                    "case {label:?}: {member} must carry the submitted manifest \
                     {manifest_name:?}, got {created_endpoint}"
                )
            });
            let url = manifest["Url"].as_str().unwrap_or_default();
            let expected = format!(
                "https://{egress_domain}/out/v1/{}/{}/{}/{manifest_name}{suffix}",
                case.group, case.channel, case.endpoint
            );
            assert_eq!(
                url, expected,
                "case {label:?}: {member} {manifest_name:?} url must be computed from the \
                 group's egress domain and the resource path"
            );
        }

        let (status, read_endpoint) = emulator
            .get_endpoint(case.group, case.channel, case.endpoint)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint expected 200, got {status} {read_endpoint}"
        );
        for member in [
            "Arn",
            "ContainerType",
            "Segment",
            "Description",
            "StartoverWindowSeconds",
            "HlsManifests",
            "LowLatencyHlsManifests",
            "DashManifests",
            "MssManifests",
            "CreatedAt",
            "ModifiedAt",
            "ETag",
            "Tags",
        ] {
            assert_eq!(
                read_endpoint[member], created_endpoint[member],
                "case {label:?}: GetOriginEndpoint {member} must equal what the create stored"
            );
        }
    }
}

/// RULE 2 — a repeat create at any level conflicts and leaves the stored resource alone.
#[tokio::test]
async fn a_repeat_create_conflicts_and_changes_nothing() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );
    assert_eq!(
        emulator
            .create_endpoint("grp", "chan", "ep", minimal_endpoint("TS"))
            .await
            .0,
        200
    );

    // One row per level of the hierarchy: the whole hierarchy refuses a duplicate name,
    // not just the top of it.
    let (_, group_before) = emulator.get_group("grp").await;
    let (_, channel_before) = emulator.get_channel("grp", "chan").await;
    let (_, endpoint_before) = emulator.get_endpoint("grp", "chan", "ep").await;

    let repeats = [
        (
            "channel group",
            emulator
                .create_group("grp", json!({"Description": "different"}))
                .await,
        ),
        (
            "channel",
            emulator
                .create_channel("grp", "chan", json!({"Description": "different"}))
                .await,
        ),
        (
            "origin endpoint",
            emulator
                .create_endpoint(
                    "grp",
                    "chan",
                    "ep",
                    json!({
                        "ContainerType": "CMAF",
                        "Description": "different",
                        "HlsManifests": [{"ManifestName": "index"}],
                    }),
                )
                .await,
        ),
    ];
    for (level, (status, body)) in repeats {
        assert_eq!(
            status, 409,
            "case {level:?}: a repeat create expected 409, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ConflictException",
            "case {level:?}: expected ConflictException, got {body}"
        );
    }

    assert_eq!(
        emulator.get_group("grp").await.1,
        group_before,
        "a refused create must not have changed the channel group"
    );
    assert_eq!(
        emulator.get_channel("grp", "chan").await.1,
        channel_before,
        "a refused create must not have changed the channel"
    );
    assert_eq!(
        emulator.get_endpoint("grp", "chan", "ep").await.1,
        endpoint_before,
        "a refused create must not have changed the origin endpoint"
    );
}

/// RULE 3 — an unresolvable parent chain is a 404, whichever operation addresses it.
#[tokio::test]
async fn an_unresolvable_parent_chain_is_not_found() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("real", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("real", "real", json!({})).await.0,
        200
    );

    let cases = [
        ("create a channel under a missing group", emulator.create_channel("ghost", "c", json!({})).await),
        (
            "create an endpoint under a missing group",
            emulator
                .create_endpoint("ghost", "real", "e", minimal_endpoint("TS"))
                .await,
        ),
        (
            "create an endpoint under a missing channel",
            emulator
                .create_endpoint("real", "ghost", "e", minimal_endpoint("TS"))
                .await,
        ),
        ("get a missing group", emulator.get_group("ghost").await),
        ("get a channel under a missing group", emulator.get_channel("ghost", "real").await),
        ("get a missing channel", emulator.get_channel("real", "ghost").await),
        (
            "get a missing endpoint",
            emulator.get_endpoint("real", "real", "ghost").await,
        ),
        (
            "list under a missing group",
            emulator.list_endpoints("ghost", "real", "").await,
        ),
        (
            "list under a missing channel",
            emulator.list_endpoints("real", "ghost", "").await,
        ),
    ];
    for (label, (status, body)) in cases {
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// RULE 4 — paging covers every endpoint of the addressed channel exactly once.
///
/// The rows vary the number of endpoints and the page size, including both published
/// bounds of the model's `MaxResults` range (1 and 100) and the absent member (which
/// takes the modelled default of 10).
#[tokio::test]
async fn listing_pages_over_every_endpoint_of_the_channel_exactly_once() {
    let cases: &[(usize, Option<i32>)] = &[
        (1, Some(1)),
        (2, Some(1)),
        (5, Some(2)),
        (7, Some(100)),
        (12, None),
        (25, Some(10)),
    ];

    for &(count, max_results) in cases {
        let label = format!("{count} endpoints, maxResults={max_results:?}");
        let emulator = Emulator::new();
        assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
        assert_eq!(
            emulator.create_channel("grp", "chan", json!({})).await.0,
            200
        );
        // A second channel in the same group, so the list must not leak across channels.
        assert_eq!(
            emulator.create_channel("grp", "other", json!({})).await.0,
            200
        );
        assert_eq!(
            emulator
                .create_endpoint("grp", "other", "not-mine", minimal_endpoint("TS"))
                .await
                .0,
            200
        );

        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            let name = format!("ep-{index:03}");
            assert_eq!(
                emulator
                    .create_endpoint("grp", "chan", &name, minimal_endpoint("TS"))
                    .await
                    .0,
                200,
                "case {label:?}: creating {name} must succeed"
            );
            expected.push(name);
        }
        expected.sort();

        let page_cap = max_results.unwrap_or(10) as usize;
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(limit) = max_results {
                query.push(format!("maxResults={limit}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };

            let (status, body) = emulator.list_endpoints("grp", "chan", &query).await;
            assert_eq!(
                status, 200,
                "case {label:?}: ListOriginEndpoints expected 200, got {status} {body}"
            );
            let items = body["Items"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_cap,
                "case {label:?}: a page carried {} items, more than maxResults {page_cap}",
                items.len()
            );
            for item in &items {
                assert_eq!(
                    item["ChannelName"], json!("chan"),
                    "case {label:?}: the list must only report this channel's endpoints, got \
                     {item}"
                );
                seen.push(
                    item["OriginEndpointName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= count + 2,
                "case {label:?}: paging did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {label:?}: paging must report every endpoint of the channel once, in name \
             order"
        );
    }
}

/// A token this service did not issue is a 400 `ValidationException`, not an empty page.
#[tokio::test]
async fn a_pagination_token_the_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );

    for token in ["zzz", "not-a-token", "%7B%7D"] {
        let (status, body) = emulator
            .list_endpoints("grp", "chan", &format!("?nextToken={token}"))
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE 5 — a delete refuses while children remain, removes the resource, and succeeds
/// again once it is already gone.
#[tokio::test]
async fn deletes_refuse_non_empty_parents_are_effective_and_are_idempotent() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );
    assert_eq!(
        emulator
            .create_endpoint("grp", "chan", "ep", minimal_endpoint("TS"))
            .await
            .0,
        200
    );

    // Both levels that have children refuse, and for the same reason.
    let refusals = [
        ("channel group with a channel", emulator.delete_group("grp").await),
        (
            "channel with an origin endpoint",
            emulator.delete_channel("grp", "chan").await,
        ),
    ];
    for (label, (status, body)) in refusals {
        assert_eq!(
            status, 409,
            "case {label:?}: deleting a non-empty parent expected 409, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {body}"
        );
    }
    assert_eq!(
        emulator.get_group("grp").await.0,
        200,
        "a refused delete must leave the channel group in place"
    );

    // Innermost first, which is the only order the refusals above allow.
    for (label, status) in [
        ("origin endpoint", emulator.delete_endpoint("grp", "chan", "ep").await.0),
        ("channel", emulator.delete_channel("grp", "chan").await.0),
        ("channel group", emulator.delete_group("grp").await.0),
    ] {
        assert!(
            (200..300).contains(&status),
            "case {label:?}: delete expected 2xx, got {status}"
        );
    }

    for (label, status) in [
        (
            "origin endpoint",
            emulator.get_endpoint("grp", "chan", "ep").await.0,
        ),
        ("channel", emulator.get_channel("grp", "chan").await.0),
        ("channel group", emulator.get_group("grp").await.0),
    ] {
        assert_eq!(
            status, 404,
            "case {label:?}: a deleted resource must read as 404, got {status}"
        );
    }

    // `@idempotent`: deleting what is already absent succeeds, in any order.
    for (label, status) in [
        (
            "origin endpoint",
            emulator.delete_endpoint("grp", "chan", "ep").await.0,
        ),
        ("channel", emulator.delete_channel("grp", "chan").await.0),
        ("channel group", emulator.delete_group("grp").await.0),
    ] {
        assert!(
            (200..300).contains(&status),
            "case {label:?}: re-deleting an absent resource expected 2xx, got {status}"
        );
    }
}

/// RULE 6 — every store a create grows is BOUNDED: past its ceiling the create is refused
/// and stores nothing, so what the emulator holds is set by the ceiling rather than by how
/// many requests arrive. A resource-COUNT ceiling is the modelled
/// `ServiceQuotaExceededException` (HTTP 402, "The request would cause a service quota to
/// be exceeded"); an over-sized manifest SET is the input failure the model names,
/// `ValidationException` / `NUM_MANIFESTS_HIGH` (see
/// `tests/all/manifest_count_range.rs` for that rule over a table of counts).
///
/// The ceiling VALUES are the emulator's own, declared as constants in `src/state.rs` —
/// the model carries no quota and no `@length` on the manifest lists — and the four blocks
/// below drive one each: the record COUNT at all three levels of the hierarchy, and the
/// per-record manifest count that decides how large one endpoint record gets.
#[tokio::test]
async fn a_create_at_a_store_ceiling_is_refused_and_stores_nothing() {
    use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
        MAX_CHANNELS_PER_GROUP, MAX_CHANNEL_GROUPS, MAX_MANIFESTS_PER_ORIGIN_ENDPOINT,
        MAX_ORIGIN_ENDPOINTS_PER_CHANNEL,
    };

    // Channel groups per account and Region.
    let emulator = Emulator::new();
    for index in 0..MAX_CHANNEL_GROUPS {
        let (status, body) = emulator
            .create_group(&format!("g-{index}"), json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case \"group {index} of {MAX_CHANNEL_GROUPS}\": expected 200, got {status} {body}"
        );
    }
    let (status, body) = emulator.create_group("one-too-many", json!({})).await;
    assert_eq!(
        status, 402,
        "case \"group past the ceiling\": expected 402, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ServiceQuotaExceededException",
        "case \"group past the ceiling\": expected ServiceQuotaExceededException, got {body}"
    );
    assert_eq!(
        emulator.get_group("one-too-many").await.0,
        404,
        "case \"group past the ceiling\": a refused create must store nothing"
    );
    assert_eq!(
        emulator.get_group("g-0").await.0,
        200,
        "case \"group past the ceiling\": the groups already stored must be untouched"
    );

    // Channels per channel group.
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group("grp", json!({})).await;
    assert_eq!(
        status, 200,
        "case \"channels setup\": CreateChannelGroup expected 200, got {status} {body}"
    );
    for index in 0..MAX_CHANNELS_PER_GROUP {
        let (status, body) = emulator
            .create_channel("grp", &format!("c-{index}"), json!({}))
            .await;
        assert_eq!(
            status, 200,
            "case \"channel {index} of {MAX_CHANNELS_PER_GROUP}\": expected 200, got {status} {body}"
        );
    }
    let (status, body) = emulator
        .create_channel("grp", "one-too-many", json!({}))
        .await;
    assert_eq!(
        status, 402,
        "case \"channel past the ceiling\": expected 402, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ServiceQuotaExceededException",
        "case \"channel past the ceiling\": expected ServiceQuotaExceededException, got {body}"
    );
    assert_eq!(
        emulator.get_channel("grp", "one-too-many").await.0,
        404,
        "case \"channel past the ceiling\": a refused create must store nothing"
    );

    // Origin endpoints per channel.
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group("grp", json!({})).await;
    assert_eq!(
        status, 200,
        "case \"endpoints setup\": CreateChannelGroup expected 200, got {status} {body}"
    );
    let (status, body) = emulator.create_channel("grp", "chan", json!({})).await;
    assert_eq!(
        status, 200,
        "case \"endpoints setup\": CreateChannel expected 200, got {status} {body}"
    );
    for index in 0..MAX_ORIGIN_ENDPOINTS_PER_CHANNEL {
        let (status, body) = emulator
            .create_endpoint("grp", "chan", &format!("e-{index}"), minimal_endpoint("TS"))
            .await;
        assert_eq!(
            status, 200,
            "case \"endpoint {index} of {MAX_ORIGIN_ENDPOINTS_PER_CHANNEL}\": expected 200, \
             got {status} {body}"
        );
    }
    let (status, body) = emulator
        .create_endpoint("grp", "chan", "one-too-many", minimal_endpoint("TS"))
        .await;
    assert_eq!(
        status, 402,
        "case \"endpoint past the ceiling\": expected 402, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ServiceQuotaExceededException",
        "case \"endpoint past the ceiling\": expected ServiceQuotaExceededException, got {body}"
    );
    assert_eq!(
        emulator.get_endpoint("grp", "chan", "one-too-many").await.0,
        404,
        "case \"endpoint past the ceiling\": a refused create must store nothing"
    );

    // Manifests per origin endpoint, counted across every family together: the ceiling
    // itself is accepted, one past it is refused and stores nothing.
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group("grp", json!({})).await;
    assert_eq!(
        status, 200,
        "case \"manifests setup\": CreateChannelGroup expected 200, got {status} {body}"
    );
    let (status, body) = emulator.create_channel("grp", "chan", json!({})).await;
    assert_eq!(
        status, 200,
        "case \"manifests setup\": CreateChannel expected 200, got {status} {body}"
    );
    // Spread over the three families a CMAF container serves: MSS belongs to ISM, so
    // adding it here would be refused on the container rule rather than the ceiling.
    let spread = MAX_MANIFESTS_PER_ORIGIN_ENDPOINT / 3;
    let manifests = |count: usize, prefix: &str| -> Value {
        (0..count)
            .map(|index| json!({"ManifestName": format!("{prefix}-{index}")}))
            .collect()
    };
    let (status, body) = emulator
        .create_endpoint(
            "grp",
            "chan",
            "at-the-ceiling",
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": manifests(spread, "hls"),
                "LowLatencyHlsManifests": manifests(spread, "llhls"),
                "DashManifests": manifests(MAX_MANIFESTS_PER_ORIGIN_ENDPOINT - 2 * spread, "dash"),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"manifests at the ceiling\": expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_endpoint(
            "grp",
            "chan",
            "one-too-many",
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": manifests(MAX_MANIFESTS_PER_ORIGIN_ENDPOINT + 1, "hls"),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"manifests past the ceiling\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ValidationException",
        "case \"manifests past the ceiling\": expected ValidationException, got {body}"
    );
    assert_eq!(
        body.get("ValidationExceptionType").and_then(Value::as_str),
        Some("NUM_MANIFESTS_HIGH"),
        "case \"manifests past the ceiling\": expected NUM_MANIFESTS_HIGH, got {body}"
    );
    assert_eq!(
        emulator.get_endpoint("grp", "chan", "one-too-many").await.0,
        404,
        "case \"manifests past the ceiling\": a refused create must store nothing"
    );
}
