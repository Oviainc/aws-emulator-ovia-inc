// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateInvestigationGroup` ADDRESSED BY THE GROUP'S ARN.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: an update addresses its group through the group's ARN, so for
//! every member the operation accepts the change reaches the stored record, every member
//! the request omitted is left alone and the change is recorded on
//! `lastModifiedBy`/`lastModifiedAt`; the same update addressed by a bare label — the
//! group's own name included — resolves no group and is answered with the modelled
//! `ResourceNotFoundException` (404) without touching the record. The modelled pattern
//! (`smithy_query(query="shape",shape="InvestigationGroupIdentifier")`) admits the bare
//! label beside the ARN
//! (`^(?:[\-_A-Za-z0-9]{1,512}|arn:...:investigation-group\/[A-Za-z0-9]{16})$`), which is
//! a well-formedness rule and not an address.
//!
//! Rows span the MEMBER SET the operation accepts
//! (`smithy_query(query="operation",operation="UpdateInvestigationGroup")`:
//! `roleArn`, `encryptionConfiguration`, `tagKeyBoundaries`,
//! `chatbotNotificationChannel`, `isCloudTrailEventHistoryEnabled`,
//! `crossAccountConfigurations`) and so differ along the member's type — boolean,
//! string, structure, list, map, list of structures — and along how many members one
//! request carries: one, all of them, and none. Each row is applied through the ARN and
//! then attempted again through the name.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// A second role ARN, so a replacement is distinguishable from the created value.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRoleTwo";

/// Real AWS Regions; past them [`region`] synthesizes a name. Each row needs a Region
/// of its own per group, because an account holds one investigation group per Region.
const REGIONS: &[&str] = &["us-west-2", "us-east-1", "eu-central-1", "ap-southeast-4"];

/// Members whose value the request under test did not set and that a reader must
/// therefore report unchanged are compared member by member; these two are what a
/// successful mutation is allowed to move, so they are excluded from that comparison
/// and asserted separately.
const MUTATION_STAMPS: &[&str] = &["lastModifiedAt", "lastModifiedBy"];

/// The error code this service's resource operations report an unresolvable identifier
/// with (`smithy_query(query="shape",shape="ResourceNotFoundException")`: `@httpError
/// 404`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

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

/// `GetInvestigationGroup` through `identifier` in `region`, which must report the
/// group.
async fn get_ok(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    case: &str,
) -> Map<String, Value> {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{}", encode(identifier)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the group back must answer 200, got {status} / {body:?}"
    );
    body.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a group document, got {body:?}"))
        .clone()
}

