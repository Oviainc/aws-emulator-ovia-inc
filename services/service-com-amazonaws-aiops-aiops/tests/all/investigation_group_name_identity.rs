// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHAT a group's NAME addresses when it travels as an
//! `{identifier}`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a group's name addresses the group RECORD that currently carries
//! that name in the Region the request was signed for, matched character for character —
//! so the stored name resolves the same record its ARN does, any other label (a different
//! case, a longer or shorter label, a name no group carries any more) resolves nothing,
//! a name reused by a later create resolves the NEW record, and addressing a reporting
//! operation by name changes nothing a later caller sees.
//!
//! The member's documentation is
//! `smithy_query(query="doc",shape="com.amazonaws.aiops#GetInvestigationGroupRequest",member="identifier")`:
//! "Specify either the name or the ARN of the investigation group that you want to
//! view." — the same sentence on `Update`, `Delete` and the three policy operations;
//! `smithy_query(query="shape",shape="InvestigationGroupIdentifier")` spells the
//! alternation out as
//! `^(?:[\-_A-Za-z0-9]{1,512}|arn:...:investigation-group\/[A-Za-z0-9]{16})$`.
//!
//! The equivalence of the two forms across all six identifier operations is tabulated in
//! `investigation_group_identifier_forms.rs`; this suite pins the IDENTITY the name
//! stands for: exactness, currency, and purity of the read.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The error code this service's resource operations report an unresolvable identifier
/// with (`smithy_query(query="shape",shape="ResourceNotFoundException")`: `@httpError
/// 404`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// Real AWS Regions, one per row that needs a group of its own: an account holds one
/// investigation group per Region.
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "eu-central-1",
    "ap-southeast-4",
    "us-gov-west-1",
    "ca-central-1",
];

/// The Region row `n` signs for; past [`REGIONS`] a synthetic name inside what a SigV4
/// credential scope admits (`[a-zA-Z0-9-]`).
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:04}"), |name| (*name).to_string())
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on and the
/// handlers read the request's Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
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
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Create a group named `name` in `region` that must succeed, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, created) = call_in(
        router,
        region,
        "POST",
        "/investigationGroups",
        Some(&json!({ "name": name, "roleArn": ROLE_ARN })),
    )
    .await;
    assert_eq!(
        status, 201,
        "create {name:?} in {region}: expected 201, got {status} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("create {name:?} in {region}: response carried no arn, got {created:?}")
        })
        .to_string()
}

/// `GetInvestigationGroup` addressed by `identifier` in `region`.
async fn get_group(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{identifier}"),
        None,
    )
    .await
}

/// The names the rows span, each inside the bare-label alternative of
/// `InvestigationGroupIdentifier` (`[\-_A-Za-z0-9]{1,512}`) so it can travel as the
/// `{identifier}` path segment: the recorded hyphenated label, a mixed-case label, an
/// underscore-and-digit label, and the shortest label the class admits.
const NAMES: &[&str] = &["my-group", "MyGroup", "group_42_prod", "a"];

/// A label the stored name is NOT: how a failure names the row, and how to derive it
/// from the group's own name. Each derived label still satisfies the bare-label class,
/// so it reaches the handler as a well-formed identifier and the answer is the
/// resolver's, not the constraint walk's.
struct NearMiss {
    case: &'static str,
    derive: fn(&str) -> String,
}

/// The near misses every row is probed with: a name differs from another name in any
/// character, case included.
const NEAR_MISSES: &[NearMiss] = &[
    NearMiss {
        case: "the name upper-cased",
        derive: |name| name.to_uppercase(),
    },
    NearMiss {
        case: "the name lower-cased",
        derive: |name| name.to_lowercase(),
    },
    NearMiss {
        case: "the name with one more character",
        derive: |name| format!("{name}X"),
    },
    NearMiss {
        case: "the name with one more leading character",
        derive: |name| format!("X{name}"),
    },
    NearMiss {
        case: "the name with its last character dropped",
        derive: |name| {
            let mut shorter = name.to_string();
            shorter.pop();
            // The class admits no empty label, so a one-character name shortens to a
            // different one-character label instead.
            if shorter.is_empty() {
                "z".to_string()
            } else {
                shorter
            }
        },
    },
    NearMiss {
        case: "the name with its separators removed",
        derive: |name| name.replace(['-', '_'], ""),
    },
];

