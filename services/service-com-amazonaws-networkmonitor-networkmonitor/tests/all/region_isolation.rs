// SPDX-License-Identifier: CC0-1.0

//! A monitor is a REGIONAL resource, and this suite drives that rule over the
//! wire.
//!
//! CloudWatch Network Monitor answers on one endpoint per region
//! (`smithy_query(query="service_info")` ->
//! `https://networkmonitor.{Region}.{PartitionResult#dnsSuffix}`, auth scheme
//! `sigv4` with signing name `networkmonitor`), and every ARN it mints carries
//! the region, so:
//!
//! * a monitor exists only in the region whose endpoint created it: every read
//!   and every mutation signed for another region answers 404
//!   `ResourceNotFoundException` and leaves the monitor exactly as it was;
//! * `ListMonitors` reports the monitors of the region it was signed for and no
//!   other region's;
//! * one `monitorName` names an INDEPENDENT monitor in each region, each with
//!   its own ARN naming its own region, its own `aggregationPeriod`, its own
//!   probes and its own tags;
//! * a `clientToken` replays only in the region it was sent to;
//! * a tagging call reaches only resources of the region it was signed for, and
//!   an ARN of another region's resource is the tag APIs' own 404
//!   `NotFoundException`.
//!
//! Each case names the region pair it drives, so the table spans several
//! regions rather than the one pair a single example would pin.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::monitor_and_probe::{error_code, page_names, router, string_at};
use super::resource_tags::TAG_NOT_FOUND_CODE;

/// The one account id a generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The regions the table drives. A request's region is the one in its SigV4
/// credential scope, so these are simply published AWS region names spanning
/// several partitions' naming forms (three- and four-segment, two-digit index):
/// `us-east-1`, `eu-central-1`, `ap-southeast-2`, `us-gov-west-1`.
const REGIONS: &[&str] = &["us-east-1", "eu-central-1", "ap-southeast-2", "us-gov-west-1"];

/// A SigV4 `Authorization` naming this service's signing name (`networkmonitor`)
/// — how the router picks the service — and `region` in its credential scope,
/// which is the region the request addresses.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTONLY/20260101/{region}/networkmonitor/aws4_request, \
         SignedHeaders=host, Signature=00"
    )
}

/// Send one request signed for `region` and return its status and parsed JSON
/// body (`Value::Null` for an empty body).
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("authorization", authorization(region));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |b| {
        bytes::Bytes::copy_from_slice(b.as_bytes())
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A subnet ARN of `region` to use as a probe's `sourceArn`.
fn source_arn_in(region: &str) -> String {
    format!("arn:aws:ec2:{region}:{ACCOUNT}:subnet/subnet-0123456789abcdef0")
}

/// The ARN this service mints for a monitor of `region`.
fn monitor_arn_in(region: &str, name: &str) -> String {
    format!("arn:aws:networkmonitor:{region}:{ACCOUNT}:monitor/{name}")
}

/// A `CreateMonitor` body naming one probe, so the created monitor holds a
/// resource of its own for the cross-region cases to try to reach.
fn create_body(region: &str, name: &str, aggregation_period: i64) -> String {
    format!(
        r#"{{"monitorName":"{name}","aggregationPeriod":{aggregation_period},
             "tags":{{"home":"{region}"}},
             "probes":[{{"sourceArn":"{}","destination":"10.0.0.1",
                         "protocol":"ICMP","packetSize":56}}]}}"#,
        source_arn_in(region)
    )
}

async fn create_monitor_in(router: &MultiServiceRouter, region: &str, body: &str) -> (u16, Value) {
    call_in(router, region, "POST", "/monitors", Some(body)).await
}

async fn get_monitor_in(router: &MultiServiceRouter, region: &str, name: &str) -> (u16, Value) {
    call_in(router, region, "GET", &format!("/monitors/{name}"), None).await
}

async fn list_monitors_in(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call_in(router, region, "GET", "/monitors", None).await
}

/// The `probeId` of the single probe a `GetMonitor` / `CreateMonitor` body
/// reports.
fn only_probe_id(body: &Value) -> String {
    let probes = body
        .get("probes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        probes.len(),
        1,
        "expected exactly one probe, got {probes:?}"
    );
    string_at(&probes[0], "probeId")
}

