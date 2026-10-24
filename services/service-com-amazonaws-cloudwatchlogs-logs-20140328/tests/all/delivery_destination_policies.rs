// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutDeliveryDestinationPolicy` and `GetDeliveryDestinationPolicy`:
//! the IAM policy an account assigns to one of its delivery destinations.
//!
//! The rule under test: a delivery destination holds at most one delivery destination
//! policy, so assigning one stores the document the request carries VERBATIM on the
//! destination it names and REPLACES whatever that destination held, and both the assigning
//! call and every later read report those stored contents; a destination that has had none
//! assigned reports no policy at all; a `deliveryDestinationName` the account does not hold
//! is refused as a missing resource by both operations; and a policy that is not an IAM
//! policy document — a JSON object — is refused as an invalid parameter.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: policy documents
//! of several forms and sizes (the smallest JSON object, one statement, the two statements
//! the operation's documentation describes, nested conditions, non-ASCII contents,
//! pretty-printed whitespace, and a document on the published maximum length and one past
//! it), destinations of several `DeliveryDestinationType`s, and destination names at both
//! published length bounds.

use serde_json::{json, Value};

use crate::harness::Logs;

/// The operations under test, plus the destination operations a round trip goes through.
const PUT_POLICY: &str = "PutDeliveryDestinationPolicy";
const GET_POLICY: &str = "GetDeliveryDestinationPolicy";
const PUT_DESTINATION: &str = "PutDeliveryDestination";
const GET_DESTINATION: &str = "GetDeliveryDestination";

/// The refusals these operations model for the cases below.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// Destination resources of three vendors a delivery destination can represent, so the rule
/// is asserted on more than one kind of destination.
const BUCKET_ARN: &str = "arn:aws:s3:::vended-logs-bucket";
const LOG_GROUP_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/one:*";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs";

/// The `DeliveryDestinationType` of a trace destination, the one kind that is created with no
/// `deliveryDestinationConfiguration`. Source: the model's `DeliveryDestinationType` enum.
const TRACE: &str = "XRAY";

/// The published bounds of `DeliveryDestinationPolicy`. Source: the model's
/// `DeliveryDestinationPolicy` shape (`@length` 1..51200), which
/// `PutDeliveryDestinationPolicyRequest$deliveryDestinationPolicy` and `Policy$deliveryDestinationPolicy`
/// both target.
const POLICY_MAX_LENGTH: usize = 51_200;

/// A delivery destination to assign policies to: how it is created, and the name it is
/// created under.
#[derive(Debug)]
struct Destination {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// `name` (`DeliveryDestinationName`, 1..60, `^[\w-]*$`).
    name: String,
    /// The `destinationResourceArn` this destination represents, or `None` for the trace
    /// destination, which is created from its type alone.
    resource_arn: Option<&'static str>,
}

impl Destination {
    fn of(label: &'static str, name: &str, resource_arn: &'static str) -> Self {
        Self {
            label,
            name: name.to_string(),
            resource_arn: Some(resource_arn),
        }
    }

    /// The `PutDeliveryDestination` request that creates this destination.
    fn put_body(&self) -> Value {
        match self.resource_arn {
            Some(resource_arn) => json!({
                "name": self.name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource_arn},
            }),
            None => json!({"name": self.name, "deliveryDestinationType": TRACE}),
        }
    }
}

/// Destinations spanning the vendors and both published bounds of `DeliveryDestinationName`
/// (`@length` 1..60).
fn destinations() -> Vec<Destination> {
    vec![
        Destination::of("an S3 bucket destination", "vended-logs-bucket", BUCKET_ARN),
        Destination::of("a log group destination", "vended_logs_group", LOG_GROUP_ARN),
        Destination::of("a Firehose destination", "vended-logs-stream", FIREHOSE_ARN),
        Destination {
            label: "a trace destination, created from its type alone",
            name: "vended-traces".to_string(),
            resource_arn: None,
        },
        Destination::of("a name at the minimum length", "d", BUCKET_ARN),
        Destination::of(
            "a name at the maximum length",
            &"n".repeat(60),
            FIREHOSE_ARN,
        ),
    ]
}

/// One policy document, as a client sends it.
#[derive(Debug)]
struct PolicyCase {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// The `deliveryDestinationPolicy` string, sent and reported verbatim.
    document: String,
}

impl PolicyCase {
    fn of(label: &'static str, document: &str) -> Self {
        Self {
            label,
            document: document.to_string(),
        }
    }
}

