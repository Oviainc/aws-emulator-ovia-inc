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

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state::{
    MAX_COLLECTION_ENTRIES, MAX_INVESTIGATION_GROUPS, MAX_TAGS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for; the ARNs must carry exactly this one.
const REGION: &str = "us-west-2";
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

/// A request signed for `aiops` in [`REGION`], the signal the router routes on and
/// the handlers read the region and the caller principal from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{REGION}/aiops/aws4_request, \
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
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
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

async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/investigationGroups", Some(body)).await
}

async fn get(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/investigationGroups/{}", label(identifier)),
        None,
    )
    .await
}

/// Create a group that must succeed, and return its ARN.
async fn create_ok(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, created) = create(router, body).await;
    assert_eq!(status, 201, "create {body}: expected 201, got {created:?}");
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {body}: response carried no arn, got {created:?}"))
        .to_string()
}

/// Assert `arn` satisfies the modelled `InvestigationGroupArn` pattern, and answer
/// its 16-character id segment.
fn assert_group_arn(case: &str, arn: &str) -> String {
    let prefix = format!("arn:aws:aiops:{REGION}:{ACCOUNT}:investigation-group/");
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
    /// Whether the bare name is addressable: the modelled identifier's bare form is
    /// `[\-_A-Za-z0-9]{1,512}`, narrower than the name's own pattern.
    name_addressable: bool,
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
        GroupCase {
            name: "minimal-group",
            request: json!({ "name": "minimal-group", "roleArn": ROLE_ARN }),
            expected: json!({
                "name": "minimal-group",
                "roleArn": ROLE_ARN,
                "retentionInDays": 90,
                "isCloudTrailEventHistoryEnabled": true,
            }),
            name_addressable: true,
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
                "encryptionConfiguration": { "type": "AWS_OWNED_KEY" },
                "tagKeyBoundaries": ["Application", "CostCenter"],
                "chatbotNotificationChannel": {
                    "arn:aws:sns:us-west-2:000000000000:aiops-alerts": [
                        "arn:aws:chatbot::000000000000:chat-configuration/slack-channel/ops"
                    ]
                },
                "crossAccountConfigurations": [
                    { "sourceRoleArn": "arn:aws:iam::000000000000:role/SourceOne" }
                ],
            }),
            expected: json!({
                "name": "retention-low-bound",
                "roleArn": ROLE_ARN,
                "retentionInDays": 7,
                "isCloudTrailEventHistoryEnabled": false,
                "encryptionConfiguration": { "type": "AWS_OWNED_KEY" },
                "tagKeyBoundaries": ["Application", "CostCenter"],
                "chatbotNotificationChannel": {
                    "arn:aws:sns:us-west-2:000000000000:aiops-alerts": [
                        "arn:aws:chatbot::000000000000:chat-configuration/slack-channel/ops"
                    ]
                },
                "crossAccountConfigurations": [
                    { "sourceRoleArn": "arn:aws:iam::000000000000:role/SourceOne" }
                ],
            }),
            name_addressable: true,
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
                    { "sourceRoleArn": "arn:aws:iam::000000000000:role/SourceOne" },
                    { "sourceRoleArn": "arn:aws:iam::000000000000:role/SourceTwo" },
                    { "sourceRoleArn": "arn:aws:iam::000000000000:role/SourceThree" }
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
                    { "sourceRoleArn": "arn:aws:iam::000000000000:role/SourceOne" },
                    { "sourceRoleArn": "arn:aws:iam::000000000000:role/SourceTwo" },
                    { "sourceRoleArn": "arn:aws:iam::000000000000:role/SourceThree" }
                ],
            }),
            name_addressable: true,
        },
        // A retention inside the range, and a name using the punctuation the
        // modelled name pattern allows but the bare identifier form does not — so
        // this row is addressable only by id or ARN.
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
                "tagKeyBoundaries": ["Service"],
            }),
            name_addressable: false,
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

