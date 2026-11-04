// SPDX-License-Identifier: CC0-1.0

//! The SENTENCES this service states when it refuses a call, end to end over the
//! wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`)
//! through the real `MultiServiceRouter`.
//!
//! The RULE asserted here: where the service itself words a refusal — a
//! precondition on a delete, a page token it cannot resume from, a quota on a
//! tag write, a combination of parameters it does not accept, a construct its
//! pattern reader rejects — the message is that condition's OWN sentence,
//! stated verbatim, with nothing of the request interpolated into it: no
//! resource name, no count, no emulator-authored "Reason: …" clause explaining
//! the rule. Only where the refusal is ABOUT a submitted value does the message
//! carry a part of the request, and then only the part the service names (the
//! member a pattern stopped on, and the position in the pattern text).
//!
//! Each table spans its class rather than one case: the delete rows differ along
//! the resource name and the number of blocking children, the token rows along
//! the listing and the form of the token (not a token at all, well formed but
//! not one of ours, one of ours gone stale), the tag rows along the resource
//! kind and how the quota is crossed (already-held plus new, one oversized
//! request, one key past a full set), the permission rows along which member
//! conflicts and which one is missing, the connection rows along the
//! authorization type and the writer, and the pattern rows along the kind and
//! the depth of the node the reader stopped on.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// Largest number of tags one resource carries: "You can associate as many as 50
/// tags with a resource" (`TagResource`'s documentation, via
/// `smithy_query(query="doc", shape="TagResource")`). Spelled out here, not
/// imported, so a wrong bound in the implementation cannot make a test pass.
const TAG_QUOTA: usize = 50;

/// The emulator: one router carrying only this service over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One `awsJson1_1` call to EventBridge over the router.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` an error body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// This emulator's account id — the account segment of every ARN it mints.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// `arn:aws:events:<region>:<account>:event-bus/<name>`, built here rather than
/// read from the implementation.
fn bus_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/{name}", account())
}

/// `arn:aws:events:<region>:<account>:rule/<name>` — a rule on the account's
/// default bus.
fn default_bus_rule_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:rule/{name}", account())
}

/// A target ARN pointing at some other service's resource.
fn target_arn(resource: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{}:{resource}", account())
}

/// One refused call: assert the status and the wire code, and hand the message
/// back, so a rule about the message ITSELF can be asserted over a SET of calls
/// rather than against one sentence written out beside the implementation.
async fn refusal_message(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    code: &str,
) -> String {
    let (status, body) = events(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        code,
        "case {case:?}: expected {code}, got {body}"
    );
    message(&body).to_string()
}

/// Assert one refused call: the status, the wire code and the whole message.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    code: &str,
    expected: &str,
) {
    let actual = refusal_message(router, case, operation, request, code).await;
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the refusal to state {expected:?}, got {actual:?}"
    );
}

/// Assert one accepted call, so a row that proves a refusal is read beside the
/// requests the same rule admits.
async fn accepted(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let (status, body) = events(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer HTTP 200, got {status} {body}"
    );
    body
}

/// Put a rule on `bus` (the default bus when `None`), carrying `targets` targets.
async fn given_rule(
    router: &MultiServiceRouter,
    case: &str,
    bus: Option<&str>,
    rule: &str,
    targets: usize,
) {
    let mut request = json!({
        "Name": rule,
        "EventPattern": r#"{"source":["com.demo.orders"]}"#,
    });
    if let Some(bus) = bus {
        request["EventBusName"] = json!(bus);
    }
    accepted(router, case, "PutRule", &request).await;
    if targets == 0 {
        return;
    }
    let listed: Vec<Value> = (0..targets)
        .map(|index| {
            json!({
                "Id": format!("t{index}"),
                "Arn": target_arn(&format!("queue-{index}")),
            })
        })
        .collect();
    let mut request = json!({ "Rule": rule, "Targets": listed });
    if let Some(bus) = bus {
        request["EventBusName"] = json!(bus);
    }
    accepted(router, case, "PutTargets", &request).await;
}

