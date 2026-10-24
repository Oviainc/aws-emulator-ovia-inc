// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the REGIONAL scope of identifier resolution.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: this service is reached through per-Region endpoints
//! (`https://aiops.{Region}.{dnsSuffix}`, `smithy_query(query="service_info")`), so an
//! endpoint holds only the investigation groups of the Region a request is signed for.
//! Every operation that resolves a group through an identifier therefore resolves it
//! only among that Region's groups: an identifier naming a group of ANOTHER Region is
//! reported absent — the resource surface's modelled `ResourceNotFoundException` (404)
//! and the tagging frontend's `NotFoundException` (404) — and nothing of either Region's
//! state changes. The same rule is what keeps `ListInvestigationGroups` and
//! `GetInvestigationGroup` from contradicting each other in one Region.
//!
//! Rows differ along the pair of Regions (owner and caller), along whether the calling
//! endpoint holds a group of its own, along the direction the call crosses in, and along
//! the operation — reporting and mutating alike, including the delete.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// A second role ARN, so an `UpdateInvestigationGroup` that wrongly succeeded would be
/// visible in the stored record.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRoleTwo";

/// The policy document the owner's own Region attaches, so a cross-Region put or delete
/// that wrongly succeeded would be visible.
const OWN_POLICY: &str = r#"{"Version":"2012-10-17","Sid":"own-region"}"#;

/// The document a cross-Region `PutInvestigationGroupPolicy` tries to attach.
const FOREIGN_POLICY: &str = r#"{"Version":"2012-10-17","Sid":"another-region"}"#;

/// The tag the owner's own Region writes, and the key a cross-Region `UntagResource`
/// tries to remove.
const OWN_TAG_KEY: &str = "Team";
const OWN_TAG_VALUE: &str = "ops";

/// The error code the RESOURCE operations report an unresolvable identifier with: this
/// service's own modelled `ResourceNotFoundException`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`: `@httpError 404`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code the TAGGING operations report an unresolvable `resourceArn` with: the
/// tagging frontend's own `NotFoundException` — see `state::TAGGING_RESOURCE_NOT_FOUND`.
const TAGGING_NOT_FOUND: &str = "NotFoundException";

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

/// Percent-encode everything a URI component may not carry literally — what an SDK does
/// for an `@httpLabel` path segment.
fn encode(value: &str) -> String {
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

/// Create a group in `region` that must succeed, and answer its ARN.
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
    let arn = created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("create {name:?} in {region}: response carried no arn, got {created:?}")
        })
        .to_string();
    assert!(
        arn.contains(&format!(":aiops:{region}:")),
        "create {name:?} in {region}: expected an ARN naming that Region, got {arn:?}"
    );
    arn
}

/// The ARNs `ListInvestigationGroups` reports in `region`.
async fn listed(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let (status, body) = call_in(router, region, "GET", "/investigationGroups", None).await;
    assert_eq!(
        status, 200,
        "listing {region}: expected 200, got {status} / {body:?}"
    );
    body.get("investigationGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("listing {region}: no investigationGroups in {body:?}"))
        .iter()
        .filter_map(|item| item.get("arn").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Everything about one group a cross-Region call could change, read through the
/// group's OWN Region: its reported body, its tag set and its policy document (`None`
/// when it carries none).
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    body: Value,
    tags: BTreeMap<String, String>,
    policy: Option<String>,
}

/// Read `arn` back through its own Region, asserting the reads succeed.
async fn snapshot(router: &MultiServiceRouter, region: &str, arn: &str, case: &str) -> Snapshot {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{}", encode(arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a group must read back in its own Region {region}, \
         got {status} / {body:?}"
    );

    let (status, tags_body) = call_in(router, region, "GET", &format!("/tags/{}", encode(arn)), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a group's tags must read back in its own Region {region}, \
         got {status} / {tags_body:?}"
    );
    let tags = tags_body
        .get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: ListTagsForResource reported no tag set, got {tags_body:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();

    let (status, policy_body) = call_in(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{}/policy", encode(arn)),
        None,
    )
    .await;
    let policy = match status {
        200 => Some(
            policy_body
                .get("policy")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a 200 policy read carried no document, got {policy_body:?}")
                })
                .to_string(),
        ),
        404 => None,
        other => panic!(
            "case {case:?}: a policy read answers 200 or 404, got {other} / {policy_body:?}"
        ),
    };

    Snapshot { body, tags, policy }
}

