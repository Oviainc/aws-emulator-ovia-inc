// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the investigation-group slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: each of these operations answers from the ONE persisted
//! investigation-group record, so anything a create or an update stored is what a
//! later Get, List or GetPolicy reports, and anything never stored (or since
//! deleted) is a modelled 404.
//!
//! An account holds ONE investigation group per Region, so every row below that needs
//! its own group signs for its own Region ([`region`]).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state::{
    AWS_OWNED_KEY_TYPE, MAX_COLLECTION_ENTRIES, MAX_INVESTIGATION_GROUPS, MAX_TAGS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// Regions the rows that each need their own group sign for, in order.
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "us-east-2",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "ap-southeast-1",
    "ap-northeast-1",
    "sa-east-1",
    "ca-central-1",
];

/// The Region row `n` signs for.
///
/// An account holds one investigation group per Region, so a row that needs its own
/// group needs its own Region. The names are real AWS Regions; past the list they are
/// synthetic ones inside what a SigV4 credential scope admits (`[a-zA-Z0-9-]`).
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:04}"), |name| (*name).to_string())
}
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";
/// Length of the ARN's id segment, from the modelled `InvestigationGroupArn`
/// pattern `...:investigation-group/[A-Za-z0-9]{16}$`.
const GROUP_ID_LEN: usize = 16;

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
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&raw).to_string())
        })
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// [`call_in`] in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, uri, body).await
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

async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(router, region, "POST", "/investigationGroups", Some(body)).await
}

async fn get_in(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{}", label(identifier)),
        None,
    )
    .await
}

/// Create a group that must succeed in `region`, and return its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, body: &Value) -> String {
    let (status, created) = create(router, region, body).await;
    assert_eq!(status, 201, "create {body}: expected 201, got {created:?}");
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {body}: response carried no arn, got {created:?}"))
        .to_string()
}

/// Assert `arn` satisfies the modelled `InvestigationGroupArn` for `region`, and
/// answer its 16-character id segment.
fn assert_group_arn(case: &str, region: &str, arn: &str) -> String {
    let prefix = format!("arn:aws:aiops:{region}:{ACCOUNT}:investigation-group/");
    let id = arn
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case {case:?}: expected an ARN starting {prefix}, got {arn:?}"));
    assert_eq!(
        id.len(),
        GROUP_ID_LEN,
        "case {case:?}: expected a {GROUP_ID_LEN}-character id segment, got {id:?}"
    );
    assert!(
        id.chars().all(char::is_alphanumeric) && id.is_ascii(),
        "case {case:?}: expected an [A-Za-z0-9] id segment, got {id:?}"
    );
    id.to_string()
}

/// A row of the create/read-back table: a request body, and the members a later
/// `GetInvestigationGroup` must report for it.
struct GroupCase {
    /// The name, which is also how the row is reported in a failure.
    name: &'static str,
    /// The create request body.
    request: Value,
    /// Every member the read-back must carry, exactly.
    expected: Value,
}

