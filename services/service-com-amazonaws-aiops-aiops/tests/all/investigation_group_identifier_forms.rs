// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two FORMS an `InvestigationGroupIdentifier` may take.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: every `{identifier}` member of this service addresses a group
//! by EITHER of the two forms its documentation and its modelled pattern admit — the
//! group's NAME or the group's ARN — resolved among the groups of the Region the
//! request was signed for, and an identifier that is neither a stored group's name nor
//! a stored group's ARN is reported absent. The member's own documentation is
//! `smithy_query(query="doc",shape="com.amazonaws.aiops#DeleteInvestigationGroupRequest",member="identifier")`:
//! "Specify either the name or the ARN of the investigation group that you want to
//! delete." — and the same sentence on `Get`, `Update`, and the three policy
//! operations; the modelled pattern
//! (`smithy_query(query="shape",shape="InvestigationGroupIdentifier")`) spells the
//! alternation out as `^(?:[\-_A-Za-z0-9]{1,512}|arn:...:investigation-group\/[A-Za-z0-9]{16})$`.
//!
//! The TAGGING operations are the contrast, and are asserted here too: their
//! `resourceArn` is documented as the resource's ARN with no name alternative, so a
//! name addresses no tag set.
//!
//! Rows differ along the NAME — its length (each bound of the label class), its
//! character classes, its case, and a name shaped like an id — and the rule is asserted
//! against every operation that takes an identifier, reporting and mutating alike.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// A second role ARN, so an update addressed by name is visible in the stored record.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRoleTwo";

/// A policy document the policy operations attach and read back.
const POLICY: &str = r#"{"Version":"2012-10-17","Sid":"by-name"}"#;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error code this service's resource operations report an unresolvable identifier
/// with (`smithy_query(query="shape",shape="ResourceNotFoundException")`: `@httpError
/// 404`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code the tagging frontend reports an unresolvable `resourceArn` with — see
/// `state::TAGGING_RESOURCE_NOT_FOUND`.
const TAGGING_NOT_FOUND: &str = "NotFoundException";

/// The error code a request the modelled constraint walk refuses carries.
const VALIDATION: &str = "ValidationException";

/// Real AWS Regions, one per row that needs a group of its own: an account holds one
/// investigation group per Region.
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "eu-central-1",
    "ap-southeast-4",
    "us-gov-west-1",
    "ca-central-1",
    "eu-west-3",
    "sa-east-1",
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

/// Percent-encode everything a URI component may not carry literally — what an SDK does
/// for a non-greedy `@httpLabel` path segment.
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

/// The id segment of a group ARN — a form the modelled pattern admits as a bare label
/// but which NO `{identifier}` member documents, so it addresses no group.
fn id_of(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("a group ARN carries an id segment, got {arn:?}"))
        .to_string()
}

/// One row of the name table: how a failure names it, and the group name it creates.
struct NameCase {
    case: &'static str,
    name: String,
}

/// The names the rows span. Each is inside the modelled `name`
/// (`StringWithPatternAndLengthLimits`, len 1..512,
/// `^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$`) AND inside the bare-label alternative of
/// `InvestigationGroupIdentifier` (`[\-_A-Za-z0-9]{1,512}`), which is what lets the name
/// travel as the `{identifier}` path segment: the recorded form, both published length
/// bounds of that class (1 and 512), the underscore and digit characters it admits,
/// mixed case, and a name shaped exactly like a 16-character group id.
fn name_cases() -> Vec<NameCase> {
    vec![
        NameCase {
            case: "a hyphenated lowercase label",
            name: "my-group".to_string(),
        },
        NameCase {
            case: "the shortest label the class admits",
            name: "a".to_string(),
        },
        NameCase {
            case: "the longest label the class admits",
            name: "n".repeat(512),
        },
        NameCase {
            case: "underscores and digits",
            name: "group_42_prod".to_string(),
        },
        NameCase {
            case: "mixed case",
            name: "MyGroupName".to_string(),
        },
        NameCase {
            case: "a name shaped like a group id",
            name: "AbCdEfGhIjKlMnOp".to_string(),
        },
    ]
}