/// One operation that resolves an investigation group through an identifier.
struct Op {
    /// The operation's name, as a failure reports it.
    name: &'static str,
    method: &'static str,
    /// The URI, with `{}` standing for the url-encoded identifier.
    template: &'static str,
    /// The request body, for the operations that take one.
    body: Option<Value>,
    /// The error code the operation's surface reports an unresolvable identifier with.
    not_found_code: &'static str,
}

/// Every operation of this service that addresses a group by identifier: the six
/// resource operations (which report this service's modelled `ResourceNotFoundException`)
/// and the three tagging operations (served by the tagging frontend, which reports its
/// own `NotFoundException`). Each mutating row carries a payload whose effect would be
/// visible in the stored record had the call wrongly resolved.
fn ops() -> Vec<Op> {
    vec![
        Op {
            name: "GetInvestigationGroup",
            method: "GET",
            template: "/investigationGroups/{}",
            body: None,
            not_found_code: RESOURCE_NOT_FOUND,
        },
        Op {
            name: "UpdateInvestigationGroup",
            method: "PATCH",
            template: "/investigationGroups/{}",
            body: Some(json!({ "roleArn": OTHER_ROLE_ARN })),
            not_found_code: RESOURCE_NOT_FOUND,
        },
        Op {
            name: "DeleteInvestigationGroup",
            method: "DELETE",
            template: "/investigationGroups/{}",
            body: None,
            not_found_code: RESOURCE_NOT_FOUND,
        },
        Op {
            name: "GetInvestigationGroupPolicy",
            method: "GET",
            template: "/investigationGroups/{}/policy",
            body: None,
            not_found_code: RESOURCE_NOT_FOUND,
        },
        Op {
            name: "PutInvestigationGroupPolicy",
            method: "POST",
            template: "/investigationGroups/{}/policy",
            body: Some(json!({ "policy": FOREIGN_POLICY })),
            not_found_code: RESOURCE_NOT_FOUND,
        },
        Op {
            name: "DeleteInvestigationGroupPolicy",
            method: "DELETE",
            template: "/investigationGroups/{}/policy",
            body: None,
            not_found_code: RESOURCE_NOT_FOUND,
        },
        Op {
            name: "ListTagsForResource",
            method: "GET",
            template: "/tags/{}",
            body: None,
            not_found_code: TAGGING_NOT_FOUND,
        },
        Op {
            name: "TagResource",
            method: "POST",
            template: "/tags/{}",
            body: Some(json!({ "tags": { "WrittenFromAnotherRegion": "no" } })),
            not_found_code: TAGGING_NOT_FOUND,
        },
        Op {
            name: "UntagResource",
            method: "DELETE",
            template: "/tags/{}?tagKeys=Team",
            body: None,
            not_found_code: TAGGING_NOT_FOUND,
        },
    ]
}

/// One row of the cross-Region table.
struct Pair {
    /// How a failure names the row.
    case: &'static str,
    /// The Region the group under test belongs to.
    owner: &'static str,
    /// The Region the cross-Region calls are signed for.
    caller: &'static str,
    /// Whether the calling Region holds a group of its OWN, so the row covers both an
    /// endpoint that is empty and one that already answers for a group.
    caller_holds_group: bool,
}

/// The pairs: the Region names are real AWS Regions (and, in the last row, two longer
/// names inside what a SigV4 credential scope admits, `[a-zA-Z0-9-]`), spanning two
/// Regions of one geography, two geographies, a GovCloud Region beside a commercial one,
/// two names sharing a prefix, and both directions of the crossing.
fn pairs() -> Vec<Pair> {
    vec![
        Pair {
            case: "us-west-2 group addressed from an empty us-east-1",
            owner: "us-west-2",
            caller: "us-east-1",
            caller_holds_group: false,
        },
        Pair {
            case: "us-east-1 group addressed from a us-west-2 that holds its own",
            owner: "us-east-1",
            caller: "us-west-2",
            caller_holds_group: true,
        },
        Pair {
            case: "two Region names sharing a prefix",
            owner: "us-east-1",
            caller: "us-east-2",
            caller_holds_group: true,
        },
        Pair {
            case: "a GovCloud Region addressed from a commercial one",
            owner: "us-gov-west-1",
            caller: "us-east-1",
            caller_holds_group: false,
        },
        Pair {
            case: "Europe addressed from Asia Pacific",
            owner: "eu-central-1",
            caller: "ap-southeast-4",
            caller_holds_group: true,
        },
        Pair {
            case: "a longer Region name and its prefix",
            owner: "ap-southeast-7",
            caller: "ap-southeast-70",
            caller_holds_group: false,
        },
    ]
}

