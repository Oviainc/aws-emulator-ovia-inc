// SPDX-License-Identifier: CC0-1.0

//! A call that CHANGES a rule is authorized against the calling principal's identity-based
//! policies, evaluated with the TARGET rule's own tags — end to end over the wire through the
//! real `MultiServiceRouter`, with IAM and the token service mounted beside EventBridge.
//!
//! The RULE asserted here: changing a rule is the action `events:<OperationName>` on that
//! rule's ARN, so it is allowed only when an `Allow` about it in the identity-based policies
//! in force on the principal the request's credential speaks for covers it, and an explicit
//! `Deny` refuses it whatever grants it; a statement's `Condition` is decided with the
//! `aws:ResourceTag/<key>` values of the ADDRESSED rule, so the decision follows that rule's
//! tags and nothing of the request. A call no document grants is refused with
//! `AccessDeniedException` and the rule is left exactly as it was.
//!
//! The rows span the class rather than the one recorded case:
//!
//! * [`every_rule_mutator_follows_the_target_rules_tags`] varies the OPERATION — every
//!   operation of this service that writes a rule — and asserts the same tag-scoped grant
//!   decides each of them, with the rule's whole observable state (its configuration, its
//!   targets and its tags) unchanged by a refused call and changed by an accepted one.
//! * [`a_tag_scoped_statement_is_decided_against_the_rules_own_tags`] varies the STATEMENT
//!   and the rule's TAGS: the condition operator (equality, wildcard, negation, existence,
//!   the `IfExists` suffix), whether the rule carries the key at all, the shape of the tag
//!   key and of its value, the statement's effect, the resource it names, a principal holding
//!   no document at all and the account's own credential.
//! * [`retagging_the_rule_changes_the_answer_to_the_identical_call`] holds the call fixed and
//!   moves the tag, which is what makes the decision a fact of the rule rather than of the
//!   request.
//!
//! The principal is an assumed ROLE, minted by the token service, because that is the
//! identity a least-privilege policy is attached to; one row of the statement table presents
//! the account's own credential instead, which holds no identity-based policies.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element reader this suite needs are the ones
// `tests/all/target_dispatch.rs` owns for this test binary, so a request is built and read
// exactly one way across the crate.
use crate::target_dispatch::{form_encode, xml_text};

/// The Region every call below is signed for.
const REGION: &str = "eu-west-2";

/// The access key id the account's own administrative calls present: the account holds no
/// record of it, so it resolves to the account's own published credential.
const ACCOUNT_KEY: &str = "AKIDTESTTESTTEST";

/// The bus the rules below live on, and a second rule name used only as another statement's
/// `Resource`.
const BUS: &str = "scoped-bus";
const OTHER_RULE: &str = "elsewhere";

/// The tag key the statements below scope themselves by, and the two values the rules carry.
const TAG_KEY: &str = "team";
const GRANTED_VALUE: &str = "alpha";
const OTHER_VALUE: &str = "beta";

/// The second tag every rule of the operation table carries, so a tag REMOVAL has a key to
/// remove that is not the one the grant is scoped by.
const EXTRA_TAG_KEY: &str = "extra";
const EXTRA_TAG_VALUE: &str = "1";

/// The role the scoped policy is attached to, and the session name it is assumed under.
const ROLE_NAME: &str = "rule-editor";
const SESSION_NAME: &str = "probe";

/// The name of the inline policy the role carries.
const POLICY_NAME: &str = "edit-tagged-rules";

/// The pattern every rule below is created with.
const PATTERN: &str = r#"{"source":["com.demo.orders"]}"#;

/// EventBridge, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the role a session is minted of is the record IAM
/// holds, and this service reads both.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing: the `Authorization` header carries only the credential scope,
    // which is where a service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    Emulator { router }
}

/// The emulator's account id, which every ARN below carries.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of the rule `name` on [`BUS`], the form this service mints.
fn rule_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:rule/{BUS}/{name}", account())
}

/// The ARN of the IAM role the scoped policy is attached to.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE_NAME}", account())
}

/// One EventBridge `awsJson1_1` call presenting `access_key_id`, as its status and parsed
/// body. The access key id is what selects the principal the request speaks for.
async fn events_as(
    emulator: &Emulator,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&payload).to_string()))
    };
    (status, parsed)
}