/// A delete blocked by a child the caller must remove first states that
/// precondition as ONE sentence: it names neither the resource the request
/// carried nor how many children block it, so every refused delete of that class
/// reads the same.
#[tokio::test]
async fn a_delete_precondition_states_one_sentence_whatever_the_resource_or_the_count() {
    let router = emulator();

    // RULE: a rule that still carries targets is refused whatever the rule is
    // called, whichever bus it is on, and however many targets it carries.
    const RULE_HAS_TARGETS: &str = "Rule can't be deleted since it has targets.";
    // (case, the bus the rule is on, the rule name, how many targets it carries)
    let rule_rows: [(&str, Option<&str>, &str, usize); 4] = [
        ("one target, on the default bus", None, "orders", 1),
        ("three targets, on the default bus", None, "dots.and-dashes_2", 3),
        ("one target, on a custom bus", Some("blocked"), "one", 1),
        ("five targets, on a custom bus", Some("blocked"), "five", 5),
    ];
    accepted(
        &router,
        "the bus the rows below put rules on",
        "CreateEventBus",
        &json!({ "Name": "blocked" }),
    )
    .await;
    for (case, bus, rule, targets) in rule_rows {
        given_rule(&router, case, bus, rule, targets).await;
        let mut request = json!({ "Name": rule, "Force": false });
        if let Some(bus) = bus {
            request["EventBusName"] = json!(bus);
        }
        refused(
            &router,
            case,
            "DeleteRule",
            &request,
            "ValidationException",
            RULE_HAS_TARGETS,
        )
        .await;
        // The same delete succeeds once the targets are gone, so the row above
        // is about the precondition rather than about the rule.
        let ids: Vec<Value> = (0..targets).map(|index| json!(format!("t{index}"))).collect();
        let mut removal = json!({ "Rule": rule, "Ids": ids });
        if let Some(bus) = bus {
            removal["EventBusName"] = json!(bus);
        }
        accepted(&router, case, "RemoveTargets", &removal).await;
        accepted(&router, case, "DeleteRule", &request).await;
    }

    // RULE: a bus that still carries rules is refused whatever the bus is called
    // and however many rules remain on it.
    const BUS_HAS_RULES: &str = "EventBus can't be deleted since it has rules.";
    // (case, the bus name, how many rules remain on it)
    let bus_rows: [(&str, &str, usize); 3] = [
        ("one rule", "one-rule", 1),
        ("three rules", "three.rules", 3),
        ("ten rules", "ten_rules", 10),
    ];
    for (case, bus, rules) in bus_rows {
        accepted(&router, case, "CreateEventBus", &json!({ "Name": bus })).await;
        for index in 0..rules {
            given_rule(&router, case, Some(bus), &format!("r{index}"), 0).await;
        }
        refused(
            &router,
            case,
            "DeleteEventBus",
            &json!({ "Name": bus }),
            "ValidationException",
            BUS_HAS_RULES,
        )
        .await;
        for index in 0..rules {
            accepted(
                &router,
                case,
                "DeleteRule",
                &json!({ "Name": format!("r{index}"), "EventBusName": bus, "Force": false }),
            )
            .await;
        }
        accepted(&router, case, "DeleteEventBus", &json!({ "Name": bus })).await;
    }

    // RULE: the account's default bus is not deletable, and the refusal names the
    // bus the request asked to delete.
    refused(
        &router,
        "the account's default bus",
        "DeleteEventBus",
        &json!({ "Name": "default" }),
        "ValidationException",
        "Cannot delete event bus default.",
    )
    .await;
}

