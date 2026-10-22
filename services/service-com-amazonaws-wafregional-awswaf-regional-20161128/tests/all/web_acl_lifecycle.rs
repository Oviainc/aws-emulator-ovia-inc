// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) WebACL lifecycle, end to end over the wire:
//! `GetChangeToken`, `CreateWebACL` and `GetWebACL`.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSWAF_Regional_20161128.<Op>`, awsJson1_1), so what these tests
//! assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::WAF_ACTION_TYPE_VALUES;
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_TAGS_PER_RESOURCE, MAX_WEB_ACLS, RESERVED_METRIC_NAMES,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::EmulatedService;

/// The region most requests below sign for; `WebACLArn` must name it.
const REGION: &str = "us-east-1";

/// A router serving this one service over a fresh engine, as `build_router` does in
/// production.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation, signing for `region`, and return `(http status, parsed body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        // The routing signal an SDK request carries: the SigV4 credential scope names
        // this service's signing name (`waf-regional`) and the caller's region.
        // Request authentication is off for this router, so the signature itself is
        // never checked.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20250818/{region}/waf-regional/aws4_request, \
                 SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000"
            ),
        )
        .header("host", format!("waf-regional.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSWAF_Regional_20161128.{operation}"),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Call one operation in [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// Succeed-or-explain: the call must answer 200.
async fn ok_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} expected 200, got {status}: {parsed}"
    );
    parsed
}

/// The wire error code (`__type`) of an error body.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert `operation` refused `request` with HTTP 400 and the named modelled error.
async fn expect_error(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
    expected: &str,
    label: &str,
) -> Value {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {label}: {operation} must answer 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains(expected),
        "case {label}: expected {expected}, got {} ({body})",
        error_code(&body)
    );
    body
}

/// The change token a mutating call must carry. Each one is consumed by exactly one
/// mutating call, so every caller below asks for its own.
async fn change_token(router: &MultiServiceRouter) -> String {
    let body = ok_call(router, "GetChangeToken", &json!({})).await;
    body["ChangeToken"]
        .as_str()
        .unwrap_or_else(|| panic!("GetChangeToken returned no ChangeToken: {body}"))
        .to_string()
}

/// A `CreateWebACL` request body with a freshly minted change token.
async fn create_request(
    router: &MultiServiceRouter,
    name: &str,
    metric_name: &str,
    default_action: &str,
) -> Value {
    let token = change_token(router).await;
    json!({
        "Name": name,
        "MetricName": metric_name,
        "DefaultAction": {"Type": default_action},
        "ChangeToken": token,
    })
}

/// Create a WebACL and return its `WebACLId`.
async fn create_web_acl(
    router: &MultiServiceRouter,
    name: &str,
    metric_name: &str,
    default_action: &str,
) -> String {
    let request = create_request(router, name, metric_name, default_action).await;
    let body = ok_call(router, "CreateWebACL", &request).await;
    body["WebACL"]["WebACLId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWebACL returned no WebACLId: {body}"))
        .to_string()
}

/// Create a WebACL with a token the caller already holds, and return its `WebACLId`.
async fn create_web_acl_with_token(
    router: &MultiServiceRouter,
    name: &str,
    metric_name: &str,
    change_token: &str,
) -> String {
    let body = ok_call(
        router,
        "CreateWebACL",
        &json!({
            "Name": name,
            "MetricName": metric_name,
            "DefaultAction": {"Type": "ALLOW"},
            "ChangeToken": change_token,
        }),
    )
    .await;
    body["WebACL"]["WebACLId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWebACL returned no WebACLId: {body}"))
        .to_string()
}

/// The `WebACL` `GetWebACL` reports for `web_acl_id`.
async fn get_web_acl(router: &MultiServiceRouter, web_acl_id: &str) -> Value {
    let body = ok_call(router, "GetWebACL", &json!({"WebACLId": web_acl_id})).await;
    body["WebACL"].clone()
}

/// `ResourceId` values AWS WAF Classic hands out are 36-character UUID-shaped
/// strings, and the model bounds `WebACLId` at `len 1..128, pattern \S`.
fn is_resource_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(groups.iter())
            .all(|(len, group)| group.len() == *len && group.chars().all(|c| c.is_ascii_hexdigit()))
        && (1..=128).contains(&id.len())
        && !id.chars().any(char::is_whitespace)
}