/// One EventBridge call the account's own credential makes, which must be served.
async fn events(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = events_as(emulator, ACCOUNT_KEY, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} by the account's own credential answered {status} {answered}"
    );
    answered
}

/// One awsQuery call against the service whose signing name is `signing_name`, presenting
/// `access_key_id`, as its status and raw body.
async fn query(
    emulator: &Emulator,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = emulator.router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Create the role, attach `policy` to it when the case holds one, assume it through the
/// token service, and answer `(access key id, the principal ARN STS reports)` — which is the
/// identity a refusal names.
async fn assumed_role(emulator: &Emulator, case: &str, policy: Option<&Value>) -> (String, String) {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", account()) },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, body) = query(
        emulator,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status} {body}"
    );
    if let Some(policy) = policy {
        put_role_policy(emulator, case, policy).await;
    }
    let (status, body) = query(
        emulator,
        ACCOUNT_KEY,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", role_arn()),
            ("RoleSessionName", SESSION_NAME.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssumeRole answered {status} {body}"
    );
    let key = xml_text(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("case {case:?}: AssumeRole answered no AccessKeyId: {body}"));
    // The principal the session speaks for, read from the answer rather than spelled here:
    // it is what `sts:GetCallerIdentity` reports and what a refusal blames.
    let principal = xml_text(&body, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: AssumeRole answered no Arn: {body}"));
    (key, principal)
}

/// Make `policy` the role's whole inline policy from now on.
async fn put_role_policy(emulator: &Emulator, case: &str, policy: &Value) {
    let (status, body) = query(
        emulator,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME.to_string()),
            ("PolicyName", POLICY_NAME.to_string()),
            ("PolicyDocument", policy.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy answered {status} {body}"
    );
}

/// An identity-based document holding `statements`.
fn document(statements: Vec<Value>) -> Value {
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// One identity-based statement: `effect` on `action` over `resource`, scoped by `condition`.
fn statement(effect: &str, action: &str, resource: &str, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A clause over the tag `key` of the ADDRESSED resource, under `operator`.
fn on_resource_tag(operator: &str, key: &str, value: &str) -> Value {
    json!({ operator: { format!("aws:ResourceTag/{key}"): value } })
}

/// Create [`BUS`] and the rule `name` on it, in `state`, carrying `tags` and one target when
/// `with_target`.
async fn given_rule(
    emulator: &Emulator,
    case: &str,
    name: &str,
    state: &str,
    tags: &[(&str, &str)],
    with_target: bool,
) {
    let tag_list: Vec<Value> = tags
        .iter()
        .map(|(key, value)| json!({ "Key": key, "Value": value }))
        .collect();
    events(
        emulator,
        case,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": BUS,
            "EventPattern": PATTERN,
            "State": state,
            "Tags": tag_list,
        }),
    )
    .await;
    if with_target {
        events(
            emulator,
            case,
            "PutTargets",
            &json!({
                "Rule": name,
                "EventBusName": BUS,
                "Targets": [{ "Id": "t1", "Arn": probe_target() }],
            }),
        )
        .await;
    }
}

/// The ARN a target of these rules points at. Nothing is ever published here: the suite is
/// about who may EDIT a rule, not about delivery.
fn probe_target() -> String {
    format!("arn:aws:sqs:{REGION}:{}:probe", account())
}

/// Create the bus the rules live on.
async fn given_bus(emulator: &Emulator, case: &str) {
    events(emulator, case, "CreateEventBus", &json!({ "Name": BUS })).await;
}

/// The wire error code of a failure body (`__type` on awsJson), without its namespace.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` a failure body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Which of IAM's rules withheld the grant, which is what a refusal's closing clause names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// Nothing in force allowed the call.
    NoGrant,
    /// An explicit deny in the caller's identity-based policy.
    ExplicitDeny,
}

impl Refusal {
    /// The sentence this refusal carries for `principal` asking for `action` on `resource`.
    fn message(self, principal: &str, action: &str, resource: &str) -> String {
        let reason = match self {
            Self::NoGrant => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => {
                "with an explicit deny in an identity-based policy".to_string()
            }
        };
        format!(
            "User: {principal} is not authorized to perform: {action} on resource: {resource} \
             {reason}"
        )
    }
}