/// The two statements the operation's documentation describes — "One statement enables
/// general logs delivery, and the other allows delivery to the chosen destination" — as one
/// IAM policy document.
fn two_statement_policy(destination_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AllowLogDeliveryActions",
                "Effect": "Allow",
                "Principal": {"Service": "delivery.logs.amazonaws.com"},
                "Action": ["logs:PutLogEvents", "logs:CreateLogStream"],
                "Resource": destination_arn,
            },
            {
                "Sid": "AllowLogDeliveryForResource",
                "Effect": "Allow",
                "Principal": {"Service": "delivery.logs.amazonaws.com"},
                "Action": "logs:PutDeliverySource",
                "Resource": "*",
            },
        ],
    })
    .to_string()
}

/// A JSON object of exactly `length` characters: the smallest object padded through one
/// `Sid` value, so a case can sit on a published length bound.
fn policy_of_length(length: usize) -> String {
    let skeleton = r#"{"Sid":""}"#;
    let padding = length - skeleton.len();
    format!(r#"{{"Sid":"{}"}}"#, "p".repeat(padding))
}

/// Policy documents spanning form, size, nesting, character set and whitespace.
fn policies() -> Vec<PolicyCase> {
    vec![
        PolicyCase::of("the smallest JSON object", "{}"),
        PolicyCase::of(
            "a one-statement policy",
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "delivery.logs.amazonaws.com"},
                    "Action": "logs:PutLogEvents",
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
        PolicyCase::of(
            "the two statements the operation documents",
            &two_statement_policy(BUCKET_ARN),
        ),
        PolicyCase::of(
            "a statement with a nested condition",
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "delivery.logs.amazonaws.com"},
                    "Action": ["logs:PutLogEvents"],
                    "Resource": "*",
                    "Condition": {
                        "StringEquals": {"aws:SourceAccount": ["123456789012", "210987654321"]},
                        "ArnLike": {"aws:SourceArn": "arn:aws:logs:eu-west-2:123456789012:*"},
                    },
                }],
            })
            .to_string(),
        ),
        PolicyCase::of(
            "a policy carrying non-ASCII contents",
            &json!({"Sid": "lieferung-\u{00fc}ber-gr\u{00f6}\u{00dfe}-\u{65e5}\u{672c}"}).to_string(),
        ),
        PolicyCase::of(
            "a pretty-printed policy, whitespace and all",
            "{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ ]\n}",
        ),
        PolicyCase::of(
            "a policy on the published maximum length",
            &policy_of_length(POLICY_MAX_LENGTH),
        ),
    ]
}

/// The `policy.deliveryDestinationPolicy` a response carries, or `None` when the response
/// carries no `policy` at all.
fn reported_policy(operation: &str, body: &Value, response: &Value) -> Option<String> {
    let policy = response.get("policy")?;
    Some(
        policy
            .get("deliveryDestinationPolicy")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "{operation} {body}: a reported policy carries deliveryDestinationPolicy, \
                     got {response}"
                )
            })
            .to_string(),
    )
}

/// The policy the service reports for `name`, through a wire `GetDeliveryDestinationPolicy`.
async fn get_policy(logs: &Logs, name: &str) -> Option<String> {
    let body = json!({"deliveryDestinationName": name});
    let response = logs.ok(GET_POLICY, &body).await;
    reported_policy(GET_POLICY, &body, &response)
}

/// Assign one policy, returning the contents the assigning call itself reports.
async fn put_policy(logs: &Logs, name: &str, document: &str) -> Option<String> {
    let body = json!({"deliveryDestinationName": name, "deliveryDestinationPolicy": document});
    let response = logs.ok(PUT_POLICY, &body).await;
    reported_policy(PUT_POLICY, &body, &response)
}

/// Every policy document, assigned to every destination, is reported VERBATIM by the call
/// that assigns it and read back by `GetDeliveryDestinationPolicy` — the round trip through
/// stored state.
#[tokio::test]
async fn an_assigned_policy_is_reported_and_read_back() {
    let logs = Logs::new();
    let destinations = destinations();
    let policies = policies();

    for destination in &destinations {
        logs.ok(PUT_DESTINATION, &destination.put_body()).await;
    }

    for destination in &destinations {
        for policy in &policies {
            let case = (destination.label, policy.label);
            let assigned = put_policy(&logs, &destination.name, &policy.document).await;
            assert_eq!(
                assigned.as_deref(),
                Some(policy.document.as_str()),
                "case {case:?}: {PUT_POLICY} reports the policy it just assigned, got {assigned:?}"
            );

            let read_back = get_policy(&logs, &destination.name).await;
            assert_eq!(
                read_back.as_deref(),
                Some(policy.document.as_str()),
                "case {case:?}: {GET_POLICY} reports the assigned policy, got {read_back:?}"
            );

            // Reading twice reports the same contents: a read changes nothing.
            let again = get_policy(&logs, &destination.name).await;
            assert_eq!(
                again, read_back,
                "case {case:?}: a second read reports the same policy, got {again:?}"
            );
        }
    }
}

