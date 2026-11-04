// SPDX-License-Identifier: CC0-1.0

//! Namespaces: the create and the read that reports it, over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a namespace is identified by `(AwsAccountId, Namespace)` and
//! must be unique within that account; `CreateNamespace` persists it, reports its ARN,
//! the Region the call ran in, the identity store it was given and the non-terminal
//! creation status of an asynchronous create, a second create of the same name is
//! refused as a conflict, and `DescribeNamespace` answers from that stored record — reporting the
//! terminal status once the clock owes it. So the rows below vary what the rule
//! generalises over (the name's form and length, whether tags are submitted, and the
//! account) rather than re-running one recorded call.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// A third account that creates nothing.
const EMPTY_ACCOUNT: &str = "123456789012";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";

fn namespace_arn(account: &str, namespace: &str) -> String {
    namespace_arn_in(REGION, account, namespace)
}

/// The ARN a namespace created in `region` under `(account, namespace)` carries.
///
/// `pub(crate)` with the region as a parameter so the sibling suites shape one
/// namespace request the same way: a URI or an ARN that drifted between two copies
/// would hide a defect rather than show it.
pub(crate) fn namespace_arn_in(region: &str, account: &str, namespace: &str) -> String {
    format!("arn:aws:quicksight:{region}:{account}:namespace/{namespace}")
}

async fn create_namespace(harness: &Harness, account: &str, body: Value) -> (u16, Value) {
    create_namespace_in(harness, REGION, account, body).await
}

/// `CreateNamespace`: `POST /accounts/{AwsAccountId}`, signed for `region`.
/// Source: smithy_query(query="operation", operation="CreateNamespace").
pub(crate) async fn create_namespace_in(
    harness: &Harness,
    region: &str,
    account: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call_in_region(region, "POST", &format!("/accounts/{account}"), Some(body))
        .await
}

async fn describe_namespace(harness: &Harness, account: &str, namespace: &str) -> (u16, Value) {
    describe_namespace_in(harness, REGION, account, namespace).await
}

/// `DescribeNamespace`: `GET /accounts/{AwsAccountId}/namespaces/{Namespace}`, signed
/// for `region`.
/// Source: smithy_query(query="operation", operation="DescribeNamespace").
pub(crate) async fn describe_namespace_in(
    harness: &Harness,
    region: &str,
    account: &str,
    namespace: &str,
) -> (u16, Value) {
    harness
        .call_in_region(
            region,
            "GET",
            &format!("/accounts/{account}/namespaces/{namespace}"),
            None,
        )
        .await
}

/// Bring the creation transition the clock owes this namespace forward, so a case can
/// observe the terminal status without sleeping for the span.
pub(crate) fn settle_creation(harness: &Harness, account: &str, namespace: &str) {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let mut store = state.lock();
    let record = store
        .namespaces
        .get_mut(&(account.to_string(), namespace.to_string()))
        .unwrap_or_else(|| panic!("case {namespace:?}: the created namespace is not stored"));
    record.creation_settles_at = SystemTime::now() - Duration::from_millis(1);
}

/// One row of the create-then-read table.
struct Row {
    /// The case label every assertion names.
    case: &'static str,
    account: &'static str,
    /// The `Namespace` the create names; the model admits len 0..64 over the pattern
    /// `^[a-zA-Z0-9._-]*$`.
    /// Source: smithy_query(query="operation", operation="CreateNamespace").
    namespace: String,
    /// The tags the create submits, if any.
    tags: Option<Value>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "plain-name",
            account: ACCOUNT,
            namespace: "sales".to_string(),
            tags: None,
        },
        Row {
            case: "punctuated-name-with-tags",
            account: ACCOUNT,
            namespace: "finance.eu-west_2".to_string(),
            tags: Some(json!([
                { "Key": "team", "Value": "finance" },
                { "Key": "cost-centre", "Value": "4711" },
            ])),
        },
        Row {
            // The shortest name the pattern admits beside the empty one.
            case: "single-character-name",
            account: ACCOUNT,
            namespace: "n".to_string(),
            tags: None,
        },
        Row {
            // The longest name the model admits: `Namespace` is len 0..64.
            case: "longest-name",
            account: ACCOUNT,
            namespace: "n".repeat(64),
            tags: Some(json!([{ "Key": "edge", "Value": "max-length" }])),
        },
        Row {
            case: "other-account",
            account: OTHER_ACCOUNT,
            namespace: "sales".to_string(),
            tags: None,
        },
    ]
}