/// Assert `answered` is the refusal `refusal` states for `action` on `resource`.
fn assert_refused(
    case: &str,
    status: u16,
    answered: &Value,
    refusal: Refusal,
    principal: &str,
    action: &str,
    resource: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the call to be refused, got {status} {answered}"
    );
    let code = error_code(answered);
    assert_eq!(
        code, "AccessDeniedException",
        "case {case:?}: expected the code AccessDeniedException, got {code:?} in {answered}"
    );
    let expected = refusal.message(principal, action, resource);
    let reported = message(answered);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the refusal {expected:?}, got {reported:?}"
    );
}

/// Everything about one rule a client can read back: its configuration, its target list and
/// its tags, each with the status the read answered — so a rule a call DELETED is as visible
/// here as one it changed. Read with the account's own credential, which no statement of this
/// suite bounds.
async fn snapshot(emulator: &Emulator, name: &str) -> Value {
    let (described, describe) = events_as(
        emulator,
        ACCOUNT_KEY,
        "DescribeRule",
        &json!({ "Name": name, "EventBusName": BUS }),
    )
    .await;
    let (targeted, targets) = events_as(
        emulator,
        ACCOUNT_KEY,
        "ListTargetsByRule",
        &json!({ "Rule": name, "EventBusName": BUS }),
    )
    .await;
    let (tagged, tags) = events_as(
        emulator,
        ACCOUNT_KEY,
        "ListTagsForResource",
        &json!({ "ResourceARN": rule_arn(name) }),
    )
    .await;
    json!({
        "describe": [described, describe],
        "targets": [targeted, targets],
        "tags": [tagged, tags],
    })
}