/// The table: the rule is "a Get reports what the create stored", so the rows differ
/// along what the rule generalises over — which optional members are present, the
/// retention value (both published bounds and inside), collection sizes, the
/// encryption type, and the shape of the name.
fn group_cases() -> Vec<GroupCase> {
    vec![
        // Only the two required members: the documented defaults must be stored.
        // `retentionInDays`: "If you omit this parameter, the default of 90 days is
        // used." `isCloudTrailEventHistoryEnabled`: "The default is true."
        // `encryptionConfiguration`: "If you omit this parameter, CloudWatch
        // investigations will use an Amazon Web Services key to encrypt the data."
        GroupCase {
            name: "minimal-group",
            request: json!({ "name": "minimal-group", "roleArn": ROLE_ARN }),
            expected: json!({
                "name": "minimal-group",
                "roleArn": ROLE_ARN,
                "retentionInDays": 90,
                "isCloudTrailEventHistoryEnabled": true,
                "encryptionConfiguration": { "type": AWS_OWNED_KEY_TYPE },
            }),
        },
        // The low published bound of the modelled `retentionInDays` range (7..90),
        // the AWS-owned encryption type, and every collection at size 1-2.
        GroupCase {
            name: "retention-low-bound",
            request: json!({
                "name": "retention-low-bound",
                "roleArn": ROLE_ARN,
                "retentionInDays": 7,
                "isCloudTrailEventHistoryEnabled": false,
                "encryptionConfiguration": { "type": AWS_OWNED_KEY_TYPE },
                "tagKeyBoundaries": ["Application", "CostCenter"],
                "chatbotNotificationChannel": {
                    "arn:aws:sns:us-west-2:000000000000:aiops-alerts": [
                        "arn:aws:chatbot::000000000000:chat-configuration/slack-channel/ops"
                    ]
                },
                "crossAccountConfigurations": [
                    { "sourceRoleArn": "arn:aws:iam::111122223333:role/SourceOne" }
                ],
            }),
            expected: json!({
                "name": "retention-low-bound",
                "roleArn": ROLE_ARN,
                "retentionInDays": 7,
                "isCloudTrailEventHistoryEnabled": false,
                "encryptionConfiguration": { "type": AWS_OWNED_KEY_TYPE },
                "tagKeyBoundaries": ["Application", "CostCenter"],
                "chatbotNotificationChannel": {
                    "arn:aws:sns:us-west-2:000000000000:aiops-alerts": [
                        "arn:aws:chatbot::000000000000:chat-configuration/slack-channel/ops"
                    ]
                },
                "crossAccountConfigurations": [
                    { "sourceRoleArn": "arn:aws:iam::111122223333:role/SourceOne" }
                ],
            }),
        },
        // The high published bound of the same range, the customer-managed
        // encryption type with its key, and a longer cross-account list.
        GroupCase {
            name: "retention-high-bound",
            request: json!({
                "name": "retention-high-bound",
                "roleArn": "arn:aws:iam::000000000000:role/AnotherAIOpsRole",
                "retentionInDays": 90,
                "encryptionConfiguration": {
                    "type": "CUSTOMER_MANAGED_KMS_KEY",
                    "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/abcd-1234"
                },
                "crossAccountConfigurations": [
                    { "sourceRoleArn": "arn:aws:iam::111122223333:role/SourceOne" },
                    { "sourceRoleArn": "arn:aws:iam::111122223333:role/SourceTwo" },
                    { "sourceRoleArn": "arn:aws:iam::444455556666:role/SourceThree" }
                ],
            }),
            expected: json!({
                "name": "retention-high-bound",
                "roleArn": "arn:aws:iam::000000000000:role/AnotherAIOpsRole",
                "retentionInDays": 90,
                "isCloudTrailEventHistoryEnabled": true,
                "encryptionConfiguration": {
                    "type": "CUSTOMER_MANAGED_KMS_KEY",
                    "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/abcd-1234"
                },
                "crossAccountConfigurations": [
                    { "sourceRoleArn": "arn:aws:iam::111122223333:role/SourceOne" },
                    { "sourceRoleArn": "arn:aws:iam::111122223333:role/SourceTwo" },
                    { "sourceRoleArn": "arn:aws:iam::444455556666:role/SourceThree" }
                ],
            }),
        },
        // A retention inside the range, and a name using the punctuation the
        // modelled name pattern allows but the bare identifier form does not.
        GroupCase {
            name: "punctuated name",
            request: json!({
                "name": "Prod [us-west-2] (tier.1): billing",
                "roleArn": ROLE_ARN,
                "retentionInDays": 30,
                "tagKeyBoundaries": ["Service"],
            }),
            expected: json!({
                "name": "Prod [us-west-2] (tier.1): billing",
                "roleArn": ROLE_ARN,
                "retentionInDays": 30,
                "isCloudTrailEventHistoryEnabled": true,
                "encryptionConfiguration": { "type": AWS_OWNED_KEY_TYPE },
                "tagKeyBoundaries": ["Service"],
            }),
        },
    ]
}

/// Every member of `expected` must appear in `actual` with exactly that value.
fn assert_members(case: &str, via: &str, expected: &Value, actual: &Value) {
    let expected = expected.as_object().expect("an object of expected members");
    for (member, want) in expected {
        let got = actual.get(member);
        assert_eq!(
            got,
            Some(want),
            "case {case:?} read via {via}: expected {member} = {want}, got {got:?} \
             in the whole response {actual:?}"
        );
    }
}