/// The `WebACLArn` a web ACL minted in `region` must carry:
/// `arn:aws:waf-regional:<region>:<the emulator's account>:webacl/<WebACLId>`.
fn expected_arn(region: &str, web_acl_id: &str) -> String {
    format!(
        "arn:aws:waf-regional:{region}:{}:webacl/{web_acl_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// `CreateWebACL` stores a new WebACL under a minted id and answers the projection of
/// that record; `GetWebACL` reads the same record back.
///
/// The table spans the `DefaultAction` catalog (`WafActionType`: the model's
/// `WAF_ACTION_TYPE_VALUES`), `Name` and `MetricName` lengths and forms. `ResourceName`
/// carries no uniqueness constraint, so the last two rows share a name and must still
/// be two distinct resources.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_web_acl() {
    let router = router();
    let longest_name = "n".repeat(128);
    let longest_metric_name = "M".repeat(128);
    let cases: &[(&str, &str, &str, &str)] = &[
        // (label, Name, MetricName, DefaultAction.Type)
        ("allow by default", "bootstrap-webacl", "bootstrapWebACL", "ALLOW"),
        ("block by default", "edge-block", "edgeBlock", "BLOCK"),
        ("count by default", "edge-count", "edgeCount", "COUNT"),
        ("one character of each (the minimum length)", "x", "m", "ALLOW"),
        (
            "128 characters of each (the maximum length)",
            longest_name.as_str(),
            longest_metric_name.as_str(),
            "BLOCK",
        ),
        (
            "a name with mixed case, digits and separators",
            "Prod_WebACL-2",
            "ProdWebACL2",
            "ALLOW",
        ),
        ("an interior space in the name", "corp office acl", "corpOffice", "BLOCK"),
        ("a name shared with the next row", "shared-name", "sharedOne", "ALLOW"),
        ("the same name again", "shared-name", "sharedTwo", "BLOCK"),
    ];

    // Every modelled default action is exercised by some row above.
    for action in WAF_ACTION_TYPE_VALUES {
        assert!(
            cases.iter().any(|(_, _, _, case_action)| case_action == action),
            "the table must cover the modelled WafActionType {action:?}"
        );
    }

    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for (label, name, metric_name, default_action) in cases {
        let request = create_request(&router, name, metric_name, default_action).await;
        let token = request["ChangeToken"].as_str().unwrap_or_default().to_string();
        let created = ok_call(&router, "CreateWebACL", &request).await;

        assert_eq!(
            created["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label}: CreateWebACL must report the change token it consumed: {created}"
        );

        let web_acl = created["WebACL"].clone();
        let web_acl_id = web_acl["WebACLId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: no WebACLId was returned: {created}"))
            .to_string();
        assert!(
            is_resource_id(&web_acl_id),
            "case {label}: expected a minted WebACLId, got {web_acl_id:?}"
        );
        assert!(
            seen_ids.insert(web_acl_id.clone()),
            "case {label}: WebACLId {web_acl_id} was already handed out"
        );
        assert_eq!(
            web_acl["Name"].as_str(),
            Some(*name),
            "case {label}: expected Name {name:?}, got {}",
            web_acl["Name"]
        );
        assert_eq!(
            web_acl["MetricName"].as_str(),
            Some(*metric_name),
            "case {label}: expected MetricName {metric_name:?}, got {}",
            web_acl["MetricName"]
        );
        assert_eq!(
            web_acl["DefaultAction"]["Type"].as_str(),
            Some(*default_action),
            "case {label}: expected DefaultAction.Type {default_action:?}, got {}",
            web_acl["DefaultAction"]
        );
        assert_eq!(
            web_acl["Rules"].as_array().map(Vec::len),
            Some(0),
            "case {label}: a newly created WebACL activates no Rules, got {}",
            web_acl["Rules"]
        );
        assert_eq!(
            web_acl["WebACLArn"].as_str(),
            Some(expected_arn(REGION, &web_acl_id).as_str()),
            "case {label}: expected WebACLArn {:?}, got {}",
            expected_arn(REGION, &web_acl_id),
            web_acl["WebACLArn"]
        );

        // The round trip: the reader reports the record that was stored, field for
        // field, not a re-rendering of the create request.
        let fetched = get_web_acl(&router, &web_acl_id).await;
        assert_eq!(
            fetched, web_acl,
            "case {label}: GetWebACL must read back the created WebACL, got {fetched} \
             after {web_acl}"
        );
    }
}

/// `WebACLArn` names the region the caller signed for, and the web ACL keeps that ARN
/// however a later reader signs: the ARN is minted once, with the resource. A scope
/// region that is not shaped like a region name falls back to the emulator's default,
/// since the real service's ARN region is endpoint-derived rather than caller-asserted.
#[tokio::test]
async fn web_acl_arn_names_the_region_the_creating_caller_signed_for() {
    let router = router();
    // Regions differing in shape and length, plus scope values no AWS region name has.
    for (label, scope_region, arn_region) in [
        ("the default region", "us-east-1", "us-east-1"),
        ("another commercial region", "eu-west-3", "eu-west-3"),
        ("a longer region name", "ap-southeast-4", "ap-southeast-4"),
        ("a scope region with underscores", "us_east_1", "us-east-1"),
        ("a scope region of one group", "local", "us-east-1"),
        ("a scope region in upper case", "US-EAST-1", "us-east-1"),
    ] {
        let token = {
            let body = call_in_region(&router, scope_region, "GetChangeToken", &json!({}))
                .await
                .1;
            body["ChangeToken"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label}: GetChangeToken returned no token: {body}"))
                .to_string()
        };
        let (status, created) = call_in_region(
            &router,
            scope_region,
            "CreateWebACL",
            &json!({
                "Name": format!("regional-{scope_region}"),
                "MetricName": "regionalAcl",
                "DefaultAction": {"Type": "ALLOW"},
                "ChangeToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: CreateWebACL expected 200, got {status}: {created}"
        );
        let web_acl_id = created["WebACL"]["WebACLId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: no WebACLId was returned: {created}"))
            .to_string();
        let expected = expected_arn(arn_region, &web_acl_id);
        assert_eq!(
            created["WebACL"]["WebACLArn"].as_str(),
            Some(expected.as_str()),
            "case {label}: expected WebACLArn {expected:?}, got {}",
            created["WebACL"]["WebACLArn"]
        );

        // A reader in a different region reports the stored ARN, not one rebuilt for
        // its own region.
        let (status, fetched) = call_in_region(
            &router,
            "us-west-2",
            "GetWebACL",
            &json!({"WebACLId": web_acl_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: GetWebACL expected 200, got {status}: {fetched}"
        );
        assert_eq!(
            fetched["WebACL"]["WebACLArn"].as_str(),
            Some(expected.as_str()),
            "case {label}: a read from another region must report the stored ARN \
             {expected:?}, got {}",
            fetched["WebACL"]["WebACLArn"]
        );
    }
}

/// `MetricName` "can contain only alphanumeric characters (A-Z, a-z, 0-9) ... It can't
/// contain whitespace or metric names reserved for AWS WAF, including "All" and
/// "Default_Action."" — so a name outside that set is refused, and one inside it is
/// stored. Source: the model's `MetricName` member documentation.
#[tokio::test]
async fn metric_names_outside_the_documented_set_are_refused() {
    let router = router();

    let longest = "m".repeat(128);
    let accepted: &[(&str, &str)] = &[
        ("alphanumeric with digits", "edgeAcl2024"),
        ("one character (the minimum length)", "e"),
        ("128 characters (the maximum length)", longest.as_str()),
        ("all upper case", "EDGEACL"),
        // A reserved name is reserved as a WHOLE name, not as a substring.
        ("a reserved name as a prefix", "AllRequests"),
        ("a reserved name as a suffix", "metricAll"),
    ];
    for (label, metric_name) in accepted {
        let request = create_request(&router, "metric-probe", metric_name, "ALLOW").await;
        let created = ok_call(&router, "CreateWebACL", &request).await;
        assert_eq!(
            created["WebACL"]["MetricName"].as_str(),
            Some(*metric_name),
            "case {label}: expected MetricName {metric_name:?} to be stored, got {created}"
        );
    }

    let mut refused: Vec<(String, String)> = vec![
        ("an interior space".to_string(), "edge acl".to_string()),
        ("a leading space".to_string(), " edgeAcl".to_string()),
        ("an underscore".to_string(), "edge_acl".to_string()),
        ("a hyphen".to_string(), "edge-acl".to_string()),
        ("a dot".to_string(), "edge.acl".to_string()),
        ("a non-ASCII letter".to_string(), "edgeAclé".to_string()),
    ];
    // Every name the model reserves for AWS WAF, including the two the documentation
    // names, is refused.
    for reserved in RESERVED_METRIC_NAMES {
        refused.push((format!("the reserved name {reserved}"), (*reserved).to_string()));
    }
    for (label, metric_name) in &refused {
        let request = create_request(&router, "metric-probe", metric_name, "ALLOW").await;
        expect_error(
            &router,
            "CreateWebACL",
            &request,
            "WAFDisallowedNameException",
            label,
        )
        .await;
    }

    // One past the modelled `@length` bound of 1..128 is refused as invalid input
    // before the handler ever runs.
    let too_long = "m".repeat(129);
    let request = create_request(&router, "metric-probe", &too_long, "ALLOW").await;
    let (status, body) = call(&router, "CreateWebACL", &request).await;
    assert_eq!(
        status, 400,
        "case 129 characters (one past the maximum length): expected 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException")
            || error_code(&body).contains("WAFDisallowedNameException"),
        "case 129 characters (one past the maximum length): expected the request to be \
         refused as invalid input, got {}",
        error_code(&body)
    );
}

/// A `DefaultAction.Type` outside the modelled `WafActionType` enum is refused:
/// "You tried to create a WebACL with a DefaultAction Type other than ALLOW, BLOCK, or
/// COUNT" is what `WAFInvalidParameterException` documents.
#[tokio::test]
async fn a_default_action_outside_the_modelled_enum_is_refused() {
    let router = router();
    for (label, action) in [
        ("a type the enum does not name", "DENY"),
        ("the right value in the wrong case", "allow"),
        ("a value with trailing whitespace", "ALLOW "),
        ("the empty string", ""),
    ] {
        let request = create_request(&router, "action-probe", "actionProbe", action).await;
        let (status, body) = call(&router, "CreateWebACL", &request).await;
        assert_eq!(
            status, 400,
            "case {label}: CreateWebACL must refuse DefaultAction.Type {action:?}, \
             got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("WAFInvalidParameterException")
                || error_code(&body).contains("ValidationException"),
            "case {label}: expected the request to be refused as invalid input, got {}",
            error_code(&body)
        );
    }
}

/// `CreateWebACL` follows the change-token protocol: it consumes a `PROVISIONED` token
/// exactly once ("each create, update, or delete request must use a unique change
/// token"), and a request it refuses before making the change leaves the token usable.
#[tokio::test]
async fn create_web_acl_consumes_its_change_token_exactly_once() {
    let router = router();

    let token = change_token(&router).await;
    // A refusal is not a change, so it must not spend the token.
    expect_error(
        &router,
        "CreateWebACL",
        &json!({
            "Name": "token-probe",
            "MetricName": "All",
            "DefaultAction": {"Type": "ALLOW"},
            "ChangeToken": token,
        }),
        "WAFDisallowedNameException",
        "a refused create does not consume the token",
    )
    .await;

    let created = ok_call(
        &router,
        "CreateWebACL",
        &json!({
            "Name": "token-probe",
            "MetricName": "tokenProbe",
            "DefaultAction": {"Type": "ALLOW"},
            "ChangeToken": token,
        }),
    )
    .await;
    assert_eq!(
        created["ChangeToken"].as_str(),
        Some(token.as_str()),
        "CreateWebACL must report the change token it consumed: {created}"
    );

    for (label, stale) in [
        ("already consumed", token.clone()),
        (
            "never minted",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
    ] {
        expect_error(
            &router,
            "CreateWebACL",
            &json!({
                "Name": "token-probe",
                "MetricName": "tokenProbe",
                "DefaultAction": {"Type": "ALLOW"},
                "ChangeToken": stale,
            }),
            "WAFStaleDataException",
            label,
        )
        .await;
    }
}

/// `GetWebACL` refuses an id that names no stored WebACL — "the referenced object
/// doesn't exist" — and reading one twice reports the same record, since a read writes
/// nothing.
#[tokio::test]
async fn get_web_acl_refuses_an_id_that_names_no_web_acl() {
    let router = router();

    let web_acl_id = create_web_acl(&router, "reader-probe", "readerProbe", "BLOCK").await;
    // An id minted for a DIFFERENT resource type is not a WebACLId either.
    let ip_set_token = change_token(&router).await;
    let ip_set = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "not-a-web-acl", "ChangeToken": ip_set_token}),
    )
    .await;
    let ip_set_id = ip_set["IPSet"]["IPSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIPSet returned no IPSetId: {ip_set}"))
        .to_string();

    for (label, id) in [
        ("a UUID this service never minted", "11111111-2222-4333-8444-555555555555"),
        ("an id of another resource type", ip_set_id.as_str()),
        ("a value that is not an id at all", "no-such-web-acl"),
    ] {
        expect_error(
            &router,
            "GetWebACL",
            &json!({"WebACLId": id}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
    }

    let first = get_web_acl(&router, &web_acl_id).await;
    let second = get_web_acl(&router, &web_acl_id).await;
    assert_eq!(
        first, second,
        "a read must not change what a later caller sees: {first} then {second}"
    );
}

/// `Tags` are persisted with the resource and change nothing about the `WebACL` the
/// service reports: the modelled `WebACL` shape carries no tags.
#[tokio::test]
async fn tags_given_at_create_do_not_change_the_reported_web_acl() {
    let router = router();
    for (label, tags) in [
        ("one tag", json!([{"Key": "team", "Value": "edge"}])),
        (
            "several tags, one with an empty value",
            json!([{"Key": "team", "Value": "edge"}, {"Key": "cost", "Value": ""}]),
        ),
        (
            "the same key twice (one tag, the last value winning)",
            json!([{"Key": "team", "Value": "first"}, {"Key": "team", "Value": "second"}]),
        ),
        (
            "the maximum number of tags a resource may carry",
            Value::Array(
                (0..MAX_TAGS_PER_RESOURCE)
                    .map(|i| json!({"Key": format!("key-{i}"), "Value": format!("value-{i}")}))
                    .collect(),
            ),
        ),
    ] {
        let token = change_token(&router).await;
        let created = ok_call(
            &router,
            "CreateWebACL",
            &json!({
                "Name": "tagged-acl",
                "MetricName": "taggedAcl",
                "DefaultAction": {"Type": "ALLOW"},
                "ChangeToken": token,
                "Tags": tags,
            }),
        )
        .await;
        let web_acl = created["WebACL"].clone();
        let web_acl_id = web_acl["WebACLId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: no WebACLId was returned: {created}"))
            .to_string();
        assert!(
            web_acl.get("Tags").is_none(),
            "case {label}: the modelled WebACL carries no Tags member, got {web_acl}"
        );
        let fetched = get_web_acl(&router, &web_acl_id).await;
        assert_eq!(
            fetched, web_acl,
            "case {label}: GetWebACL must read back the created WebACL, got {fetched} \
             after {web_acl}"
        );
    }

    // One past the per-resource tag bound is a resource-limit refusal: the retained
    // tag state of one record is bounded, as the count of records is.
    let token = change_token(&router).await;
    let too_many: Vec<Value> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|i| json!({"Key": format!("key-{i}"), "Value": format!("value-{i}")}))
        .collect();
    expect_error(
        &router,
        "CreateWebACL",
        &json!({
            "Name": "tagged-acl",
            "MetricName": "taggedAcl",
            "DefaultAction": {"Type": "ALLOW"},
            "ChangeToken": token,
            "Tags": Value::Array(too_many),
        }),
        "WAFLimitsExceededException",
        "one past the per-resource tag limit",
    )
    .await;
}

