// SPDX-License-Identifier: CC0-1.0

//! The region a lifecycle policy belongs to, end to end over HTTP.
//!
//! The rule under test: Amazon Data Lifecycle Manager is a regional control
//! plane, so a lifecycle policy exists only in the region whose endpoint created
//! it — `GetLifecyclePolicies` reports the policies of the region the call is
//! made in, and every operation that addresses a policy by id or by ARN resolves
//! it in that region alone, answering `ResourceNotFoundException` (HTTP 404) for
//! one that lives in another region.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same
//! path a client takes. The region of a call is the region of its SigV4
//! credential scope, which is what a per-region endpoint signs for.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSDataLifecycleManagerDefaultRole";

/// The modelled error a policy that does not exist is reported with, and its
/// status: `ResourceNotFoundException (HTTP 404)` (the crate's `OPERATIONS.md`
/// error list for each of the policy and tag operations).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
const RESOURCE_NOT_FOUND_STATUS: u16 = 404;

/// The `Code` member that error carries, spelled out here rather than read from
/// the crate so a wrong constant cannot make this test pass.
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";

/// The status `TagResource` answers (`SUCCESS_STATUS` 204, a body-less write) as
/// against the 200 of the other tag operations.
const TAG_RESOURCE_STATUS: u16 = 204;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The ids `GetLifecyclePolicies` reported, in the order it reported them.
    fn listed_ids(&self) -> Vec<String> {
        self.body["Policies"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "GetLifecyclePolicies must return a Policies list, got {:?}",
                    self.body
                )
            })
            .iter()
            .map(|summary| summary["PolicyId"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// A SigV4 credential scope naming this service in `region` — what makes a call
/// a call to that region's endpoint.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{region}.amazonaws.com"))
        .header("authorization", signed_for(region));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create_in(router: &MultiServiceRouter, region: &str, request: Value) -> Answer {
    call_in(router, region, "POST", "/policies", Some(request)).await
}

async fn get_in(router: &MultiServiceRouter, region: &str, id: &str) -> Answer {
    call_in(router, region, "GET", &format!("/policies/{id}"), None).await
}

async fn list_in(router: &MultiServiceRouter, region: &str) -> Answer {
    call_in(router, region, "GET", "/policies", None).await
}

/// `/tags/{ResourceArn}` for `arn`. The label is a single path segment, so the
/// ARN's `/` is percent-encoded.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", arn.replace('/', "%2F"))
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The ARN the policy `id` reports, read back through `GetLifecyclePolicy` in the
/// region that holds it.
async fn policy_arn(router: &MultiServiceRouter, region: &str, id: &str) -> String {
    let read = get_in(router, region, id).await;
    let arn = at(&read.body["Policy"], "/PolicyArn");
    arn.as_str()
        .unwrap_or_else(|| {
            panic!("a policy read in {region:?} must report its ARN, got {:?}", read.body)
        })
        .to_string()
}

/// One region pair, with the kind of policy created in its home region.
struct Case {
    /// The region the create is signed for: the one region the policy exists in.
    home: &'static str,
    /// Another region, whose callers must see nothing of it.
    away: &'static str,
    label: &'static str,
    policy: Value,
}

/// A custom EBS snapshot policy's `PolicyDetails`, as a client sends it.
fn custom_snapshot_details() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// A custom EBS-backed AMI policy's `PolicyDetails`.
fn custom_image_details() -> Value {
    json!({
        "PolicyType": "IMAGE_MANAGEMENT",
        "ResourceTypes": ["INSTANCE"],
        "TargetTags": [{"Key": "backup", "Value": "weekly"}],
        "Schedules": [{
            "Name": "weekly",
            "CreateRule": {"Interval": 12, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 2},
        }],
    })
}

/// The rows the rule is asserted over: four disjoint region pairs, spanning the
/// kinds of policy a create can mint (a default policy of each
/// `DefaultPolicyTypeValues`, a custom snapshot policy and a custom AMI policy),
/// since the rule is about where a policy lives rather than what it configures.
fn cases() -> Vec<Case> {
    vec![
        Case {
            home: "us-east-1",
            away: "eu-west-1",
            label: "a default VOLUME policy created in us-east-1",
            policy: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "default volume policy",
                "State": "ENABLED",
                "DefaultPolicy": "VOLUME",
            }),
        },
        Case {
            home: "ap-northeast-1",
            away: "us-west-2",
            label: "a default INSTANCE policy created in ap-northeast-1",
            policy: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "default instance policy",
                "State": "DISABLED",
                "DefaultPolicy": "INSTANCE",
            }),
        },
        Case {
            home: "eu-central-1",
            away: "sa-east-1",
            label: "a custom snapshot policy created in eu-central-1",
            policy: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "custom snapshot policy",
                "State": "ENABLED",
                "Tags": {"team": "backups"},
                "PolicyDetails": custom_snapshot_details(),
            }),
        },
        Case {
            home: "ca-central-1",
            away: "ap-southeast-2",
            label: "a custom AMI policy created in ca-central-1",
            policy: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "custom image policy",
                "State": "ENABLED",
                "PolicyDetails": custom_image_details(),
            }),
        },
    ]
}