/// A `NextToken` this service cannot resume from is refused with one sentence
/// naming the parameter, whichever listing was asked and whatever the token
/// looked like: no reason clause, and nothing of the token or the listing in it.
#[tokio::test]
async fn a_next_token_this_service_cannot_resume_from_is_refused_with_one_sentence() {
    let router = emulator();
    const INVALID_NEXT_TOKEN: &str = "Parameter NextToken is not valid.";
    given_rule(&router, "the rule the target listing pages", None, "paged", 2).await;

    // The token forms differ along WHY this service cannot resume from them: text
    // that is not one of its tokens at all, text that decodes but carries no
    // position, and an empty-ish token.
    //
    // `7b2262223a2278227d` is the hex of `{"b":"x"}`: well formed as far as the
    // decoder gets, and carrying no position it could resume at.
    let tokens = ["not-a-token", "7b2262223a2278227d", "zzzz", "0"];
    // (case, the listing, the request members it needs beside NextToken)
    let listings: [(&str, &str, Value); 8] = [
        ("event buses", "ListEventBuses", json!({})),
        ("rules", "ListRules", json!({})),
        ("targets of a rule", "ListTargetsByRule", json!({ "Rule": "paged" })),
        ("connections", "ListConnections", json!({})),
        ("archives", "ListArchives", json!({})),
        ("api destinations", "ListApiDestinations", json!({})),
        ("replays", "ListReplays", json!({})),
        (
            "rule names by target",
            "ListRuleNamesByTarget",
            json!({ "TargetArn": target_arn("queue-0") }),
        ),
    ];
    for (listing, operation, members) in listings {
        for token in tokens {
            let case = format!("{listing} resumed from {token:?}");
            let mut request = members.clone();
            request["NextToken"] = json!(token);
            refused(
                &router,
                &case,
                operation,
                &request,
                "ValidationException",
                INVALID_NEXT_TOKEN,
            )
            .await;
        }
    }

    // A token this service DID issue, whose position is gone by the time it comes
    // back: the same sentence, because it is the same answer — this listing
    // cannot resume from it.
    let page = accepted(
        &router,
        "the first page of one of two targets",
        "ListTargetsByRule",
        &json!({ "Rule": "paged", "Limit": 1 }),
    )
    .await;
    let stale = page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of one of two targets carries a NextToken: {page}"))
        .to_string();
    accepted(
        &router,
        "the target the token resumes after",
        "RemoveTargets",
        &json!({ "Rule": "paged", "Ids": ["t0"] }),
    )
    .await;
    refused(
        &router,
        "a token of this service whose position is gone",
        "ListTargetsByRule",
        &json!({ "Rule": "paged", "Limit": 1, "NextToken": stale }),
        "ValidationException",
        INVALID_NEXT_TOKEN,
    )
    .await;
}