/// A created group reads back through its ARN — and through nothing else: the bare
/// identifier forms the modelled pattern admits resolve no group.
#[tokio::test]
async fn a_created_group_reads_back_through_its_arn() {
    let router = emulator();
    let cases = group_cases();

    // Create them all first — one per Region, since an account holds one group per
    // Region — so each read-back also proves the store keeps the rows apart rather
    // than answering with the most recent one.
    let mut arns = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let region = region(index);
        let arn = create_ok(&router, &region, &case.request).await;
        assert_group_arn(case.name, &region, &arn);
        arns.push((region, arn));
    }

    for (case, (region, arn)) in cases.iter().zip(&arns) {
        let id = assert_group_arn(case.name, region, arn);

        let (status, body) = get_in(&router, region, arn).await;
        assert_eq!(
            status, 200,
            "case {:?} read via its ARN: expected 200, got {status} / {body:?}",
            case.name
        );
        assert_members(case.name, "arn", &case.expected, &body);
        assert_eq!(
            body.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {:?}: expected arn {arn}, got {body:?}",
            case.name
        );

        // A read never changes what the next caller sees.
        let (_, again) = get_in(&router, region, arn).await;
        assert_eq!(
            again, body,
            "case {:?}: a second read must answer the identical body",
            case.name
        );

        // The bare forms the identifier pattern also admits name no group. A name
        // outside that bare class (`[\-_A-Za-z0-9]{1,512}`) never even reaches the
        // handler: the modelled pattern refuses it at the boundary.
        let name = case.request["name"].as_str().unwrap_or_default().to_string();
        let name_is_bare_identifier = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        for (form, identifier, want) in [
            (
                "its own name",
                name,
                if name_is_bare_identifier { 404 } else { 400 },
            ),
            ("its own bare id", id, 404),
        ] {
            let (status, answer) = get_in(&router, region, &identifier).await;
            assert_eq!(
                status, want,
                "case {:?} read via {form}: a bare identifier resolves no group, \
                 got {status} / {answer:?}",
                case.name
            );
        }

        // The timestamps and the principal come from the create, not from the read.
        let created_at = body
            .get("createdAt")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {:?}: no createdAt in {body:?}", case.name));
        assert!(
            created_at > 1_600_000_000,
            "case {:?}: expected createdAt in epoch SECONDS, got {created_at}",
            case.name
        );
        assert_eq!(
            body.get("lastModifiedAt").and_then(Value::as_i64),
            Some(created_at),
            "case {:?}: an untouched group's lastModifiedAt must equal createdAt, got {body:?}",
            case.name
        );
        for member in ["createdBy", "lastModifiedBy"] {
            let principal = body.get(member).and_then(Value::as_str).unwrap_or("");
            assert!(
                !principal.is_empty()
                    && principal
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_/:.".contains(c)),
                "case {:?}: {member} must satisfy ^[\\-_/A-Za-z0-9:.]+$, got {principal:?}",
                case.name
            );
        }
    }

    // A member the request never carried and the service has no default for is
    // reported by nobody: the minimal row has no tag key boundaries and no channel.
    let (minimal_region, minimal_arn) = &arns[0];
    let (_, minimal) = get_in(&router, minimal_region, minimal_arn).await;
    for absent in [
        "tagKeyBoundaries",
        "chatbotNotificationChannel",
        "crossAccountConfigurations",
    ] {
        assert!(
            minimal.get(absent).is_none(),
            "minimal-group: {absent} was never set, so it must be absent, got {minimal:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_a_modelled_404_not_a_hollow_200() {
    let router = emulator();
    let existing = create_ok(
        &router,
        REGION,
        &json!({ "name": "the-one-group", "roleArn": ROLE_ARN }),
    )
    .await;
    let existing_id = assert_group_arn("the-one-group", REGION, &existing);

    // Each row is a well-formed identifier that names nothing: the existing group's
    // own name and bare id (which the service resolves nothing from), a name and an
    // id-shaped string nothing carries, a syntactically valid ARN of a group never
    // created, and the existing group's ARN with ONE field changed — a different
    // partition, region or account names a different resource, so none of them may
    // resolve to this group.
    let unknown = [
        "the-one-group".to_string(),
        existing_id.clone(),
        "never-created".to_string(),
        "AAAAAAAAAAAAAAAA".to_string(),
        format!("arn:aws:aiops:{REGION}:{ACCOUNT}:investigation-group/ZZZZZZZZZZZZZZZZ"),
        format!("arn:aws-cn:aiops:{REGION}:{ACCOUNT}:investigation-group/{existing_id}"),
        format!("arn:aws:aiops:eu-central-1:{ACCOUNT}:investigation-group/{existing_id}"),
        format!("arn:aws:aiops:{REGION}:210987654321:investigation-group/{existing_id}"),
    ];
    for identifier in &unknown {
        for (method, uri, body) in [
            ("GET", format!("/investigationGroups/{}", label(identifier)), None),
            (
                "PATCH",
                format!("/investigationGroups/{}", label(identifier)),
                Some(json!({ "isCloudTrailEventHistoryEnabled": true })),
            ),
            (
                "DELETE",
                format!("/investigationGroups/{}", label(identifier)),
                None,
            ),
            (
                "GET",
                format!("/investigationGroups/{}/policy", label(identifier)),
                None,
            ),
            (
                "POST",
                format!("/investigationGroups/{}/policy", label(identifier)),
                Some(json!({ "policy": "{\"Version\":\"2012-10-17\"}" })),
            ),
        ] {
            let (status, answer) = call(&router, method, &uri, body.as_ref()).await;
            assert_eq!(
                status, 404,
                "case {method} {uri:?}: expected 404, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {method} {uri:?}: expected ResourceNotFoundException, got {answer:?}"
            );
        }
    }
}

/// `ListInvestigationGroups` is regional and an account holds one group per Region
/// (`state::MAX_INVESTIGATION_GROUPS_PER_REGION`), so a Region's listing carries
/// exactly the groups that Region holds, never another Region's, and never a
/// `nextToken` — there is never a second page to name. A token this operation did not
/// mint is still refused.
///
/// Rows differ along the Region, along whether it holds a group at all, and along the
/// page size: each published bound of the modelled `maxResults` range (1..50), a size
/// larger than the listing, and the absent-parameter default.
#[tokio::test]
async fn list_reports_exactly_the_groups_of_the_region_it_is_signed_for() {
    let router = emulator();

    // Four Regions hold a group; a fifth deliberately holds none.
    let mut expected: Vec<(String, (String, String))> = Vec::new();
    for index in 0..4 {
        let region = region(index);
        let name = format!("group-{index:02}");
        let arn = create_ok(&router, &region, &json!({ "name": name, "roleArn": ROLE_ARN })).await;
        expected.push((region, (arn, name)));
    }
    let empty_region = region(9);

    for page_size in [None, Some(1), Some(2), Some(50)] {
        for (region, want) in &expected {
            let case = format!("{region} with maxResults={page_size:?}");
            let mut uri = "/investigationGroups".to_string();
            if let Some(size) = page_size {
                uri.push_str(&format!("?maxResults={size}"));
            }

            let (status, body) = call_in(&router, region, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200 from {uri}, got {status} / {body:?}"
            );
            let items = body
                .get("investigationGroups")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: no investigationGroups in {body:?}"));
            let collected: Vec<(String, String)> = items
                .iter()
                .map(|item| {
                    (
                        item.get("arn")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect();
            assert_eq!(
                collected,
                vec![want.clone()],
                "case {case}: a Region lists exactly its own group, got {body:?}"
            );
            assert!(
                body.get("nextToken").is_none(),
                "case {case}: one group never fills a page, so no token may be issued, \
                 got {body:?}"
            );
        }

        // A Region holding no group reports an empty listing, not another Region's.
        let case = format!("{empty_region} with maxResults={page_size:?}");
        let (status, body) = call_in(&router, &empty_region, "GET", "/investigationGroups", None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} / {body:?}"
        );
        assert_eq!(
            body.get("investigationGroups").and_then(Value::as_array),
            Some(&vec![]),
            "case {case}: a Region with no group lists nothing, got {body:?}"
        );
    }

    // A token this operation never issued is refused, not silently ignored.
    let (status, body) = call(
        &router,
        "GET",
        "/investigationGroups?nextToken=not-a-token",
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "a forged nextToken must be refused, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "a forged nextToken must be a ValidationException, got {body:?}"
    );
}

#[tokio::test]
async fn update_replaces_only_the_members_the_request_carries() {
    let router = emulator();

    // Each row patches a different member set; everything it does not name must
    // survive untouched.
    let patches = [
        json!({ "isCloudTrailEventHistoryEnabled": false }),
        json!({ "roleArn": "arn:aws:iam::000000000000:role/UpdatedAIOpsRole" }),
        json!({ "tagKeyBoundaries": ["Team", "Env", "Service"] }),
        json!({
            "chatbotNotificationChannel": {
                "arn:aws:sns:us-west-2:000000000000:patched-topic": [
                    "arn:aws:chatbot::000000000000:chat-configuration/slack-channel/one",
                    "arn:aws:chatbot::000000000000:chat-configuration/slack-channel/two"
                ]
            }
        }),
        json!({ "encryptionConfiguration": { "type": "AWS_OWNED_KEY" } }),
        json!({
            "crossAccountConfigurations": [
                { "sourceRoleArn": "arn:aws:iam::111122223333:role/Patched" }
            ]
        }),
        // No member at all: a no-op patch still succeeds and still changes nothing
        // but the modification stamp.
        json!({}),
    ];

    for (index, patch) in patches.iter().enumerate() {
        let region = region(index);
        let name = format!("patched-{index}");
        let arn = create_ok(
            &router,
            &region,
            &json!({ "name": name, "roleArn": ROLE_ARN, "retentionInDays": 14 }),
        )
        .await;
        let (_, before) = get_in(&router, &region, &arn).await;

        let (status, answer) = call_in(
            &router,
            &region,
            "PATCH",
            &format!("/investigationGroups/{}", label(&arn)),
            Some(patch),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {patch}: expected 200 from the update, got {status} / {answer:?}"
        );

        let (status, after) = get_in(&router, &region, &arn).await;
        assert_eq!(status, 200, "case {patch}: read-back failed with {after:?}");

        // Every patched member now reports the patched value.
        assert_members(&name, "arn", patch, &after);

        // Nothing else moved: identity, creation facts and un-patched members.
        for stable in ["arn", "name", "createdBy", "createdAt"] {
            assert_eq!(
                after.get(stable),
                before.get(stable),
                "case {patch}: {stable} must not change on update, was {before:?} now {after:?}"
            );
        }
        assert_eq!(
            after.get("retentionInDays"),
            before.get("retentionInDays"),
            "case {patch}: UpdateInvestigationGroup does not accept retentionInDays, \
             so it must survive: was {before:?} now {after:?}"
        );
        let patched = patch.as_object().expect("an object patch");
        for member in ["roleArn", "isCloudTrailEventHistoryEnabled"] {
            if !patched.contains_key(member) {
                assert_eq!(
                    after.get(member),
                    before.get(member),
                    "case {patch}: {member} was not patched, so it must survive"
                );
            }
        }

        // The update is recorded: `lastModifiedAt` advances past what the read before
        // it reported, and is never earlier than `createdAt`.
        let created_at = after.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        let last_modified_at = after
            .get("lastModifiedAt")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let before_modified_at = before
            .get("lastModifiedAt")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        assert!(
            last_modified_at > before_modified_at,
            "case {patch}: expected lastModifiedAt to advance past {before_modified_at}, \
             got {last_modified_at}"
        );
        assert!(
            last_modified_at >= created_at,
            "case {patch}: expected lastModifiedAt >= createdAt, got {last_modified_at} < {created_at}"
        );
    }
}

#[tokio::test]
async fn a_policy_round_trips_byte_for_byte() {
    let router = emulator();

    // Rows differ along the document, inside what the operation accepts as a JSON
    // policy document: the smallest object, a realistic policy, one carrying the
    // tab/newline/carriage-return and Latin-1 bytes the modelled
    // `^[\u0009\u000A\u000D\u0020-\u00FF]+$` pattern admits, and a long one.
    let documents = [
        "{}".to_string(),
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": "aiops.alarms.cloudwatch.amazonaws.com" },
                "Action": ["aiops:CreateInvestigation", "aiops:CreateInvestigationEvent"],
                "Resource": "*"
            }]
        })
        .to_string(),
        "{\n\t\"Version\": \"2012-10-17\",\r\n\t\"Sid\": \"caf\u{e9}-\u{fc}ber\"\n}".to_string(),
        format!("{{\"Sid\":\"{}\"}}", "A".repeat(4096)),
    ];

    for (index, document) in documents.iter().enumerate() {
        let region = region(index);
        let name = format!("policy-group-{index}");
        let arn = create_ok(&router, &region, &json!({ "name": name, "roleArn": ROLE_ARN })).await;
        let case = format!("{name} ({} bytes)", document.len());

        // Before a Put there is no policy resource, which is a 404 — not an empty 200.
        let (status, body) = call_in(
            &router,
            &region,
            "GET",
            &format!("/investigationGroups/{}/policy", label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: a group with no policy must answer 404, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {body:?}"
        );

        let (status, put) = call_in(
            &router,
            &region,
            "POST",
            &format!("/investigationGroups/{}/policy", label(&arn)),
            Some(&json!({ "policy": document })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the put, got {status} / {put:?}"
        );
        assert_eq!(
            put.get("investigationGroupArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case}: the put must answer the group's own ARN, got {put:?}"
        );

        // Read it back twice through the ARN: the bytes are the ones stored, and a
        // read does not consume them.
        for via in [arn.clone(), arn.clone()] {
            let (status, got) = call_in(
                &router,
                &region,
                "GET",
                &format!("/investigationGroups/{}/policy", label(&via)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case} via {via}: expected 200, got {status} / {got:?}"
            );
            assert_eq!(
                got.get("policy").and_then(Value::as_str),
                Some(document.as_str()),
                "case {case} via {via}: the policy must come back byte-identical, got {got:?}"
            );
            assert_eq!(
                got.get("investigationGroupArn").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case} via {via}: expected the group's ARN, got {got:?}"
            );
        }

        // A second put replaces the document rather than appending to it.
        let replacement = format!("{{\"Sid\":\"replaced-{index}\"}}");
        call_in(
            &router,
            &region,
            "POST",
            &format!("/investigationGroups/{}/policy", label(&arn)),
            Some(&json!({ "policy": replacement })),
        )
        .await;
        let (_, got) = call_in(
            &router,
            &region,
            "GET",
            &format!("/investigationGroups/{}/policy", label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            got.get("policy").and_then(Value::as_str),
            Some(replacement.as_str()),
            "case {case}: a second put must replace the stored document, got {got:?}"
        );
    }
}

/// `tags` entries that satisfy the modelled `TagKey` (1..128) and `TagValue`
/// (1..256) patterns, so only the ENTRY COUNT — which the model does not bound — is
/// under test.
fn tag_map(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|n| (format!("key{n}"), Value::String(format!("value{n}"))))
            .collect(),
    )
}

