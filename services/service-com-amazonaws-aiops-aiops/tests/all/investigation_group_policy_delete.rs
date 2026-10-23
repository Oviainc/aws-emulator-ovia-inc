// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteInvestigationGroupPolicy`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a delete of a group's resource policy removes the stored
//! document from that ONE persisted investigation-group record and answers an empty
//! 200, leaving the group itself and every other group's policy untouched; a call
//! that has no document to remove is a modelled `ResourceNotFoundException`, naming
//! the GROUP when no group matches `{identifier}` and the POLICY when the group
//! exists while carrying none.
//!
//! An account holds ONE investigation group per Region, so every row below that needs
//! its own group signs for its own Region ([`region`]).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state;
use smithy_server_runtime::MultiServiceRouter;

/// The region a case signs for when it only needs one.
const REGION: &str = "us-west-2";

/// Regions the rows that each need their own group sign for, in order.
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "eu-central-1",
    "eu-west-2",
    "ap-southeast-1",
    "ap-northeast-1",
];

/// The Region row `n` signs for.
///
/// An account holds one investigation group per Region, so a row that needs its own
/// group needs its own Region. The names are real AWS Regions; past the list they are
/// synthetic ones inside what a SigV4 credential scope admits (`[a-zA-Z0-9-]`).
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:03}"), |name| (*name).to_string())
}

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

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

/// A request signed for `aiops` in `region`, the signal the router routes on and
/// the handlers read the region and the caller principal from.
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

/// Status, modelled error shape (empty on a success) and parsed body of one call. An
/// empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .map(|value| value.rsplit('#').next().unwrap_or(value).to_string())
        .unwrap_or_default();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, error_type, parsed)
}

/// [`call_in`] in [`REGION`]: every operation below but the create resolves its group
/// through an ARN, which names its own Region.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, Value) {
    call_in(router, REGION, method, uri, body).await
}

/// Percent-encode a value for use as a single `@httpLabel` path segment, which is
/// what an SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Create a group that must succeed in `region`, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, code, created) = call_in(
        router,
        region,
        "POST",
        "/investigationGroups",
        Some(&json!({ "name": name, "roleArn": ROLE_ARN })),
    )
    .await;
    assert_eq!(
        status, 201,
        "create {name:?}: expected 201, got {status} / {code} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {name:?}: response carried no arn, got {created:?}"))
        .to_string()
}

async fn put_policy(
    router: &MultiServiceRouter,
    identifier: &str,
    document: &str,
) -> (u16, String, Value) {
    call(
        router,
        "POST",
        &format!("/investigationGroups/{}/policy", label(identifier)),
        Some(&json!({ "policy": document })),
    )
    .await
}

async fn get_policy(router: &MultiServiceRouter, identifier: &str) -> (u16, String, Value) {
    call(
        router,
        "GET",
        &format!("/investigationGroups/{}/policy", label(identifier)),
        None,
    )
    .await
}

async fn delete_policy(router: &MultiServiceRouter, identifier: &str) -> (u16, String, Value) {
    call(
        router,
        "DELETE",
        &format!("/investigationGroups/{}/policy", label(identifier)),
        None,
    )
    .await
}

/// The message this service words an absent-resource 404 with, for the modelled
/// resource `resource`.
fn absent(resource: &str) -> String {
    format!("{resource} does not exist.")
}

fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The id segment of a group ARN — a bare form the modelled
/// `InvestigationGroupIdentifier` admits and which the service resolves no group from.
fn id_of(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("a group ARN carries an id segment, got {arn:?}"))
        .to_string()
}