/// A policy is reachable from the region it was created in and from no other: the
/// region's own list reports it, every other region's list reports nothing of it,
/// and a read, an update, a delete or a tag operation signed for another region
/// is answered as though the policy does not exist.
///
/// One emulator serves every row, so eight regions hold state at once and a row's
/// assertions are about the partition rather than about an empty store.
#[tokio::test]
async fn a_policy_is_reachable_only_from_the_region_that_created_it() {
    let router = emulator();

    for case in cases() {
        let Case {
            home,
            away,
            label,
            policy,
        } = case;

        let created = create_in(&router, home, policy).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateLifecyclePolicy, got {} with {:?}",
            created.status, created.body
        );
        let id = created.policy_id();
        let arn = policy_arn(&router, home, &id).await;
        assert_eq!(
            arn,
            format!(
                "arn:aws:dlm:{home}:{}:policy/{id}",
                engine::emulator_account_id()
            ),
            "case {label:?}: the ARN must name the region the policy lives in, got {arn:?}"
        );

        // The home region reports it; the other region reports nothing at all.
        assert!(
            list_in(&router, home).await.listed_ids().contains(&id),
            "case {label:?}: {home} must list the policy it holds, got {:?}",
            list_in(&router, home).await.listed_ids()
        );
        let away_listed = list_in(&router, away).await;
        assert_eq!(
            away_listed.status, 200,
            "case {label:?}: expected HTTP 200 from GetLifecyclePolicies in {away}, got {} \
             with {:?}",
            away_listed.status, away_listed.body
        );
        assert_eq!(
            away_listed.listed_ids(),
            Vec::<String>::new(),
            "case {label:?}: {away} holds no policy, so its list must be empty, got {:?}",
            away_listed.body
        );

        // Every operation that addresses the policy by id, signed for the other
        // region, is answered as though it does not exist.
        let by_id: Vec<(&str, &str, String, Option<Value>)> = vec![
            ("GetLifecyclePolicy", "GET", format!("/policies/{id}"), None),
            (
                "UpdateLifecyclePolicy",
                "PATCH",
                format!("/policies/{id}"),
                Some(json!({"Description": "written from another region"})),
            ),
            (
                "GetLifecyclePolicies by policyIds",
                "GET",
                format!("/policies?policyIds={id}"),
                None,
            ),
            (
                "DeleteLifecyclePolicy",
                "DELETE",
                format!("/policies/{id}"),
                None,
            ),
        ];
        for (operation, method, uri, body) in by_id {
            let answer = call_in(&router, away, method, &uri, body).await;
            let case = format!("{label}, {operation} signed for {away}");
            assert_eq!(
                answer.status, RESOURCE_NOT_FOUND_STATUS,
                "case {case:?}: expected HTTP {RESOURCE_NOT_FOUND_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().ends_with(RESOURCE_NOT_FOUND_EXCEPTION),
                "case {case:?}: expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.body["Code"],
                json!(RESOURCE_NOT_FOUND_CODE),
                "case {case:?}: expected Code {RESOURCE_NOT_FOUND_CODE:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["ResourceIds"],
                json!([id]),
                "case {case:?}: ResourceIds must name the id that resolved to nothing, got {:?}",
                answer.body
            );
        }

        // And every operation that addresses it by ARN — the ARN names the home
        // region, so it resolves to nothing in any other region's store.
        let by_arn: Vec<(&str, &str, String, Option<Value>)> = vec![
            ("ListTagsForResource", "GET", tags_uri(&arn), None),
            (
                "TagResource",
                "POST",
                tags_uri(&arn),
                Some(json!({"Tags": {"env": "prod"}})),
            ),
            (
                "UntagResource",
                "DELETE",
                format!("{}?tagKeys=team", tags_uri(&arn)),
                None,
            ),
        ];
        for (operation, method, uri, body) in by_arn {
            let answer = call_in(&router, away, method, &uri, body).await;
            let case = format!("{label}, {operation} signed for {away}");
            assert_eq!(
                answer.status, RESOURCE_NOT_FOUND_STATUS,
                "case {case:?}: expected HTTP {RESOURCE_NOT_FOUND_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().ends_with(RESOURCE_NOT_FOUND_EXCEPTION),
                "case {case:?}: expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, got {:?}",
                answer.error_code()
            );
        }

        // None of those refusals touched the record: the home region still reads
        // it, with the description and the tags it already held.
        let still_there = get_in(&router, home, &id).await;
        assert_eq!(
            still_there.status, 200,
            "case {label:?}: the policy must survive every call signed for {away}, got {} \
             with {:?}",
            still_there.status, still_there.body
        );
        assert_ne!(
            at(&still_there.body["Policy"], "/Description"),
            json!("written from another region"),
            "case {label:?}: an update signed for {away} must not reach the record, got {:?}",
            at(&still_there.body["Policy"], "/Description")
        );
        let home_tags = call_in(&router, home, "GET", &tags_uri(&arn), None).await;
        assert_eq!(
            home_tags.status, 200,
            "case {label:?}: expected HTTP 200 from ListTagsForResource in {home}, got {} \
             with {:?}",
            home_tags.status, home_tags.body
        );
        assert_eq!(
            at(&home_tags.body, "/Tags")["env"],
            Value::Null,
            "case {label:?}: a TagResource signed for {away} must not reach the record, got {:?}",
            at(&home_tags.body, "/Tags")
        );

        // The home region tags it, and the home region reads that tag back.
        let tagged = call_in(
            &router,
            home,
            "POST",
            &tags_uri(&arn),
            Some(json!({"Tags": {"env": "prod"}})),
        )
        .await;
        assert_eq!(
            tagged.status, TAG_RESOURCE_STATUS,
            "case {label:?}: expected HTTP {TAG_RESOURCE_STATUS} from TagResource in {home}, \
             got {} with {:?}",
            tagged.status, tagged.body
        );
        assert_eq!(
            at(
                &call_in(&router, home, "GET", &tags_uri(&arn), None)
                    .await
                    .body,
                "/Tags"
            )["env"],
            json!("prod"),
            "case {label:?}: {home} must read back the tag it wrote"
        );

        // The home region deletes it, and then holds nothing either.
        let deleted = call_in(&router, home, "DELETE", &format!("/policies/{id}"), None).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected HTTP 200 from DeleteLifecyclePolicy in {home}, got {} \
             with {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            get_in(&router, home, &id).await.status,
            RESOURCE_NOT_FOUND_STATUS,
            "case {label:?}: a deleted policy is gone from its own region too"
        );
        assert_eq!(
            list_in(&router, home).await.listed_ids(),
            Vec::<String>::new(),
            "case {label:?}: {home} must list nothing once its only policy is deleted, got {:?}",
            list_in(&router, home).await.body
        );
    }
}

