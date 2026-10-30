// SPDX-License-Identifier: CC0-1.0

//! How the scheduling-policy plane WORDS the refusal of an identifier that cannot name a
//! scheduling policy this endpoint serves, end to end over the wire.
//!
//! The rules under test, each asserted over a SET of identifiers rather than one request:
//!
//! * an identifier whose SHAPE names no scheduling policy of this endpoint is refused with
//!   the modelled `ClientException` (HTTP 400) whose message is the pattern rule followed by
//!   the identifier the request submitted, verbatim — the sentence alone, carrying no
//!   frontend envelope and no request id inside the body;
//! * the response still names the call it answers, in the header AWS reports a request id
//!   in, so a bare refusal is correlatable without a second id in its message;
//! * where the identifier arrives as one entry of a LIST (`DescribeSchedulingPolicies$arns`),
//!   the refusal names the offending ENTRY — the first one that breaks the rule — and not
//!   whichever entry came first;
//! * a refused request writes nothing: the policies beside it stay exactly as they were.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{arn, error_code, error_message, Batch, REGION};

/// The header AWS reports a response's own request id in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The segment Batch's frontend envelope names a refused call's request id in: what a BARE
/// refusal must not carry anywhere in its message.
const ENVELOPE_REQUEST_ID: &str = "RequestId";

/// The prefix of Batch's frontend envelope, which this refusal class does not carry.
const ENVELOPE_PREFIX: &str = "Error executing request, Exception : ";

/// One operation that takes a scheduling-policy identifier: its URI and the request body
/// that carries `identifier` to it.
struct Taker {
    label: &'static str,
    uri: &'static str,
    body: fn(&str) -> Value,
}

/// Every operation that takes a scheduling-policy identifier, so the rule is shown to be
/// the identifier's and not one handler's: the reader takes a LIST of ARNs, the two writers
/// take a single one.
fn takers() -> Vec<Taker> {
    vec![
        Taker {
            label: "DescribeSchedulingPolicies",
            uri: "/v1/describeschedulingpolicies",
            body: |identifier| json!({"arns": [identifier]}),
        },
        Taker {
            label: "DeleteSchedulingPolicy",
            uri: "/v1/deleteschedulingpolicy",
            body: |identifier| json!({"arn": identifier}),
        },
        Taker {
            label: "UpdateSchedulingPolicy",
            uri: "/v1/updateschedulingpolicy",
            body: |identifier| json!({"arn": identifier}),
        },
    ]
}

/// Identifiers of a shape that names no scheduling policy of this endpoint, whichever
/// operation takes them.
///
/// The rows differ along what the rule generalises over: the KIND of malformation (not an
/// ARN at all, an ARN of another Batch resource kind, an ARN with no resource at all, an
/// empty or nested policy name) and the ARN FIELD that is wrong (partition, region,
/// account), plus the empty identifier and a name one past the length `name` documents.
fn malformed_identifiers() -> Vec<(String, String)> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    vec![
        ("an empty identifier".to_string(), String::new()),
        ("a name carrying a space".to_string(), "sp shape".to_string()),
        ("a name carrying a dot".to_string(), "sp.shape".to_string()),
        (
            "a name one past the documented length".to_string(),
            "x".repeat(state::MAX_SCHEDULING_POLICY_NAME_LEN + 1),
        ),
        (
            "an ARN with nothing after the account".to_string(),
            format!("arn:aws:batch:{REGION}:{account}"),
        ),
        ("a job-queue ARN".to_string(), arn("job-queue/jq-sp-shape")),
        (
            "a compute-environment ARN".to_string(),
            arn("compute-environment/ce-sp-shape"),
        ),
        (
            "an ARN with an empty policy name".to_string(),
            arn("scheduling-policy/"),
        ),
        (
            "an ARN carrying a nested path".to_string(),
            arn("scheduling-policy/sp-shape/extra"),
        ),
        (
            "an ARN of another partition".to_string(),
            format!("arn:aws-cn:batch:{REGION}:{account}:scheduling-policy/sp-shape"),
        ),
        (
            "an ARN of another region".to_string(),
            format!("arn:aws:batch:eu-west-3:{account}:scheduling-policy/sp-shape"),
        ),
        (
            "an ARN of another account".to_string(),
            format!("arn:aws:batch:{REGION}:111122223333:scheduling-policy/sp-shape"),
        ),
    ]
}

/// The refusal this class answers with: the pattern rule, then the identifier the request
/// submitted.
///
/// The rule sentence is spelled out here rather than read from the crate's constant, so a
/// reworded message is a failing test and not a silently updated expectation.
fn expected_message(identifier: &str) -> String {
    format!(
        "Scheduling policy arn should match a valid Scheduling policy arn pattern: {identifier}"
    )
}

/// Assert the answer to `uri` with `body` is the bare, identifier-naming refusal of
/// `identifier`, and that the response reports a request id in the header instead of in the
/// message.
async fn assert_refused(batch: &Batch, case: &str, uri: &str, body: Value, identifier: &str) {
    let (status, headers, refused) = batch.send_reported("POST", uri, Some(body)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the modelled ClientException 400, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "case {case:?}: expected the modelled ClientException, got {refused}"
    );
    let message = error_message(&refused);
    assert_eq!(
        message,
        expected_message(identifier),
        "case {case:?}: expected the pattern sentence naming the submitted identifier, got \
         {message:?}"
    );
    assert!(
        !message.starts_with(ENVELOPE_PREFIX),
        "case {case:?}: this refusal class carries no frontend envelope, got {message:?}"
    );
    assert!(
        !message.contains(ENVELOPE_REQUEST_ID),
        "case {case:?}: a request id belongs in the header and not in the message, got \
         {message:?}"
    );
    let reported = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        !reported.is_empty(),
        "case {case:?}: expected the response to report its request id in \
         {REQUEST_ID_HEADER}, got {reported:?}"
    );
}