/// The `State` a rule reads back as, or `None` when this bus holds no such rule.
async fn rule_state(emulator: &Emulator, name: &str) -> Option<String> {
    let (status, described) = events_as(
        emulator,
        ACCOUNT_KEY,
        "DescribeRule",
        &json!({ "Name": name, "EventBusName": BUS }),
    )
    .await;
    if status != 200 {
        return None;
    }
    described
        .get("State")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One row of the operation table: the operation, the request it makes against the rule
/// `name`, and the fixture that operation needs to make its effect observable.
struct OpRow {
    /// The operation, which is also the suffix of the IAM action it performs.
    operation: &'static str,
    /// The `State` the rules are created in, so the operation changes it.
    initial_state: &'static str,
    /// Whether the rules are created carrying a target, for an operation that removes one.
    with_target: bool,
}

/// Every operation of this service that WRITES a rule. `PutRule` is the create-or-update,
/// `DeleteRule` the removal, `DisableRule`/`EnableRule` the documented pause and resume,
/// `PutTargets`/`RemoveTargets` the target list, and `TagResource`/`UntagResource` the tag set
/// — the eight AWS names in one sentence as the calls that cannot be made against a managed
/// rule (`ManagedRuleException`: "you cannot modify these managed rules by using DisableRule,
/// EnableRule, PutTargets, PutRule, TagResource, or UntagResource").
fn operation_rows() -> Vec<OpRow> {
    vec![
        OpRow {
            operation: "DisableRule",
            initial_state: "ENABLED",
            with_target: false,
        },
        OpRow {
            operation: "EnableRule",
            initial_state: "DISABLED",
            with_target: false,
        },
        OpRow {
            operation: "PutRule",
            initial_state: "ENABLED",
            with_target: false,
        },
        OpRow {
            // "Before you can delete the rule, you must remove all targets": the rules of
            // this row carry none.
            operation: "DeleteRule",
            initial_state: "ENABLED",
            with_target: false,
        },
        OpRow {
            operation: "PutTargets",
            initial_state: "ENABLED",
            with_target: false,
        },
        OpRow {
            operation: "RemoveTargets",
            initial_state: "ENABLED",
            with_target: true,
        },
        OpRow {
            operation: "TagResource",
            initial_state: "ENABLED",
            with_target: false,
        },
        OpRow {
            operation: "UntagResource",
            initial_state: "ENABLED",
            with_target: false,
        },
    ]
}

/// The request `operation` makes against the rule `name` — the smallest well-formed one, so
/// what the call is refused for is the authorization decision and nothing else.
fn request_for(operation: &str, name: &str) -> Value {
    match operation {
        "DisableRule" | "EnableRule" | "DeleteRule" => {
            json!({ "Name": name, "EventBusName": BUS })
        }
        "PutRule" => json!({
            "Name": name,
            "EventBusName": BUS,
            "EventPattern": PATTERN,
            "Description": "rewritten by the scoped role",
        }),
        "PutTargets" => json!({
            "Rule": name,
            "EventBusName": BUS,
            "Targets": [{ "Id": "added", "Arn": probe_target() }],
        }),
        "RemoveTargets" => json!({ "Rule": name, "EventBusName": BUS, "Ids": ["t1"] }),
        "TagResource" => json!({
            "ResourceARN": rule_arn(name),
            "Tags": [{ "Key": "added", "Value": "2" }],
        }),
        "UntagResource" => json!({
            "ResourceARN": rule_arn(name),
            "TagKeys": [EXTRA_TAG_KEY],
        }),
        other => panic!("the suite makes no request for {other}"),
    }
}

/// RULE — every operation that writes a rule is decided by the same tag-scoped grant: the
/// call against the rule carrying the granted tag value goes through and changes the rule,
/// and the byte-identical call against the rule carrying another value is refused with
/// `AccessDeniedException` while that rule's whole observable state stays exactly as it was.
#[tokio::test]
async fn every_rule_mutator_follows_the_target_rules_tags() {
    for row in operation_rows() {
        let case = row.operation;
        let action = format!("events:{}", row.operation);
        let emulator = emulator();
        given_bus(&emulator, case).await;
        let tags = [
            (TAG_KEY, GRANTED_VALUE),
            (EXTRA_TAG_KEY, EXTRA_TAG_VALUE),
        ];
        let other_tags = [(TAG_KEY, OTHER_VALUE), (EXTRA_TAG_KEY, EXTRA_TAG_VALUE)];
        given_rule(
            &emulator,
            case,
            GRANTED_VALUE,
            row.initial_state,
            &tags,
            row.with_target,
        )
        .await;
        given_rule(
            &emulator,
            case,
            OTHER_VALUE,
            row.initial_state,
            &other_tags,
            row.with_target,
        )
        .await;

        // The role's whole grant: this one action, on any rule of this bus, only where the
        // addressed rule carries the granted tag value.
        let policy = document(vec![statement(
            "Allow",
            &action,
            &format!("arn:aws:events:{REGION}:{}:rule/{BUS}/*", account()),
            Some(on_resource_tag("StringEquals", TAG_KEY, GRANTED_VALUE)),
        )]);
        let (key, principal) = assumed_role(&emulator, case, Some(&policy)).await;

        let before = snapshot(&emulator, OTHER_VALUE).await;
        let (status, answered) = events_as(
            &emulator,
            &key,
            row.operation,
            &request_for(row.operation, OTHER_VALUE),
        )
        .await;
        assert_refused(
            case,
            status,
            &answered,
            Refusal::NoGrant,
            &principal,
            &action,
            &rule_arn(OTHER_VALUE),
        );
        let after = snapshot(&emulator, OTHER_VALUE).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused call must leave the rule untouched, got {after} \
             after {before}"
        );

        // The control: the identical call against the rule the grant covers goes through,
        // and changes something a client can read back — so the refusal above is the
        // decision and not a broken call.
        let covered_before = snapshot(&emulator, GRANTED_VALUE).await;
        let (status, answered) = events_as(
            &emulator,
            &key,
            row.operation,
            &request_for(row.operation, GRANTED_VALUE),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the covered call to be served, got {status} {answered}"
        );
        let covered_after = snapshot(&emulator, GRANTED_VALUE).await;
        assert_ne!(
            covered_after, covered_before,
            "case {case:?}: the covered call must change the rule, got {covered_after} \
             unchanged"
        );
    }
}