#[tokio::test]
async fn a_created_group_reads_back_through_every_identifier_form() {
    let router = emulator();
    let cases = group_cases();

    // Create them all first, so each read-back also proves the store keeps the rows
    // apart rather than answering with the most recent one.
    let mut arns = Vec::new();
    for case in &cases {
        let arn = create_ok(&router, &case.request).await;
        assert_group_arn(case.name, &arn);
        arns.push(arn);
    }

    for (case, arn) in cases.iter().zip(&arns) {
        let id = assert_group_arn(case.name, arn);

        let mut identifiers = vec![("arn", arn.clone()), ("id", id)];
        if case.name_addressable {
            identifiers.push((
                "name",
                case.request["name"].as_str().expect("a string name").to_string(),
            ));
        }

        let mut bodies = Vec::new();
        for (via, identifier) in &identifiers {
            let (status, body) = get(&router, identifier).await;
            assert_eq!(
                status, 200,
                "case {:?} read via {via}: expected 200, got {status} / {body:?}",
                case.name
            );
            assert_members(case.name, via, &case.expected, &body);
            assert_eq!(
                body.get("arn").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {:?} read via {via}: expected arn {arn}, got {body:?}",
                case.name
            );
            bodies.push(body);
        }

        // Every identifier form names the one record, so every form answers the
        // identical body — and a read never changes it.
        for body in &bodies {
            assert_eq!(
                body, &bodies[0],
                "case {:?}: the identifier forms answered different bodies: {bodies:?}",
                case.name
            );
        }

        // The timestamps and the principal come from the create, not from the read.
        let first = &bodies[0];
        let created_at = first
            .get("createdAt")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {:?}: no createdAt in {first:?}", case.name));
        assert!(
            created_at > 1_600_000_000,
            "case {:?}: expected createdAt in epoch SECONDS, got {created_at}",
            case.name
        );
        assert_eq!(
            first.get("lastModifiedAt").and_then(Value::as_i64),
            Some(created_at),
            "case {:?}: an untouched group's lastModifiedAt must equal createdAt, got {first:?}",
            case.name
        );
        for member in ["createdBy", "lastModifiedBy"] {
            let principal = first.get(member).and_then(Value::as_str).unwrap_or("");
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

    // A member the request never carried is reported by nobody: the minimal row has
    // no encryption configuration, no tag key boundaries and no channel.
    let (_, minimal) = get(&router, "minimal-group").await;
    for absent in [
        "encryptionConfiguration",
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
        &json!({ "name": "the-one-group", "roleArn": ROLE_ARN }),
    )
    .await;
    let existing_id = assert_group_arn("the-one-group", &existing);

    // Each row is a well-formed identifier that names nothing: a bare name, a bare
    // id-shaped string, a syntactically valid ARN of a group never created, and the
    // existing group's ARN with ONE field changed — a different partition, region or
    // account names a different resource, so none of them may resolve to this group.
    let unknown = [
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

#[tokio::test]
async fn list_pages_the_store_through_the_paginated_trait() {
    let router = emulator();

    // Eleven groups, so a page size of 1, 2, 3, 4 and the default all behave
    // differently: 11 is not a multiple of any of them.
    let mut expected: Vec<(String, String)> = Vec::new();
    for n in 0..11 {
        let name = format!("group-{n:02}");
        let arn = create_ok(&router, &json!({ "name": name, "roleArn": ROLE_ARN })).await;
        expected.push((arn, name));
    }
    expected.sort();

    // Rows differ along the page size: each published bound of the modelled
    // `maxResults` range (1..50), sizes that do and do not divide the store, a size
    // larger than the store, and the absent-parameter default.
    for page_size in [None, Some(1), Some(2), Some(3), Some(4), Some(11), Some(50)] {
        let case = format!("maxResults={page_size:?}");
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut uri = "/investigationGroups".to_string();
            let mut query = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            if !query.is_empty() {
                uri.push('?');
                uri.push_str(&query.join("&"));
            }

            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200 from {uri}, got {status} / {body:?}"
            );
            let items = body
                .get("investigationGroups")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: no investigationGroups in {body:?}"));
            if let Some(size) = page_size {
                assert!(
                    items.len() <= size as usize,
                    "case {case}: a page carried {} items, more than maxResults",
                    items.len()
                );
            }
            for item in items {
                let arn = item
                    .get("arn")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: a list item carried no arn: {item:?}"));
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: a list item carried no name: {item:?}"));
                collected.push((arn.to_string(), name.to_string()));
            }

            pages += 1;
            assert!(pages <= 32, "case {case}: pagination did not terminate");
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                // The last page omits the token, which is what ends the walk.
                break;
            }
        }

        collected.sort();
        assert_eq!(
            collected, expected,
            "case {case}: paging must visit every group exactly once"
        );
        if let Some(size) = page_size {
            let want_pages = expected.len().div_ceil(size as usize);
            assert_eq!(
                pages,
                want_pages.max(1),
                "case {case}: expected {want_pages} pages for {} groups",
                expected.len()
            );
        }
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
                { "sourceRoleArn": "arn:aws:iam::000000000000:role/Patched" }
            ]
        }),
        // No member at all: a no-op patch still succeeds and still changes nothing.
        json!({}),
    ];

    for (index, patch) in patches.iter().enumerate() {
        let name = format!("patched-{index}");
        let arn = create_ok(
            &router,
            &json!({ "name": name, "roleArn": ROLE_ARN, "retentionInDays": 14 }),
        )
        .await;
        let (_, before) = get(&router, &arn).await;

        let (status, answer) = call(
            &router,
            "PATCH",
            &format!("/investigationGroups/{}", label(&arn)),
            Some(patch),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {patch}: expected 200 from the update, got {status} / {answer:?}"
        );

        let (status, after) = get(&router, &arn).await;
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

        // The update is recorded: `lastModifiedAt` is never earlier than the create,
        // and never earlier than `createdAt`.
        let created_at = after.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        let last_modified_at = after
            .get("lastModifiedAt")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        assert!(
            last_modified_at >= created_at,
            "case {patch}: expected lastModifiedAt >= createdAt, got {last_modified_at} < {created_at}"
        );
    }
}

