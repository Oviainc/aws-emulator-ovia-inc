// SPDX-License-Identifier: CC0-1.0

//! Resolving a policy by its ARN, shared by every ARN-addressed operation.
//!
//! The rule under test: an operation that addresses a resource by `ResourceArn`
//! acts on the policy whose STORED `PolicyArn` equals that ARN, among the
//! policies of the region the request was signed for, and answers
//! `ResourceNotFoundException` (HTTP 404) — with the exception's `Code`, the
//! `ResourceType` token of the resource that was not found, and a message naming
//! the ARN that resolved to nothing — for an ARN that names none. The three
//! operations bound to `/tags/{ResourceArn}` (`ListTagsForResource`,
//! `TagResource`, `UntagResource`) share that one resolution, so each row is
//! driven through all three.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSDataLifecycleManagerDefaultRole";

/// The region most rows are signed for.
const REGION: &str = "us-east-1";
/// Another region, used for the rows about an ARN that names a policy elsewhere.
const OTHER_REGION: &str = "eu-west-1";

/// The modelled refusal for a resource that was not found, and its status
/// (`ResourceNotFoundException (HTTP 404)`, from the crate's `OPERATIONS.md`
/// error list for each tag operation).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
const RESOURCE_NOT_FOUND_STATUS: u16 = 404;
/// The members that refusal carries, spelled out here rather than read from the
/// crate so a wrong constant cannot make this test pass.
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";
const POLICY_RESOURCE_TYPE: &str = "LIFECYCLE_POLICY";

/// The status `TagResource` answers (a body-less write).
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
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
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

/// `/tags/{ResourceArn}` for `arn`. The label is a single path segment, so the
/// ARN's `/` is percent-encoded.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", arn.replace('/', "%2F"))
}

/// A custom EBS snapshot policy's `PolicyDetails`, as a client sends it.
fn snapshot_details() -> Value {
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

/// Create a policy in `region`, answering its id and its stored `PolicyArn`.
async fn create_in(router: &MultiServiceRouter, region: &str, tags: Option<Value>) -> (String, String) {
    let mut request = json!({
        "ExecutionRoleArn": ROLE_ARN,
        "Description": "arn resolution subject",
        "State": "ENABLED",
        "PolicyDetails": snapshot_details(),
    });
    if let Some(tags) = tags {
        request["Tags"] = tags;
    }
    let created = call_in(router, region, "POST", "/policies", Some(request)).await;
    assert_eq!(
        created.status, 200,
        "case {region:?}: expected HTTP 200 from CreateLifecyclePolicy, got {} with {:?}",
        created.status, created.body
    );
    let id = created.body["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {region:?}: a create must report a PolicyId, got {:?}", created.body))
        .to_string();
    let read = call_in(router, region, "GET", &format!("/policies/{id}"), None).await;
    let arn = read.body["Policy"]["PolicyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {region:?}: a policy must report its ARN, got {:?}", read.body))
        .to_string();
    (id, arn)
}

/// The tags `ListTagsForResource` reports for `arn`, as a JSON object.
async fn listed_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> Value {
    let listed = call_in(router, region, "GET", &tags_uri(arn), None).await;
    assert_eq!(
        listed.status, 200,
        "case {arn:?}: expected HTTP 200 from ListTagsForResource, got {} with {:?}",
        listed.status, listed.body
    );
    listed.body["Tags"].clone()
}

/// The three operations `/tags/{ResourceArn}` binds, as `(label, method, uri
/// suffix, body)` builders for one ARN.
fn tag_operations(arn: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        ("ListTagsForResource", "GET", tags_uri(arn), None),
        (
            "TagResource",
            "POST",
            tags_uri(arn),
            Some(json!({"Tags": {"written": "by the refused call"}})),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("{}?tagKeys=team", tags_uri(arn)),
            None,
        ),
    ]
}

