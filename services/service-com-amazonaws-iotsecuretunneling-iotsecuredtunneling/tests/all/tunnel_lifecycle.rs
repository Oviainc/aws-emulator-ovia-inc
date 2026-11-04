// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tunnel lifecycle: `OpenTunnel`, `DescribeTunnel`,
//! `ListTunnels`, `RotateTunnelAccessToken`, `CloseTunnel` and the three tag
//! operations, driven over the wire through the `MultiServiceRouter` exactly as a
//! client reaches them (`awsJson1_1`, `POST /` with `X-Amz-Target`).
//!
//! Every case is a row in a table so each rule is asserted over a class of
//! inputs rather than over the one request the issue happened to name.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::{
    StoredTunnel, MAX_CONCURRENT_TUNNELS, MAX_TAGS_PER_TUNNEL,
};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for, so an ARN's region segment is observable and
/// is not a hardcoded default.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A router carrying only this service, over the engine the test also inspects.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `[{"key":..,"value":..}, ..]` as a map, so a test compares tags by content.
fn tag_map(tags: Option<&Value>) -> BTreeMap<String, String> {
    tags.and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|tag| {
                    (
                        tag["key"].as_str().unwrap_or_default().to_string(),
                        tag["value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `id` satisfies `TunnelId`'s modelled pattern
/// `^[a-zA-Z0-9_\-+=:]{1,128}$`.
fn is_modelled_tunnel_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-+=:".contains(c))
}

/// Whether `token` satisfies `NextToken`'s modelled pattern
/// `^[a-zA-Z0-9_=-]{1,4096}$`.
fn is_modelled_next_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_=-".contains(c))
}

/// One `OpenTunnel` request, its expected persisted shape, and a label.
struct OpenCase {
    label: &'static str,
    input: Value,
    expected_description: Option<&'static str>,
    expected_thing_name: Option<&'static str>,
    expected_services: Vec<&'static str>,
    /// The timeout the tunnel must end up with: the supplied value, or the
    /// modelled default of 720 when the request names none.
    expected_timeout: i64,
    expected_tags: Vec<(&'static str, &'static str)>,
}

/// The table every `OpenTunnel`/`DescribeTunnel` assertion runs over. The rows
/// differ along what the rules generalise over: a present vs absent
/// `destinationConfig`, one vs several services, each published bound of
/// `maxLifetimeTimeoutMinutes` (1 and 720, from the model's `range`) plus an
/// interior value and an absent one, and zero vs several tags.
fn open_cases() -> Vec<OpenCase> {
    vec![
        OpenCase {
            label: "fully specified",
            input: json!({
                "description": "demo",
                "destinationConfig": {"thingName": "thing-1", "services": ["SSH"]},
                "timeoutConfig": {"maxLifetimeTimeoutMinutes": 60},
                "tags": [{"key": "env", "value": "test"}],
            }),
            expected_description: Some("demo"),
            expected_thing_name: Some("thing-1"),
            expected_services: vec!["SSH"],
            expected_timeout: 60,
            expected_tags: vec![("env", "test")],
        },
        OpenCase {
            label: "no timeoutConfig takes the modelled default",
            input: json!({
                "description": "no timeout",
                "destinationConfig": {"thingName": "thing-2", "services": ["SSH", "HTTP"]},
            }),
            expected_description: Some("no timeout"),
            expected_thing_name: Some("thing-2"),
            expected_services: vec!["SSH", "HTTP"],
            expected_timeout: 720,
            expected_tags: vec![],
        },
        OpenCase {
            label: "minimum published lifetime bound",
            input: json!({
                "destinationConfig": {"thingName": "thing-3", "services": ["VNC"]},
                "timeoutConfig": {"maxLifetimeTimeoutMinutes": 1},
            }),
            expected_description: None,
            expected_thing_name: Some("thing-3"),
            expected_services: vec!["VNC"],
            expected_timeout: 1,
            expected_tags: vec![],
        },
        OpenCase {
            label: "maximum published lifetime bound",
            input: json!({
                "description": "twelve hours",
                "destinationConfig": {
                    "thingName": "thing-4",
                    "services": ["SSH", "HTTP", "VNC"],
                },
                "timeoutConfig": {"maxLifetimeTimeoutMinutes": 720},
                "tags": [
                    {"key": "env", "value": "prod"},
                    {"key": "owner", "value": "alice"},
                ],
            }),
            expected_description: Some("twelve hours"),
            expected_thing_name: Some("thing-4"),
            expected_services: vec!["SSH", "HTTP", "VNC"],
            expected_timeout: 720,
            expected_tags: vec![("env", "prod"), ("owner", "alice")],
        },
        OpenCase {
            label: "no destinationConfig at all",
            input: json!({"description": "unbound"}),
            expected_description: Some("unbound"),
            expected_thing_name: None,
            expected_services: vec![],
            expected_timeout: 720,
            expected_tags: vec![],
        },
        OpenCase {
            label: "destinationConfig without a thingName",
            input: json!({"destinationConfig": {"services": ["SSH"]}}),
            expected_description: None,
            expected_thing_name: None,
            expected_services: vec!["SSH"],
            expected_timeout: 720,
            expected_tags: vec![],
        },
    ]
}

#[tokio::test]
async fn open_tunnel_mints_a_tunnel_that_describe_tunnel_reads_back() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let mut minted: Vec<String> = Vec::new();
    let mut minted_tokens: Vec<String> = Vec::new();

    for case in open_cases() {
        let label = case.label;
        let (status, opened) = call(&router, "OpenTunnel", &case.input).await;
        assert_eq!(status, 200, "case {label:?}: OpenTunnel answered {opened:?}");

        let tunnel_id = opened["tunnelId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let tunnel_arn = opened["tunnelArn"].as_str().unwrap_or_default().to_string();
        let source = opened["sourceAccessToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let destination = opened["destinationAccessToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert!(
            is_modelled_tunnel_id(&tunnel_id),
            "case {label:?}: tunnelId must satisfy the modelled pattern, got {tunnel_id:?}"
        );
        assert_eq!(
            tunnel_arn,
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/{tunnel_id}"),
            "case {label:?}: tunnelArn must name the minted tunnel in the signed region"
        );
        assert!(
            !source.is_empty() && !destination.is_empty() && source != destination,
            "case {label:?}: expected two distinct non-empty access tokens, \
             got {source:?} and {destination:?}"
        );
        assert!(
            !minted.contains(&tunnel_id),
            "case {label:?}: tunnelId {tunnel_id:?} was already minted for another tunnel"
        );
        for token in [&source, &destination] {
            assert!(
                !minted_tokens.contains(token),
                "case {label:?}: access token {token:?} was already handed out"
            );
        }
        minted.push(tunnel_id.clone());
        minted_tokens.push(source.clone());
        minted_tokens.push(destination.clone());

        // The response must come from the store, so read the tunnel back.
        let (status, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeTunnel answered {described:?}"
        );
        let tunnel = &described["tunnel"];
        assert_eq!(
            tunnel["tunnelId"].as_str(),
            Some(tunnel_id.as_str()),
            "case {label:?}: DescribeTunnel reported {:?}",
            tunnel["tunnelId"]
        );
        assert_eq!(
            tunnel["tunnelArn"].as_str(),
            Some(tunnel_arn.as_str()),
            "case {label:?}: DescribeTunnel reported {:?}",
            tunnel["tunnelArn"]
        );
        assert_eq!(
            tunnel["status"].as_str(),
            Some("OPEN"),
            "case {label:?}: a freshly opened tunnel is OPEN, got {:?}",
            tunnel["status"]
        );
        assert_eq!(
            tunnel["description"].as_str(),
            case.expected_description,
            "case {label:?}: description was persisted as {:?}",
            tunnel["description"]
        );
        assert_eq!(
            tunnel["destinationConfig"]["thingName"].as_str(),
            case.expected_thing_name,
            "case {label:?}: destinationConfig.thingName was persisted as {:?}",
            tunnel["destinationConfig"]["thingName"]
        );
        let services: Vec<&str> = tunnel["destinationConfig"]["services"]
            .as_array()
            .map(|entries| entries.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            services, case.expected_services,
            "case {label:?}: destinationConfig.services was persisted as {services:?}"
        );
        assert_eq!(
            tunnel["timeoutConfig"]["maxLifetimeTimeoutMinutes"].as_i64(),
            Some(case.expected_timeout),
            "case {label:?}: maxLifetimeTimeoutMinutes was persisted as {:?}",
            tunnel["timeoutConfig"]["maxLifetimeTimeoutMinutes"]
        );
        for end in ["sourceConnectionState", "destinationConnectionState"] {
            assert_eq!(
                tunnel[end]["status"].as_str(),
                Some("DISCONNECTED"),
                "case {label:?}: {end} must be DISCONNECTED until a proxy connects, got {:?}",
                tunnel[end]["status"]
            );
            assert!(
                tunnel[end]["lastUpdatedAt"].as_f64().is_some(),
                "case {label:?}: {end}.lastUpdatedAt must be set, got {:?}",
                tunnel[end]["lastUpdatedAt"]
            );
        }
        let created_at = tunnel["createdAt"].as_f64();
        let last_updated_at = tunnel["lastUpdatedAt"].as_f64();
        assert!(
            created_at.is_some() && last_updated_at >= created_at,
            "case {label:?}: expected createdAt set and lastUpdatedAt no earlier, \
             got {created_at:?} and {last_updated_at:?}"
        );
        let expected_tags: BTreeMap<String, String> = case
            .expected_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tag_map(tunnel.get("tags")),
            expected_tags,
            "case {label:?}: DescribeTunnel reported tags {:?}",
            tunnel.get("tags")
        );
        // The real API does not return access tokens from DescribeTunnel.
        for field in ["sourceAccessToken", "destinationAccessToken"] {
            assert!(
                tunnel.get(field).is_none(),
                "case {label:?}: DescribeTunnel must not echo {field}, got {:?}",
                tunnel.get(field)
            );
        }
    }

    // And the store holds exactly the tunnels that were opened.
    let stored: Vec<String> = engine
        .state::<TunnelState>()
        .lock()
        .tunnels
        .keys()
        .map(|(_, tunnel_id)| tunnel_id.clone())
        .collect();
    assert_eq!(
        stored.len(),
        minted.len(),
        "expected {} persisted tunnels, store holds {stored:?}",
        minted.len()
    );
}

#[tokio::test]
async fn list_tunnels_filters_by_thing_name_and_reports_newest_first() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // Five tunnels across three things, opened oldest-first.
    let plan = [
        ("thing-a", "first"),
        ("thing-b", "second"),
        ("thing-a", "third"),
        ("thing-c", "fourth"),
        ("thing-b", "fifth"),
    ];
    let mut ids: Vec<String> = Vec::new();
    for (thing_name, description) in plan {
        let (status, opened) = call(
            &router,
            "OpenTunnel",
            &json!({
                "description": description,
                "destinationConfig": {"thingName": thing_name, "services": ["SSH"]},
            }),
        )
        .await;
        assert_eq!(status, 200, "opening {description:?} answered {opened:?}");
        ids.push(opened["tunnelId"].as_str().unwrap_or_default().to_string());
    }
    // Newest first, which is the reverse of the order they were opened in.
    let newest_first: Vec<String> = ids.iter().rev().cloned().collect();

    // thingName is a filter over the stored destinationConfig.thingName.
    let filter_cases: Vec<(&str, Vec<String>)> = vec![
        ("thing-a", vec![ids[2].clone(), ids[0].clone()]),
        ("thing-b", vec![ids[4].clone(), ids[1].clone()]),
        ("thing-c", vec![ids[3].clone()]),
        ("thing-unknown", vec![]),
    ];
    for (thing_name, expected) in filter_cases {
        let (status, listed) =
            call(&router, "ListTunnels", &json!({"thingName": thing_name})).await;
        assert_eq!(
            status, 200,
            "case {thing_name:?}: ListTunnels answered {listed:?}"
        );
        let reported: Vec<String> = listed["tunnelSummaries"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|summary| summary["tunnelId"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported, expected,
            "case {thing_name:?}: expected {expected:?} newest-first, got {reported:?}"
        );
        assert!(
            listed.get("nextToken").is_none(),
            "case {thing_name:?}: a single page must omit nextToken, got {:?}",
            listed.get("nextToken")
        );
    }

    // An unfiltered listing reports every tunnel, newest first.
    let (_, listed) = call(&router, "ListTunnels", &json!({})).await;
    let reported: Vec<String> = listed["tunnelSummaries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|summary| summary["tunnelId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported, newest_first,
        "an unfiltered listing must report every tunnel newest-first, got {reported:?}"
    );
    // Each summary is computed from the stored tunnel.
    for summary in listed["tunnelSummaries"].as_array().unwrap_or(&vec![]) {
        let id = summary["tunnelId"].as_str().unwrap_or_default().to_string();
        let stored = engine
            .state::<TunnelState>()
            .lock()
            .tunnel(REGION, &id)
            .cloned();
        let stored = stored.unwrap_or_else(|| panic!("summary {id:?} names no stored tunnel"));
        assert_eq!(
            summary["tunnelArn"].as_str(),
            Some(stored.tunnel_arn.as_str()),
            "summary {id:?} reported arn {:?}",
            summary["tunnelArn"]
        );
        assert_eq!(
            summary["status"].as_str(),
            Some("OPEN"),
            "summary {id:?} reported status {:?}",
            summary["status"]
        );
        assert_eq!(
            summary["description"].as_str(),
            stored.description.as_deref(),
            "summary {id:?} reported description {:?}",
            summary["description"]
        );
    }

    // Paging: every page size covers the listing exactly once, in order.
    for page_size in 1..=5_i64 {
        let mut collected: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut input = json!({"maxResults": page_size});
            if let Some(token) = next.as_ref() {
                input["nextToken"] = Value::String(token.clone());
            }
            let (status, listed) = call(&router, "ListTunnels", &input).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: ListTunnels answered {listed:?}"
            );
            let page: Vec<String> = listed["tunnelSummaries"]
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .map(|summary| summary["tunnelId"].as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "page size {page_size}: a page carried {} summaries",
                page.len()
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= 10,
                "page size {page_size}: paging did not terminate after {pages} pages"
            );
            match listed.get("nextToken").and_then(Value::as_str) {
                Some(token) => {
                    assert!(
                        is_modelled_next_token(token),
                        "page size {page_size}: nextToken {token:?} must match the modelled pattern"
                    );
                    next = Some(token.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            collected, newest_first,
            "page size {page_size}: paging must cover the listing once, in order, got {collected:?}"
        );
    }
}

#[tokio::test]
async fn list_tunnels_refuses_a_next_token_it_did_not_issue() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for token in ["not-a-token", "tnl-zzzz", "AAAA"] {
        let (status, body) = call(&router, "ListTunnels", &json!({"nextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {token:?}: a foreign nextToken must be refused, answered {body:?}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {token:?}: answered {body:?}"
        );
    }
}

#[tokio::test]
async fn rotate_tunnel_access_token_replaces_only_the_selected_tokens() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // One row per modelled `ClientMode` value: (mode, source rotates, destination rotates).
    let cases = [
        ("SOURCE", true, false),
        ("DESTINATION", false, true),
        ("ALL", true, true),
    ];

    for (mode, rotates_source, rotates_destination) in cases {
        let (status, opened) = call(
            &router,
            "OpenTunnel",
            &json!({
                "description": "rotate",
                "destinationConfig": {"thingName": "thing-rotate", "services": ["SSH"]},
            }),
        )
        .await;
        assert_eq!(status, 200, "case {mode:?}: OpenTunnel answered {opened:?}");
        let tunnel_id = opened["tunnelId"].as_str().unwrap_or_default().to_string();
        let before = engine
            .state::<TunnelState>()
            .lock()
            .tunnel(REGION, &tunnel_id)
            .cloned()
            .unwrap_or_else(|| panic!("case {mode:?}: the opened tunnel was not persisted"));

        // `destinationConfig` configures the destination end, so it only rides
        // along on a rotation that covers that end; see
        // `tunnel_refusal_rules.rs` for the refusal when it does not.
        let mut rotate_input = json!({"tunnelId": tunnel_id, "clientMode": mode});
        if rotates_destination {
            rotate_input["destinationConfig"] =
                json!({"thingName": "thing-rotate", "services": ["SSH", "HTTP"]});
        }
        let (status, rotated) = call(&router, "RotateTunnelAccessToken", &rotate_input).await;
        assert_eq!(
            status, 200,
            "case {mode:?}: RotateTunnelAccessToken answered {rotated:?}"
        );
        assert_eq!(
            rotated["tunnelArn"].as_str(),
            Some(before.tunnel_arn.as_str()),
            "case {mode:?}: rotate reported arn {:?}",
            rotated["tunnelArn"]
        );
        assert_eq!(
            rotated.get("sourceAccessToken").is_some(),
            rotates_source,
            "case {mode:?}: sourceAccessToken presence was {:?}",
            rotated.get("sourceAccessToken")
        );
        assert_eq!(
            rotated.get("destinationAccessToken").is_some(),
            rotates_destination,
            "case {mode:?}: destinationAccessToken presence was {:?}",
            rotated.get("destinationAccessToken")
        );

        let after = engine
            .state::<TunnelState>()
            .lock()
            .tunnel(REGION, &tunnel_id)
            .cloned()
            .unwrap_or_else(|| panic!("case {mode:?}: the tunnel vanished on rotate"));

        // A rotated token is replaced in the store (the old one no longer names
        // the tunnel) and is the one the response returned; an unrotated one is
        // left exactly as it was.
        if rotates_source {
            assert_ne!(
                after.source_access_token, before.source_access_token,
                "case {mode:?}: the stored source token was not replaced"
            );
            assert_eq!(
                rotated["sourceAccessToken"].as_str(),
                Some(after.source_access_token.as_str()),
                "case {mode:?}: the returned source token is not the stored one"
            );
        } else {
            assert_eq!(
                after.source_access_token, before.source_access_token,
                "case {mode:?}: the source token must not rotate"
            );
        }
        if rotates_destination {
            assert_ne!(
                after.destination_access_token, before.destination_access_token,
                "case {mode:?}: the stored destination token was not replaced"
            );
            assert_eq!(
                rotated["destinationAccessToken"].as_str(),
                Some(after.destination_access_token.as_str()),
                "case {mode:?}: the returned destination token is not the stored one"
            );
        } else {
            assert_eq!(
                after.destination_access_token, before.destination_access_token,
                "case {mode:?}: the destination token must not rotate"
            );
        }
        assert!(
            after.last_updated_at > before.last_updated_at,
            "case {mode:?}: rotate must bump lastUpdatedAt"
        );
        // Rotating does not extend the tunnel's lifetime.
        assert_eq!(
            after.expires_at, before.expires_at,
            "case {mode:?}: rotate must not extend the tunnel duration"
        );

        // A supplied destinationConfig replaced the stored one; a rotation that
        // sent none left the one `OpenTunnel` stored.
        let (_, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
        let services: Vec<&str> = described["tunnel"]["destinationConfig"]["services"]
            .as_array()
            .map(|entries| entries.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let expected_services: Vec<&str> = if rotates_destination {
            vec!["SSH", "HTTP"]
        } else {
            vec!["SSH"]
        };
        assert_eq!(
            services, expected_services,
            "case {mode:?}: rotate must apply the supplied destinationConfig, got {services:?}"
        );
    }

    // A clientMode outside the modelled enum is a bad request, not a 501.
    let (status, body) = call(
        &router,
        "RotateTunnelAccessToken",
        &json!({"tunnelId": "whatever", "clientMode": "SIDEWAYS"}),
    )
    .await;
    assert!(
        status == 400,
        "an unmodelled clientMode must be refused, answered {status} {body:?}"
    );
}

#[tokio::test]
async fn close_tunnel_closes_the_tunnel_and_delete_removes_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // (label, the `delete` member as sent, whether the record must survive)
    let cases: [(&str, Option<bool>, bool); 3] = [
        ("delete omitted", None, true),
        ("delete=false", Some(false), true),
        ("delete=true", Some(true), false),
    ];

    for (label, delete, survives) in cases {
        let (status, opened) = call(
            &router,
            "OpenTunnel",
            &json!({
                "description": label,
                "destinationConfig": {"thingName": "thing-close", "services": ["SSH"]},
            }),
        )
        .await;
        assert_eq!(status, 200, "case {label:?}: OpenTunnel answered {opened:?}");
        let tunnel_id = opened["tunnelId"].as_str().unwrap_or_default().to_string();

        let mut input = json!({"tunnelId": tunnel_id});
        if let Some(delete) = delete {
            input["delete"] = Value::Bool(delete);
        }
        let (status, closed) = call(&router, "CloseTunnel", &input).await;
        assert_eq!(status, 200, "case {label:?}: CloseTunnel answered {closed:?}");

        let (status, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
        if survives {
            assert_eq!(
                status, 200,
                "case {label:?}: a closed tunnel stays readable, answered {described:?}"
            );
            assert_eq!(
                described["tunnel"]["status"].as_str(),
                Some("CLOSED"),
                "case {label:?}: status after close was {:?}",
                described["tunnel"]["status"]
            );
            let created_at = described["tunnel"]["createdAt"].as_f64();
            let last_updated_at = described["tunnel"]["lastUpdatedAt"].as_f64();
            assert!(
                last_updated_at >= created_at,
                "case {label:?}: close must stamp lastUpdatedAt, got {last_updated_at:?}"
            );
            let (_, listed) = call(&router, "ListTunnels", &json!({})).await;
            let found = listed["tunnelSummaries"]
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .any(|summary| summary["tunnelId"].as_str() == Some(tunnel_id.as_str()))
                })
                .unwrap_or(false);
            assert!(
                found,
                "case {label:?}: a closed tunnel stays listed, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: a deleted tunnel must be gone, answered {described:?}"
            );
            assert_eq!(
                described["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {label:?}: answered {described:?}"
            );
            let (_, listed) = call(&router, "ListTunnels", &json!({})).await;
            let found = listed["tunnelSummaries"]
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .any(|summary| summary["tunnelId"].as_str() == Some(tunnel_id.as_str()))
                })
                .unwrap_or(false);
            assert!(
                !found,
                "case {label:?}: a deleted tunnel must not be listed, got {listed:?}"
            );
            assert!(
                engine
                    .state::<TunnelState>()
                    .lock()
                    .tunnel(REGION, &tunnel_id)
                    .is_none(),
                "case {label:?}: the record must be removed from the store"
            );
        }
    }
}

#[tokio::test]
async fn tags_are_keyed_by_tunnel_arn_across_every_tag_operation() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let (status, opened) = call(
        &router,
        "OpenTunnel",
        &json!({
            "description": "tagged",
            "destinationConfig": {"thingName": "thing-tag", "services": ["SSH"]},
            "tags": [{"key": "env", "value": "test"}],
        }),
    )
    .await;
    assert_eq!(status, 200, "OpenTunnel answered {opened:?}");
    let tunnel_id = opened["tunnelId"].as_str().unwrap_or_default().to_string();
    let tunnel_arn = opened["tunnelArn"].as_str().unwrap_or_default().to_string();

    // One mutation (or read) of the tag store, and the tags that must be stored
    // once it has run.
    struct TagStep {
        label: &'static str,
        operation: &'static str,
        input: Value,
        expected: Vec<(&'static str, &'static str)>,
    }

    let steps: Vec<TagStep> = vec![
        TagStep {
            label: "tags set at open time",
            operation: "ListTagsForResource",
            input: json!({"resourceArn": tunnel_arn}),
            expected: vec![("env", "test")],
        },
        TagStep {
            label: "a new key is added",
            operation: "TagResource",
            input: json!({
                "resourceArn": tunnel_arn,
                "tags": [{"key": "owner", "value": "alice"}],
            }),
            expected: vec![("env", "test"), ("owner", "alice")],
        },
        TagStep {
            label: "an existing key is overwritten",
            operation: "TagResource",
            input: json!({
                "resourceArn": tunnel_arn,
                "tags": [{"key": "env", "value": "prod"}],
            }),
            expected: vec![("env", "prod"), ("owner", "alice")],
        },
        TagStep {
            label: "several keys at once",
            operation: "TagResource",
            input: json!({
                "resourceArn": tunnel_arn,
                "tags": [
                    {"key": "team", "value": "iot"},
                    {"key": "owner", "value": "bob"},
                ],
            }),
            expected: vec![("env", "prod"), ("owner", "bob"), ("team", "iot")],
        },
        TagStep {
            label: "one key is removed",
            operation: "UntagResource",
            input: json!({"resourceArn": tunnel_arn, "tagKeys": ["env"]}),
            expected: vec![("owner", "bob"), ("team", "iot")],
        },
        TagStep {
            label: "removing an unset key changes nothing",
            operation: "UntagResource",
            input: json!({"resourceArn": tunnel_arn, "tagKeys": ["absent"]}),
            expected: vec![("owner", "bob"), ("team", "iot")],
        },
        TagStep {
            label: "every key is removed",
            operation: "UntagResource",
            input: json!({"resourceArn": tunnel_arn, "tagKeys": ["owner", "team"]}),
            expected: vec![],
        },
    ];

    for step in steps {
        let TagStep {
            label,
            operation,
            input,
            expected,
        } = step;
        let (status, body) = call(&router, operation, &input).await;
        assert_eq!(
            status, 200,
            "case {label:?}: {operation} answered {body:?}"
        );
        let expected: BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        let (status, listed) = call(
            &router,
            "ListTagsForResource",
            &json!({"resourceArn": tunnel_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListTagsForResource answered {listed:?}"
        );
        assert_eq!(
            tag_map(listed.get("tags")),
            expected,
            "case {label:?}: ListTagsForResource reported {:?}",
            listed.get("tags")
        );
        if expected.is_empty() {
            assert!(
                listed.get("tags").is_none(),
                "case {label:?}: TagList is length(min: 1), so no tags means no member, got {:?}",
                listed.get("tags")
            );
        }

        // The same tags are what DescribeTunnel reports on the tunnel.
        let (_, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
        assert_eq!(
            tag_map(described["tunnel"].get("tags")),
            expected,
            "case {label:?}: DescribeTunnel reported tags {:?}",
            described["tunnel"].get("tags")
        );
    }
}

#[tokio::test]
async fn an_unknown_tunnel_or_arn_answers_resource_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // A real tunnel exists, so the refusals below are about the id or ARN named,
    // not about an empty store.
    let (_, opened) = call(
        &router,
        "OpenTunnel",
        &json!({"destinationConfig": {"thingName": "thing-missing", "services": ["SSH"]}}),
    )
    .await;
    assert!(
        opened["tunnelId"].as_str().is_some(),
        "OpenTunnel answered {opened:?}"
    );

    let unknown_id = "0f1e2d3c-4b5a-4987-8765-432112345678";
    let unknown_arn = format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/{unknown_id}");
    let cases: Vec<(&str, Value)> = vec![
        ("DescribeTunnel", json!({"tunnelId": unknown_id})),
        ("CloseTunnel", json!({"tunnelId": unknown_id})),
        (
            "CloseTunnel",
            json!({"tunnelId": unknown_id, "delete": true}),
        ),
        (
            "RotateTunnelAccessToken",
            json!({"tunnelId": unknown_id, "clientMode": "ALL"}),
        ),
        (
            "TagResource",
            json!({
                "resourceArn": unknown_arn,
                "tags": [{"key": "env", "value": "test"}],
            }),
        ),
        (
            "UntagResource",
            json!({"resourceArn": unknown_arn, "tagKeys": ["env"]}),
        ),
        (
            "ListTagsForResource",
            json!({"resourceArn": unknown_arn}),
        ),
    ];

    for (operation, input) in cases {
        let (status, body) = call(&router, operation, &input).await;
        assert_eq!(
            status, 400,
            "case {operation} {input}: expected 400, answered {status} {body:?}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {operation} {input}: answered {body:?}"
        );
    }
}

#[tokio::test]
async fn a_tunnel_closes_once_its_max_lifetime_has_elapsed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // Each published bound of `maxLifetimeTimeoutMinutes` plus an interior value
    // and the default: the tunnel is open while the lifetime has not run out and
    // closed once it has, whatever the configured value.
    for minutes in [1_i64, 60, 720] {
        let (status, opened) = call(
            &router,
            "OpenTunnel",
            &json!({
                "description": "expiring",
                "destinationConfig": {"thingName": "thing-expire", "services": ["SSH"]},
                "timeoutConfig": {"maxLifetimeTimeoutMinutes": minutes},
            }),
        )
        .await;
        assert_eq!(status, 200, "case {minutes}: OpenTunnel answered {opened:?}");
        let tunnel_id = opened["tunnelId"].as_str().unwrap_or_default().to_string();

        // Still inside its lifetime: open.
        let (_, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
        assert_eq!(
            described["tunnel"]["status"].as_str(),
            Some("OPEN"),
            "case {minutes}: a tunnel inside its lifetime is OPEN, got {:?}",
            described["tunnel"]["status"]
        );

        // Move the tunnel's whole timeline into the past, so its lifetime ran
        // out a second ago, exactly as the clock would have done on its own.
        let shift = Duration::from_secs(u64::try_from(minutes).unwrap_or(1) * 60 + 1);
        let expires_at = {
            let state = engine.state::<TunnelState>();
            let mut store = state.lock();
            let tunnel = store
                .tunnel_mut(REGION, &tunnel_id)
                .unwrap_or_else(|| panic!("case {minutes}: the opened tunnel was not persisted"));
            tunnel.created_at -= shift;
            tunnel.last_updated_at -= shift;
            tunnel.expires_at -= shift;
            let expires_at = tunnel.expires_at;
            // Keep the store's "soonest expiry" index consistent with the
            // rewound record, exactly as `OpenTunnel` does when it stores one.
            store.note_open_expiry(expires_at);
            expires_at
        };

        let (status, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
        assert_eq!(
            status, 200,
            "case {minutes}: DescribeTunnel answered {described:?}"
        );
        assert_eq!(
            described["tunnel"]["status"].as_str(),
            Some("CLOSED"),
            "case {minutes}: a tunnel past its lifetime is CLOSED, got {:?}",
            described["tunnel"]["status"]
        );
        let stored_last_updated = engine
            .state::<TunnelState>()
            .lock()
            .tunnel(REGION, &tunnel_id)
            .map(|tunnel| tunnel.last_updated_at);
        assert_eq!(
            stored_last_updated,
            Some(expires_at),
            "case {minutes}: lastUpdatedAt must be the instant the close came due"
        );

        // And the listing agrees, because both read the same store.
        let (_, listed) = call(&router, "ListTunnels", &json!({})).await;
        let status_in_listing = listed["tunnelSummaries"]
            .as_array()
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|summary| summary["tunnelId"].as_str() == Some(tunnel_id.as_str()))
            })
            .and_then(|summary| summary["status"].as_str())
            .map(str::to_string);
        assert_eq!(
            status_in_listing,
            Some("CLOSED".to_string()),
            "case {minutes}: ListTunnels reported {status_in_listing:?}"
        );
    }
}

#[tokio::test]
async fn open_tunnel_refuses_once_the_account_holds_its_quota_of_open_tunnels() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // One real tunnel, then the account filled to its concurrency quota with OPEN
    // copies of it, so the refusal below is about the OPEN count and nothing else.
    let (status, opened) = call(
        &router,
        "OpenTunnel",
        &json!({"destinationConfig": {"thingName": "thing-limit", "services": ["SSH"]}}),
    )
    .await;
    assert_eq!(status, 200, "OpenTunnel answered {opened:?}");
    let seed_id = opened["tunnelId"].as_str().unwrap_or_default().to_string();

    {
        let state = engine.state::<TunnelState>();
        let mut store = state.lock();
        let seed: StoredTunnel = store
            .tunnel(REGION, &seed_id)
            .cloned()
            .expect("the opened tunnel was not persisted");
        let mut filler = 0_usize;
        while store.open_tunnel_count(REGION) < MAX_CONCURRENT_TUNNELS {
            let tunnel_id = format!("filler-{filler}");
            let tunnel_arn = format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/{tunnel_id}");
            let mut tunnel = seed.clone();
            tunnel.tunnel_id = tunnel_id.clone();
            tunnel.tunnel_arn = tunnel_arn.clone();
            store.insert_tunnel(REGION, tunnel);
            filler += 1;
        }
    }

    // The limit is the modelled LimitExceededException.
    let stored_before = engine.state::<TunnelState>().lock().tunnels.len();
    let (status, refused) = call(
        &router,
        "OpenTunnel",
        &json!({"destinationConfig": {"thingName": "thing-limit", "services": ["SSH"]}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "an account at its open-tunnel quota must refuse OpenTunnel, answered {status} {refused:?}"
    );
    assert_eq!(
        refused["__type"].as_str(),
        Some("LimitExceededException"),
        "an account at its open-tunnel quota answered {refused:?}"
    );
    for field in ["tunnelId", "sourceAccessToken", "destinationAccessToken"] {
        assert!(
            refused.get(field).is_none(),
            "a refused open must mint no {field}, got {:?}",
            refused.get(field)
        );
    }
    assert_eq!(
        engine.state::<TunnelState>().lock().tunnels.len(),
        stored_before,
        "a refused open must not store a record"
    );

    // A plain close — `delete` absent, so the record stays describable — frees a
    // slot, because the quota counts tunnels that are concurrently OPEN.
    let (status, closed) = call(&router, "CloseTunnel", &json!({"tunnelId": seed_id})).await;
    assert_eq!(status, 200, "CloseTunnel answered {closed:?}");
    let (status, described) = call(&router, "DescribeTunnel", &json!({"tunnelId": seed_id})).await;
    assert_eq!(status, 200, "DescribeTunnel answered {described:?}");
    assert_eq!(
        described["tunnel"]["status"].as_str(),
        Some("CLOSED"),
        "the closed record must stay readable, answered {described:?}"
    );
    let (status, reopened) = call(
        &router,
        "OpenTunnel",
        &json!({"destinationConfig": {"thingName": "thing-limit", "services": ["SSH"]}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "an open must succeed once a slot was released, answered {reopened:?}"
    );
    assert!(
        reopened["tunnelId"].as_str().is_some(),
        "the reopened tunnel answered {reopened:?}"
    );
}

#[tokio::test]
async fn a_tunnel_carries_at_most_its_limit_of_tags() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // `k0..kN` as a tag list, so a case can name any size.
    let tags_of = |count: usize, prefix: &str| -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({"key": format!("{prefix}{index}"), "value": "v"}))
                .collect(),
        )
    };

    // OpenTunnel: the limit itself is accepted, one past it is refused and
    // stores nothing. Rows differ in tag count around the published bound.
    let open_cases: [(usize, bool); 3] = [
        (1, true),
        (MAX_TAGS_PER_TUNNEL, true),
        (MAX_TAGS_PER_TUNNEL + 1, false),
    ];
    for (count, accepted) in open_cases {
        let before = engine.state::<TunnelState>().lock().tunnels.len();
        let (status, body) = call(
            &router,
            "OpenTunnel",
            &json!({
                "destinationConfig": {"thingName": "thing-tags", "services": ["SSH"]},
                "tags": tags_of(count, "k"),
            }),
        )
        .await;
        let after = engine.state::<TunnelState>().lock().tunnels.len();
        if accepted {
            assert_eq!(
                status, 200,
                "case {count} tags at open: expected acceptance, answered {status} {body:?}"
            );
            let arn = body["tunnelArn"].as_str().unwrap_or_default().to_string();
            let (_, listed) =
                call(&router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
            assert_eq!(
                tag_map(listed.get("tags")).len(),
                count,
                "case {count} tags at open: stored {:?}",
                listed.get("tags")
            );
        } else {
            assert_eq!(
                status, 400,
                "case {count} tags at open: expected refusal, answered {status} {body:?}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("LimitExceededException"),
                "case {count} tags at open: answered {body:?}"
            );
            assert_eq!(
                after, before,
                "case {count} tags at open: a refused open must store no tunnel"
            );
        }
    }

    // TagResource: the limit is on the ACCUMULATED key set, so a tunnel already
    // at the limit refuses a new key, still accepts an overwrite of one it has,
    // and the refusal leaves the store untouched.
    let (status, opened) = call(
        &router,
        "OpenTunnel",
        &json!({
            "destinationConfig": {"thingName": "thing-tags", "services": ["SSH"]},
            "tags": tags_of(MAX_TAGS_PER_TUNNEL, "full"),
        }),
    )
    .await;
    assert_eq!(status, 200, "OpenTunnel answered {opened:?}");
    let tunnel_arn = opened["tunnelArn"].as_str().unwrap_or_default().to_string();

    let (status, refused) = call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": tunnel_arn,
            "tags": [{"key": "one-too-many", "value": "v"}],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a new key past the limit must be refused, answered {status} {refused:?}"
    );
    assert_eq!(
        refused["__type"].as_str(),
        Some("LimitExceededException"),
        "a new key past the limit answered {refused:?}"
    );
    let (_, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({"resourceArn": tunnel_arn}),
    )
    .await;
    let stored = tag_map(listed.get("tags"));
    assert_eq!(
        stored.len(),
        MAX_TAGS_PER_TUNNEL,
        "a refused TagResource must leave the stored tags alone, got {stored:?}"
    );
    assert!(
        !stored.contains_key("one-too-many"),
        "a refused TagResource must store none of its tags, got {stored:?}"
    );

    let (status, overwritten) = call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": tunnel_arn,
            "tags": [{"key": "full0", "value": "replaced"}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "overwriting a key a full resource already has must succeed, answered {overwritten:?}"
    );
    let (_, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({"resourceArn": tunnel_arn}),
    )
    .await;
    let stored = tag_map(listed.get("tags"));
    assert_eq!(
        stored.get("full0").map(String::as_str),
        Some("replaced"),
        "the overwrite was not stored, got {stored:?}"
    );
    assert_eq!(
        stored.len(),
        MAX_TAGS_PER_TUNNEL,
        "an overwrite must not change the key count, got {stored:?}"
    );

    // And once a key is removed there is room for a new one again.
    let (status, untagged) = call(
        &router,
        "UntagResource",
        &json!({"resourceArn": tunnel_arn, "tagKeys": ["full0"]}),
    )
    .await;
    assert_eq!(status, 200, "UntagResource answered {untagged:?}");
    let (status, tagged) = call(
        &router,
        "TagResource",
        &json!({
            "resourceArn": tunnel_arn,
            "tags": [{"key": "one-too-many", "value": "v"}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a freed slot must accept a new key, answered {status} {tagged:?}"
    );
}