#[tokio::test]
async fn a_policy_round_trips_byte_for_byte() {
    let router = emulator();

    // Rows differ along the document: a one-character minimum, a realistic policy,
    // one carrying the tab/newline/carriage-return and Latin-1 bytes the modelled
    // `^[\u0009\u000A\u000D\u0020-\u00FF]+$` pattern admits, and a long one.
    let documents = [
        "x".to_string(),
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
        let name = format!("policy-group-{index}");
        let arn = create_ok(&router, &json!({ "name": name, "roleArn": ROLE_ARN })).await;
        let case = format!("{name} ({} bytes)", document.len());

        // Before a Put there is no policy resource, which is a 404 — not an empty 200.
        let (status, body) = call(
            &router,
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

        let (status, put) = call(
            &router,
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

        // Read it back twice, and by the bare name as well as by the ARN: the bytes
        // are the ones stored, and a read does not consume them.
        for via in [arn.clone(), name.clone(), arn.clone()] {
            let (status, got) = call(
                &router,
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
        call(
            &router,
            "POST",
            &format!("/investigationGroups/{}/policy", label(&arn)),
            Some(&json!({ "policy": replacement })),
        )
        .await;
        let (_, got) = call(
            &router,
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
    // carries the collection and whether its size is at the bound or one past it.
    let cases: Vec<(String, Value, u16)> = vec![
        (
            format!("tags at the bound ({MAX_TAGS})"),
            json!({ "name": "tags-at-bound", "roleArn": ROLE_ARN, "tags": tag_map(MAX_TAGS) }),
            201,
        ),
        (
            format!("tags one past the bound ({})", MAX_TAGS + 1),
            json!({ "name": "tags-past-bound", "roleArn": ROLE_ARN, "tags": tag_map(MAX_TAGS + 1) }),
            400,
        ),
        (
            format!("tagKeyBoundaries at the bound ({MAX_COLLECTION_ENTRIES})"),
            json!({
                "name": "boundaries-at-bound",
                "roleArn": ROLE_ARN,
                "tagKeyBoundaries": tag_key_boundaries(MAX_COLLECTION_ENTRIES),
            }),
            201,
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
        ),
        (
            format!("chatbotNotificationChannel at the bound ({MAX_COLLECTION_ENTRIES})"),
            json!({
                "name": "channels-at-bound",
                "roleArn": ROLE_ARN,
                "chatbotNotificationChannel": chatbot_channels(MAX_COLLECTION_ENTRIES),
            }),
            201,
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
        ),
        (
            "crossAccountConfigurations one past its MODELLED 0..25 length".to_string(),
            json!({
                "name": "cross-past-bound",
                "roleArn": ROLE_ARN,
                "crossAccountConfigurations": (0..26)
                    .map(|n| json!({ "sourceRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/R{n}") }))
                    .collect::<Vec<_>>(),
            }),
            400,
        ),
    ];

    for (case, body, want_status) in cases {
        let (status, answer) = create(&router, &body).await;
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

    // The same bound applies to the replacement an update stores, not only to a create.
    let arn = create_ok(
        &router,
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
        let (status, answer) = call(
            &router,
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
    let (_, after) = get(&router, &arn).await;
    assert_eq!(
        after
            .get("tagKeyBoundaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(MAX_COLLECTION_ENTRIES),
        "a refused update must not have replaced the stored boundaries, got {after:?}"
    );
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_cap() {
    let router = emulator();

    // Fill the store to its cap, then one more: the create is refused with the
    // modelled quota error rather than growing the store further.
    for n in 0..MAX_INVESTIGATION_GROUPS {
        let (status, answer) = create(
            &router,
            &json!({ "name": format!("filler-{n:04}"), "roleArn": ROLE_ARN }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case fill {n}: expected 201 while under the cap, got {status} / {answer:?}"
        );
    }

    let (status, answer) = create(
        &router,
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
    let (status, body) = get(&router, "one-too-many").await;
    assert_eq!(
        status, 404,
        "the refused create must have stored nothing, got {status} / {body:?}"
    );

    // Freeing one slot lets exactly one more create through.
    let (status, _) = call(
        &router,
        "DELETE",
        "/investigationGroups/filler-0000",
        None,
    )
    .await;
    assert_eq!(status, 200, "deleting a filler group must succeed");
    let (status, answer) = create(
        &router,
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

    let names = ["doomed-one", "doomed-two", "survivor"];
    let mut arns = Vec::new();
    for name in names {
        let arn = create_ok(&router, &json!({ "name": name, "roleArn": ROLE_ARN })).await;
        call(
            &router,
            "POST",
            &format!("/investigationGroups/{}/policy", label(&arn)),
            Some(&json!({ "policy": format!("{{\"Sid\":\"{name}\"}}") })),
        )
        .await;
        arns.push(arn);
    }

    // Rows differ along the identifier form the delete uses.
    for (via, identifier) in [("arn", arns[0].clone()), ("name", names[1].to_string())] {
        let (status, answer) = call(
            &router,
            "DELETE",
            &format!("/investigationGroups/{}", label(&identifier)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case delete via {via}: expected 200, got {status} / {answer:?}"
        );

        let (status, body) = get(&router, &identifier).await;
        assert_eq!(
            status, 404,
            "case delete via {via}: a deleted group must be gone, got {status} / {body:?}"
        );

        // The policy went with it.
        let (status, body) = call(
            &router,
            "GET",
            &format!("/investigationGroups/{}/policy", label(&identifier)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case delete via {via}: the policy must go with the group, got {status} / {body:?}"
        );

        // And a second delete of the same identifier is a 404, not a second success.
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/investigationGroups/{}", label(&identifier)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case delete via {via}: deleting twice must 404, got {status} / {body:?}"
        );
    }

    let (status, body) = call(&router, "GET", "/investigationGroups", None).await;
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

    // The name a delete freed can be taken again, and gets a NEW id.
    let reborn = create_ok(&router, &json!({ "name": names[0], "roleArn": ROLE_ARN })).await;
    assert_ne!(
        reborn, arns[0],
        "a recreated group must get a fresh id, got the deleted one's ARN back"
    );
}

#[tokio::test]
async fn a_name_already_in_use_is_a_conflict() {
    let router = emulator();
    let first = create_ok(
        &router,
        &json!({ "name": "only-once", "roleArn": ROLE_ARN }),
    )
    .await;

    let (status, body) = create(
        &router,
        &json!({ "name": "only-once", "roleArn": ROLE_ARN, "retentionInDays": 30 }),
    )
    .await;
    assert_eq!(
        status, 409,
        "a duplicate name must be refused, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "a duplicate name must be a ConflictException, got {body:?}"
    );

    // The refused create stored nothing: the original is untouched and alone.
    let (_, listed) = call(&router, "GET", "/investigationGroups", None).await;
    assert_eq!(
        listed["investigationGroups"].as_array().map(Vec::len),
        Some(1),
        "a refused create must not store a group, got {listed:?}"
    );
    let (_, group) = get(&router, "only-once").await;
    assert_eq!(
        group.get("arn").and_then(Value::as_str),
        Some(first.as_str()),
        "the surviving group must be the first one, got {group:?}"
    );
    assert_eq!(
        group.get("retentionInDays").and_then(Value::as_i64),
        Some(90),
        "the refused create must not have altered the original, got {group:?}"
    );
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused() {
    let router = emulator();

    // Rows: each published bound of the modelled `retentionInDays` range (7..90) and
    // one step past it, a missing required member, both patterned members, and the
    // `EncryptionConfigurationType` value set.
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

    for (case, body, want_status) in cases {
        let (status, answer) = create(&router, &body).await;
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

#[tokio::test]
async fn the_remaining_operations_are_still_honest_501s() {
    let router = emulator();
    let arn = create_ok(&router, &json!({ "name": "tagged", "roleArn": ROLE_ARN })).await;
    let resource = label(&arn);

    // These three have their own gap issues; until then they must answer 501, never
    // an empty 200 a client cannot tell from a real implementation.
    for (method, uri, body) in [
        ("GET", format!("/tags/{resource}"), None),
        (
            "POST",
            format!("/tags/{resource}"),
            Some(json!({ "tags": { "Team": "ops" } })),
        ),
        ("DELETE", format!("/tags/{resource}?tagKeys=Team"), None),
    ] {
        let (status, answer) = call(&router, method, &uri, body.as_ref()).await;
        assert_eq!(
            status, 501,
            "case {method} {uri:?}: an unimplemented operation must answer 501, \
             got {status} / {answer:?}"
        );
    }
}