/// One row of the statement table: the tags the addressed rule carries, the document in force
/// on the caller, and the verdict.
struct StatementRow {
    case: &'static str,
    /// The tags the rule the call addresses carries.
    tags: &'static [(&'static str, &'static str)],
    /// The document in force on the caller, or `None` for a principal holding none at all.
    policy: Option<Value>,
    /// Set for the one row that presents the account's own published credential rather than
    /// an assumed role.
    account_credential: bool,
    /// The refusal the call earns, or `None` when it is served.
    refusal: Option<Refusal>,
}

/// The action every row of the statement table asks for.
const DISABLE_ACTION: &str = "events:DisableRule";

/// A document granting `DISABLE_ACTION` over every rule of [`BUS`], scoped by `condition`.
fn scoped_allow(condition: Option<Value>) -> Value {
    document(vec![statement(
        "Allow",
        DISABLE_ACTION,
        &format!("arn:aws:events:{REGION}:{}:rule/{BUS}/*", account()),
        condition,
    )])
}

/// The rows. The tag keys and values differ along what the rule generalises over: the
/// operator, whether the rule carries the key, the shape of the key and of the value, the
/// effect, the resource named and the kind of caller.
fn statement_rows() -> Vec<StatementRow> {
    vec![
        StatementRow {
            case: "StringEquals on the value the rule carries",
            tags: &[(TAG_KEY, GRANTED_VALUE)],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringEquals",
                TAG_KEY,
                GRANTED_VALUE,
            )))),
            account_credential: false,
            refusal: None,
        },
        StatementRow {
            case: "StringEquals on a value the rule does not carry",
            tags: &[(TAG_KEY, OTHER_VALUE)],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringEquals",
                TAG_KEY,
                GRANTED_VALUE,
            )))),
            account_credential: false,
            refusal: Some(Refusal::NoGrant),
        },
        StatementRow {
            case: "StringEquals on a key the rule does not carry at all",
            tags: &[("owner", GRANTED_VALUE)],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringEquals",
                TAG_KEY,
                GRANTED_VALUE,
            )))),
            account_credential: false,
            refusal: Some(Refusal::NoGrant),
        },
        StatementRow {
            case: "StringLike whose wildcard covers the rule's value",
            tags: &[(TAG_KEY, "alpha-west")],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringLike",
                TAG_KEY,
                "alpha*",
            )))),
            account_credential: false,
            refusal: None,
        },
        StatementRow {
            case: "StringLike whose wildcard does not cover the rule's value",
            tags: &[(TAG_KEY, "beta-west")],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringLike",
                TAG_KEY,
                "alpha*",
            )))),
            account_credential: false,
            refusal: Some(Refusal::NoGrant),
        },
        StatementRow {
            case: "StringNotEquals naming a value the rule does not carry",
            tags: &[(TAG_KEY, GRANTED_VALUE)],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringNotEquals",
                TAG_KEY,
                OTHER_VALUE,
            )))),
            account_credential: false,
            refusal: None,
        },
        StatementRow {
            case: "StringNotEquals naming the rule's own value",
            tags: &[(TAG_KEY, OTHER_VALUE)],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringNotEquals",
                TAG_KEY,
                OTHER_VALUE,
            )))),
            account_credential: false,
            refusal: Some(Refusal::NoGrant),
        },
        StatementRow {
            // `Null` asks about a key's PRESENCE: the addressed rule's tag set is known in
            // full, so a key it does not carry is answered rather than left undecided.
            case: "Null true on a key the rule does not carry",
            tags: &[("owner", GRANTED_VALUE)],
            policy: Some(scoped_allow(Some(
                json!({ "Null": { format!("aws:ResourceTag/{TAG_KEY}"): "true" } }),
            ))),
            account_credential: false,
            refusal: None,
        },
        StatementRow {
            case: "Null true on a key the rule does carry",
            tags: &[(TAG_KEY, GRANTED_VALUE)],
            policy: Some(scoped_allow(Some(
                json!({ "Null": { format!("aws:ResourceTag/{TAG_KEY}"): "true" } }),
            ))),
            account_credential: false,
            refusal: Some(Refusal::NoGrant),
        },
        StatementRow {
            case: "StringEqualsIfExists on a key the rule does not carry",
            tags: &[("owner", GRANTED_VALUE)],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringEqualsIfExists",
                TAG_KEY,
                GRANTED_VALUE,
            )))),
            account_credential: false,
            refusal: None,
        },
        StatementRow {
            case: "an unconditional grant over every rule of the bus",
            tags: &[(TAG_KEY, OTHER_VALUE)],
            policy: Some(scoped_allow(None)),
            account_credential: false,
            refusal: None,
        },
        StatementRow {
            case: "a deny scoped to the rule's own tag beside an unconditional grant",
            tags: &[(TAG_KEY, OTHER_VALUE)],
            policy: Some(document(vec![
                statement(
                    "Allow",
                    DISABLE_ACTION,
                    &format!("arn:aws:events:{REGION}:{}:rule/{BUS}/*", account()),
                    None,
                ),
                statement(
                    "Deny",
                    DISABLE_ACTION,
                    "*",
                    Some(on_resource_tag("StringEquals", TAG_KEY, OTHER_VALUE)),
                ),
            ])),
            account_credential: false,
            refusal: Some(Refusal::ExplicitDeny),
        },
        StatementRow {
            case: "the same deny against a rule carrying another value",
            tags: &[(TAG_KEY, GRANTED_VALUE)],
            policy: Some(document(vec![
                statement(
                    "Allow",
                    DISABLE_ACTION,
                    &format!("arn:aws:events:{REGION}:{}:rule/{BUS}/*", account()),
                    None,
                ),
                statement(
                    "Deny",
                    DISABLE_ACTION,
                    "*",
                    Some(on_resource_tag("StringEquals", TAG_KEY, OTHER_VALUE)),
                ),
            ])),
            account_credential: false,
            refusal: None,
        },
        StatementRow {
            case: "a grant naming another rule's ARN",
            tags: &[(TAG_KEY, GRANTED_VALUE)],
            policy: Some(document(vec![statement(
                "Allow",
                DISABLE_ACTION,
                &rule_arn(OTHER_RULE),
                Some(on_resource_tag("StringEquals", TAG_KEY, GRANTED_VALUE)),
            )])),
            account_credential: false,
            refusal: Some(Refusal::NoGrant),
        },
        StatementRow {
            case: "a grant over another action of the service",
            tags: &[(TAG_KEY, GRANTED_VALUE)],
            policy: Some(document(vec![statement(
                "Allow",
                "events:EnableRule",
                "*",
                Some(on_resource_tag("StringEquals", TAG_KEY, GRANTED_VALUE)),
            )])),
            account_credential: false,
            refusal: Some(Refusal::NoGrant),
        },
        StatementRow {
            // A tag key may carry a `/`, which the condition key's own family separator is:
            // the key named in the statement is the whole text after `aws:ResourceTag/`.
            case: "a tag key carrying a slash",
            tags: &[("team/sub", GRANTED_VALUE)],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringEquals",
                "team/sub",
                GRANTED_VALUE,
            )))),
            account_credential: false,
            refusal: None,
        },
        StatementRow {
            case: "a tag value outside the ASCII range",
            tags: &[(TAG_KEY, "älphä")],
            policy: Some(scoped_allow(Some(on_resource_tag(
                "StringEquals",
                TAG_KEY,
                "älphä",
            )))),
            account_credential: false,
            refusal: None,
        },
        StatementRow {
            case: "a role holding no identity-based policy at all",
            tags: &[(TAG_KEY, GRANTED_VALUE)],
            policy: None,
            account_credential: false,
            refusal: Some(Refusal::NoGrant),
        },
        StatementRow {
            case: "the account's own credential, which holds no identity-based policies",
            tags: &[(TAG_KEY, OTHER_VALUE)],
            policy: None,
            account_credential: true,
            refusal: None,
        },
    ]
}