impl Row {
    fn body(&self) -> Value {
        let mut body = json!({
            "Namespace": self.namespace,
            "IdentityStore": "QUICKSIGHT",
        });
        if let Some(tags) = &self.tags {
            body["Tags"] = tags.clone();
        }
        body
    }
}

#[tokio::test]
async fn create_namespace_persists_it_and_describe_reports_it() {
    let harness = Harness::new();

    for row in rows() {
        let case = row.case;
        let namespace = row.namespace.as_str();
        let (status, body) = create_namespace(&harness, row.account, row.body()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(namespace_arn(row.account, namespace)),
            "case {case:?}: create reported the wrong Arn: {body}"
        );
        assert_eq!(
            body["Name"],
            json!(namespace),
            "case {case:?}: create reported the wrong Name: {body}"
        );
        assert_eq!(
            body["CapacityRegion"],
            json!(REGION),
            "case {case:?}: the capacity Region is the one the call ran in: {body}"
        );
        assert_eq!(
            body["IdentityStore"],
            json!("QUICKSIGHT"),
            "case {case:?}: create reported the wrong IdentityStore: {body}"
        );
        assert_eq!(
            body["CreationStatus"],
            json!("CREATING"),
            "case {case:?}: a namespace whose creation is still in flight reports \
             CREATING: {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: create reported no RequestId: {body}"
        );

        // The read answers from the stored record while the creation is still in
        // flight: the status the create reported is the one a caller acting straight
        // after it sees.
        let (status, body) = describe_namespace(&harness, row.account, namespace).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with body {body}"
        );
        let reported = &body["Namespace"];
        assert_eq!(
            reported["Arn"],
            json!(namespace_arn(row.account, namespace)),
            "case {case:?}: describe reported the wrong Arn: {body}"
        );
        assert_eq!(
            reported["Name"],
            json!(namespace),
            "case {case:?}: describe reported the wrong Name: {body}"
        );
        assert_eq!(
            reported["CapacityRegion"],
            json!(REGION),
            "case {case:?}: describe reported the wrong CapacityRegion: {body}"
        );
        assert_eq!(
            reported["IdentityStore"],
            json!("QUICKSIGHT"),
            "case {case:?}: describe reported the wrong IdentityStore: {body}"
        );
        assert_eq!(
            reported["CreationStatus"],
            json!("CREATING"),
            "case {case:?}: before the transition is due the namespace is still being \
             created: {body}"
        );
        assert_eq!(
            reported["NamespaceError"],
            Value::Null,
            "case {case:?}: a namespace whose creation did not fail carries no error: \
             {body}"
        );

        // What the create submitted beside the namespace itself is stored too, so the
        // reads that report it have something to report.
        {
            let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
            let store = state.lock();
            let record = store
                .namespace(row.account, namespace)
                .unwrap_or_else(|| panic!("case {case:?}: the created namespace is not stored"));
            assert_eq!(
                record.tags.len(),
                row.tags
                    .as_ref()
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
                "case {case:?}: the submitted Tags are not stored as given"
            );
            // The transition the clock owes is materialised by time, not by a sleep,
            // and its span is never zero.
            let created_at = record.creation_settles_at - qs_state::NAMESPACE_CREATION_SPAN;
            assert_eq!(
                record.status_at(created_at),
                "CREATING",
                "case {case:?}: at the creating instant the namespace is not yet created"
            );
            assert_eq!(
                record.status_at(record.creation_settles_at),
                "CREATED",
                "case {case:?}: once due, the namespace reports CREATED"
            );
            assert!(
                record.creation_settles_at > created_at,
                "case {case:?}: the transition span must not be zero"
            );
        }

        // Once the clock owes the transition, every read reports the terminal status.
        settle_creation(&harness, row.account, namespace);
        let (status, body) = describe_namespace(&harness, row.account, namespace).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe after the span expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Namespace"]["CreationStatus"],
            json!("CREATED"),
            "case {case:?}: a settled namespace is ready to use: {body}"
        );

        // A second create of the same name in the same account is a conflict, whatever
        // it submits.
        let (status, body) = create_namespace(&harness, row.account, row.body()).await;
        assert_eq!(
            status, 409,
            "case {case:?}: re-creating the same namespace expected 409, got {status} \
             with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!("The namespace {namespace} already exists.")),
            "case {case:?}: the conflict must name the namespace: {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ConflictException"),
            "case {case:?}: a name already in use is a conflict: {body}"
        );
    }
}