/// A group belongs to the Region its ARN names, and only that Region's endpoint resolves
/// it: every operation that takes an identifier reports another Region's group absent and
/// leaves both Regions' state exactly as it was — while the SAME call from the group's
/// own Region succeeds.
#[tokio::test]
async fn a_group_resolves_only_in_the_region_it_belongs_to() {
    for pair in pairs() {
        let Pair {
            case,
            owner,
            caller,
            caller_holds_group,
        } = pair;
        // A fresh store per row: an account holds one group per Region, so the rows must
        // not inherit each other's slots.
        let router = emulator();

        let arn = create_ok(&router, owner, "owned-here").await;
        // Give the group state a cross-Region call could damage: a tag and a policy.
        let (status, body) = call_in(
            &router,
            owner,
            "POST",
            &format!("/tags/{}", encode(&arn)),
            Some(&json!({ "tags": { OWN_TAG_KEY: OWN_TAG_VALUE } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: tagging in the owning Region must succeed, got {status} / {body:?}"
        );
        let (status, body) = call_in(
            &router,
            owner,
            "POST",
            &format!("/investigationGroups/{}/policy", encode(&arn)),
            Some(&json!({ "policy": OWN_POLICY })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: attaching a policy in the owning Region must succeed, \
             got {status} / {body:?}"
        );

        let caller_arn = if caller_holds_group {
            Some(create_ok(&router, caller, "owned-there").await)
        } else {
            None
        };

        let before_owner = snapshot(&router, owner, &arn, case).await;
        let before_caller = match &caller_arn {
            Some(other) => Some(snapshot(&router, caller, other, case).await),
            None => None,
        };

        // Every operation, addressed from the other Region with this Region's ARN.
        for op in ops() {
            let uri = op.template.replace("{}", &encode(&arn));
            let (status, answer) = call_in(&router, caller, op.method, &uri, op.body.as_ref()).await;
            let probe = format!("{case} / {} from {caller}", op.name);
            assert_eq!(
                status, 404,
                "case {probe:?}: a Region's endpoint must not resolve another Region's \
                 group, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                op.not_found_code,
                "case {probe:?}: expected {}, got {answer:?}",
                op.not_found_code
            );

            // And the reverse crossing, when the calling Region has a group of its own.
            if let Some(other) = &caller_arn {
                let uri = op.template.replace("{}", &encode(other));
                let (status, answer) =
                    call_in(&router, owner, op.method, &uri, op.body.as_ref()).await;
                let probe = format!("{case} / {} from {owner}", op.name);
                assert_eq!(
                    status, 404,
                    "case {probe:?}: the crossing is refused in both directions, \
                     got {status} / {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    op.not_found_code,
                    "case {probe:?}: expected {}, got {answer:?}",
                    op.not_found_code
                );
            }
        }

        // Nothing moved: not the group, not its tags, not its policy — in either Region.
        let after_owner = snapshot(&router, owner, &arn, case).await;
        assert_eq!(
            after_owner, before_owner,
            "case {case:?}: a refused cross-Region call must change nothing in {owner}, \
             was {before_owner:?} now {after_owner:?}"
        );
        assert_eq!(
            after_owner.tags.get(OWN_TAG_KEY).map(String::as_str),
            Some(OWN_TAG_VALUE),
            "case {case:?}: a cross-Region UntagResource must not remove a tag, \
             got {:?}",
            after_owner.tags
        );
        assert_eq!(
            after_owner.policy.as_deref(),
            Some(OWN_POLICY),
            "case {case:?}: a cross-Region policy put or delete must not reach the \
             stored document, got {:?}",
            after_owner.policy
        );
        if let (Some(other), Some(before)) = (&caller_arn, &before_caller) {
            let after = snapshot(&router, caller, other, case).await;
            assert_eq!(
                &after, before,
                "case {case:?}: a refused cross-Region call must change nothing in \
                 {caller} either, was {before:?} now {after:?}"
            );
        }

        // The two endpoints report exactly their own groups, which is what the 404s
        // above keep consistent with what a read answers.
        let owner_listing = listed(&router, owner).await;
        assert_eq!(
            owner_listing,
            vec![arn.clone()],
            "case {case:?}: {owner} must list exactly its own group, got {owner_listing:?}"
        );
        let caller_listing = listed(&router, caller).await;
        let want_caller: Vec<String> = caller_arn.iter().cloned().collect();
        assert_eq!(
            caller_listing, want_caller,
            "case {case:?}: {caller} must list exactly its own groups, got {caller_listing:?}"
        );

        // The control: the SAME operations from the group's own Region all succeed, so
        // the 404s above are about the Region and not about the operations being broken.
        // The delete comes last, since it takes the group away.
        for op in ops() {
            if op.name == "DeleteInvestigationGroup" {
                continue;
            }
            let uri = op.template.replace("{}", &encode(&arn));
            let (status, answer) = call_in(&router, owner, op.method, &uri, op.body.as_ref()).await;
            let probe = format!("{case} / {} from {owner}", op.name);
            assert_eq!(
                status, 200,
                "case {probe:?}: the owning Region must serve the call, got {status} / {answer:?}"
            );
        }
        let (status, answer) = call_in(
            &router,
            owner,
            "DELETE",
            &format!("/investigationGroups/{}", encode(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the owning Region must serve the delete, got {status} / {answer:?}"
        );
        assert!(
            listed(&router, owner).await.is_empty(),
            "case {case:?}: the deleted group must be gone from {owner}"
        );

        // And the other Region's group is untouched by that delete.
        if let Some(other) = &caller_arn {
            let still = listed(&router, caller).await;
            assert_eq!(
                still,
                vec![other.clone()],
                "case {case:?}: {caller} keeps its own group, got {still:?}"
            );
        }
    }
}

/// One endpoint never contradicts itself: in every Region, `GetInvestigationGroup`
/// resolves exactly the groups `ListInvestigationGroups` reports there — so a group a
/// Region does not list is not readable from it, and one it lists is.
///
/// The rows are every (endpoint, group) pair over a set of Regions, some holding a group
/// and some holding none; the invariant is asserted on all of them, not only on the
/// mismatched ones.
#[tokio::test]
async fn a_region_reads_back_exactly_the_groups_it_lists() {
    let router = emulator();

    // Regions spanning geographies and naming styles; the ones past `holding` hold no
    // group at all.
    let regions = [
        "us-west-2",
        "us-east-1",
        "eu-central-1",
        "ap-southeast-4",
        "us-gov-west-1",
        "ca-central-1",
        "eu-west-3",
    ];
    let holding = 4;

    let mut arns = Vec::new();
    for region in regions.iter().take(holding) {
        arns.push(create_ok(&router, region, "listed-and-readable").await);
    }

    for region in regions {
        let listing = listed(&router, region).await;
        for arn in &arns {
            let (status, body) = call_in(
                &router,
                region,
                "GET",
                &format!("/investigationGroups/{}", encode(arn)),
                None,
            )
            .await;
            let want_status = if listing.contains(arn) { 200 } else { 404 };
            let case = format!("{arn} read from {region}");
            assert_eq!(
                status, want_status,
                "case {case:?}: a Region reads back exactly what it lists ({listing:?}), \
                 got {status} / {body:?}"
            );
            if want_status == 404 {
                assert_eq!(
                    error_code(&body),
                    RESOURCE_NOT_FOUND,
                    "case {case:?}: expected {RESOURCE_NOT_FOUND}, got {body:?}"
                );
            } else {
                assert_eq!(
                    body.get("arn").and_then(Value::as_str),
                    Some(arn.as_str()),
                    "case {case:?}: expected the addressed group back, got {body:?}"
                );
            }
        }
    }
}