/// A destination holds at most one policy, so each assignment REPLACES the one before it: the
/// contents assigned last are the contents every later reader sees, whichever order the
/// documents are assigned in.
#[tokio::test]
async fn the_policy_assigned_last_is_the_one_reported() {
    let logs = Logs::new();
    let destination = Destination::of("an S3 bucket destination", "overwritten", BUCKET_ARN);
    logs.ok(PUT_DESTINATION, &destination.put_body()).await;

    let mut documents: Vec<String> = policies().into_iter().map(|case| case.document).collect();
    // The same set assigned in the reverse order too, so what is reported follows the LAST
    // assignment rather than the first, the longest or the shortest.
    let mut reversed = documents.clone();
    reversed.reverse();
    documents.extend(reversed);

    for document in &documents {
        let assigned = put_policy(&logs, &destination.name, document).await;
        assert_eq!(
            assigned.as_deref(),
            Some(document.as_str()),
            "case {:?}: the assigning call reports the new policy, got {assigned:?}",
            document.len()
        );
        let read_back = get_policy(&logs, &destination.name).await;
        assert_eq!(
            read_back.as_deref(),
            Some(document.as_str()),
            "case {:?}: the last policy assigned is the one reported, got {read_back:?}",
            document.len()
        );
    }
}

/// A destination that exists but has had no policy assigned reports NO policy — the response
/// succeeds and carries no `policy` member — and assigning one to a sibling destination
/// leaves it reporting none.
#[tokio::test]
async fn a_destination_with_no_policy_assigned_reports_none() {
    let logs = Logs::new();
    let destinations = destinations();
    for destination in &destinations {
        logs.ok(PUT_DESTINATION, &destination.put_body()).await;
    }

    for destination in &destinations {
        let reported = get_policy(&logs, &destination.name).await;
        assert_eq!(
            reported, None,
            "case {:?}: a destination with no policy assigned reports none, got {reported:?}",
            destination.label
        );
    }

    // One destination is given a policy; every other one still reports none.
    let assigned_to = &destinations[0];
    put_policy(&logs, &assigned_to.name, "{}").await;
    for destination in destinations.iter().skip(1) {
        let reported = get_policy(&logs, &destination.name).await;
        assert_eq!(
            reported, None,
            "case {:?}: a policy on one destination is not reported for another, got {reported:?}",
            destination.label
        );
    }
}

/// A policy is assigned TO a delivery destination, so a `deliveryDestinationName` the account
/// does not hold is refused as a missing resource by BOTH operations — and the refusal stores
/// nothing, so the name still holds no policy once a destination is created under it.
#[tokio::test]
async fn a_name_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    let held = Destination::of("an S3 bucket destination", "held", BUCKET_ARN);
    logs.ok(PUT_DESTINATION, &held.put_body()).await;

    // Names that satisfy `DeliveryDestinationName` and that the account does not hold: one
    // never used, one differing from a held name only in case, one at each published length
    // bound.
    let absent = [
        ("a name never used", "absent".to_string()),
        ("a held name in another case", "HELD".to_string()),
        ("a one-character name", "x".to_string()),
        ("a name at the maximum length", "z".repeat(60)),
    ];

    for (case, name) in &absent {
        let code = logs
            .err(
                PUT_POLICY,
                &json!({"deliveryDestinationName": name, "deliveryDestinationPolicy": "{}"}),
            )
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: assigning a policy to a destination the account does not hold is \
             refused with {NOT_FOUND}, got {code}"
        );

        let code = logs
            .err(GET_POLICY, &json!({"deliveryDestinationName": name}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: reading the policy of a destination the account does not hold is \
             refused with {NOT_FOUND}, got {code}"
        );
    }

    // The refused assignments wrote nothing: a destination created under one of those names
    // afterwards holds no policy.
    let created = Destination::of("a destination created later", "absent", BUCKET_ARN);
    logs.ok(PUT_DESTINATION, &created.put_body()).await;
    let reported = get_policy(&logs, &created.name).await;
    assert_eq!(
        reported, None,
        "case \"a refused assignment persists nothing\": expected no policy, got {reported:?}"
    );
}