/// `tagKeyBoundaries` entries satisfying the same modelled `TagKey` constraints.
fn tag_key_boundaries(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|n| Value::String(format!("boundary{n}")))
            .collect(),
    )
}

/// `chatbotNotificationChannel` entries whose keys satisfy the modelled `SNSTopicArn`
/// (20..2048, `^arn:.*$`) and whose values satisfy `ChatConfigurationArns` (1..5).
fn chatbot_channels(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|n| {
                (
                    format!("arn:aws:sns:{REGION}:{ACCOUNT}:aiops-topic-{n}"),
                    json!([format!(
                        "arn:aws:chatbot::{ACCOUNT}:chat-configuration/slack-channel/c{n}"
                    )]),
                )
            })
            .collect(),
    )
}

#[tokio::test]
async fn caller_sized_collections_are_bounded_before_they_reach_the_store() {
    let router = emulator();

    // The rule: a caller-sized collection the model gives no length is accepted up to
    // this service's own bound and refused above it. Rows differ along which member
    // carries the collection and whether its size is at the bound or one past it — and
    // along the error code: the `tags` map is a tag write, refused by the tagging
    // frontend as a `BadRequestException`, while every other member is this service's
    // own `ValidationException`. Each accepted row needs its own Region.
    let cases: Vec<(String, Value, u16, &str)> = vec![
        (
            format!("tags at the bound ({MAX_TAGS})"),
            json!({ "name": "tags-at-bound", "roleArn": ROLE_ARN, "tags": tag_map(MAX_TAGS) }),
            201,
            "",
        ),
        (
            format!("tags one past the bound ({})", MAX_TAGS + 1),
            json!({ "name": "tags-past-bound", "roleArn": ROLE_ARN, "tags": tag_map(MAX_TAGS + 1) }),
            400,
            "BadRequestException",
        ),
        (
            format!("tagKeyBoundaries at the bound ({MAX_COLLECTION_ENTRIES})"),
            json!({
                "name": "boundaries-at-bound",
                "roleArn": ROLE_ARN,
                "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES),
            }),
            201,
            "",
        ),
        (
            format!(
                "tagKeyBoundaries one past the bound ({})",
                MAX_COLLECTION_ENTRIES + 1
            ),
            json!({
                "name": "boundaries-past-bound",
                "roleArn": ROLE_ARN,
                "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES + 1),
            }),
            400,
            "ValidationException",
        ),
        (
            format!("chatbotNotificationChannel at the bound ({MAX_COLLECTION_ENTRIES})"),
            json!({
                "name": "channels-at-bound",
                "roleArn": ROLE_ARN,
                "chatbotNotificationChannel": chatbot_channels(MAX_COLLECTION_ENTRIES),
            }),
            201,
            "",
        ),
        (
            format!(
                "chatbotNotificationChannel one past the bound ({})",
                MAX_COLLECTION_ENTRIES + 1
            ),
            json!({
                "name": "channels-past-bound",
                "roleArn": ROLE_ARN,
                "chatbotNotificationChannel": chatbot_channels(MAX_COLLECTION_ENTRIES + 1),
            }),
            400,
            "ValidationException",
        ),
        (
            "crossAccountConfigurations one past its MODELLED 0..25 length".to_string(),
            json!({
                "name": "cross-past-bound",
                "roleArn": ROLE_ARN,
                "crossAccountConfigurations": (0..26)
                    .map(|n| json!({ "sourceRoleArn": format!("arn:aws:iam::111122223333:role/R{n}") }))
                    .collect::<Vec<_>>(),
            }),
            400,
            "ValidationException",
        ),
    ];

    for (index, (case, body, want_status, want_code)) in cases.into_iter().enumerate() {
        let (status, answer) = create(&router, &region(index), &body).await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected {want_status}, got {status} / {answer:?}"
        );
        if want_status == 400 {
            assert_eq!(
                error_code(&answer),
                want_code,
                "case {case:?}: expected {want_code}, got {answer:?}"
            );
        }
    }

    // The same bound applies to the replacement an update stores, not only to a create.
    let region = region(9);
    let arn = create_ok(
        &router,
        &region,
        &json!({ "name": "patched-bounds", "roleArn": ROLE_ARN }),
    )
    .await;
    for (case, patch, want_status) in [
        (
            "update tagKeyBoundaries at the bound",
            json!({ "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES) }),
            200u16,
        ),
        (
            "update tagKeyBoundaries one past the bound",
            json!({ "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES + 1) }),
            400,
        ),
        (
            "update chatbotNotificationChannel one past the bound",
            json!({ "chatbotNotificationChannel": chatbot_channels(MAX_COLLECTION_ENTRIES + 1) }),
            400,
        ),
    ] {
        let (status, answer) = call_in(
            &router,
            &region,
            "PATCH",
            &format!("/investigationGroups/{}", label(&arn)),
            Some(&patch),
        )
        .await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected {want_status}, got {status} / {answer:?}"
        );
    }

    // A refused update left the stored group exactly as the accepted one left it.
    let (_, after) = get_in(&router, &region, &arn).await;
    assert_eq!(
        after
            .get("tagKeyBoundaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(MAX_COLLECTION_ENTRIES),
        "a refused update must not have replaced the stored boundaries, got {after:?}"
    );
}

/// The store is finite ACROSS Regions too: each Region holds its one group, and the
/// emulator's own cross-region guard refuses a create past
/// [`MAX_INVESTIGATION_GROUPS`] rather than growing further. Freeing one slot lets
/// exactly one more create through.
#[tokio::test]
async fn the_store_refuses_to_grow_past_its_cap() {
    let router = emulator();

    // Fill the store to its cap — one group per Region, since that is the AWS quota —
    // then one more: the create is refused with the modelled quota error rather than
    // growing the store further.
    let mut filled = Vec::new();
    for n in 0..MAX_INVESTIGATION_GROUPS {
        let region = region(n + REGIONS.len());
        let (status, answer) = create(
            &router,
            &region,
            &json!({ "name": format!("filler-{n:04}"), "roleArn": ROLE_ARN }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case fill {n}: expected 201 while under the cap, got {status} / {answer:?}"
        );
        filled.push((
            region,
            answer
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ));
    }

    // A Region that holds nothing yet is still refused: the cap is the store's, not
    // the Region's.
    let fresh_region = region(MAX_INVESTIGATION_GROUPS + REGIONS.len());
    let (status, answer) = create(
        &router,
        &fresh_region,
        &json!({ "name": "one-too-many", "roleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        status, 402,
        "a create past the cap must be refused, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ServiceQuotaExceededException",
        "a create past the cap must be a ServiceQuotaExceededException, got {answer:?}"
    );
    let (_, listed) = call_in(&router, &fresh_region, "GET", "/investigationGroups", None).await;
    assert_eq!(
        listed.get("investigationGroups").and_then(Value::as_array),
        Some(&vec![]),
        "the refused create must have stored nothing, got {listed:?}"
    );

    // Freeing one slot lets exactly one more create through.
    let (victim_region, victim_arn) = &filled[0];
    let (status, _) = call_in(
        &router,
        victim_region,
        "DELETE",
        &format!("/investigationGroups/{}", label(victim_arn)),
        None,
    )
    .await;
    assert_eq!(status, 200, "deleting a filler group must succeed");
    let (status, answer) = create(
        &router,
        &fresh_region,
        &json!({ "name": "one-too-many", "roleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        status, 201,
        "a create must be accepted once a slot is free, got {status} / {answer:?}"
    );
}

#[tokio::test]
async fn delete_removes_the_group_from_every_reader() {
    let router = emulator();

    // One group per Region, each carrying a policy; rows differ along the Region the
    // delete addresses.
    let names = ["doomed-one", "doomed-two", "survivor"];
    let mut groups = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let region = region(index);
        let arn = create_ok(&router, &region, &json!({ "name": name, "roleArn": ROLE_ARN })).await;
        call_in(
            &router,
            &region,
            "POST",
            &format!("/investigationGroups/{}/policy", label(&arn)),
            Some(&json!({ "policy": format!("{{\"Sid\":\"{name}\"}}") })),
        )
        .await;
        groups.push((region, arn));
    }

    for (region, arn) in groups.iter().take(2) {
        let (status, answer) = call_in(
            &router,
            region,
            "DELETE",
            &format!("/investigationGroups/{}", label(arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case delete in {region}: expected 200, got {status} / {answer:?}"
        );

        let (status, body) = get_in(&router, region, arn).await;
        assert_eq!(
            status, 404,
            "case delete in {region}: a deleted group must be gone, got {status} / {body:?}"
        );

        // The policy went with it.
        let (status, body) = call_in(
            &router,
            region,
            "GET",
            &format!("/investigationGroups/{}/policy", label(arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case delete in {region}: the policy must go with the group, got {status} / {body:?}"
        );

        // And a second delete of the same identifier is a 404, not a second success.
        let (status, body) = call_in(
            &router,
            region,
            "DELETE",
            &format!("/investigationGroups/{}", label(arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case delete in {region}: deleting twice must 404, got {status} / {body:?}"
        );

        // The Region's listing is empty again, and its one slot is free.
        let (_, listed) = call_in(&router, region, "GET", "/investigationGroups", None).await;
        assert_eq!(
            listed.get("investigationGroups").and_then(Value::as_array),
            Some(&vec![]),
            "case delete in {region}: the Region must list nothing, got {listed:?}"
        );
    }

    // The undeleted group is untouched, and still the only one its Region lists.
    let (survivor_region, survivor_arn) = &groups[2];
    let (status, body) = call_in(
        &router,
        survivor_region,
        "GET",
        "/investigationGroups",
        None,
    )
    .await;
    assert_eq!(status, 200, "list after deletes failed with {body:?}");
    let listed: Vec<&str> = body["investigationGroups"]
        .as_array()
        .expect("investigationGroups")
        .iter()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(
        listed,
        vec!["survivor"],
        "only the undeleted group may remain, got {body:?}"
    );
    let (status, _) = get_in(&router, survivor_region, survivor_arn).await;
    assert_eq!(status, 200, "the survivor must still read back");

    // The Region a delete emptied takes a new group, which gets a NEW id.
    let (reborn_region, deleted_arn) = &groups[0];
    let reborn = create_ok(
        &router,
        reborn_region,
        &json!({ "name": names[0], "roleArn": ROLE_ARN }),
    )
    .await;
    assert_ne!(
        &reborn, deleted_arn,
        "a recreated group must get a fresh id, got the deleted one's ARN back"
    );
}

/// A group's name is not its identity: the Region is. The same name may be used in
/// another Region, where it names a DIFFERENT group with its own ARN, and neither
/// group's Region lists the other.
#[tokio::test]
async fn the_same_name_in_another_region_is_a_different_group() {
    let router = emulator();
    let name = "shared-across-regions";

    let mut created = Vec::new();
    for index in 0..3 {
        let region = region(index);
        let arn = create_ok(
            &router,
            &region,
            &json!({ "name": name, "roleArn": ROLE_ARN, "retentionInDays": 7 + index as i64 }),
        )
        .await;
        created.push((region, arn));
    }

    for (index, (region, arn)) in created.iter().enumerate() {
        // Each is its own resource, carrying what its own create stored.
        let (status, body) = get_in(&router, region, arn).await;
        assert_eq!(
            status, 200,
            "case {region}: expected 200, got {status} / {body:?}"
        );
        assert_eq!(
            body.get("retentionInDays").and_then(Value::as_i64),
            Some(7 + index as i64),
            "case {region}: each same-named group keeps its own configuration, got {body:?}"
        );

        // And no other Region's ARN resolves in it.
        for (other_region, other_arn) in &created {
            if other_region == region {
                continue;
            }
            assert_ne!(
                arn, other_arn,
                "case {region}: two Regions' groups must not share an ARN"
            );
        }

        let (_, listed) = call_in(&router, region, "GET", "/investigationGroups", None).await;
        let arns: Vec<&str> = listed["investigationGroups"]
            .as_array()
            .expect("investigationGroups")
            .iter()
            .filter_map(|item| item.get("arn").and_then(Value::as_str))
            .collect();
        assert_eq!(
            arns,
            vec![arn.as_str()],
            "case {region}: a Region lists only its own group, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused() {
    let router = emulator();

    // Rows: each published bound of the modelled `retentionInDays` range (7..90) and
    // one step past it, a missing required member, both patterned members, and the
    // `EncryptionConfigurationType` value set. Each accepted row needs its own Region.
    let cases: Vec<(&str, Value, u16)> = vec![
        ("retention one below the low bound", json!({ "name": "r6", "roleArn": ROLE_ARN, "retentionInDays": 6 }), 400),
        ("retention at the low bound", json!({ "name": "r7", "roleArn": ROLE_ARN, "retentionInDays": 7 }), 201),
        ("retention at the high bound", json!({ "name": "r90", "roleArn": ROLE_ARN, "retentionInDays": 90 }), 201),
        ("retention one above the high bound", json!({ "name": "r91", "roleArn": ROLE_ARN, "retentionInDays": 91 }), 400),
        ("name absent", json!({ "roleArn": ROLE_ARN }), 400),
        ("roleArn absent", json!({ "name": "no-role" }), 400),
        ("name empty", json!({ "name": "", "roleArn": ROLE_ARN }), 400),
        ("name outside its pattern", json!({ "name": "bad/name", "roleArn": ROLE_ARN }), 400),
        ("roleArn outside its pattern", json!({ "name": "bad-role", "roleArn": "not-an-arn-but-long-enough" }), 400),
        ("roleArn shorter than its minimum", json!({ "name": "short-role", "roleArn": "arn:aws:iam" }), 400),
        (
            "encryption type outside the modelled value set",
            json!({ "name": "bad-enc", "roleArn": ROLE_ARN, "encryptionConfiguration": { "type": "MY_OWN_KEY" } }),
            400,
        ),
        (
            "encryption type inside the modelled value set",
            json!({ "name": "good-enc", "roleArn": ROLE_ARN, "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/k" } }),
            201,
        ),
    ];

    for (index, (case, body, want_status)) in cases.into_iter().enumerate() {
        let (status, answer) = create(&router, &region(index), &body).await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected {want_status}, got {status} / {answer:?}"
        );
        if want_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {answer:?}"
            );
        }
    }

    // The same rule on the list page size: the modelled `maxResults` range is 1..50.
    for (case, query, want_status) in [
        ("maxResults below the low bound", "maxResults=0", 400u16),
        ("maxResults at the low bound", "maxResults=1", 200),
        ("maxResults at the high bound", "maxResults=50", 200),
        ("maxResults above the high bound", "maxResults=51", 400),
    ] {
        let (status, answer) =
            call(&router, "GET", &format!("/investigationGroups?{query}"), None).await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected {want_status}, got {status} / {answer:?}"
        );
    }
}

// The three tagging operations used to be asserted here as honest 501s. They are
// implemented now, and their end-to-end rules live in `tests/all/resource_tags.rs`.