#[tokio::test]
async fn deleting_a_policy_removes_the_stored_document_and_keeps_the_group() {
    let router = emulator();

    // Rows differ along the policy document. The documents span the modelled
    // `InvestigationGroupPolicyDocument` (len 1..32768, pattern
    // `^[\u0009\u000A\u000D\u0020-\u00FF]+$`) inside what the operation accepts as a
    // JSON policy document: the smallest object, a realistic policy, one carrying the
    // tab/newline/carriage-return and Latin-1 bytes that pattern admits, and a long
    // one. Each row needs its own group, so each signs for its own Region.
    let documents = [
        "{}".to_string(),
        json!({
            "Version": "2008-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": "aiops.alarms.cloudwatch.amazonaws.com" },
                "Action": ["aiops:CreateInvestigation", "aiops:CreateInvestigationEvent"],
                "Resource": "*"
            }]
        })
        .to_string(),
        "{\n\t\"Version\": \"2012-10-17\",\r\n\t\"Sid\": \"caf\u{e9}-\u{fc}ber\"\n}".to_string(),
        format!("{{\"Sid\":\"{}\"}}", "B".repeat(8192)),
    ];

    for (index, document) in documents.iter().enumerate() {
        let name = format!("delete-policy-{index}");
        let arn = create_ok(&router, &region(index), &name).await;
        // The ARN is the only identifier that resolves the group, so it is what every
        // call below addresses.
        let identifier = arn.clone();
        let case = format!("{name} ({} byte document)", document.len());

        let (status, code, put) = put_policy(&router, &arn, document).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the put, got {status} / {code} / {put:?}"
        );

        let (status, code, got) = get_policy(&router, &arn).await;
        assert_eq!(
            (status, got.get("policy").and_then(Value::as_str)),
            (200, Some(document.as_str())),
            "case {case}: the put must be readable before the delete, \
             got {status} / {code} / {got:?}"
        );

        let (status, code, deleted) = delete_policy(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the delete, got {status} / {code} / {deleted:?}"
        );
        assert!(
            matches!(&deleted, Value::Null | Value::Object(_)),
            "case {case}: the delete answers an empty output, got {deleted:?}"
        );

        // The document is gone from the store: the read now reports the POLICY absent,
        // not the group.
        let (status, code, after) = get_policy(&router, &arn).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: a deleted policy must be absent, got {status} / {after:?}"
        );
        assert_eq!(
            message(&after),
            absent(state::INVESTIGATION_GROUP_POLICY_RESOURCE),
            "case {case}: expected the policy reported absent, got {after:?}"
        );

        // The GROUP survived its policy: it is still readable.
        let (status, code, group) = call(
            &router,
            "GET",
            &format!("/investigationGroups/{}", label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the group must survive the policy delete, \
             got {status} / {code} / {group:?}"
        );
        assert_eq!(
            group.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case}: expected the same group back, got {group:?}"
        );
        let created_at = group.get("createdAt").and_then(Value::as_i64).unwrap_or(-1);
        let modified_at = group
            .get("lastModifiedAt")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        assert!(
            modified_at >= created_at && created_at > 0,
            "case {case}: expected lastModifiedAt >= createdAt > 0, \
             got {modified_at} / {created_at}"
        );

        // And the policy resource can be created again: the delete removed the
        // document, not the group's ability to carry one.
        let replacement = format!("{{\"Sid\":\"re-put-{index}\"}}");
        let (status, code, put) = put_policy(&router, &identifier, &replacement).await;
        assert_eq!(
            status, 200,
            "case {case}: a put after the delete must succeed, \
             got {status} / {code} / {put:?}"
        );
        let (status, _, got) = get_policy(&router, &identifier).await;
        assert_eq!(
            (status, got.get("policy").and_then(Value::as_str)),
            (200, Some(replacement.as_str())),
            "case {case}: expected the re-put document back, got {got:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_policy_that_is_not_there_is_the_absent_resource_it_names() {
    let router = emulator();

    let group_absent = absent(state::INVESTIGATION_GROUP_RESOURCE);
    let policy_absent = absent(state::INVESTIGATION_GROUP_POLICY_RESOURCE);
    assert_ne!(
        group_absent, policy_absent,
        "the two conditions must be distinguishable, got {group_absent:?} for both"
    );

    // Rows: identifiers that resolve to no group at all — the bare name and bare id
    // forms (which the service resolves nothing from even when a group carries them),
    // a label nothing carries, and ARNs that differ from a stored one in exactly one
    // field.
    let arn = create_ok(&router, REGION, "policy-bearer").await;
    let id = id_of(&arn);
    for (form, identifier) in [
        ("the group's own name", "policy-bearer".to_string()),
        ("the group's own bare id", id.clone()),
        ("a name no group carries", "no-such-group".to_string()),
        ("an id no group carries", "A".repeat(16)),
        (
            "an ARN in another region",
            arn.replace(&format!(":{REGION}:"), ":eu-west-1:"),
        ),
        (
            "an ARN in another account",
            arn.replace(":000000000000:", ":111122223333:"),
        ),
        (
            "an ARN in another partition",
            arn.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
        (
            "an ARN carrying another id",
            arn.replace(&id, &"Z".repeat(16)),
        ),
    ] {
        let (status, code, body) = delete_policy(&router, &identifier).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {form:?}: expected a 404 ResourceNotFoundException, got {status} / {body:?}"
        );
        assert_eq!(
            message(&body),
            group_absent,
            "case {form:?}: expected the group reported absent, got {body:?}"
        );
        assert!(
            !message(&body).contains(&identifier),
            "case {form:?}: the message must not echo the identifier {identifier:?}, \
             got {body:?}"
        );
    }

    // Rows: a group that exists and carries no document — never put, and already
    // deleted — reports the POLICY absent instead, addressed by the ARN that resolves
    // it.
    for (form, identifier) in [("arn", arn.clone())] {
        let case = format!("never put, via {form}");
        let (status, code, body) = delete_policy(&router, &identifier).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: expected a 404 ResourceNotFoundException, got {status} / {body:?}"
        );
        assert_eq!(
            message(&body),
            policy_absent,
            "case {case}: expected the policy reported absent, got {body:?}"
        );

        let case = format!("deleted twice, via {form}");
        let (status, code, put) = put_policy(&router, &identifier, r#"{"Version":"2012-10-17"}"#).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the put, got {status} / {code} / {put:?}"
        );
        let (status, code, first) = delete_policy(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case}: the first delete must succeed, got {status} / {code} / {first:?}"
        );
        let (status, code, second) = delete_policy(&router, &identifier).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: a second delete has nothing to remove, got {status} / {second:?}"
        );
        assert_eq!(
            message(&second),
            policy_absent,
            "case {case}: expected the policy reported absent, got {second:?}"
        );
    }

    // And once the GROUP is gone the same call reports the group again, not the policy.
    let (status, code, body) = call(
        &router,
        "DELETE",
        &format!("/investigationGroups/{}", label(&arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the group: expected 200, got {status} / {code} / {body:?}"
    );
    let (status, code, body) = delete_policy(&router, &arn).await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"after the group is deleted\": expected a 404, got {status} / {body:?}"
    );
    assert_eq!(
        message(&body),
        group_absent,
        "case \"after the group is deleted\": expected the group reported absent, got {body:?}"
    );
}

