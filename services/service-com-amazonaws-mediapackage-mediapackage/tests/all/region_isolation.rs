// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Region scoping of this control plane, driven over real HTTP
//! through the same `MultiServiceRouter` the emulator boots, so what is asserted is the
//! wire behaviour.
//!
//! The rule under test: every MediaPackage resource belongs to the one Region the request
//! that created it was served in, so every later operation resolves an id, an ARN or a
//! listing only among the resources of the Region ITS OWN request is served in — a read
//! or a mutation addressed to any other Region answers `NotFoundException` (404) and
//! changes nothing, a listing there does not report it, and the same id may be created
//! independently in each Region (it "must be unique within the region"), where it names a
//! separate resource whose ARN and whose ingest or playback URLs carry that Region.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: WHICH
//! two Regions are involved — the emulator's own default Region as the one that holds the
//! resource and as the one that must not see it, two Regions on different continents, and
//! two Regions sharing a prefix, so nothing can pass by comparing a prefix — and, inside a
//! row, which resource type and which operation the second Region addresses.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The crate's shared test client. `call_in` is the region-parameterised round trip, which
// this suite needs because the Region a request is served in is its subject.
use crate::channel_lifecycle::{call_in, emulator, error_code, ACCOUNT, CROSS_ACCOUNT_HARVEST_ROLE};
// The tagging operations address a resource by ARN inside one percent-encoded path
// segment; the escaping lives once, in the tagging suite.
use crate::resource_tags::{tags_path, urlencode};

/// The Region pairs every suite below is driven over: the Region a resource is created in
/// ("home"), and a second Region that must not see it ("foreign").
///
/// The rows span what the rule generalises over: the emulator's own default Region
/// (`us-east-1`, the Region an unsigned request is served in) as home and as foreign, two
/// Regions on different continents, and two Regions sharing a prefix — so a scope that
/// compared Region prefixes rather than Regions would fail a row.
const REGION_PAIRS: &[(&str, &str)] = &[
    ("us-east-1", "eu-west-1"),
    ("eu-west-1", "us-east-1"),
    ("us-east-1", "us-east-2"),
    ("ap-southeast-2", "eu-central-1"),
];