/// A name addresses a group by exact match: the stored name reports the group, and a
/// label differing from it in any character — case included — reports no group.
#[tokio::test]
async fn a_name_addresses_a_group_only_on_an_exact_match() {
    for (index, name) in NAMES.iter().enumerate() {
        let router = emulator();
        let region = region(index);
        let arn = create_ok(&router, &region, name).await;

        let (status, body) = get_group(&router, &region, name).await;
        assert_eq!(
            (status, body.get("arn").and_then(Value::as_str)),
            (200, Some(arn.as_str())),
            "case {name:?}: the stored name must address the group, got {status} / {body:?}"
        );

        for NearMiss { case, derive } in NEAR_MISSES {
            let probe = derive(name);
            if probe == *name {
                // This row's name is unchanged by this transformation (a lower-case
                // name has no upper-case neighbour to miss on), so it is not a near
                // miss here.
                continue;
            }
            let case = format!("{name:?} probed by {case} ({probe:?})");
            let (status, answer) = get_group(&router, &region, &probe).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (404, RESOURCE_NOT_FOUND),
                "case {case}: expected 404 {RESOURCE_NOT_FOUND}, got {status} / {answer:?}"
            );
        }

        // The probes above left the group exactly where it was.
        let (status, body) = get_group(&router, &region, name).await;
        assert_eq!(
            (status, body.get("arn").and_then(Value::as_str)),
            (200, Some(arn.as_str())),
            "case {name:?}: the refused probes must leave the group addressable by name, \
             got {status} / {body:?}"
        );
    }
}

/// A name addresses the record that carries it NOW: once the group is deleted the name
/// addresses nothing, and a group created afterwards with the same name is addressed by
/// it — as the NEW record, never the old ARN.
#[tokio::test]
async fn a_name_addresses_the_record_that_carries_it_now() {
    for (index, name) in NAMES.iter().enumerate() {
        let router = emulator();
        let region = region(index);
        let first = create_ok(&router, &region, name).await;

        let (status, deleted) = call_in(
            &router,
            &region,
            "DELETE",
            &format!("/investigationGroups/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a delete addressed by name must succeed, got {status} / {deleted:?}"
        );

        let (status, absent) = get_group(&router, &region, name).await;
        assert_eq!(
            (status, error_code(&absent).as_str()),
            (404, RESOURCE_NOT_FOUND),
            "case {name:?}: the name of a deleted group addresses nothing, \
             got {status} / {absent:?}"
        );

        let second = create_ok(&router, &region, name).await;
        assert_ne!(
            second, first,
            "case {name:?}: the group created after the delete is a new resource, \
             got the old ARN {second}"
        );
        let (status, body) = get_group(&router, &region, name).await;
        assert_eq!(
            (status, body.get("arn").and_then(Value::as_str)),
            (200, Some(second.as_str())),
            "case {name:?}: the reused name must address the new record, got {status} / {body:?}"
        );
        let (status, old) = get_group(&router, &region, &urlencode(&first)).await;
        assert_eq!(
            (status, error_code(&old).as_str()),
            (404, RESOURCE_NOT_FOUND),
            "case {name:?}: the deleted group's ARN addresses nothing, got {status} / {old:?}"
        );
    }
}

/// Percent-encode everything a URI component may not carry literally — what an SDK does
/// for a non-greedy `@httpLabel` path segment, needed only for the ARN form (a bare label
/// carries no reserved character).
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Reading a group by name is a pure read: repeated reads report the identical body, the
/// stored modification stamp does not move, and the Region still holds exactly the one
/// group it held before — whichever form addressed the read.
#[tokio::test]
async fn reading_a_group_by_name_changes_nothing_a_later_caller_sees() {
    for (index, name) in NAMES.iter().enumerate() {
        let router = emulator();
        let region = region(index);
        let arn = create_ok(&router, &region, name).await;
        let encoded_arn = urlencode(&arn);

        let (status, first) = get_group(&router, &region, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the name must read the group, got {status} / {first:?}"
        );

        // Four reads alternating between the two forms: every one reports the same
        // record, byte for byte.
        for round in 0..2 {
            for (form, identifier) in [
                ("the name", (*name).to_string()),
                ("the ARN", encoded_arn.clone()),
            ] {
                let (status, again) = get_group(&router, &region, &identifier).await;
                assert_eq!(
                    (status, &again),
                    (200, &first),
                    "case {name:?} round {round} via {form}: a read must report the identical \
                     record, got {status} / {again:?}"
                );
            }
        }

        // The listing still reports exactly that one group, so no read minted, moved or
        // reclaimed anything.
        let (status, listing) =
            call_in(&router, &region, "GET", "/investigationGroups", None).await;
        let listed: Vec<&str> = listing
            .get("investigationGroups")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {name:?}: no investigationGroups in {listing:?}"))
            .iter()
            .filter_map(|item| item.get("arn").and_then(Value::as_str))
            .collect();
        assert_eq!(
            (status, listed.as_slice()),
            (200, [arn.as_str()].as_slice()),
            "case {name:?}: the reads must leave the Region holding exactly its one group, \
             got {status} / {listing:?}"
        );

        // And the Region's single group slot is still taken, so no read consumed the
        // quota either.
        let (status, refused) = call_in(
            &router,
            &region,
            "POST",
            "/investigationGroups",
            Some(&json!({ "name": "one-group-too-many", "roleArn": ROLE_ARN })),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (402, "ServiceQuotaExceededException"),
            "case {name:?}: the Region still holds its one group, got {status} / {refused:?}"
        );
    }
}