/// A tag write that would leave a resource past the per-resource quota is
/// refused with ONE sentence: the same one for every resource kind and however
/// the quota is crossed, carrying neither the resource the request named, nor a
/// count, nor a reason clause of this emulator's own.
///
/// The rule is asserted over the SET of ways the quota is crossed rather than
/// against a sentence written out beside the implementation: every row has to
/// answer the same message, and that message has to be free of anything taken
/// from the call that was refused.
#[tokio::test]
async fn a_tag_write_past_the_per_resource_quota_states_one_sentence() {
    let router = emulator();

    accepted(
        &router,
        "the bus the rows below tag",
        "CreateEventBus",
        &json!({ "Name": "tagged" }),
    )
    .await;
    given_rule(&router, "the rule the rows below tag", None, "tagged-rule", 0).await;

    // (case, the refusal's message), one entry per way of crossing the quota.
    let mut refusals: Vec<(String, String)> = Vec::new();
    // (case, the ARN of the resource the rows tag)
    let resources = [
        ("a custom event bus", bus_arn("tagged")),
        ("a rule on the default bus", default_bus_rule_arn("tagged-rule")),
    ];
    for (resource, arn) in resources {
        let tags = |range: std::ops::Range<usize>| -> Value {
            json!(range
                .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
                .collect::<Vec<Value>>())
        };

        // Already-held tags plus new ones: the quota is charged on the set the
        // write would leave, so 40 held and 11 named is one past it.
        accepted(
            &router,
            &format!("{resource}: forty tags"),
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tags(0..40) }),
        )
        .await;
        let case = format!("{resource}: eleven more onto forty");
        let refusal = refusal_message(
            &router,
            &case,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tags(40..51) }),
            "ValidationException",
        )
        .await;
        refusals.push((case, refusal));
        // Ten more leaves exactly the quota, and re-setting a key already held
        // leaves the set the size it was: neither is refused.
        accepted(
            &router,
            &format!("{resource}: ten more onto forty"),
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tags(40..50) }),
        )
        .await;
        accepted(
            &router,
            &format!("{resource}: a key already held, at the quota"),
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": [{ "Key": "k0", "Value": "rewritten" }] }),
        )
        .await;
        // One NEW key onto a full set.
        let case = format!("{resource}: one new key onto a full set");
        let refusal = refusal_message(
            &router,
            &case,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": [{ "Key": "one-too-many", "Value": "x" }] }),
            "ValidationException",
        )
        .await;
        refusals.push((case, refusal));
    }

    // One request carrying more than the quota, against a resource holding no
    // tags at all.
    accepted(
        &router,
        "the bus the oversized request names",
        "CreateEventBus",
        &json!({ "Name": "untagged" }),
    )
    .await;
    let oversized: Vec<Value> = (0..=TAG_QUOTA)
        .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
        .collect();
    let case = "one request past the quota, onto a resource with no tags".to_string();
    let refusal = refusal_message(
        &router,
        &case,
        "TagResource",
        &json!({ "ResourceARN": bus_arn("untagged"), "Tags": oversized }),
        "ValidationException",
    )
    .await;
    refusals.push((case, refusal));

    // ONE sentence for the whole class: every row above answered the same
    // message, so none of them was composed from the call it refused.
    let (first_case, first) = refusals
        .first()
        .cloned()
        .expect("the rows above record at least one refusal");
    assert!(
        !first.is_empty(),
        "case {first_case:?}: the refusal states nothing at all"
    );
    for (case, refusal) in &refusals {
        assert_eq!(
            refusal, &first,
            "case {case:?}: expected the same sentence as {first_case:?} ({first:?}), got \
             {refusal:?}"
        );
        // Nothing of the request or of the stored set is in it: not the
        // resource, not the count the write would have left, and no reason
        // clause explaining the rule.
        for fragment in [
            "tagged", "untagged", "arn:aws:events", "51", "41", "Reason:",
        ] {
            assert!(
                !refusal.contains(fragment),
                "case {case:?}: the refusal must not carry {fragment:?}, got {refusal:?}"
            );
        }
    }
}