/// The message of each refusal is ONE fixed sentence for that exception class: the
/// same string for every request in the class, carrying nothing of the request.
///
/// Source: AWS WAF Classic's recorded behaviour — the identical sentence for a
/// never-created id and for a deleted one, with no request-derived content.
#[tokio::test]
async fn each_refusal_class_answers_one_fixed_request_independent_sentence() {
    let router = router();

    /// The `message` of an error body.
    fn message(body: &Value) -> String {
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
            .to_string()
    }

    // Two absent ids, differing in form and length: one sentence, naming neither.
    let absent = [
        "11111111-2222-4333-8444-555555555555",
        "an-id-that-was-never-minted",
    ];
    let mut not_found: Vec<String> = Vec::new();
    for id in absent {
        let body = expect_error(
            &router,
            "GetWebACL",
            &json!({"WebACLId": id}),
            "WAFNonexistentItemException",
            id,
        )
        .await;
        let text = message(&body);
        assert!(
            !text.contains(id),
            "case {id}: the not-found message must not echo the requested id, got {text:?}"
        );
        not_found.push(text);
    }
    assert_eq!(
        not_found[0], not_found[1],
        "every WAFNonexistentItemException must carry the same sentence, got {not_found:?}"
    );

    // Two stale tokens: one sentence, naming neither token.
    let consumed = change_token(&router).await;
    create_web_acl_with_token(&router, "fixed-message-probe", "fixedMessage", &consumed).await;
    let stale = [consumed.as_str(), "11111111-2222-4333-8444-555555555555"];
    let mut stale_messages: Vec<String> = Vec::new();
    for token in stale {
        let body = expect_error(
            &router,
            "CreateWebACL",
            &json!({
                "Name": "fixed-message-probe",
                "MetricName": "fixedMessage",
                "DefaultAction": {"Type": "ALLOW"},
                "ChangeToken": token,
            }),
            "WAFStaleDataException",
            token,
        )
        .await;
        let text = message(&body);
        assert!(
            !text.contains(token),
            "case {token}: the stale-token message must not echo the token, got {text:?}"
        );
        stale_messages.push(text);
    }
    assert_eq!(
        stale_messages[0], stale_messages[1],
        "every WAFStaleDataException must carry the same sentence, got {stale_messages:?}"
    );

    // Two disallowed metric names: one sentence, naming neither name.
    let mut disallowed: Vec<String> = Vec::new();
    for metric_name in ["All", "edge acl"] {
        let request = create_request(&router, "fixed-message-probe", metric_name, "ALLOW").await;
        let body = expect_error(
            &router,
            "CreateWebACL",
            &request,
            "WAFDisallowedNameException",
            metric_name,
        )
        .await;
        let text = message(&body);
        assert!(
            !text.contains(metric_name),
            "case {metric_name}: the disallowed-name message must not echo the name, \
             got {text:?}"
        );
        disallowed.push(text);
    }
    assert_eq!(
        disallowed[0], disallowed[1],
        "every WAFDisallowedNameException must carry the same sentence, got {disallowed:?}"
    );
}

/// The number of WebACLs an account can hold is bounded, so the create past the bound
/// is `WAFLimitsExceededException` — "the maximum number of WebACL objects that you
/// can create for an AWS account".
#[tokio::test]
async fn creating_past_the_account_web_acl_limit_is_refused() {
    let router = router();
    for index in 0..MAX_WEB_ACLS {
        let request = create_request(
            &router,
            &format!("acl-{index}"),
            &format!("acl{index}"),
            "ALLOW",
        )
        .await;
        let created = ok_call(&router, "CreateWebACL", &request).await;
        assert!(
            created["WebACL"]["WebACLId"].is_string(),
            "case WebACL {index} of {MAX_WEB_ACLS}: expected a created WebACL, got {created}"
        );
    }

    let request = create_request(&router, "one-too-many", "oneTooMany", "ALLOW").await;
    let token = request["ChangeToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    expect_error(
        &router,
        "CreateWebACL",
        &request,
        "WAFLimitsExceededException",
        "one past the account limit",
    )
    .await;
    // The refused create made no change, so its token is still the outstanding one.
    assert_eq!(
        change_token(&router).await,
        token,
        "a create refused at the limit must not consume its change token"
    );
}