#[tokio::test]
async fn deleting_one_policy_leaves_every_other_groups_policy_alone() {
    let router = emulator();

    // Rows: several groups each carrying their own document, so what the delete
    // removes is keyed to the ONE record it resolved and not to the path. An account
    // holds one group per Region, so each peer lives in its own Region.
    let names = ["peer-one", "peer-two", "peer-three", "peer-four"];
    let mut groups = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let arn = create_ok(&router, &region(index), name).await;
        let document = format!("{{\"Sid\":\"{name}\"}}");
        let (status, code, put) = put_policy(&router, &arn, &document).await;
        assert_eq!(
            status, 200,
            "case {name}: expected 200 from the put, got {status} / {code} / {put:?}"
        );
        groups.push((*name, arn, document));
    }

    // Delete the second group's policy, addressed by its ARN.
    let (status, code, body) = delete_policy(&router, &groups[1].1).await;
    assert_eq!(
        status, 200,
        "deleting {}'s policy: expected 200, got {status} / {code} / {body:?}",
        names[1]
    );

    for (name, arn, document) in &groups {
        let (status, code, got) = get_policy(&router, arn).await;
        if *name == names[1] {
            assert_eq!(
                (status, code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {name}: its policy was the one deleted, got {status} / {got:?}"
            );
        } else {
            assert_eq!(
                (status, got.get("policy").and_then(Value::as_str)),
                (200, Some(document.as_str())),
                "case {name}: an untouched policy must still read back, \
                 got {status} / {code} / {got:?}"
            );
        }
    }
}