/// RULE — a statement's `Condition` is decided with the `aws:ResourceTag/<key>` values of the
/// ADDRESSED rule, under every operator this evaluator decides, and the verdict is both the
/// HTTP answer and whether the rule's `State` moved.
#[tokio::test]
async fn a_tag_scoped_statement_is_decided_against_the_rules_own_tags() {
    for row in statement_rows() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case).await;
        given_rule(&emulator, case, GRANTED_VALUE, "ENABLED", row.tags, false).await;

        let (key, principal) = if row.account_credential {
            (
                ACCOUNT_KEY.to_string(),
                ::service_common::caller_principal::ambient_user_arn(),
            )
        } else {
            assumed_role(&emulator, case, row.policy.as_ref()).await
        };

        let (status, answered) = events_as(
            &emulator,
            &key,
            "DisableRule",
            &json!({ "Name": GRANTED_VALUE, "EventBusName": BUS }),
        )
        .await;

        match row.refusal {
            Some(refusal) => {
                assert_refused(
                    case,
                    status,
                    &answered,
                    refusal,
                    &principal,
                    DISABLE_ACTION,
                    &rule_arn(GRANTED_VALUE),
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be served, got {status} {answered}"
            ),
        }

        // What the call DID, read where a client reads it: a refused call leaves the rule
        // enabled, an accepted one disables it.
        let expected = if row.refusal.is_some() {
            "ENABLED"
        } else {
            "DISABLED"
        };
        let reached = rule_state(&emulator, GRANTED_VALUE).await;
        assert_eq!(
            reached.as_deref(),
            Some(expected),
            "case {case:?}: expected the rule to read back {expected}, got {reached:?}"
        );
    }
}