/// A group is addressed by its NAME exactly as it is by its ARN, on every operation that
/// takes an `{identifier}`: the reads report the same group, the writes reach the same
/// record (and are visible through the ARN), and the delete removes it — which frees the
/// Region's single group slot for the next create.
#[tokio::test]
async fn every_identifier_operation_resolves_a_group_by_name_as_well_as_by_arn() {
    for (index, NameCase { case, name }) in name_cases().into_iter().enumerate() {
        // Each row needs its own group, so each row signs for its own Region.
        let router = emulator();
        let region = region(index);
        let arn = create_ok(&router, &region, &name).await;
        let by_name = encode(&name);
        let by_arn = encode(&arn);

        // GetInvestigationGroup: the name reports the SAME group the ARN does.
        let (status, via_arn) = call_in(
            &router,
            &region,
            "GET",
            &format!("/investigationGroups/{by_arn}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the ARN must read the group, got {status} / {via_arn:?}"
        );
        let (status, via_name) = call_in(
            &router,
            &region,
            "GET",
            &format!("/investigationGroups/{by_name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the name must read the group, got {status} / {via_name:?}"
        );
        assert_eq!(
            via_name, via_arn,
            "case {case:?}: both identifier forms address one record, \
             via the name {via_name:?} vs via the ARN {via_arn:?}"
        );
        assert_eq!(
            via_name.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the group's own name back, got {via_name:?}"
        );

        // UpdateInvestigationGroup through the name reaches the stored record, which the
        // ARN then reports.
        let (status, updated) = call_in(
            &router,
            &region,
            "PATCH",
            &format!("/investigationGroups/{by_name}"),
            Some(&json!({ "roleArn": OTHER_ROLE_ARN })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an update addressed by name must succeed, got {status} / {updated:?}"
        );
        let (_, after) = call_in(
            &router,
            &region,
            "GET",
            &format!("/investigationGroups/{by_arn}"),
            None,
        )
        .await;
        assert_eq!(
            after.get("roleArn").and_then(Value::as_str),
            Some(OTHER_ROLE_ARN),
            "case {case:?}: the update addressed by name must be visible through the ARN, \
             got {after:?}"
        );

        // PutInvestigationGroupPolicy / GetInvestigationGroupPolicy /
        // DeleteInvestigationGroupPolicy, all addressed by the name.
        let (status, put) = call_in(
            &router,
            &region,
            "POST",
            &format!("/investigationGroups/{by_name}/policy"),
            Some(&json!({ "policy": POLICY })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a policy put addressed by name must succeed, got {status} / {put:?}"
        );
        assert_eq!(
            put.get("investigationGroupArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the put must answer the addressed group's ARN, got {put:?}"
        );
        for (via, identifier) in [("the name", &by_name), ("the ARN", &by_arn)] {
            let (status, got) = call_in(
                &router,
                &region,
                "GET",
                &format!("/investigationGroups/{identifier}/policy"),
                None,
            )
            .await;
            assert_eq!(
                (status, got.get("policy").and_then(Value::as_str)),
                (200, Some(POLICY)),
                "case {case:?}: the stored document must read back via {via}, \
                 got {status} / {got:?}"
            );
        }
        let (status, removed) = call_in(
            &router,
            &region,
            "DELETE",
            &format!("/investigationGroups/{by_name}/policy"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a policy delete addressed by name must succeed, \
             got {status} / {removed:?}"
        );
        let (status, gone) = call_in(
            &router,
            &region,
            "GET",
            &format!("/investigationGroups/{by_arn}/policy"),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (404, RESOURCE_NOT_FOUND),
            "case {case:?}: the document deleted by name must be gone, got {status} / {gone:?}"
        );

        // DeleteInvestigationGroup through the name removes the group itself.
        let (status, deleted) = call_in(
            &router,
            &region,
            "DELETE",
            &format!("/investigationGroups/{by_name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a delete addressed by name must succeed, got {status} / {deleted:?}"
        );
        let (status, absent) = call_in(
            &router,
            &region,
            "GET",
            &format!("/investigationGroups/{by_arn}"),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&absent).as_str()),
            (404, RESOURCE_NOT_FOUND),
            "case {case:?}: the deleted group must not read back through its ARN, \
             got {status} / {absent:?}"
        );
        let remaining = listed(&router, &region).await;
        assert!(
            remaining.is_empty(),
            "case {case:?}: the deleted group must be gone from the listing, got {remaining:?}"
        );

        // And the Region's single group slot is free again, so the lifecycle is not
        // wedged by a delete that was addressed by name.
        let replacement = create_ok(&router, &region, "created-after-the-delete").await;
        assert_ne!(
            replacement, arn,
            "case {case:?}: a group created after the delete is a new resource, \
             got the old ARN {replacement}"
        );
    }
}

/// An identifier that is neither a stored group's name nor its exact ARN resolves
/// nothing: every operation reports the group absent and the stored group is untouched.
///
/// Rows span the forms the modelled pattern admits but the member does not document (a
/// bare id), labels nothing carries, the name of a group that belongs to ANOTHER Region,
/// and ARNs differing from the stored one in exactly one field.
#[tokio::test]
async fn an_identifier_that_names_no_group_in_the_region_resolves_nothing() {
    let router = emulator();
    let home = region(0);
    let elsewhere = region(1);
    let name = "resolvable-by-name";
    let arn = create_ok(&router, &home, name).await;
    let id = id_of(&arn);
    // A group of another Region carrying its own distinct name: this endpoint may not
    // resolve it by name either.
    let foreign_name = "owned-by-another-region";
    let foreign_arn = create_ok(&router, &elsewhere, foreign_name).await;

    let unresolvable = [
        ("the group's own bare id", id.clone()),
        ("a label nothing carries", "no-such-group".to_string()),
        ("an id-shaped label nothing carries", "A".repeat(16)),
        (
            "the name of another Region's group",
            foreign_name.to_string(),
        ),
        ("another Region's group ARN", foreign_arn.clone()),
        (
            "the ARN with another region",
            arn.replace(&format!(":{home}:"), ":eu-north-1:"),
        ),
        (
            "the ARN with another account",
            arn.replace(ACCOUNT, "111122223333"),
        ),
        (
            "the ARN with another partition",
            arn.replace("arn:aws:", "arn:aws-cn:"),
        ),
        ("the ARN with another id", arn.replace(&id, &"Z".repeat(16))),
        (
            "the name as a suffix of a longer label",
            format!("{name}-2"),
        ),
    ];

    // Every operation that resolves a group through `{identifier}`, as
    // (name, method, uri template, body).
    let operations: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("GetInvestigationGroup", "GET", "/investigationGroups/{}", None),
        (
            "UpdateInvestigationGroup",
            "PATCH",
            "/investigationGroups/{}",
            Some(json!({ "roleArn": OTHER_ROLE_ARN })),
        ),
        (
            "DeleteInvestigationGroup",
            "DELETE",
            "/investigationGroups/{}",
            None,
        ),
        (
            "GetInvestigationGroupPolicy",
            "GET",
            "/investigationGroups/{}/policy",
            None,
        ),
        (
            "PutInvestigationGroupPolicy",
            "POST",
            "/investigationGroups/{}/policy",
            Some(json!({ "policy": POLICY })),
        ),
        (
            "DeleteInvestigationGroupPolicy",
            "DELETE",
            "/investigationGroups/{}/policy",
            None,
        ),
    ];

    for (form, identifier) in &unresolvable {
        for (operation, method, template, body) in &operations {
            let case = format!("{operation} addressed by {form}");
            let uri = template.replace("{}", &encode(identifier));
            let (status, answer) = call_in(&router, &home, method, &uri, body.as_ref()).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                RESOURCE_NOT_FOUND,
                "case {case:?}: expected {RESOURCE_NOT_FOUND}, got {answer:?}"
            );
        }
    }

    // Nothing above reached the stored group: it still reads back through BOTH of its
    // forms, with the role it was created with and no policy attached.
    for (via, identifier) in [("its name", name.to_string()), ("its ARN", arn.clone())] {
        let (status, body) = call_in(
            &router,
            &home,
            "GET",
            &format!("/investigationGroups/{}", encode(&identifier)),
            None,
        )
        .await;
        assert_eq!(
            (status, body.get("roleArn").and_then(Value::as_str)),
            (200, Some(ROLE_ARN)),
            "the refused calls must leave the group alone, read via {via}: \
             got {status} / {body:?}"
        );
    }
    let (status, policy) = call_in(
        &router,
        &home,
        "GET",
        &format!("/investigationGroups/{}/policy", encode(name)),
        None,
    )
    .await;
    assert_eq!(
        (status, error_code(&policy).as_str()),
        (404, RESOURCE_NOT_FOUND),
        "a refused put must have attached no policy, got {status} / {policy:?}"
    );
    // And the other Region keeps its own group.
    let there = listed(&router, &elsewhere).await;
    assert_eq!(
        there,
        vec![foreign_arn],
        "the other Region keeps exactly its own group, got {there:?}"
    );
}

/// A name the modelled `name` member admits but the identifier's bare-label class does
/// not (`^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$` is wider than `[\-_A-Za-z0-9]{1,512}`) never
/// reaches a handler as an identifier: the constraint walk refuses it with 400
/// `ValidationException`, while the group's ARN still addresses it.
#[tokio::test]
async fn a_name_outside_the_identifier_label_class_is_refused_at_the_boundary() {
    let router = emulator();
    // Rows differ along the character that leaves the label class: a space, brackets
    // and parentheses, a dot, a colon, and braces — each admitted by `name` only.
    let names = [
        "Prod billing",
        "Prod [us-west-2] (tier.1)",
        "team.alpha",
        "scope:prod",
        "{braced}",
    ];
    for (index, name) in names.iter().enumerate() {
        let region = region(index);
        let arn = create_ok(&router, &region, name).await;
        let (status, answer) = call_in(
            &router,
            &region,
            "GET",
            &format!("/investigationGroups/{}", encode(name)),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, VALIDATION),
            "case {name:?}: a name outside the label class is refused at the boundary, \
             got {status} / {answer:?}"
        );
        let (status, body) = call_in(
            &router,
            &region,
            "GET",
            &format!("/investigationGroups/{}", encode(&arn)),
            None,
        )
        .await;
        assert_eq!(
            (status, body.get("name").and_then(Value::as_str)),
            (200, Some(*name)),
            "case {name:?}: the ARN must still address the group, got {status} / {body:?}"
        );
    }
}

/// The contrast the model draws: the TAGGING operations take a `resourceArn`, documented
/// as "The ARN of the CloudWatch investigations resource" with no name alternative, so a
/// name addresses no tag set while the ARN does.
#[tokio::test]
async fn the_tagging_operations_take_the_arn_and_not_the_name() {
    let router = emulator();
    let home = region(0);
    let name = "tagged-by-arn-only";
    let arn = create_ok(&router, &home, name).await;

    let operations: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("ListTagsForResource", "GET", "/tags/{}", None),
        (
            "TagResource",
            "POST",
            "/tags/{}",
            Some(json!({ "tags": { "Team": "ops" } })),
        ),
        ("UntagResource", "DELETE", "/tags/{}?tagKeys=Team", None),
    ];

    for (operation, method, template, body) in &operations {
        let uri = template.replace("{}", &encode(name));
        let (status, answer) = call_in(&router, &home, method, &uri, body.as_ref()).await;
        assert_eq!(
            status, 404,
            "case {operation:?} addressed by name: expected 404, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            TAGGING_NOT_FOUND,
            "case {operation:?} addressed by name: expected {TAGGING_NOT_FOUND}, got {answer:?}"
        );

        let uri = template.replace("{}", &encode(&arn));
        let (status, answer) = call_in(&router, &home, method, &uri, body.as_ref()).await;
        assert_eq!(
            status, 200,
            "case {operation:?} addressed by the ARN: expected 200, got {status} / {answer:?}"
        );
    }
}
