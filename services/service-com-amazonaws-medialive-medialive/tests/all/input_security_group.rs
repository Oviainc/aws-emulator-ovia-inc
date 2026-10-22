// SPDX-License-Identifier: CC0-1.0

//! The MediaLive input security group lifecycle, end to end over the restJson1
//! wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::TRANSIENT_STATE_SPAN;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming `medialive`, which is how the router decides
/// this service claims the request, and `us-west-2`, which is the region every
/// ARN the request mints must name.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20240101/us-west-2/medialive/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account segment of every ARN a generated emulator emits
/// (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// Long enough for a transient channel state to have come due, with margin.
fn wait_for_the_clock() {
    std::thread::sleep(TRANSIENT_STATE_SPAN + Duration::from_millis(200));
}

fn medialive() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // This suite exercises input security group behaviour, not request signing.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_medialive_medialive::EmulatedService::new(engine),
    );
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "medialive.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");

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

/// `CreateInputSecurityGroup`, asserting the modelled 200 and returning the
/// `SecurityGroup` it reports.
async fn create_group(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/prod/inputSecurityGroups", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateInputSecurityGroup: expected 200, got {status}: {response}"
    );
    response["securityGroup"].clone()
}

async fn describe_group(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/prod/inputSecurityGroups/{id}"), None).await
}

/// The error code a restJson1 error response carries, as a client reads it.
fn error_code(status: u16, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map_or_else(|| format!("<no __type, status {status}>"), ToString::to_string)
}

/// The `whitelistRules` projection of a list of CIDRs, as the wire carries it
/// (`InputWhitelistRule$Cidr`, jsonName `cidr`).
fn whitelist(cidrs: &[&str]) -> Value {
    Value::Array(cidrs.iter().map(|cidr| json!({ "cidr": cidr })).collect())
}

/// Rule: `CreateInputSecurityGroup` persists the request's whitelist rules (in
/// order) and tags under a minted numeric `Id` with an ARN built from the
/// request's region and that id, and every later read of the group answers from
/// that stored record.
///
/// The table spans what the rule generalises over: how many rules the request
/// carries (none, one, several), the prefix lengths they use — including both
/// published IPv4 bounds, /0 and /32 — the order they arrive in, and whether
/// tags are present. Members from
/// `smithy_query(query="operation", operation="CreateInputSecurityGroup")`.
#[tokio::test]
async fn created_groups_are_read_back_through_describe() {
    // (case, request body, the CIDRs the group must report, in order)
    let table: [(&str, Value, &[&str]); 5] = [
        ("no rules at all", json!({}), &[]),
        (
            "one host route",
            json!({ "whitelistRules": [{ "cidr": "192.0.2.1/32" }] }),
            &["192.0.2.1/32"],
        ),
        (
            "both published prefix bounds",
            json!({ "whitelistRules": [{ "cidr": "0.0.0.0/0" }, { "cidr": "10.0.0.0/32" }] }),
            &["0.0.0.0/0", "10.0.0.0/32"],
        ),
        (
            "several rules keep their order",
            json!({
                "whitelistRules": [
                    { "cidr": "203.0.113.0/24" },
                    { "cidr": "198.51.100.128/25" },
                    { "cidr": "10.0.0.0/8" },
                ]
            }),
            &["203.0.113.0/24", "198.51.100.128/25", "10.0.0.0/8"],
        ),
        (
            "rules and tags",
            json!({
                "whitelistRules": [{ "cidr": "172.16.0.0/12" }],
                "tags": { "env": "test", "owner": "fixer" },
            }),
            &["172.16.0.0/12"],
        ),
    ];

    for (case, request, cidrs) in table {
        let router = medialive();
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let created = create_group(&router, case, request).await;

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a numeric input security group id, got {:?}",
            created["id"]
        );
        assert_eq!(
            created["arn"],
            json!(format!(
                "arn:aws:medialive:us-west-2:{ACCOUNT}:inputSecurityGroup:{id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["whitelistRules"],
            whitelist(cidrs),
            "case {case:?}: expected the request's rules in order, got {:?}",
            created["whitelistRules"]
        );
        assert_eq!(
            created["tags"], expected_tags,
            "case {case:?}: expected the request's tags, got {:?}",
            created["tags"]
        );
        // A brand-new group is used by nothing, so it is IDLE with no inputs and
        // no channels (`InputSecurityGroupState`: IDLE | IN_USE | UPDATING |
        // DELETED).
        assert_eq!(
            created["state"],
            json!("IDLE"),
            "case {case:?}: expected a new group to be IDLE, got {:?}",
            created["state"]
        );
        assert_eq!(
            created["inputs"],
            json!([]),
            "case {case:?}: expected no inputs to use a new group, got {:?}",
            created["inputs"]
        );
        assert_eq!(
            created["channels"],
            json!([]),
            "case {case:?}: expected no channels to use a new group, got {:?}",
            created["channels"]
        );

        // Round trip: the reader answers from the stored record, so it reports
        // exactly what the create reported.
        let (status, described) = describe_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInputSecurityGroup: expected 200, got {status}: {described}"
        );
        for member in ["arn", "id", "inputs", "state", "tags", "whitelistRules", "channels"] {
            assert_eq!(
                described[member], created[member],
                "case {case:?}: expected describe to report the created {member}, got {:?}",
                described[member]
            );
        }
    }
}

