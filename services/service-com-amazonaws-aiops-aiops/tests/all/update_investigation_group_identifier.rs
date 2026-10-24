// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateInvestigationGroup` ADDRESSED BY THE GROUP'S NAME.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: an update addressed by either form the operation's `identifier`
//! admits reaches the SAME stored record, so for every member the operation accepts the
//! name form applies the change exactly as the ARN form does, leaves every member the
//! request omitted alone, and records the change on `lastModifiedBy`/`lastModifiedAt`.
//! The member's own documentation is
//! `smithy_query(query="doc",shape="com.amazonaws.aiops#UpdateInvestigationGroupRequest",member="identifier")`:
//! "Specify either the name or the ARN of the investigation group that you want to
//! modify."; the modelled pattern
//! (`smithy_query(query="shape",shape="InvestigationGroupIdentifier")`) spells the
//! alternation out as
//! `^(?:[\-_A-Za-z0-9]{1,512}|arn:...:investigation-group\/[A-Za-z0-9]{16})$`.
//!
//! Rows span the MEMBER SET the operation accepts
//! (`smithy_query(query="operation",operation="UpdateInvestigationGroup")`:
//! `roleArn`, `encryptionConfiguration`, `tagKeyBoundaries`,
//! `chatbotNotificationChannel`, `isCloudTrailEventHistoryEnabled`,
//! `crossAccountConfigurations`) and so differ along the member's type — boolean,
//! string, structure, list, map, list of structures — and along how many members one
//! request carries: one, all of them, and none. Each row is applied twice over twin
//! groups, once through the name and once through the ARN, and the two resulting
//! records are compared.

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

/// Members that differ between two groups by construction — the ARN names the Region
/// and the id, and the two twins are created at whatever instant the clock reads — so
/// they are dropped before the two records are compared.
const PER_RESOURCE_MEMBERS: &[&str] = &["arn", "createdAt", "lastModifiedAt"];

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

/// A group document without the members two distinct resources differ in by
/// construction, so what remains is the configuration alone.
fn configuration(group: &Map<String, Value>) -> Map<String, Value> {
    group
        .iter()
        .filter(|(member, _)| !PER_RESOURCE_MEMBERS.contains(&member.as_str()))
        .map(|(member, value)| (member.clone(), value.clone()))
        .collect()
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

/// An update addressed by the group's NAME applies the change to the stored record —
/// for every member the operation accepts — exactly as the same update addressed by the
/// group's ARN does, and leaves the members it omitted alone.
#[tokio::test]
async fn an_update_addressed_by_name_applies_every_member_as_the_arn_form_does() {
    for (index, UpdateCase {
        case,
        patch,
        expected,
    }) in update_cases().into_iter().enumerate()
    {
        // Twin groups, one per Region (an account holds one group per Region): the row
        // is applied to one through its name and to the other through its ARN.
        let router = emulator();
        let named_region = region(2 * index);
        let arn_region = region(2 * index + 1);
        let name = "patched-investigation-group";
        let named_arn = create_ok(&router, &named_region, name).await;
        let arn_arn = create_ok(&router, &arn_region, name).await;

        let before = get_ok(&router, &named_region, name, case).await;

        update_ok(&router, &named_region, name, &patch, case).await;
        update_ok(&router, &arn_region, &arn_arn, &patch, case).await;

        // The record the name addressed is the one the ARN reads back.
        let after = get_ok(&router, &named_region, name, case).await;
        let after_via_arn = get_ok(&router, &named_region, &named_arn, case).await;
        assert_eq!(
            after, after_via_arn,
            "case {case:?}: both identifier forms address one record, \
             via the name {after:?} vs via the ARN {after_via_arn:?}"
        );

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

        // And the two forms produced the same configuration on their own records.
        let via_arn = get_ok(&router, &arn_region, &arn_arn, case).await;
        assert_eq!(
            configuration(&after),
            configuration(&via_arn),
            "case {case:?}: the name form and the ARN form must apply the same change, \
             via the name {after:?} vs via the ARN {via_arn:?}"
        );
    }
}

/// Repeated updates addressed by the name each reach the same record: a boolean member
/// round-trips through both of its values, and every read reports the value the last
/// update set.
#[tokio::test]
async fn repeated_updates_addressed_by_name_each_reach_the_stored_record() {
    let router = emulator();
    let home = region(0);
    let name = "toggled-investigation-group";
    let arn = create_ok(&router, &home, name).await;

    // The created default, then the two values the member can take, set alternately.
    let created = get_ok(&router, &home, name, "the created group").await;
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
            name,
            &json!({ "isCloudTrailEventHistoryEnabled": want }),
            &case,
        )
        .await;
        for (via, identifier) in [("the name", name), ("the ARN", arn.as_str())] {
            let group = get_ok(&router, &home, identifier, &case).await;
            assert_eq!(
                group.get("isCloudTrailEventHistoryEnabled"),
                Some(&json!(want)),
                "case {case:?}: read via {via} must report {want}, got {group:?}"
            );
        }
        let group = get_ok(&router, &home, name, &case).await;
        let now = last_modified_at(&group, &case);
        assert!(
            now > previous,
            "case {case:?}: each applied update advances lastModifiedAt, \
             was {previous}, got {now}"
        );
        previous = now;
    }
}