/// RULE — the decision is a fact of the TARGET rule, not of the request: the byte-identical
/// call is refused while the rule carries another tag value, served once the rule is retagged
/// to the granted one, and refused again once the tag moves back.
#[tokio::test]
async fn retagging_the_rule_changes_the_answer_to_the_identical_call() {
    let case = "the tag moved under a fixed call";
    let emulator = emulator();
    given_bus(&emulator, case).await;
    given_rule(
        &emulator,
        case,
        OTHER_VALUE,
        "ENABLED",
        &[(TAG_KEY, OTHER_VALUE)],
        false,
    )
    .await;
    let policy = scoped_allow(Some(on_resource_tag(
        "StringEquals",
        TAG_KEY,
        GRANTED_VALUE,
    )));
    let (key, principal) = assumed_role(&emulator, case, Some(&policy)).await;
    let request = json!({ "Name": OTHER_VALUE, "EventBusName": BUS });

    let (status, answered) = events_as(&emulator, &key, "DisableRule", &request).await;
    assert_refused(
        case,
        status,
        &answered,
        Refusal::NoGrant,
        &principal,
        DISABLE_ACTION,
        &rule_arn(OTHER_VALUE),
    );
    assert_eq!(
        rule_state(&emulator, OTHER_VALUE).await.as_deref(),
        Some("ENABLED"),
        "case {case:?}: the refused call must leave the rule enabled"
    );

    // Retagged to the value the grant names, by the account's own credential.
    events(
        &emulator,
        case,
        "TagResource",
        &json!({
            "ResourceARN": rule_arn(OTHER_VALUE),
            "Tags": [{ "Key": TAG_KEY, "Value": GRANTED_VALUE }],
        }),
    )
    .await;
    let (status, answered) = events_as(&emulator, &key, "DisableRule", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the retagged rule must admit the identical call, got {status} \
         {answered}"
    );
    assert_eq!(
        rule_state(&emulator, OTHER_VALUE).await.as_deref(),
        Some("DISABLED"),
        "case {case:?}: the served call must disable the rule"
    );

    // And back: the tag returned to the value the grant does not name refuses the call
    // again, so the acceptance above was the tag's doing.
    events(
        &emulator,
        case,
        "EnableRule",
        &json!({ "Name": OTHER_VALUE, "EventBusName": BUS }),
    )
    .await;
    events(
        &emulator,
        case,
        "TagResource",
        &json!({
            "ResourceARN": rule_arn(OTHER_VALUE),
            "Tags": [{ "Key": TAG_KEY, "Value": OTHER_VALUE }],
        }),
    )
    .await;
    let (status, answered) = events_as(&emulator, &key, "DisableRule", &request).await;
    assert_refused(
        case,
        status,
        &answered,
        Refusal::NoGrant,
        &principal,
        DISABLE_ACTION,
        &rule_arn(OTHER_VALUE),
    );
    assert_eq!(
        rule_state(&emulator, OTHER_VALUE).await.as_deref(),
        Some("ENABLED"),
        "case {case:?}: the tag moved back must refuse the call again"
    );
}