/// `UpdateInvestigationGroup` through `identifier` in `region`, which must succeed.
async fn update_ok(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    patch: &Value,
    case: &str,
) {
    let (status, answer) = call_in(
        router,
        region,
        "PATCH",
        &format!("/investigationGroups/{}", encode(identifier)),
        Some(patch),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: an update addressed by this form must answer 200, \
         got {status} / {answer:?}"
    );
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

/// The `lastModifiedAt` stamp a group reports.
fn last_modified_at(group: &Map<String, Value>, case: &str) -> i64 {
    group
        .get("lastModifiedAt")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a lastModifiedAt, got {group:?}"))
}

/// One row: a request body, and the members it is expected to change.
struct UpdateCase {
    case: &'static str,
    patch: Value,
    /// `(member, the value a later read must report)` for each member the body carries.
    expected: Vec<(&'static str, Value)>,
}

/// An SNS topic ARN long enough for the modelled `chatbotNotificationChannel` key
/// (len 20..2048) and one chat-application configuration ARN for it (the list is
/// modelled 1..5).
fn chat_channel() -> Value {
    json!({
        "arn:aws:sns:us-west-2:000000000000:investigation-notifications": [
            "arn:aws:chatbot::000000000000:chat-configuration/slack-channel/ops"
        ]
    })
}

/// A customer managed key configuration, whose `type` is a value of the modelled
/// `EncryptionConfigurationType` set
/// (`smithy_query(query="shape",shape="EncryptionConfigurationType")`).
fn customer_managed_key() -> Value {
    json!({
        "type": "CUSTOMER_MANAGED_KMS_KEY",
        "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/11111111-2222-3333-4444-555555555555"
    })
}

/// A cross-account configuration naming an account other than the investigating one,
/// which is what this service accepts there.
fn cross_account() -> Value {
    json!([{ "sourceRoleArn": "arn:aws:iam::111122223333:role/CrossAccountInvestigation" }])
}

/// One row per member the operation accepts, plus a request carrying all of them and a
/// request carrying none.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a boolean member turned off",
            patch: json!({ "isCloudTrailEventHistoryEnabled": false }),
            expected: vec![("isCloudTrailEventHistoryEnabled", json!(false))],
        },
        UpdateCase {
            case: "a string member replaced",
            patch: json!({ "roleArn": OTHER_ROLE_ARN }),
            expected: vec![("roleArn", json!(OTHER_ROLE_ARN))],
        },
        UpdateCase {
            case: "a structure member replaced",
            patch: json!({ "encryptionConfiguration": customer_managed_key() }),
            expected: vec![("encryptionConfiguration", customer_managed_key())],
        },
        UpdateCase {
            case: "a list member replaced",
            patch: json!({ "tagKeyBoundaries": ["Service", "Tier"] }),
            expected: vec![("tagKeyBoundaries", json!(["Service", "Tier"]))],
        },
        UpdateCase {
            case: "a map member replaced",
            patch: json!({ "chatbotNotificationChannel": chat_channel() }),
            expected: vec![("chatbotNotificationChannel", chat_channel())],
        },
        UpdateCase {
            case: "a list-of-structures member replaced",
            patch: json!({ "crossAccountConfigurations": cross_account() }),
            expected: vec![("crossAccountConfigurations", cross_account())],
        },
        UpdateCase {
            case: "every member the operation accepts at once",
            patch: json!({
                "isCloudTrailEventHistoryEnabled": false,
                "roleArn": OTHER_ROLE_ARN,
                "encryptionConfiguration": customer_managed_key(),
                "tagKeyBoundaries": ["Service"],
                "chatbotNotificationChannel": chat_channel(),
                "crossAccountConfigurations": cross_account(),
            }),
            expected: vec![
                ("isCloudTrailEventHistoryEnabled", json!(false)),
                ("roleArn", json!(OTHER_ROLE_ARN)),
                ("encryptionConfiguration", customer_managed_key()),
                ("tagKeyBoundaries", json!(["Service"])),
                ("chatbotNotificationChannel", chat_channel()),
                ("crossAccountConfigurations", cross_account()),
            ],
        },
        UpdateCase {
            case: "a request carrying no member at all",
            patch: json!({}),
            expected: Vec::new(),
        },
    ]
}