/// `arn:aws:mediapackage:<region>:<account>:<type>/<id>`, as this service mints it.
fn resource_arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:mediapackage:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// The `channels` array of a `ListChannels` body, as ids.
fn listed_ids(body: &Value, member: &str) -> Vec<String> {
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a listing body carries {member}, got {body:?}"))
        .iter()
        .map(|record| {
            record
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `arn` of a record body.
fn arn_of(record: &Value) -> &str {
    record.get("arn").and_then(Value::as_str).unwrap_or_default()
}

/// Create a channel `id` in `region`, asserting it succeeded, and answer the record.
async fn create_channel_in(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    body: &Value,
) -> Value {
    let (status, created) = call_in(router, region, "POST", "/channels", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {region:?}: CreateChannel {id:?} expected 200, got {status} {created:?}"
    );
    created
}

/// Create an origin endpoint `id` in `region` packaging `channel_id`, asserting it
/// succeeded, and answer the record.
async fn create_endpoint_in(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    channel_id: &str,
) -> Value {
    let (status, created) = call_in(
        router,
        region,
        "POST",
        "/origin_endpoints",
        Some(&json!({
            "channelId": channel_id,
            "id": id,
            "hlsPackage": {},
            // A harvest exports content a startover window retains, so every endpoint
            // this suite creates keeps one: the harvest-job rows below submit against it.
            "startoverWindowSeconds": 3_600
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {region:?}: CreateOriginEndpoint {id:?} expected 200, got {status} {created:?}"
    );
    created
}

/// One `CreateHarvestJob` body harvesting `endpoint_id`.
fn harvest_body(endpoint_id: &str) -> Value {
    json!({
        "originEndpointId": endpoint_id,
        "startTime": "2026-01-01T00:00:00Z",
        "endTime": "2026-01-01T01:00:00Z",
        "s3Destination": {
            "bucketName": "harvest-bucket",
            "manifestKey": "index.m3u8",
            "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
        }
    })
}

/// Assert that `status`/`body` is this service's `NotFoundException` (404).
fn assert_not_found(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {body:?}"
    );
}

/// Assert that `status`/`body` is this service's `UnprocessableEntityException` (422).
fn assert_unprocessable(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Reads and listings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_channel_resolves_and_lists_only_in_the_region_it_was_created_in() {
    const ID: &str = "region-probe";

    for (home, foreign) in REGION_PAIRS {
        let case = format!("a channel of {home} read in {foreign}");
        let router = emulator();

        let created = create_channel_in(&router, home, ID, &json!({ "id": ID })).await;
        let home_arn = resource_arn(home, "channels", ID);
        assert_eq!(
            arn_of(&created),
            home_arn,
            "case {case:?}: the create must mint the home Region's ARN, got {created:?}"
        );

        // The Region that holds it answers the record.
        let (status, described) =
            call_in(&router, home, "GET", &format!("/channels/{ID}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel in {home} expected 200, got {status} {described:?}"
        );
        assert_eq!(
            arn_of(&described), home_arn,
            "case {case:?}: DescribeChannel in {home} must report the stored record, got {described:?}"
        );

        // No other Region holds it.
        let (status, body) =
            call_in(&router, foreign, "GET", &format!("/channels/{ID}"), None).await;
        assert_not_found(&case, status, &body);

        let (status, listed) = call_in(&router, foreign, "GET", "/channels", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListChannels in {foreign} expected 200, got {status} {listed:?}"
        );
        assert_eq!(
            listed_ids(&listed, "channels"),
            Vec::<String>::new(),
            "case {case:?}: ListChannels in {foreign} must report nothing, got {listed:?}"
        );
        let (_, listed) = call_in(&router, home, "GET", "/channels", None).await;
        assert_eq!(
            listed_ids(&listed, "channels"),
            vec![ID.to_string()],
            "case {case:?}: ListChannels in {home} must report the channel, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_endpoint_and_a_harvest_job_resolve_only_in_their_own_region() {
    const CHANNEL: &str = "scoped-channel";
    const ENDPOINT: &str = "scoped-endpoint";
    const JOB: &str = "scoped-harvest";

    for (home, foreign) in REGION_PAIRS {
        let case = format!("an endpoint and a job of {home} read in {foreign}");
        let router = emulator();

        create_channel_in(&router, home, CHANNEL, &json!({ "id": CHANNEL })).await;
        let endpoint = create_endpoint_in(&router, home, ENDPOINT, CHANNEL).await;
        assert_eq!(
            arn_of(&endpoint),
            resource_arn(home, "origin_endpoints", ENDPOINT),
            "case {case:?}: the create must mint the home Region's ARN, got {endpoint:?}"
        );
        let url = endpoint
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            url.contains(*home),
            "case {case:?}: the playback URL must name {home}, got {url:?}"
        );

        let (status, job) =
            call_in(&router, home, "POST", "/harvest_jobs", Some(&{
                let mut body = harvest_body(ENDPOINT);
                body["id"] = json!(JOB);
                body
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateHarvestJob in {home} expected 200, got {status} {job:?}"
        );
        assert_eq!(
            arn_of(&job),
            resource_arn(home, "harvest_jobs", JOB),
            "case {case:?}: the job's ARN must name {home}, got {job:?}"
        );

        // Neither resource exists in any other Region, and neither is listed there.
        let (status, body) = call_in(
            &router,
            foreign,
            "GET",
            &format!("/origin_endpoints/{ENDPOINT}"),
            None,
        )
        .await;
        assert_not_found(&case, status, &body);
        let (status, body) =
            call_in(&router, foreign, "GET", &format!("/harvest_jobs/{JOB}"), None).await;
        assert_not_found(&case, status, &body);

        let (_, listed) = call_in(&router, foreign, "GET", "/origin_endpoints", None).await;
        assert_eq!(
            listed_ids(&listed, "originEndpoints"),
            Vec::<String>::new(),
            "case {case:?}: ListOriginEndpoints in {foreign} must report nothing, got {listed:?}"
        );
        let (_, listed) = call_in(&router, foreign, "GET", "/harvest_jobs", None).await;
        assert_eq!(
            listed_ids(&listed, "harvestJobs"),
            Vec::<String>::new(),
            "case {case:?}: ListHarvestJobs in {foreign} must report nothing, got {listed:?}"
        );
        // The home Region still reports both, so nothing above moved them.
        let (_, listed) = call_in(&router, home, "GET", "/origin_endpoints", None).await;
        assert_eq!(
            listed_ids(&listed, "originEndpoints"),
            vec![ENDPOINT.to_string()],
            "case {case:?}: ListOriginEndpoints in {home} must report the endpoint, got {listed:?}"
        );
        let (_, listed) = call_in(&router, home, "GET", "/harvest_jobs", None).await;
        assert_eq!(
            listed_ids(&listed, "harvestJobs"),
            vec![JOB.to_string()],
            "case {case:?}: ListHarvestJobs in {home} must report the job, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Creates: an id is unique WITHIN a Region, and a reference resolves within one
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_same_id_names_an_independent_resource_in_each_region() {
    const CHANNEL: &str = "twin-channel";
    const ENDPOINT: &str = "twin-endpoint";
    const JOB: &str = "twin-harvest";

    for (home, foreign) in REGION_PAIRS {
        let case = format!("the same ids created in {home} and in {foreign}");
        let router = emulator();

        // Each Region gets its own channel, endpoint and harvest job under the SAME ids:
        // an id only has to be unique within a Region, so none of these is a duplicate.
        for (region, description) in [(home, "the home copy"), (foreign, "the foreign copy")] {
            create_channel_in(
                &router,
                region,
                CHANNEL,
                &json!({ "id": CHANNEL, "description": description }),
            )
            .await;
            create_endpoint_in(&router, region, ENDPOINT, CHANNEL).await;
            let (status, job) = call_in(&router, region, "POST", "/harvest_jobs", Some(&{
                let mut body = harvest_body(ENDPOINT);
                body["id"] = json!(JOB);
                body
            }))
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateHarvestJob in {region} expected 200, got {status} {job:?}"
            );
        }

        // Each copy is its own resource: its ARN names its own Region, and it keeps the
        // description that Region's create sent.
        for (region, description) in [(home, "the home copy"), (foreign, "the foreign copy")] {
            let (status, described) =
                call_in(&router, region, "GET", &format!("/channels/{CHANNEL}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeChannel in {region} expected 200, got {status} {described:?}"
            );
            assert_eq!(
                arn_of(&described),
                resource_arn(region, "channels", CHANNEL),
                "case {case:?}: the channel of {region} must carry its own ARN, got {described:?}"
            );
            assert_eq!(
                described.get("description").and_then(Value::as_str),
                Some(description),
                "case {case:?}: the channel of {region} must keep its own description, got {described:?}"
            );
            for (resource_type, id, path) in [
                ("origin_endpoints", ENDPOINT, format!("/origin_endpoints/{ENDPOINT}")),
                ("harvest_jobs", JOB, format!("/harvest_jobs/{JOB}")),
            ] {
                let (status, described) = call_in(&router, region, "GET", &path, None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a read of {id:?} in {region} expected 200, got {status} {described:?}"
                );
                assert_eq!(
                    arn_of(&described),
                    resource_arn(region, resource_type, id),
                    "case {case:?}: {id:?} in {region} must carry its own ARN, got {described:?}"
                );
            }
        }

        // And a duplicate WITHIN one Region is still refused.
        let (status, body) = call_in(
            &router,
            foreign,
            "POST",
            "/channels",
            Some(&json!({ "id": CHANNEL })),
        )
        .await;
        assert_unprocessable(&case, status, &body);
    }
}

#[tokio::test]
async fn a_create_resolves_the_resource_it_references_in_its_own_region_only() {
    const CHANNEL: &str = "referenced-channel";
    const ENDPOINT: &str = "referenced-endpoint";

    for (home, foreign) in REGION_PAIRS {
        let case = format!("a create in {foreign} referencing a resource of {home}");
        let router = emulator();

        create_channel_in(&router, home, CHANNEL, &json!({ "id": CHANNEL })).await;
        create_endpoint_in(&router, home, ENDPOINT, CHANNEL).await;

        // `channelId` names a channel only the home Region holds, so there is nothing in
        // this Region for the endpoint to package.
        let (status, body) = call_in(
            &router,
            foreign,
            "POST",
            "/origin_endpoints",
            Some(&json!({
                "channelId": CHANNEL,
                "id": ENDPOINT,
                "hlsPackage": {}
            })),
        )
        .await;
        assert_unprocessable(&case, status, &body);

        // Likewise `originEndpointId`: the endpoint it names is another Region's.
        let (status, body) = call_in(&router, foreign, "POST", "/harvest_jobs", Some(&{
            let mut harvest = harvest_body(ENDPOINT);
            harvest["id"] = json!("cross-region-harvest");
            harvest
        }))
        .await;
        assert_unprocessable(&case, status, &body);

        // Neither refused create left anything behind in either Region.
        let (_, listed) = call_in(&router, foreign, "GET", "/origin_endpoints", None).await;
        assert_eq!(
            listed_ids(&listed, "originEndpoints"),
            Vec::<String>::new(),
            "case {case:?}: the refused create must store nothing, got {listed:?}"
        );
        let (_, listed) = call_in(&router, home, "GET", "/harvest_jobs", None).await;
        assert_eq!(
            listed_ids(&listed, "harvestJobs"),
            Vec::<String>::new(),
            "case {case:?}: the refused submit must store nothing, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Mutations and deletes
// ---------------------------------------------------------------------------

/// Every write this service serves on a resource addressed by its `{Id}` label, as the
/// method, the path for that id and the body it carries — so the row set below covers
/// each one rather than only a chosen mutation.
fn channel_writes(id: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        (
            "UpdateChannel",
            "PUT",
            format!("/channels/{id}"),
            Some(json!({ "description": "written by another Region" })),
        ),
        (
            "ConfigureLogs",
            "PUT",
            format!("/channels/{id}/configure_logs"),
            Some(json!({ "egressAccessLogs": { "logGroupName": "/aws/foreign" } })),
        ),
        (
            "RotateChannelCredentials",
            "PUT",
            format!("/channels/{id}/credentials"),
            None,
        ),
        ("DeleteChannel", "DELETE", format!("/channels/{id}"), None),
    ]
}

#[tokio::test]
async fn a_write_addressed_to_another_region_is_not_found_and_changes_nothing() {
    const CHANNEL: &str = "untouchable-channel";
    const ENDPOINT: &str = "untouchable-endpoint";

    for (home, foreign) in REGION_PAIRS {
        for (operation, method, path, body) in channel_writes(CHANNEL) {
            let case = format!("{operation} in {foreign} against the channel of {home}");
            let router = emulator();
            let created = create_channel_in(
                &router,
                home,
                CHANNEL,
                &json!({ "id": CHANNEL, "description": "the stored description" }),
            )
            .await;

            let (status, refused) = call_in(&router, foreign, method, &path, body.as_ref()).await;
            assert_not_found(&case, status, &refused);

            // The home Region's record is byte-for-byte what its create stored.
            let (status, described) =
                call_in(&router, home, "GET", &format!("/channels/{CHANNEL}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the channel of {home} must still be readable, got {status} {described:?}"
            );
            assert_eq!(
                described, created,
                "case {case:?}: the channel of {home} must be unchanged, got {described:?}"
            );
        }

        // The same for the two writes addressed to an origin endpoint.
        for (operation, method, path, body) in [
            (
                "UpdateOriginEndpoint",
                "PUT",
                format!("/origin_endpoints/{ENDPOINT}"),
                Some(json!({ "hlsPackage": {}, "description": "written by another Region" })),
            ),
            (
                "DeleteOriginEndpoint",
                "DELETE",
                format!("/origin_endpoints/{ENDPOINT}"),
                None,
            ),
        ] {
            let case = format!("{operation} in {foreign} against the endpoint of {home}");
            let router = emulator();
            create_channel_in(&router, home, CHANNEL, &json!({ "id": CHANNEL })).await;
            let created = create_endpoint_in(&router, home, ENDPOINT, CHANNEL).await;

            let (status, refused) = call_in(&router, foreign, method, &path, body.as_ref()).await;
            assert_not_found(&case, status, &refused);

            let (status, described) = call_in(
                &router,
                home,
                "GET",
                &format!("/origin_endpoints/{ENDPOINT}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the endpoint of {home} must still be readable, got {status} {described:?}"
            );
            assert_eq!(
                described, created,
                "case {case:?}: the endpoint of {home} must be unchanged, got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_write_reaches_only_its_own_regions_copy_of_an_id() {
    const CHANNEL: &str = "two-region-channel";

    for (home, foreign) in REGION_PAIRS {
        let case = format!("writes in {foreign} beside the same id in {home}");
        let router = emulator();

        let home_record = create_channel_in(
            &router,
            home,
            CHANNEL,
            &json!({ "id": CHANNEL, "description": "the home description" }),
        )
        .await;
        create_channel_in(
            &router,
            foreign,
            CHANNEL,
            &json!({ "id": CHANNEL, "description": "the foreign description" }),
        )
        .await;

        // An update, a log subscription and a credential rotation in the foreign Region.
        let (status, updated) = call_in(
            &router,
            foreign,
            "PUT",
            &format!("/channels/{CHANNEL}"),
            Some(&json!({ "description": "updated in the foreign Region" })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateChannel in {foreign} expected 200, got {status} {updated:?}"
        );
        let (status, logged) = call_in(
            &router,
            foreign,
            "PUT",
            &format!("/channels/{CHANNEL}/configure_logs"),
            Some(&json!({ "ingressAccessLogs": { "logGroupName": "/aws/foreign-ingress" } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ConfigureLogs in {foreign} expected 200, got {status} {logged:?}"
        );
        let (status, rotated) = call_in(
            &router,
            foreign,
            "PUT",
            &format!("/channels/{CHANNEL}/credentials"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: RotateChannelCredentials in {foreign} expected 200, got {status} {rotated:?}"
        );

        // The home Region's channel is exactly as its create left it.
        let (_, described) =
            call_in(&router, home, "GET", &format!("/channels/{CHANNEL}"), None).await;
        assert_eq!(
            described, home_record,
            "case {case:?}: the channel of {home} must be unchanged, got {described:?}"
        );

        // A delete in the foreign Region removes the foreign copy and only it.
        let (status, deleted) = call_in(
            &router,
            foreign,
            "DELETE",
            &format!("/channels/{CHANNEL}"),
            None,
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteChannel in {foreign} expected 202, got {status} {deleted:?}"
        );
        let (status, body) = call_in(
            &router,
            foreign,
            "GET",
            &format!("/channels/{CHANNEL}"),
            None,
        )
        .await;
        assert_not_found(&case, status, &body);
        let (status, described) =
            call_in(&router, home, "GET", &format!("/channels/{CHANNEL}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the channel of {home} must survive the delete, got {status} {described:?}"
        );
        assert_eq!(
            described, home_record,
            "case {case:?}: the channel of {home} must be unchanged, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tagging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_tagging_request_resolves_an_arn_only_in_its_own_region() {
    const CHANNEL: &str = "tagged-channel";

    for (home, foreign) in REGION_PAIRS {
        let case = format!("tagging in {foreign} with an ARN of {home}");
        let router = emulator();

        create_channel_in(
            &router,
            home,
            CHANNEL,
            &json!({ "id": CHANNEL, "tags": { "copy": "home" } }),
        )
        .await;
        create_channel_in(
            &router,
            foreign,
            CHANNEL,
            &json!({ "id": CHANNEL, "tags": { "copy": "foreign" } }),
        )
        .await;
        let home_arn = resource_arn(home, "channels", CHANNEL);
        let foreign_arn = resource_arn(foreign, "channels", CHANNEL);

        // Each Region reports the tags of its OWN copy, for its own ARN.
        for (region, arn, expected) in [
            (home, &home_arn, "home"),
            (foreign, &foreign_arn, "foreign"),
        ] {
            let (status, listed) = call_in(&router, region, "GET", &tags_path(arn), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource in {region} expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                listed.get("tags"),
                Some(&json!({ "copy": expected })),
                "case {case:?}: {region} must report its own copy's tags, got {listed:?}"
            );
        }

        // An ARN naming another Region resolves to nothing, on each of the three
        // tagging operations — and leaves both copies' tag sets alone.
        let (status, body) = call_in(&router, foreign, "GET", &tags_path(&home_arn), None).await;
        assert_not_found(&case, status, &body);
        let (status, body) = call_in(
            &router,
            foreign,
            "POST",
            &tags_path(&home_arn),
            Some(&json!({ "tags": { "written": "across regions" } })),
        )
        .await;
        assert_not_found(&case, status, &body);
        let (status, body) = call_in(
            &router,
            foreign,
            "DELETE",
            &format!("{}?tagKeys={}", tags_path(&home_arn), urlencode("copy")),
            None,
        )
        .await;
        assert_not_found(&case, status, &body);
        for (region, arn, expected) in [
            (home, &home_arn, "home"),
            (foreign, &foreign_arn, "foreign"),
        ] {
            let (_, listed) = call_in(&router, region, "GET", &tags_path(arn), None).await;
            assert_eq!(
                listed.get("tags"),
                Some(&json!({ "copy": expected })),
                "case {case:?}: the tags of {region} must be untouched, got {listed:?}"
            );
        }

        // A tag write with the serving Region's own ARN reaches that Region's copy only.
        let (status, body) = call_in(
            &router,
            foreign,
            "POST",
            &tags_path(&foreign_arn),
            Some(&json!({ "tags": { "added": "in the foreign Region" } })),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: TagResource in {foreign} expected 204, got {status} {body:?}"
        );
        let (_, listed) = call_in(&router, foreign, "GET", &tags_path(&foreign_arn), None).await;
        assert_eq!(
            listed.get("tags"),
            Some(&json!({ "copy": "foreign", "added": "in the foreign Region" })),
            "case {case:?}: the foreign copy must carry the new tag, got {listed:?}"
        );
        let (_, listed) = call_in(&router, home, "GET", &tags_path(&home_arn), None).await;
        assert_eq!(
            listed.get("tags"),
            Some(&json!({ "copy": "home" })),
            "case {case:?}: the home copy must be untouched, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_of_a_region_that_holds_no_such_resource_is_not_found() {
    const CHANNEL: &str = "single-region-channel";

    for (home, foreign) in REGION_PAIRS {
        let case = format!("an ARN of {foreign}, where no such channel exists");
        let router = emulator();
        create_channel_in(&router, home, CHANNEL, &json!({ "id": CHANNEL })).await;

        // Well-formed, and this very request's Region — but that Region holds no channel
        // under the id, so there is no tag set to report and none to write.
        let foreign_arn = resource_arn(foreign, "channels", CHANNEL);
        let (status, body) = call_in(&router, foreign, "GET", &tags_path(&foreign_arn), None).await;
        assert_not_found(&case, status, &body);
        let (status, body) = call_in(
            &router,
            foreign,
            "POST",
            &tags_path(&foreign_arn),
            Some(&json!({ "tags": { "team": "video" } })),
        )
        .await;
        assert_not_found(&case, status, &body);

        // And the home Region's own ARN still resolves there.
        let home_arn = resource_arn(home, "channels", CHANNEL);
        let (status, listed) = call_in(&router, home, "GET", &tags_path(&home_arn), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource in {home} expected 200, got {status} {listed:?}"
        );
    }
}