#[tokio::test]
async fn namespace_identity_is_the_account_and_the_name() {
    let harness = Harness::new();
    let shared = "shared-namespace";

    for (case, account) in [("first-account", ACCOUNT), ("second-account", OTHER_ACCOUNT)] {
        let (status, body) = create_namespace(
            &harness,
            account,
            json!({ "Namespace": shared, "IdentityStore": "QUICKSIGHT" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: one namespace name per account, so this create expected \
             200, got {status} with {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(namespace_arn(account, shared)),
            "case {case:?}: the ARN must carry the owning account: {body}"
        );
    }

    for (case, account) in [("first-account", ACCOUNT), ("second-account", OTHER_ACCOUNT)] {
        let (status, body) = describe_namespace(&harness, account, shared).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Namespace"]["Arn"],
            json!(namespace_arn(account, shared)),
            "case {case:?}: describe must report that account's own namespace: {body}"
        );
    }

    // An account that created nothing holds no namespace under that name.
    let (status, body) = describe_namespace(&harness, EMPTY_ACCOUNT, shared).await;
    assert_eq!(
        status, 404,
        "an account that created no namespace has none to describe, got {status} with {body}"
    );
    assert_eq!(
        body["Message"],
        json!(format!(
            "Namespace {shared} not found in account {EMPTY_ACCOUNT}."
        )),
        "the refusal must name the namespace that was not found: {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceNotFoundException"),
        "a name no namespace is held under is not found: {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("NAMESPACE"),
        "the refusal is about a namespace: {body}"
    );
}

#[tokio::test]
async fn every_account_already_has_the_default_namespace() {
    let harness = Harness::new();

    // The namespace the model's `Namespace` members tell a caller with no custom
    // namespace to address exists in every account, ready to use, and no create mints
    // it. Source: smithy_query(query="doc", shape="DescribeUserRequest",
    // member="Namespace") — "Currently, you should set this to default".
    for (case, account) in [
        ("untouched-account", ACCOUNT),
        ("another-untouched-account", OTHER_ACCOUNT),
        ("third-untouched-account", EMPTY_ACCOUNT),
    ] {
        let (status, body) = describe_namespace(&harness, account, "default").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the default namespace is describable, got {status} with {body}"
        );
        let reported = &body["Namespace"];
        assert_eq!(
            reported["Name"],
            json!("default"),
            "case {case:?}: describe reported the wrong Name: {body}"
        );
        assert_eq!(
            reported["Arn"],
            json!(namespace_arn(account, "default")),
            "case {case:?}: the default namespace's ARN carries its account: {body}"
        );
        assert_eq!(
            reported["CreationStatus"],
            json!("CREATED"),
            "case {case:?}: the default namespace is ready to use: {body}"
        );
        assert_eq!(
            reported["IdentityStore"],
            json!("QUICKSIGHT"),
            "case {case:?}: the default namespace uses the only modelled store: {body}"
        );

        // So creating it is a conflict rather than a second namespace of that name.
        let (status, body) = create_namespace(
            &harness,
            account,
            json!({ "Namespace": "default", "IdentityStore": "QUICKSIGHT" }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: creating a namespace that exists expected 409, got {status} \
             with {body}"
        );
        assert_eq!(
            body["Message"],
            json!("The namespace default already exists."),
            "case {case:?}: the conflict must name the namespace: {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ConflictException"),
            "case {case:?}: a name already in use is a conflict: {body}"
        );
    }
}