/// "Creates and assigns an IAM policy": an IAM policy is a JSON object, so a
/// `deliveryDestinationPolicy` that is not one is refused as an invalid parameter — and the
/// refusal leaves the policy the destination already held in place.
#[tokio::test]
async fn a_policy_that_is_not_an_iam_policy_document_is_refused() {
    let logs = Logs::new();
    let destination = Destination::of("an S3 bucket destination", "guarded", BUCKET_ARN);
    logs.ok(PUT_DESTINATION, &destination.put_body()).await;
    let kept = two_statement_policy(BUCKET_ARN);
    put_policy(&logs, &destination.name, &kept).await;

    let rejected = [
        ("plain text", "not a policy at all".to_string()),
        ("a one-character document", "p".to_string()),
        ("a JSON array of statements", r#"[{"Effect":"Allow"}]"#.to_string()),
        ("a JSON string", r#""{\"Effect\":\"Allow\"}""#.to_string()),
        ("a JSON number", "17".to_string()),
        ("a JSON null", "null".to_string()),
        ("a truncated object", r#"{"Statement":"#.to_string()),
        ("an object with a trailing comma", r#"{"Sid":"a",}"#.to_string()),
        // One character past the published maximum length of `DeliveryDestinationPolicy`
        // (`@length` 1..51200), whose in-bound sibling is a passing row above.
        (
            "one character past the maximum length",
            policy_of_length(POLICY_MAX_LENGTH + 1),
        ),
        // One character short of the published minimum (`@length` min 1).
        ("an empty document", String::new()),
    ];

    for (case, document) in &rejected {
        let code = logs
            .err(
                PUT_POLICY,
                &json!({
                    "deliveryDestinationName": destination.name,
                    "deliveryDestinationPolicy": document,
                }),
            )
            .await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: a policy that is not an IAM policy document is refused with \
             {VALIDATION}, got {code}"
        );

        let reported = get_policy(&logs, &destination.name).await;
        assert_eq!(
            reported.as_deref(),
            Some(kept.as_str()),
            "case {case:?}: a refused assignment leaves the assigned policy in place, \
             got {reported:?}"
        );
    }
}

/// The policy is not one of the "delivery destination parameters" a later
/// `PutDeliveryDestination` overwrites, and assigning a policy is not a change to those
/// parameters either: each operation leaves the other's state alone.
#[tokio::test]
async fn a_policy_and_the_destination_parameters_do_not_overwrite_each_other() {
    let logs = Logs::new();
    let name = "paired";
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
            "outputFormat": "json",
        }),
    )
    .await;

    let document = two_statement_policy(BUCKET_ARN);
    put_policy(&logs, name, &document).await;

    // Assigning the policy left every destination parameter as the put set it.
    let destination = logs
        .ok(GET_DESTINATION, &json!({"name": name}))
        .await
        .get("deliveryDestination")
        .cloned()
        .expect("case \"after assigning a policy\": the destination is reported");
    assert_eq!(
        destination.get("outputFormat").and_then(Value::as_str),
        Some("json"),
        "case \"after assigning a policy\": expected outputFormat json, got {destination}"
    );
    assert_eq!(
        destination.get("deliveryDestinationType").and_then(Value::as_str),
        Some("S3"),
        "case \"after assigning a policy\": expected the S3 destination type, got {destination}"
    );

    // Updating the destination's parameters — a different resource, a different output
    // format — leaves the assigned policy where it is.
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": FIREHOSE_ARN},
            "outputFormat": "raw",
        }),
    )
    .await;
    let reported = get_policy(&logs, name).await;
    assert_eq!(
        reported.as_deref(),
        Some(document.as_str()),
        "case \"after updating the destination parameters\": the assigned policy stays, \
         got {reported:?}"
    );

    let updated = logs
        .ok(GET_DESTINATION, &json!({"name": name}))
        .await
        .get("deliveryDestination")
        .cloned()
        .expect("case \"after updating the destination\": the destination is reported");
    assert_eq!(
        updated.get("deliveryDestinationType").and_then(Value::as_str),
        Some("FH"),
        "case \"after updating the destination\": expected the Firehose destination type, \
         got {updated}"
    );
}
