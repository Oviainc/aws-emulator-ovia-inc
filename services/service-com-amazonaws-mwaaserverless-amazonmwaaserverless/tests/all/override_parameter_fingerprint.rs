// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `StartWorkflowRun`'s `@idempotencyToken` promises about a
//! request carrying `OverrideParameters`: repeating the token with the same request
//! replays the run that token already started, and presenting it with a request whose
//! parameters differ ANYWHERE — at the top level or nested inside a document — is the
//! modelled `ConflictException`.
//!
//! The rule the rows below assert is that the comparison is decided by the parameters'
//! VALUES and by nothing else: not by the order the wire carried an object's members in,
//! and not by the order a deserialised container happens to iterate in. An
//! `OverrideParameters` value is an `aws_smithy_types::Document`, whose objects are
//! `HashMap`s, so a comparison that read a container's iteration order would answer
//! differently on two deserialisations of the SAME bytes — which is why each row re-sends
//! its request several times rather than once.
//!
//! The rows differ along what the rule generalises over: how deeply a document nests, how
//! many members an object holds, the type of each value (object, array, string, the three
//! number kinds, boolean, null) — including a float against an integer of the same value
//! and zero against negative zero, which only the value's kind and spelling tell apart —
//! the order of an array's elements, and whether a member is present at all.

use crate::workflow_lifecycle::{role_arn, Harness};

/// How many times a row re-sends its identical request. A document object's iteration
/// order is its own instance's, so one repeat could agree by chance; several cannot.
const REPEATS: usize = 4;

/// One row of the table: the `OverrideParameters` a request carries, and parameters that
/// differ from them in a value the request supplies.
struct Row {
    case: &'static str,
    parameters: serde_json::Value,
    differing: serde_json::Value,
}

/// An object with `members` members, so a row can span map sizes rather than only the
/// two-member case: the more members an object holds, the less often two deserialisations
/// of it iterate alike.
fn wide_object(members: usize, changed: Option<usize>) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    for index in 0..members {
        let value = if changed == Some(index) {
            format!("value-{index}-changed")
        } else {
            format!("value-{index}")
        };
        object.insert(format!("key-{index}"), serde_json::json!(value));
    }
    serde_json::Value::Object(object)
}

/// The parameter sets the rule covers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a nested object with two members",
            parameters: serde_json::json!({"config": {"region": "us-east-1", "zone": "a"}}),
            differing: serde_json::json!({"config": {"region": "us-east-1", "zone": "b"}}),
        },
        Row {
            case: "a nested object with eight members",
            parameters: serde_json::json!({"config": wide_object(8, None)}),
            differing: serde_json::json!({"config": wide_object(8, Some(5))}),
        },
        Row {
            case: "objects nested four deep",
            parameters: serde_json::json!({"a": {"b": {"c": {"d": 1, "e": 2}}}}),
            differing: serde_json::json!({"a": {"b": {"c": {"d": 1, "e": 3}}}}),
        },
        Row {
            case: "two top-level members, each an object",
            parameters: serde_json::json!({
                "left": {"alpha": 1, "beta": 2},
                "right": {"alpha": 1, "beta": 2},
            }),
            differing: serde_json::json!({
                "left": {"alpha": 1, "beta": 2},
                "right": {"alpha": 1, "beta": 3},
            }),
        },
        Row {
            case: "an array of objects, differing in element order",
            parameters: serde_json::json!({
                "tasks": [{"name": "x", "retries": 1}, {"name": "y", "retries": 2}],
            }),
            differing: serde_json::json!({
                "tasks": [{"name": "y", "retries": 2}, {"name": "x", "retries": 1}],
            }),
        },
        Row {
            case: "an array of scalars, differing in element order",
            parameters: serde_json::json!({"xs": [1, 2, 3]}),
            differing: serde_json::json!({"xs": [1, 3, 2]}),
        },
        Row {
            case: "every document value kind in one object",
            parameters: serde_json::json!({
                "scalars": {
                    "flag": true,
                    "count": 3,
                    "below": -7,
                    "ratio": 1.5,
                    "name": "n",
                    "nothing": null,
                    "list": ["a", "b"],
                },
            }),
            differing: serde_json::json!({
                "scalars": {
                    "flag": false,
                    "count": 3,
                    "below": -7,
                    "ratio": 1.5,
                    "name": "n",
                    "nothing": null,
                    "list": ["a", "b"],
                },
            }),
        },
        Row {
            case: "a number against the same text as a string",
            parameters: serde_json::json!({"n": {"value": 3}}),
            differing: serde_json::json!({"n": {"value": "3"}}),
        },
        Row {
            case: "a float against an integer of the same value",
            parameters: serde_json::json!({"n": {"value": 1.0}}),
            differing: serde_json::json!({"n": {"value": 1}}),
        },
        Row {
            case: "zero against negative zero",
            parameters: serde_json::json!({"n": {"value": 0.0}}),
            differing: serde_json::json!({"n": {"value": -0.0}}),
        },
        Row {
            case: "an empty nested object against one holding a member",
            parameters: serde_json::json!({"config": {}}),
            differing: serde_json::json!({"config": {"region": "us-east-1"}}),
        },
        Row {
            case: "a member whose value is null against the member being absent",
            parameters: serde_json::json!({"config": {"region": null, "zone": "a"}}),
            differing: serde_json::json!({"config": {"zone": "a"}}),
        },
        Row {
            case: "a one-member object against a one-element array",
            parameters: serde_json::json!({"config": {"0": "x"}}),
            differing: serde_json::json!({"config": ["x"]}),
        },
        Row {
            case: "a flat string value, the case a Document::String already covered",
            parameters: serde_json::json!({"token": "flat"}),
            differing: serde_json::json!({"token": "other"}),
        },
    ]
}