/// Rule: `CreateInputSecurityGroup` has no idempotency token, so every create
/// mints its own group, and each one is readable on its own id while the others
/// keep their rules.
#[tokio::test]
async fn each_create_mints_its_own_group() {
    let router = medialive();
    // (case, the group's single CIDR)
    let table: [(&str, &str); 3] = [
        ("first", "10.1.0.0/16"),
        ("second", "10.2.0.0/16"),
        // Same rules as the first row: a repeated request is still a new group.
        ("repeat of the first", "10.1.0.0/16"),
    ];

    let mut created: Vec<(String, String)> = Vec::new();
    for (case, cidr) in table {
        let group = create_group(
            &router,
            case,
            json!({ "whitelistRules": [{ "cidr": cidr }] }),
        )
        .await;
        let id = group["id"].as_str().unwrap_or_default().to_string();
        assert!(
            created.iter().all(|(seen, _)| seen != &id),
            "case {case:?}: expected a fresh id, got the already-minted {id:?}"
        );
        created.push((id, cidr.to_string()));
    }

    for (id, cidr) in &created {
        let (status, described) = describe_group(&router, id).await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected the group to still exist, got {status}: {described}"
        );
        assert_eq!(
            described["whitelistRules"],
            whitelist(&[cidr.as_str()]),
            "case {id:?}: expected its own rule {cidr:?}, got {:?}",
            described["whitelistRules"]
        );
    }
}