/// RULE: an identifier whose shape names no scheduling policy of this endpoint is refused
/// 400 / `ClientException` with the pattern sentence followed by the identifier the request
/// submitted — bare, with no frontend envelope and no request id in the message.
#[tokio::test]
async fn a_malformed_scheduling_policy_identifier_is_refused_naming_it() {
    let batch = Batch::new();

    for (label, identifier) in malformed_identifiers() {
        for taker in takers() {
            let case = format!("{label} on {}", taker.label);
            assert_refused(
                &batch,
                &case,
                taker.uri,
                (taker.body)(&identifier),
                &identifier,
            )
            .await;
        }
    }
}

/// RULE: `DescribeSchedulingPolicies$arns` is "a list of … Amazon Resource Name (ARN)
/// entries", so a BARE NAME — the form the writers resolve — is refused there too, and the
/// refusal names the name that was submitted.
#[tokio::test]
async fn a_bare_name_is_refused_by_the_reader_and_resolved_by_the_writers() {
    let batch = Batch::new();

    // Names of different shapes, each a name the `name` member documents: the refusal is
    // about the FORM the reader takes and not about any one name.
    let names = [
        "sp-reader-name",
        "SP_Reader_Name-2",
        "s",
        &"a".repeat(state::MAX_SCHEDULING_POLICY_NAME_LEN),
    ];

    for name in names {
        let created = batch
            .ok("/v1/createschedulingpolicy", json!({"name": name}))
            .await;
        let policy_arn = created["arn"].as_str().unwrap_or_default().to_string();

        // The reader takes the ARN form alone, so the policy's own NAME names nothing there.
        assert_refused(
            &batch,
            &format!("the bare name {name:?} on DescribeSchedulingPolicies"),
            "/v1/describeschedulingpolicies",
            json!({"arns": [name]}),
            name,
        )
        .await;

        // The same name IS an identifier for the writer that resolves it, which proves the
        // refusal above is the reader's form rule and not a rejection of the name itself.
        let (status, answered) = batch
            .call("/v1/updateschedulingpolicy", json!({"arn": name}))
            .await;
        assert_eq!(
            status, 200,
            "case the bare name {name:?} on UpdateSchedulingPolicy: expected the name to \
             resolve, got {status} {answered}"
        );

        // And the policy is still readable by its ARN after the refusal.
        let body = batch
            .ok(
                "/v1/describeschedulingpolicies",
                json!({"arns": [&policy_arn]}),
            )
            .await;
        assert_eq!(
            body["schedulingPolicies"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case the bare name {name:?}: a refused read must leave the policy describable, \
             got {body}"
        );
    }
}

/// RULE: the refusal names the ENTRY of `arns` that broke the rule — the first one that
/// does — whatever its position beside entries that are valid, and the refused call writes
/// nothing.
#[tokio::test]
async fn the_refusal_names_the_offending_entry_of_the_list() {
    let batch = Batch::new();

    let created = batch
        .ok("/v1/createschedulingpolicy", json!({"name": "sp-entry"}))
        .await;
    let policy_arn = created["arn"].as_str().unwrap_or_default().to_string();
    let absent = arn("scheduling-policy/sp-entry-absent");
    let first_bad = "sp entry one".to_string();
    let second_bad = arn("job-queue/jq-entry");

    // Each row is the list as submitted and the entry the refusal must name: the bad entry
    // alone, after a valid one, after one that is valid but names nothing, and two bad
    // entries, where the FIRST of them is the one named.
    let rows: Vec<(&str, Vec<String>, String)> = vec![
        (
            "a bad entry alone",
            vec![first_bad.clone()],
            first_bad.clone(),
        ),
        (
            "a bad entry after a stored one",
            vec![policy_arn.clone(), first_bad.clone()],
            first_bad.clone(),
        ),
        (
            "a bad entry after an absent one",
            vec![absent.clone(), second_bad.clone()],
            second_bad.clone(),
        ),
        (
            "two bad entries",
            vec![second_bad.clone(), first_bad.clone()],
            second_bad.clone(),
        ),
        (
            "a bad entry between stored ones",
            vec![policy_arn.clone(), first_bad.clone(), policy_arn.clone()],
            first_bad.clone(),
        ),
    ];

    for (case, arns, offending) in rows {
        assert_refused(
            &batch,
            case,
            "/v1/describeschedulingpolicies",
            json!({"arns": arns}),
            &offending,
        )
        .await;
        // The refusal is effect-free: the stored policy is still there, under its ARN.
        let body = batch
            .ok(
                "/v1/describeschedulingpolicies",
                json!({"arns": [&policy_arn]}),
            )
            .await;
        assert_eq!(
            body["schedulingPolicies"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case {case:?}: a refused describe must change nothing, got {body}"
        );
    }
}