/// RULE — an ARN that names no policy in the calling region is refused the same
/// way by every operation that addresses a resource by ARN, and the refusal
/// names that ARN.
///
/// The rows differ along WHY the ARN resolves to nothing: the policy lives in
/// another region, the id was never minted, the policy was created and deleted,
/// the account or the partition is another one, and the id differs from a live
/// one only in case — each a well-formed `PolicyArn` the input pattern admits.
#[tokio::test]
async fn an_arn_naming_no_policy_is_refused_identically_by_every_tag_operation() {
    let router = emulator();
    let account = engine::emulator_account_id();

    // A live policy in the calling region, so each refusal is about resolution
    // rather than about an empty store, and a live one elsewhere.
    let (live_id, live_arn) = create_in(&router, REGION, Some(json!({"team": "backups"}))).await;
    let (_, elsewhere_arn) = create_in(&router, OTHER_REGION, Some(json!({"team": "backups"}))).await;

    // One created in the calling region and then deleted: its ARN was once real.
    let (deleted_id, deleted_arn) = create_in(&router, REGION, None).await;
    let gone = call_in(&router, REGION, "DELETE", &format!("/policies/{deleted_id}"), None).await;
    assert_eq!(
        gone.status, 200,
        "the subject policy must delete, got {} with {:?}",
        gone.status, gone.body
    );

    let rows: Vec<(&str, String)> = vec![
        ("a policy that lives in another region", elsewhere_arn),
        (
            // The id counter mints ids from one upward, so the top of the
            // sixteen-hex-digit space is an id this store has not minted.
            "an id that was never minted",
            format!("arn:aws:dlm:{REGION}:{account}:policy/policy-ffffffffffffffff"),
        ),
        ("a policy that was created and deleted", deleted_arn),
        (
            "another account's policy",
            format!("arn:aws:dlm:{REGION}:999999999999:policy/{live_id}"),
        ),
        (
            "another partition's policy",
            format!("arn:aws-cn:dlm:{REGION}:{account}:policy/{live_id}"),
        ),
        (
            "the live id in upper case",
            format!(
                "arn:aws:dlm:{REGION}:{account}:policy/{}",
                live_id.to_uppercase()
            ),
        ),
    ];

    for (row, arn) in rows {
        for (operation, method, uri, body) in tag_operations(&arn) {
            let answer = call_in(&router, REGION, method, &uri, body).await;
            let case = format!("{row}, {operation} with {arn}");
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
                answer.body["ResourceType"],
                json!(POLICY_RESOURCE_TYPE),
                "case {case:?}: expected ResourceType {POLICY_RESOURCE_TYPE:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["Message"],
                json!(format!("The resource {{{arn}}} was not found.")),
                "case {case:?}: the refusal must name the ARN that resolved to nothing, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["ResourceIds"],
                Value::Null,
                "case {case:?}: the request named an ARN, so no id is echoed, got {:?}",
                answer.body
            );
        }
    }

    // And no refused call reached the live policy: it still holds exactly the
    // tags its create stored.
    assert_eq!(
        listed_tags(&router, REGION, &live_arn).await,
        json!({"team": "backups"}),
        "a refused ARN must leave the live policy's tags alone"
    );
}

/// RULE — the same resolution on the serving side: each operation acts on the
/// policy whose stored `PolicyArn` the request names, and on no other.
///
/// The rows differ along how many policies share the region, whether the policy
/// was created with tags, and which region holds it, so a write through one ARN
/// is observed against every other policy in the store.
#[tokio::test]
async fn every_tag_operation_acts_on_the_policy_its_arn_names() {
    let router = emulator();

    // Three policies: two in the calling region (one tagged at create, one
    // bare), one in another region.
    let (_, tagged_arn) = create_in(&router, REGION, Some(json!({"team": "backups"}))).await;
    let (_, bare_arn) = create_in(&router, REGION, None).await;
    let (_, other_region_arn) = create_in(&router, OTHER_REGION, Some(json!({"team": "elsewhere"}))).await;

    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a policy created with tags",
            REGION,
            tagged_arn.as_str(),
            json!({"team": "backups"}),
        ),
        (
            "a policy created with no tags",
            REGION,
            bare_arn.as_str(),
            json!({}),
        ),
        (
            "a policy in another region, called in that region",
            OTHER_REGION,
            other_region_arn.as_str(),
            json!({"team": "elsewhere"}),
        ),
    ];

    for (row, region, arn, created_tags) in &rows {
        let case = format!("{row} ({arn})");
        assert_eq!(
            listed_tags(&router, region, arn).await,
            *created_tags,
            "case {case:?}: ListTagsForResource must report the tags the create stored"
        );

        // TagResource writes through the ARN; ListTagsForResource reads it back.
        let tagged = call_in(
            &router,
            region,
            "POST",
            &tags_uri(arn),
            Some(json!({"Tags": {"env": "prod"}})),
        )
        .await;
        assert_eq!(
            tagged.status, TAG_RESOURCE_STATUS,
            "case {case:?}: expected HTTP {TAG_RESOURCE_STATUS} from TagResource, got {} with {:?}",
            tagged.status, tagged.body
        );
        let after_tag = listed_tags(&router, region, arn).await;
        assert_eq!(
            after_tag["env"],
            json!("prod"),
            "case {case:?}: the tag written through this ARN must be readable through it, got {after_tag:?}"
        );

        // UntagResource removes through the same ARN, leaving the rest.
        let untagged = call_in(
            &router,
            region,
            "DELETE",
            &format!("{}?tagKeys=env", tags_uri(arn)),
            None,
        )
        .await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: expected HTTP 200 from UntagResource, got {} with {:?}",
            untagged.status, untagged.body
        );
        let after_untag = listed_tags(&router, region, arn).await;
        assert_eq!(
            after_untag, *created_tags,
            "case {case:?}: removing the written key must restore the created tag set, got \
             {after_untag:?}"
        );
    }

    // A write through one ARN reached one policy only: every other policy still
    // reports the tag set its create stored.
    for (row, region, arn, created_tags) in &rows {
        let case = format!("{row} ({arn})");
        assert_eq!(
            listed_tags(&router, region, arn).await,
            *created_tags,
            "case {case:?}: a tag written through another ARN must not reach this policy"
        );
    }
}