/// Create the workflow a row's run is started under, without a `ClientToken` of its own.
async fn workflow_for(harness: &Harness, name: &str) -> String {
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/overrides.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string()
}

#[tokio::test]
async fn an_identical_retry_replays_whatever_the_override_parameters_hold() {
    let token = "retry-after-a-timeout";

    for row in rows() {
        // A fresh emulator per row, so the stored run count below is this row's alone.
        let harness = Harness::new("us-west-2");
        let arn = workflow_for(&harness, "overridden").await;
        let request = serde_json::json!({
            "WorkflowArn": arn,
            "ClientToken": token,
            "OverrideParameters": row.parameters,
        });

        let (status, started) = harness.call("StartWorkflowRun", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the first StartWorkflowRun to succeed, got {status} {started:?}",
            row.case
        );
        let run_id = started["RunId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a started run has a RunId", row.case))
            .to_string();

        // The retry a client makes after a timeout, several times over: each one replays
        // the run the token already started rather than starting another or being
        // refused as a token reused for a different request.
        for attempt in 1..=REPEATS {
            let (status, replay) = harness.call("StartWorkflowRun", request.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: an identical retry must replay the run the token \
                 started, got {status} {replay:?}",
                row.case
            );
            assert_eq!(
                replay["RunId"].as_str(),
                Some(run_id.as_str()),
                "case {:?} attempt {attempt}: expected the run {run_id:?}, got {:?}",
                row.case,
                replay["RunId"]
            );
        }

        // The parameters the first call persisted, read back through the run reader: the
        // replay reported a run that really holds what the request supplied.
        let (status, read) = harness
            .call(
                "GetWorkflowRun",
                serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetWorkflowRun to succeed, got {status} {read:?}",
            row.case
        );
        let reported = &read["OverrideParameters"];
        assert_eq!(
            reported, &row.parameters,
            "case {:?}: expected the run to report the parameters it was started with ({:?}), \
             got {reported:?}",
            row.case, row.parameters
        );

        // The SAME token with parameters that differ somewhere: the modelled conflict,
        // worded the way every reused-token refusal of this service is worded.
        let mut changed = request.clone();
        changed["OverrideParameters"] = row.differing.clone();
        let (status, conflict) = harness.call("StartWorkflowRun", changed).await;
        assert_eq!(
            status, 409,
            "case {:?}: expected ConflictException (409) for the token reused with different \
             parameters, got {status} {conflict:?}",
            row.case
        );
        assert_eq!(
            conflict["Message"].as_str(),
            Some(
                format!(
                    "ClientToken {token} was already used for a StartWorkflowRun request with \
                     different parameters"
                )
                .as_str()
            ),
            "case {:?}: expected the conflict to name the token and the operation, got {:?}",
            row.case,
            conflict["Message"]
        );

        let stored = harness.stored_run_count(&arn);
        assert_eq!(
            stored, 1,
            "case {:?}: expected exactly one run per ClientToken, got {stored}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_order_an_objects_members_arrive_in_is_no_part_of_the_request() {
    // The same document, written with its members in three different orders. (Whether the
    // orders survive into the request bytes depends on whether anything in the build
    // enabled `serde_json`'s `preserve_order` feature; where it does not, every spelling
    // serialises in key order, and the replay below is still exactly what must happen.)
    let spellings = [
        r#"{"config": {"region": "us-east-1", "zone": "a", "tier": "gold"},
            "limits": {"cpu": 2, "memory": 512}}"#,
        r#"{"limits": {"memory": 512, "cpu": 2},
            "config": {"tier": "gold", "zone": "a", "region": "us-east-1"}}"#,
        r#"{"config": {"zone": "a", "region": "us-east-1", "tier": "gold"},
            "limits": {"cpu": 2, "memory": 512}}"#,
    ];
    let token = "one-document-three-spellings";

    let harness = Harness::new("eu-west-1");
    let arn = workflow_for(&harness, "reordered").await;
    let mut started: Option<String> = None;

    for spelling in spellings {
        let parameters: serde_json::Value =
            serde_json::from_str(spelling).expect("the spelling is a JSON document");
        let (status, answer) = harness
            .call(
                "StartWorkflowRun",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "ClientToken": token,
                    "OverrideParameters": parameters,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: expected StartWorkflowRun to succeed, got {status} {answer:?}"
        );
        let run_id = answer["RunId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {spelling:?}: a started run has a RunId"))
            .to_string();
        match &started {
            None => started = Some(run_id),
            Some(first) => assert_eq!(
                &run_id, first,
                "case {spelling:?}: one document however its members are written is one request, \
                 so the token must replay {first:?}, got {run_id:?}"
            ),
        }
    }

    let stored = harness.stored_run_count(&arn);
    assert_eq!(
        stored, 1,
        "expected the three spellings of one document to start exactly one run, got {stored}"
    );
}