/// `resourceArn` is bound to a NON-greedy `@httpLabel` (`/tags/{resourceArn}`),
/// so an ARN travels as ONE path segment with its `/` and `:` percent-encoded.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Every ordered pair of distinct regions in [`REGIONS`]: the region a monitor
/// is created in, and the region a later call is signed for.
fn region_pairs() -> Vec<(&'static str, &'static str)> {
    REGIONS
        .iter()
        .flat_map(|home| {
            REGIONS
                .iter()
                .filter(move |other| *other != home)
                .map(move |other| (*home, *other))
        })
        .collect()
}

/// One call aimed at a region that holds nothing: the operation, how it is sent,
/// and the body it carries.
struct Attempt {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<String>,
}

/// Every operation that addresses a monitor by name, aimed at a region that does
/// not hold it.
fn attempts(region: &str, name: &str, probe_id: &str) -> Vec<Attempt> {
    vec![
        Attempt {
            case: "GetMonitor",
            method: "GET",
            path: format!("/monitors/{name}"),
            body: None,
        },
        Attempt {
            case: "UpdateMonitor",
            method: "PATCH",
            path: format!("/monitors/{name}"),
            body: Some(r#"{"aggregationPeriod":30}"#.to_string()),
        },
        Attempt {
            case: "DeleteMonitor",
            method: "DELETE",
            path: format!("/monitors/{name}"),
            body: None,
        },
        Attempt {
            case: "CreateProbe",
            method: "POST",
            path: format!("/monitors/{name}/probes"),
            body: Some(format!(
                r#"{{"probe":{{"sourceArn":"{}","destination":"10.0.0.2",
                     "protocol":"ICMP","packetSize":56}}}}"#,
                source_arn_in(region)
            )),
        },
        Attempt {
            case: "GetProbe",
            method: "GET",
            path: format!("/monitors/{name}/probes/{probe_id}"),
            body: None,
        },
        Attempt {
            case: "UpdateProbe",
            method: "PATCH",
            path: format!("/monitors/{name}/probes/{probe_id}"),
            body: Some(r#"{"packetSize":120}"#.to_string()),
        },
        Attempt {
            case: "DeleteProbe",
            method: "DELETE",
            path: format!("/monitors/{name}/probes/{probe_id}"),
            body: None,
        },
    ]
}

/// A monitor created through one region's endpoint is unreachable through every
/// other: each operation that names it answers 404 `ResourceNotFoundException`
/// and changes nothing, and the monitor is still intact in its own region
/// afterwards.
#[tokio::test]
async fn every_operation_reaches_only_the_region_it_was_signed_for() {
    const NAME: &str = "regional-monitor";

    for (home, other) in region_pairs() {
        let router = router();
        let (status, created) = create_monitor_in(&router, home, &create_body(home, NAME, 60)).await;
        assert_eq!(
            status, 200,
            "case {home:?}: expected CreateMonitor 200, got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "monitorArn"),
            monitor_arn_in(home, NAME),
            "case {home:?}: a monitor's ARN names the region that created it, got {created:?}"
        );
        let probe_id = only_probe_id(&get_monitor_in(&router, home, NAME).await.1);

        for attempt in attempts(other, NAME, &probe_id) {
            let (status, body) = call_in(
                &router,
                other,
                attempt.method,
                &attempt.path,
                attempt.body.as_deref(),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {home:?} -> {other:?} {}: expected HTTP 404, got {status} {body:?}",
                attempt.case
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {home:?} -> {other:?} {}: expected ResourceNotFoundException, got {body:?}",
                attempt.case
            );
        }

        let (status, listed) = list_monitors_in(&router, other).await;
        assert_eq!(
            status, 200,
            "case {other:?}: expected ListMonitors 200, got {status} {listed:?}"
        );
        assert!(
            page_names(&listed).is_empty(),
            "case {home:?} -> {other:?}: a region holding no monitor lists none, got {listed:?}"
        );

        // Nothing the refused calls carried reached the stored monitor.
        let (status, home_read) = get_monitor_in(&router, home, NAME).await;
        assert_eq!(
            status, 200,
            "case {home:?}: expected GetMonitor 200, got {status} {home_read:?}"
        );
        assert_eq!(
            home_read.get("aggregationPeriod").and_then(Value::as_i64),
            Some(60),
            "case {home:?} -> {other:?}: a cross-region update changes nothing, got {home_read:?}"
        );
        assert_eq!(
            only_probe_id(&home_read),
            probe_id,
            "case {home:?} -> {other:?}: a cross-region delete removes nothing, got {home_read:?}"
        );
        assert_eq!(
            page_names(&list_monitors_in(&router, home).await.1),
            vec![NAME.to_string()],
            "case {home:?}: the monitor is listed in its own region"
        );
    }
}