/// Every parameter combination the permission calls do not accept is refused
/// with that combination's own sentence, naming the members the rule is about
/// and echoing nothing the request carried.
#[tokio::test]
async fn a_permission_call_states_one_sentence_per_rejected_combination() {
    let router = emulator();
    let bus = "granting";
    accepted(
        &router,
        "the bus the rows below grant on",
        "CreateEventBus",
        &json!({ "Name": bus }),
    )
    .await;
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "FromDocument",
            "Effect": "Allow",
            "Principal": { "AWS": "111122223333" },
            "Action": "events:PutEvents",
            "Resource": bus_arn(bus),
        }],
    })
    .to_string();
    let condition = json!({
        "Type": "StringEquals",
        "Key": "aws:PrincipalOrgID",
        "Value": "o-1234567890",
    });

    // RULE: `Policy` describes the whole policy, so it cannot be combined with
    // the members that name one grant — whichever of them the request carries.
    const POLICY_AND_GRANT_MEMBERS: &str =
        "Action, Principal, StatementId and Condition could not be combined with Policy parameter.";
    // (case, the grant members sent beside Policy)
    let combinations: [(&str, Value); 5] = [
        ("a statement id", json!({ "StatementId": "sid" })),
        ("an action", json!({ "Action": "events:PutEvents" })),
        ("a principal", json!({ "Principal": "*" })),
        ("a condition", json!({ "Condition": condition.clone() })),
        (
            "every grant member at once",
            json!({
                "StatementId": "sid",
                "Action": "events:PutEvents",
                "Principal": "*",
                "Condition": condition.clone(),
            }),
        ),
    ];
    for (case, members) in combinations {
        let mut request = members.clone();
        request["EventBusName"] = json!(bus);
        request["Policy"] = json!(policy);
        refused(
            &router,
            case,
            "PutPermission",
            &request,
            "ValidationException",
            POLICY_AND_GRANT_MEMBERS,
        )
        .await;
    }

    // RULE: without `Policy` the grant form needs all three of its members, and
    // the refusal names the first one the request lacks in the order
    // `PutPermissionRequest` declares them.
    // (case, the members sent, the member the refusal names)
    let incomplete: [(&str, Value, &str); 5] = [
        ("no member at all", json!({}), "Action"),
        (
            "a statement id and a principal",
            json!({ "StatementId": "sid", "Principal": "*" }),
            "Action",
        ),
        (
            "an action and a statement id",
            json!({ "StatementId": "sid", "Action": "events:PutEvents" }),
            "Principal",
        ),
        (
            "an action and a principal",
            json!({ "Action": "events:PutEvents", "Principal": "*" }),
            "StatementId",
        ),
        (
            "a condition alone",
            json!({ "Condition": condition.clone() }),
            "Action",
        ),
    ];
    for (case, members, named) in incomplete {
        let mut request = members.clone();
        request["EventBusName"] = json!(bus);
        refused(
            &router,
            case,
            "PutPermission",
            &request,
            "ValidationException",
            &format!("Parameter(s) {named} must be specified."),
        )
        .await;
    }

    // RULE: a grant on an event bus enables publication onto it, so any other
    // action is refused.
    for action in ["events:PutRule", "events:DescribeEventBus", "events:PutEvent"] {
        refused(
            &router,
            &format!("a grant of {action}"),
            "PutPermission",
            &json!({
                "EventBusName": bus,
                "StatementId": "sid",
                "Action": action,
                "Principal": "*",
            }),
            "ValidationException",
            "Provided value in parameter 'action' is not supported.",
        )
        .await;
    }

    // RULE: a revoke names one grant or asks for every grant, never both and
    // never neither.
    refused(
        &router,
        "a revoke of one grant and of every grant at once",
        "RemovePermission",
        &json!({ "EventBusName": bus, "StatementId": "sid", "RemoveAllPermissions": true }),
        "ValidationException",
        "StatementId could not be combined with All parameter.",
    )
    .await;
    refused(
        &router,
        "a revoke naming neither",
        "RemovePermission",
        &json!({ "EventBusName": bus }),
        "ValidationException",
        "Parameter(s) StatementId must be specified.",
    )
    .await;
    // The forms the same rules ADMIT: the whole policy on its own, and a grant
    // carrying all three members — so the rows above are about the combination
    // rather than about the call.
    accepted(
        &router,
        "the whole policy on its own",
        "PutPermission",
        &json!({ "EventBusName": bus, "Policy": policy }),
    )
    .await;
    accepted(
        &router,
        "one grant naming every member",
        "PutPermission",
        &json!({
            "EventBusName": bus,
            "StatementId": "sid",
            "Action": "events:PutEvents",
            "Principal": "*",
            "Condition": condition,
        }),
    )
    .await;
    accepted(
        &router,
        "a revoke of the grant just installed",
        "RemovePermission",
        &json!({ "EventBusName": bus, "StatementId": "sid" }),
    )
    .await;
}