/// Rule: a whitelist rule must name an IPv4 CIDR with a prefix length of 0 to 32
/// — the member is documented "List of IPv4 CIDR addresses to whitelist" — and a
/// request carrying anything else is refused with the operation's modelled
/// `BadRequestException` (400) instead of creating a group.
///
/// The table spans the ways a value can fall outside that rule: no prefix at
/// all, a prefix one past the published bound, a non-numeric or negative prefix,
/// an octet out of range, an IPv6 prefix, an empty string and a rule that names
/// no CIDR. The valid bounds (/0 and /32) are a row of the create suite above.
#[tokio::test]
async fn whitelist_rules_outside_the_ipv4_cidr_rule_are_refused() {
    // (case, the whitelistRules the request carries)
    let table: [(&str, Value); 9] = [
        ("bare address, no prefix", json!([{ "cidr": "10.0.0.0" }])),
        ("prefix one past the bound", json!([{ "cidr": "10.0.0.0/33" }])),
        ("negative prefix", json!([{ "cidr": "10.0.0.0/-1" }])),
        ("non-numeric prefix", json!([{ "cidr": "10.0.0.0/x" }])),
        ("octet out of range", json!([{ "cidr": "256.0.0.0/8" }])),
        ("too few octets", json!([{ "cidr": "10.0.0/8" }])),
        ("an IPv6 prefix", json!([{ "cidr": "2001:db8::/32" }])),
        ("empty string", json!([{ "cidr": "" }])),
        ("a rule that names no CIDR", json!([{}])),
    ];

    for (case, rules) in table {
        let router = medialive();
        let (status, body) = call(
            &router,
            "POST",
            "/prod/inputSecurityGroups",
            Some(json!({ "whitelistRules": rules })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected the operation's own BadRequestException, got {body}"
        );

        // A refused request persists no group and leaves the operation working:
        // the create that follows it succeeds and is readable.
        let created = create_group(&router, case, json!({})).await;
        let (status, described) = describe_group(
            &router,
            created["id"].as_str().unwrap_or_default(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the following create to persist a group, got {status}: {described}"
        );
    }
}

/// Rule: a group's `State`, `Inputs` and `Channels` report what currently uses
/// it, so a group is `IN_USE` exactly while some resource in state references it
/// and `IDLE` otherwise — and the reader tracks every later create and delete
/// rather than freezing what the create saw.
#[tokio::test]
async fn a_group_is_in_use_exactly_while_a_resource_references_it() {
    let router = medialive();
    let attached = create_group(
        &router,
        "attached",
        json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
    )
    .await;
    let attached_id = attached["id"].as_str().unwrap_or_default().to_string();
    let untouched = create_group(&router, "untouched", json!({})).await;
    let untouched_id = untouched["id"].as_str().unwrap_or_default().to_string();

    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "uses-the-group",
            "channelSecurityGroups": [attached_id.clone()],
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {channel}");
    let channel_id = channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (_, described) = describe_group(&router, &attached_id).await;
    assert_eq!(
        described["state"],
        json!("IN_USE"),
        "case \"a channel attached\": expected IN_USE, got {:?}",
        described["state"]
    );
    assert_eq!(
        described["channels"],
        json!([channel_id.clone()]),
        "case \"a channel attached\": expected the attaching channel's id, got {:?}",
        described["channels"]
    );

    let (_, other) = describe_group(&router, &untouched_id).await;
    assert_eq!(
        other["state"],
        json!("IDLE"),
        "case \"no resource attached\": expected IDLE, got {:?}",
        other["state"]
    );
    assert_eq!(
        other["channels"],
        json!([]),
        "case \"no resource attached\": expected no channels, got {:?}",
        other["channels"]
    );
    assert_eq!(
        other["inputs"],
        json!([]),
        "case \"no resource attached\": expected no inputs, got {:?}",
        other["inputs"]
    );

    // The other half of the same rule: an INPUT that names the group puts it
    // IN_USE too, and the group reports that input's id.
    let (status, created_input) = call(
        &router,
        "POST",
        "/prod/inputs",
        Some(json!({
            "name": "uses-the-other-group",
            "type": "URL_PULL",
            "inputSecurityGroups": [untouched_id.clone()],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateInput: expected 201, got {status}: {created_input}"
    );
    let input_id = created_input["input"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (_, attached_by_input) = describe_group(&router, &untouched_id).await;
    assert_eq!(
        attached_by_input["state"],
        json!("IN_USE"),
        "case \"an input attached\": expected IN_USE, got {:?}",
        attached_by_input["state"]
    );
    assert_eq!(
        attached_by_input["inputs"],
        json!([input_id]),
        "case \"an input attached\": expected the attaching input's id, got {:?}",
        attached_by_input["inputs"]
    );

    // Deleting the channel releases the group: once the delete has come due the
    // group is used by nothing again.
    wait_for_the_clock();
    let (status, deleting) = call(
        &router,
        "DELETE",
        &format!("/prod/channels/{channel_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteChannel: expected 200, got {status}: {deleting}");
    wait_for_the_clock();

    let (_, released) = describe_group(&router, &attached_id).await;
    assert_eq!(
        released["state"],
        json!("IDLE"),
        "case \"the attached channel deleted\": expected IDLE again, got {:?}",
        released["state"]
    );
    assert_eq!(
        released["channels"],
        json!([]),
        "case \"the attached channel deleted\": expected no channels, got {:?}",
        released["channels"]
    );
}

/// Rule: `DescribeInputSecurityGroup` on an id no group has answers the
/// operation's modelled `NotFoundException` (404).
///
/// The table spans the kinds of id that name no group, including a channel id —
/// the two id spaces are separate, so a channel's id is not a group's.
#[tokio::test]
async fn describe_refuses_an_id_no_group_has() {
    let router = medialive();
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "not-a-security-group" })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {channel}");
    let channel_id = channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // One group exists, so the store is not simply empty.
    create_group(&router, "a real group", json!({})).await;

    // (case, the id to read)
    let table: [(&str, String); 4] = [
        ("zero", "0".to_string()),
        ("a numeric id never minted", "9999999".to_string()),
        ("a non-numeric id", "not-an-id".to_string()),
        ("a channel's id", channel_id),
    ];

    for (case, id) in table {
        let (status, body) = describe_group(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

/// Rule: the input security group store is bounded, so a create past the ceiling
/// answers the operation's modelled throttle rather than growing the emulator's
/// heap, and every group created up to the ceiling is still readable.
#[tokio::test]
async fn the_group_store_is_bounded() {
    use service_com_amazonaws_medialive_medialive::state::MAX_INPUT_SECURITY_GROUPS;

    let router = medialive();
    let mut ids = Vec::new();
    for index in 0..MAX_INPUT_SECURITY_GROUPS {
        let group = create_group(
            &router,
            "filling the store",
            json!({ "whitelistRules": [{ "cidr": format!("10.0.{}.0/24", index % 256) }] }),
        )
        .await;
        ids.push(group["id"].as_str().unwrap_or_default().to_string());
    }

    let (status, body) = call(
        &router,
        "POST",
        "/prod/inputSecurityGroups",
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 429,
        "case {MAX_INPUT_SECURITY_GROUPS} stored: expected the create to be throttled, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "TooManyRequestsException",
        "case {MAX_INPUT_SECURITY_GROUPS} stored: expected TooManyRequestsException, got {body}"
    );

    for (index, id) in ids.iter().enumerate() {
        let (status, described) = describe_group(&router, id).await;
        assert_eq!(
            status, 200,
            "case group {index}: expected it to still be readable, got {status}: {described}"
        );
        assert_eq!(
            described["whitelistRules"],
            whitelist(&[format!("10.0.{}.0/24", index % 256).as_str()]),
            "case group {index}: expected its own rule, got {:?}",
            described["whitelistRules"]
        );
    }
}