/// The same `monitorName` is an independent monitor in each region: each create
/// succeeds, each ARN names its own region, each keeps its own
/// `aggregationPeriod` and tags, each region lists only its own, and deleting
/// one leaves the others readable.
#[tokio::test]
async fn one_monitor_name_is_an_independent_resource_in_each_region() {
    const NAME: &str = "shared-name";
    let router = router();

    // Each row differs in the region it is created through and in the
    // `aggregationPeriod` it asks for (the two documented values), so a read
    // that returned another region's record would report the wrong period.
    let rows: Vec<(&str, i64)> = REGIONS
        .iter()
        .enumerate()
        .map(|(index, region)| (*region, if index % 2 == 0 { 30 } else { 60 }))
        .collect();

    for (region, period) in &rows {
        let (status, created) =
            create_monitor_in(&router, region, &create_body(region, NAME, *period)).await;
        assert_eq!(
            status, 200,
            "case {region:?}: the same name is free in every region, got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "monitorArn"),
            monitor_arn_in(region, NAME),
            "case {region:?}: expected an ARN naming this region, got {created:?}"
        );
    }

    for (region, period) in &rows {
        let (status, read) = get_monitor_in(&router, region, NAME).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected GetMonitor 200, got {status} {read:?}"
        );
        assert_eq!(
            read.get("aggregationPeriod").and_then(Value::as_i64),
            Some(*period),
            "case {region:?}: expected the period this region's create asked for, got {read:?}"
        );
        assert_eq!(
            read.get("tags"),
            Some(&serde_json::json!({"home": region})),
            "case {region:?}: expected the tags this region's create carried, got {read:?}"
        );
        assert_eq!(
            page_names(&list_monitors_in(&router, region).await.1),
            vec![NAME.to_string()],
            "case {region:?}: a listing reports this region's one monitor"
        );
    }

    // An update in one region moves only that region's record.
    let (updated_region, _) = rows[0];
    let (status, updated) = call_in(
        &router,
        updated_region,
        "PATCH",
        &format!("/monitors/{NAME}"),
        Some(r#"{"aggregationPeriod":60}"#),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected UpdateMonitor 200, got {status} {updated:?}"
    );
    for (region, period) in &rows[1..] {
        let (_, read) = get_monitor_in(&router, region, NAME).await;
        assert_eq!(
            read.get("aggregationPeriod").and_then(Value::as_i64),
            Some(*period),
            "case {region:?}: an update elsewhere left this region's period alone, got {read:?}"
        );
    }

    // And a delete in one region destroys only that region's record. A monitor
    // that still holds a probe in service is not deleted, so its probe is
    // switched off first.
    let probe_id = only_probe_id(&get_monitor_in(&router, updated_region, NAME).await.1);
    let (status, off) = call_in(
        &router,
        updated_region,
        "PATCH",
        &format!("/monitors/{NAME}/probes/{probe_id}"),
        Some(r#"{"state":"INACTIVE"}"#),
    )
    .await;
    assert_eq!(status, 200, "expected UpdateProbe 200, got {status} {off:?}");
    let (status, deleted) = call_in(
        &router,
        updated_region,
        "DELETE",
        &format!("/monitors/{NAME}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DeleteMonitor 200, got {status} {deleted:?}"
    );
    let (status, gone) = get_monitor_in(&router, updated_region, NAME).await;
    assert_eq!(
        status, 404,
        "case {updated_region:?}: the deleted monitor is gone, got {status} {gone:?}"
    );
    for (region, _) in &rows[1..] {
        let (status, read) = get_monitor_in(&router, region, NAME).await;
        assert_eq!(
            status, 200,
            "case {region:?}: a delete elsewhere left this region's monitor, got {status} {read:?}"
        );
    }
}

/// A `clientToken` replays only in the region it was sent to: the same token
/// sent to another region creates that region's own monitor rather than
/// replaying the first.
#[tokio::test]
async fn a_client_token_replays_only_in_the_region_it_was_sent_to() {
    const NAME: &str = "token-monitor";
    let router = router();

    for region in REGIONS {
        let body = format!(
            r#"{{"monitorName":"{NAME}","aggregationPeriod":30,"clientToken":"one-token"}}"#
        );
        let (status, created) = create_monitor_in(&router, region, &body).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected CreateMonitor 200, got {status} {created:?}"
        );
        assert_eq!(
            string_at(&created, "monitorArn"),
            monitor_arn_in(region, NAME),
            "case {region:?}: a token of another region replays nothing here, got {created:?}"
        );

        // In its OWN region the token still replays the monitor it minted.
        let (status, replayed) = create_monitor_in(&router, region, &body).await;
        assert_eq!(
            status, 200,
            "case {region:?}: a retry with the same token succeeds, got {status} {replayed:?}"
        );
        assert_eq!(
            string_at(&replayed, "monitorArn"),
            monitor_arn_in(region, NAME),
            "case {region:?}: the retry replays this region's monitor, got {replayed:?}"
        );
    }
}

/// A tagging call reaches only the region it was signed for: the ARN of another
/// region's monitor or probe names nothing this endpoint holds.
#[tokio::test]
async fn tagging_reaches_only_the_region_it_was_signed_for() {
    const NAME: &str = "tagged-monitor";

    for (home, other) in region_pairs() {
        let router = router();
        let (status, created) = create_monitor_in(&router, home, &create_body(home, NAME, 30)).await;
        assert_eq!(
            status, 200,
            "case {home:?}: expected CreateMonitor 200, got {status} {created:?}"
        );
        let (_, home_read_before) = get_monitor_in(&router, home, NAME).await;
        // `CreateMonitorOutput` models no `probes` member
        // (`smithy_query(query="operation",operation="CreateMonitor")`), so the
        // probe the create stored is read back through `GetMonitor`.
        let probe_arn = home_read_before
            .get("probes")
            .and_then(Value::as_array)
            .and_then(|probes| probes.first())
            .map(|probe| string_at(probe, "probeArn"))
            .unwrap_or_default();
        let monitor_arn = string_at(&created, "monitorArn");
        assert!(
            probe_arn.starts_with(&format!("arn:aws:networkmonitor:{home}:")),
            "case {home:?}: a probe's ARN names the region that created it, got {probe_arn:?}"
        );

        for (case, resource_arn) in [("monitor", &monitor_arn), ("probe", &probe_arn)] {
            let encoded = encode_label(resource_arn);
            let tagging: [(&str, &str, String, Option<&str>); 3] = [
                (
                    "TagResource",
                    "POST",
                    format!("/tags/{encoded}"),
                    Some(r#"{"tags":{"env":"prod"}}"#),
                ),
                (
                    "UntagResource",
                    "DELETE",
                    format!("/tags/{encoded}?tagKeys=home"),
                    None,
                ),
                (
                    "ListTagsForResource",
                    "GET",
                    format!("/tags/{encoded}"),
                    None,
                ),
            ];
            for (operation, method, path, body) in tagging {
                let (status, answered) = call_in(&router, other, method, &path, body).await;
                assert_eq!(
                    status, 404,
                    "case {home:?} -> {other:?} {operation} on a {case} ARN: expected HTTP 404, \
                     got {status} {answered:?}"
                );
                assert_eq!(
                    error_code(&answered),
                    TAG_NOT_FOUND_CODE,
                    "case {home:?} -> {other:?} {operation} on a {case} ARN: expected \
                     {TAG_NOT_FOUND_CODE}, got {answered:?}"
                );
            }

            // The same ARN, signed for its own region, reaches the resource.
            let (status, tags) = call_in(
                &router,
                home,
                "GET",
                &format!("/tags/{}", encode_label(resource_arn)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {home:?} {case}: expected ListTagsForResource 200, got {status} {tags:?}"
            );
        }

        // Nothing the refused tagging calls carried reached the stored monitor.
        let (_, read) = get_monitor_in(&router, home, NAME).await;
        assert_eq!(
            read.get("tags"),
            Some(&serde_json::json!({"home": home})),
            "case {home:?} -> {other:?}: cross-region tagging changed nothing, got {read:?}"
        );
    }
}