/// `AuthParameters` that do not name exactly one authorization type are refused
/// with that rule's own sentence — after the clause naming the call that failed —
/// whichever type the request asked for and whichever writer it reached.
#[tokio::test]
async fn a_connection_write_whose_auth_parameters_name_no_single_type_states_one_sentence() {
    let router = emulator();
    const ONE_TYPE: &str = "Parameter AuthParameters is not valid. Reason: The value for \
                            AuthParameters must include exactly one authorization type.";
    let api_key = json!({ "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" } });
    let basic = json!({ "BasicAuthParameters": { "Username": "u", "Password": "p" } });
    let invocation = json!({
        "InvocationHttpParameters": {
            "HeaderParameters": [{ "Key": "x-plain", "Value": "v", "IsValueSecret": false }],
        },
    });

    // (case, the AuthorizationType, the AuthParameters)
    let rows: Vec<(&str, &str, Value)> = vec![
        ("no parameter at all, API_KEY", "API_KEY", json!({})),
        ("no parameter at all, BASIC", "BASIC", json!({})),
        (
            "no parameter at all, OAUTH_CLIENT_CREDENTIALS",
            "OAUTH_CLIENT_CREDENTIALS",
            json!({}),
        ),
        (
            "invocation parameters, which name no authorization type",
            "API_KEY",
            invocation.clone(),
        ),
        (
            "two authorization types at once",
            "BASIC",
            json!({
                "BasicAuthParameters": { "Username": "u", "Password": "p" },
                "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
            }),
        ),
    ];
    for (index, (case, authorization_type, parameters)) in rows.into_iter().enumerate() {
        refused(
            &router,
            case,
            "CreateConnection",
            &json!({
                "Name": format!("refused-{index}"),
                "AuthorizationType": authorization_type,
                "AuthParameters": parameters,
            }),
            "ValidationException",
            &format!("Failed to create the connection(s). {ONE_TYPE}"),
        )
        .await;
    }

    // The same rule on the UPDATE writer, whose refusal names its own call.
    accepted(
        &router,
        "the connection the update rows start from",
        "CreateConnection",
        &json!({
            "Name": "updatable",
            "AuthorizationType": "API_KEY",
            "AuthParameters": api_key.clone(),
        }),
    )
    .await;
    for (case, parameters) in [
        ("an update naming no authorization type", json!({})),
        ("an update naming only invocation parameters", invocation),
        (
            "an update naming two authorization types",
            json!({
                "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
                "BasicAuthParameters": { "Username": "u", "Password": "p" },
            }),
        ),
    ] {
        refused(
            &router,
            case,
            "UpdateConnection",
            &json!({ "Name": "updatable", "AuthParameters": parameters }),
            "ValidationException",
            &format!("Failed to update the connection(s). {ONE_TYPE}"),
        )
        .await;
    }

    // What the rule ADMITS: exactly one authorization type, on either writer —
    // and an update that carries no `AuthParameters` at all, which leaves the
    // credentials the connection already holds.
    accepted(
        &router,
        "a create naming exactly one authorization type",
        "CreateConnection",
        &json!({
            "Name": "accepted",
            "AuthorizationType": "BASIC",
            "AuthParameters": basic,
        }),
    )
    .await;
    accepted(
        &router,
        "an update naming exactly one authorization type",
        "UpdateConnection",
        &json!({
            "Name": "updatable",
            "AuthorizationType": "API_KEY",
            "AuthParameters": api_key.clone(),
        }),
    )
    .await;
    // Exactly one authorization type is named, but nothing says which type it is
    // for: `AuthorizationType` is the member that selects the block in force, so
    // the update is refused on ITS own sentence rather than on the one above.
    refused(
        &router,
        "an update naming one type's credentials and no AuthorizationType",
        "UpdateConnection",
        &json!({ "Name": "updatable", "AuthParameters": api_key }),
        "ValidationException",
        "Failed to update the connection(s). Parameter AuthParameters is not valid. Reason: \
         AuthorizationType must be provided when AuthParameters is provided.",
    )
    .await;
    accepted(
        &router,
        "an update carrying no authorization parameters",
        "UpdateConnection",
        &json!({ "Name": "updatable", "Description": "left alone" }),
    )
    .await;
}