/// An update addressed by the group's ARN applies the change to the stored record — for
/// every member the operation accepts — and leaves the members it omitted alone, while
/// the same update addressed by the group's NAME resolves no group and changes nothing.
#[tokio::test]
async fn an_update_addressed_by_the_arn_applies_every_member() {
    for (index, UpdateCase {
        case,
        patch,
        expected,
    }) in update_cases().into_iter().enumerate()
    {
        // One group per row, in its own Region (an account holds one group per Region).
        let router = emulator();
        let home = region(index);
        let name = "patched-investigation-group";
        let arn = create_ok(&router, &home, name).await;

        let before = get_ok(&router, &home, &arn, case).await;

        // The same request addressed by the group's own name addresses no group, so it
        // is refused and the record below is the one the ARN form produced alone.
        let (status, refused) = call_in(
            &router,
            &home,
            "PATCH",
            &format!("/investigationGroups/{}", encode(name)),
            Some(&patch),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, RESOURCE_NOT_FOUND),
            "case {case:?}: an update addressed by the name must be refused, \
             got {status} / {refused:?}"
        );
        let untouched = get_ok(&router, &home, &arn, case).await;
        assert_eq!(
            untouched, before,
            "case {case:?}: the refused update must leave the record as it was, \
             expected {before:?}, got {untouched:?}"
        );

        update_ok(&router, &home, &arn, &patch, case).await;

        let after = get_ok(&router, &home, &arn, case).await;

        // Every member the request carried now reads back as the request asked.
        for (member, want) in &expected {
            assert_eq!(
                after.get(*member),
                Some(want),
                "case {case:?}: expected {member} to read back as {want}, \
                 got {:?}",
                after.get(*member)
            );
        }

        // Every other member is untouched — including the ones the update may move,
        // which are asserted below rather than here.
        let changed: Vec<&str> = expected.iter().map(|(member, _)| *member).collect();
        let untouched = |members: &Map<String, Value>| -> Vec<String> {
            members
                .keys()
                .filter(|member| {
                    !changed.contains(&member.as_str())
                        && !MUTATION_STAMPS.contains(&member.as_str())
                })
                .cloned()
                .collect()
        };
        for member in untouched(&before) {
            assert_eq!(
                after.get(&member),
                before.get(&member),
                "case {case:?}: {member} was not in the request and must be unchanged, \
                 expected {:?}, got {:?}",
                before.get(&member),
                after.get(&member)
            );
        }
        for member in untouched(&after) {
            assert_eq!(
                after.get(&member),
                before.get(&member),
                "case {case:?}: {member} was not in the request and must not appear, \
                 expected {:?}, got {:?}",
                before.get(&member),
                after.get(&member)
            );
        }

        // A successful update records itself: the stamp advances, whatever the clock
        // did between the two calls. Every member of this operation is optional, so a
        // request carrying none is still an accepted update of the record and is
        // stamped like any other — `lastModifiedAt` is what lets a caller notice a
        // configuration change and order two writers, which a stamp that sometimes
        // stood still could not do.
        let (was, now) = (
            last_modified_at(&before, case),
            last_modified_at(&after, case),
        );
        assert!(
            now > was,
            "case {case:?}: a successful update must advance lastModifiedAt, \
             was {was}, got {now}"
        );
        assert_eq!(
            after.get("lastModifiedBy"),
            before.get("createdBy"),
            "case {case:?}: lastModifiedBy must name the calling principal, got {after:?}"
        );
    }
}

/// Repeated updates addressed by the ARN each reach the same record: a boolean member
/// round-trips through both of its values, and every read reports the value the last
/// update set, while a read or an update through the name reports the group absent.
#[tokio::test]
async fn repeated_updates_addressed_by_the_arn_each_reach_the_stored_record() {
    let router = emulator();
    let home = region(0);
    let name = "toggled-investigation-group";
    let arn = create_ok(&router, &home, name).await;

    // The created default, then the two values the member can take, set alternately.
    let created = get_ok(&router, &home, &arn, "the created group").await;
    assert_eq!(
        created.get("isCloudTrailEventHistoryEnabled"),
        Some(&json!(true)),
        "the created group carries the documented default, got {created:?}"
    );

    let mut previous = last_modified_at(&created, "the created group");
    for (step, want) in [false, true, false, true].into_iter().enumerate() {
        let case = format!("step {step}: set to {want}");
        update_ok(
            &router,
            &home,
            &arn,
            &json!({ "isCloudTrailEventHistoryEnabled": want }),
            &case,
        )
        .await;
        let group = get_ok(&router, &home, &arn, &case).await;
        assert_eq!(
            group.get("isCloudTrailEventHistoryEnabled"),
            Some(&json!(want)),
            "case {case:?}: a read through the ARN must report {want}, got {group:?}"
        );
        let (status, refused) = call_in(
            &router,
            &home,
            "GET",
            &format!("/investigationGroups/{}", encode(name)),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, RESOURCE_NOT_FOUND),
            "case {case:?}: a read through the name must report the group absent, \
             got {status} / {refused:?}"
        );
        let group = get_ok(&router, &home, &arn, &case).await;
        let now = last_modified_at(&group, &case);
        assert!(
            now > previous,
            "case {case:?}: each applied update advances lastModifiedAt, \
             was {previous}, got {now}"
        );
        previous = now;
    }
}