/// The same request body sent to two regions makes two independent policies: each
/// region holds its own record, under its own id and its own ARN, and a write or
/// a delete in one leaves the other exactly as it was.
#[tokio::test]
async fn the_same_create_in_two_regions_makes_two_independent_policies() {
    let router = emulator();
    let request = json!({
        "ExecutionRoleArn": ROLE_ARN,
        "Description": "identical in both regions",
        "State": "ENABLED",
        "DefaultPolicy": "VOLUME",
    });

    let first_region = "us-east-1";
    let second_region = "eu-west-1";
    let first = create_in(&router, first_region, request.clone())
        .await
        .policy_id();
    let second = create_in(&router, second_region, request.clone())
        .await
        .policy_id();

    assert_ne!(
        first, second,
        "case {first_region:?}/{second_region:?}: two policies must not share an id"
    );
    for (region, id) in [(first_region, &first), (second_region, &second)] {
        assert_eq!(
            list_in(&router, region).await.listed_ids(),
            vec![id.clone()],
            "case {region:?}: a region lists exactly its own policies, got {:?}",
            list_in(&router, region).await.body
        );
        let arn = policy_arn(&router, region, id).await;
        assert_eq!(
            arn,
            format!(
                "arn:aws:dlm:{region}:{}:policy/{id}",
                engine::emulator_account_id()
            ),
            "case {region:?}: the ARN must name the region the policy lives in, got {arn:?}"
        );
    }

    // An update in the first region reaches that region's record alone.
    let updated = call_in(
        &router,
        first_region,
        "PATCH",
        &format!("/policies/{first}"),
        Some(json!({"Description": "only the first region"})),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {first_region:?}: expected HTTP 200 from UpdateLifecyclePolicy, got {} with {:?}",
        updated.status, updated.body
    );
    assert_eq!(
        at(
            &get_in(&router, second_region, &second).await.body["Policy"],
            "/Description"
        ),
        json!("identical in both regions"),
        "case {second_region:?}: the other region's policy must be untouched"
    );

    // And a delete in the first region leaves the second region's policy whole.
    let deleted = call_in(
        &router,
        first_region,
        "DELETE",
        &format!("/policies/{first}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case {first_region:?}: expected HTTP 200 from DeleteLifecyclePolicy, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        get_in(&router, second_region, &second).await.status,
        200,
        "case {second_region:?}: a delete in {first_region} must not remove this region's policy"
    );
    assert_eq!(
        get_in(&router, first_region, &first).await.status,
        RESOURCE_NOT_FOUND_STATUS,
        "case {first_region:?}: the deleted policy must be gone from the region that held it"
    );
}