/// What a refused event PATTERN states: the construct's own sentence, the MEMBER
/// the reader stopped on named as the pattern spells it — not the dotted field
/// path it sits under — and the position one character into the node the reader
/// had entered when it refused.
#[tokio::test]
async fn a_pattern_refusal_names_the_member_it_stopped_on_and_where_it_stopped() {
    let router = emulator();
    let event = json!({
        "version": "0",
        "id": "9c2f6f6a-6f0e-4a3e-9f1b-7a1d2c3b4e5f",
        "account": account(),
        "source": "com.demo.orders",
        "time": "2026-03-04T10:00:00Z",
        "region": REGION,
        "resources": [],
        "detail-type": "Order Placed",
        "detail": { "k": "ok" },
    })
    .to_string();

    // (case, the pattern, the reason, the prefix of the pattern the reader had
    //  consumed when it refused — up to and including the first character of the
    //  node it had entered)
    let rows: [(&str, &str, &str, &str); 9] = [
        // A match expression names ONE match type, so the reader stops on the
        // second one it names, having read that name, the colon after it and the
        // first character of its argument. The rows differ along the KIND of
        // that argument and along how deep the expression sits.
        (
            "two match types, the second taking a string",
            r#"{"detail": {"k": [{"prefix": "a", "suffix": "z"}]}}"#,
            "Only one key allowed in match expression",
            r#"{"detail": {"k": [{"prefix": "a", "suffix": ""#,
        ),
        (
            "two match types, the second taking a number",
            r#"{"detail": {"k": [{"prefix": "a", "numeric": 42}]}}"#,
            "Only one key allowed in match expression",
            r#"{"detail": {"k": [{"prefix": "a", "numeric": 4"#,
        ),
        (
            "two match types, the second taking an object",
            r#"{"detail": {"k": [{"prefix": "a", "anything-but": {"prefix": "b"}}]}}"#,
            "Only one key allowed in match expression",
            r#"{"detail": {"k": [{"prefix": "a", "anything-but": {"#,
        ),
        (
            "two match types, the second taking a list",
            r#"{"detail": {"k": [{"prefix": "a", "anything-but": ["b"]}]}}"#,
            "Only one key allowed in match expression",
            r#"{"detail": {"k": [{"prefix": "a", "anything-but": ["#,
        ),
        (
            "two match types, three fields and two objects in",
            r#"{"source": ["s"], "detail-type": ["t"], "detail": {"a": {"b": [{"prefix": "a", "suffix": "z"}]}}}"#,
            "Only one key allowed in match expression",
            r#"{"source": ["s"], "detail-type": ["t"], "detail": {"a": {"b": [{"prefix": "a", "suffix": ""#,
        ),
        // A field's value is a term list or a nested pattern object, and the
        // refusal names the MEMBER the reader stopped on, however deep it sits.
        (
            "a top-level member whose value is a bare string",
            r#"{"source": "com.demo.orders"}"#,
            r#""source" must be an object or an array"#,
            r#"{"source": ""#,
        ),
        (
            "a member one object in whose value is a bare string",
            r#"{"detail": {"status": "FULFILLED"}}"#,
            r#""status" must be an object or an array"#,
            r#"{"detail": {"status": ""#,
        ),
        (
            "a member two objects in whose value is a boolean",
            r#"{"detail": {"a": {"b": true}}}"#,
            r#""b" must be an object or an array"#,
            r#"{"detail": {"a": {"b": true"#,
        ),
        (
            "a member whose value is a number, two fields in",
            r#"{"source": ["s"], "detail-type": 7}"#,
            r#""detail-type" must be an object or an array"#,
            r#"{"source": ["s"], "detail-type": 7"#,
        ),
    ];

    for (case, pattern, reason, read) in rows {
        assert!(
            pattern.starts_with(read),
            "case {case:?}: the row's prefix {read:?} must be a prefix of {pattern:?}"
        );
        // The column counts CHARACTERS from the start of the line, and the
        // position is the one just past the text the reader had consumed.
        let expected = format!(
            "Event pattern is not valid. Reason: {reason}\n at [Source: (String)\"{pattern}\"; \
             line: 1, column: {}]",
            1 + read.chars().count()
        );
        refused(
            &router,
            case,
            "TestEventPattern",
            &json!({ "EventPattern": pattern, "Event": event }),
            "InvalidEventPatternException",
            &expected,
        )
        .await;
        // One validator reads every pattern this service is handed, so a stored
        // pattern is refused in the same words.
        refused(
            &router,
            case,
            "PutRule",
            &json!({ "Name": "refused", "EventPattern": pattern }),
            "InvalidEventPatternException",
            &expected,
        )
        .await;
    }
}
