// SPDX-License-Identifier: CC0-1.0

//! A create whose rollback the request SUPPRESSED parks in `CREATE_FAILED` and keeps what it
//! made, whatever made the create fail.
//!
//! RULE: "Specifies whether to disable rollback of the stack if the stack operation fails"
//! (`CreateStackInput$DisableRollback`, which `OnFailure=DO_NOTHING` also spells): a create
//! that fails with rollback suppressed terminates in `CREATE_FAILED` and the resources it had
//! already created stay — in the services that own them and in the stack's own resource view —
//! so an operator can inspect the partial stack. That holds for every way a create fails: a
//! resource the owning service refused, and a resource-signal wait the clock ran out on
//! ("CloudFormation doesn't proceed with a stack creation or update until resources receive
//! the required number of signals or the timeout period is exceeded").
//!
//! The rows span what the rule generalises over: how the create was made to fail (an owning
//! service's refusal, a wait condition nothing signals, and a client's own FAILURE signal —
//! "a failure signal causes CloudFormation to immediately fail the stack creation or
//! update"), and how the request suppressed the rollback (`DisableRollback=true`,
//! `OnFailure=DO_NOTHING`).

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

use crate::stack_lifecycle::{create_stack, member, router, wait_for_the_transition};
use crate::update_rollback_recovery::{
    parameter_value, resource_status_of, status_of,
};
use crate::wait_conditions::signal;

/// How long a row waits past a declaration's own span before reading the outcome: the expiry
/// is due on the clock, and the status the failure owes falls due with it.
const PAST_THE_WAIT: Duration = Duration::from_millis(1_900);

/// One row: what made the create fail, and how the request asked for the failed state to be
/// kept.
struct Kept {
    case: &'static str,
    stack: &'static str,
    /// The template: it always declares a parameter the create DOES make, so the row can
    /// prove the kept state really holds it.
    template: String,
    /// The request members that suppress the rollback.
    suppressed_by: Vec<(String, String)>,
    /// The name of the parameter the create makes, which has to survive.
    kept: &'static str,
    /// The logical id whose creation fails.
    failing: &'static str,
    /// Whether the row has to wait for a wait condition's own span before reading.
    waits: bool,
    /// The logical id the row signals FAILURE on, for the row whose create a client's own
    /// signal fails.
    signals_failure_on: Option<&'static str>,
}

/// The class the rule covers: a create that fails with its rollback suppressed.
fn kept_creates() -> Vec<Kept> {
    vec![
        Kept {
            case: "a wait condition nothing signals, DisableRollback=true",
            stack: "kept-wait-disabled",
            kept: "/kept-wait-disabled/made",
            failing: "Wait",
            waits: true,
            signals_failure_on: None,
            suppressed_by: vec![member("DisableRollback", "true")],
            template: json!({
                "Resources": {
                    "Made": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/kept-wait-disabled/made",
                            "Type": "String",
                            "Value": "made-by-the-create",
                        },
                    },
                    "Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
                    "Wait": {
                        "Type": "AWS::CloudFormation::WaitCondition",
                        "DependsOn": "Made",
                        "Properties": {
                            "Handle": {"Ref": "Handle"},
                            "Timeout": "1",
                            "Count": "1",
                        },
                    },
                },
            })
            .to_string(),
        },
        Kept {
            case: "a wait condition nothing signals, OnFailure=DO_NOTHING",
            stack: "kept-wait-do-nothing",
            kept: "/kept-wait-do-nothing/made",
            failing: "Wait",
            waits: true,
            signals_failure_on: None,
            suppressed_by: vec![member("OnFailure", "DO_NOTHING")],
            template: json!({
                "Resources": {
                    "Made": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/kept-wait-do-nothing/made",
                            "Type": "String",
                            "Value": "made-by-the-create",
                        },
                    },
                    "Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
                    "Wait": {
                        "Type": "AWS::CloudFormation::WaitCondition",
                        "DependsOn": "Made",
                        "Properties": {
                            "Handle": {"Ref": "Handle"},
                            "Timeout": "1",
                            "Count": "1",
                        },
                    },
                },
            })
            .to_string(),
        },
        Kept {
            case: "a parameter the owning service refuses, DisableRollback=true",
            stack: "kept-refused-disabled",
            kept: "/kept-refused-disabled/made",
            failing: "Clash",
            waits: false,
            signals_failure_on: None,
            suppressed_by: vec![member("DisableRollback", "true")],
            template: json!({
                "Resources": {
                    "Made": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/kept-refused-disabled/made",
                            "Type": "String",
                            "Value": "made-by-the-create",
                        },
                    },
                    "Clash": {
                        "Type": "AWS::SSM::Parameter",
                        "DependsOn": "Made",
                        "Properties": {
                            "Name": "/kept-refused-disabled/made",
                            "Type": "String",
                            "Value": "the-same-name-twice",
                        },
                    },
                },
            })
            .to_string(),
        },
        Kept {
            // "A failure signal causes CloudFormation to immediately fail the stack creation
            // or update": the wait's own span is long, so what fails this create is the
            // client's signal and nothing else.
            case: "a client's FAILURE signal on the gate, DisableRollback=true",
            stack: "kept-signalled-failure",
            kept: "/kept-signalled-failure/made",
            failing: "Wait",
            waits: false,
            signals_failure_on: Some("Wait"),
            suppressed_by: vec![member("DisableRollback", "true")],
            template: json!({
                "Resources": {
                    "Made": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/kept-signalled-failure/made",
                            "Type": "String",
                            "Value": "made-by-the-create",
                        },
                    },
                    "Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
                    "Wait": {
                        "Type": "AWS::CloudFormation::WaitCondition",
                        "DependsOn": "Made",
                        "Properties": {
                            "Handle": {"Ref": "Handle"},
                            "Timeout": "600",
                            "Count": "1",
                        },
                    },
                },
            })
            .to_string(),
        },
    ]
}

#[tokio::test]
async fn a_failed_create_whose_rollback_is_suppressed_parks_and_keeps_what_it_made() {
    for row in kept_creates() {
        let case = row.case;
        let router = router();
        let (http_status, body) =
            create_stack(&router, row.stack, &row.template, &row.suppressed_by).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to be accepted, got {http_status} {body}"
        );
        if row.waits {
            tokio::time::sleep(PAST_THE_WAIT).await;
        }
        if let Some(gate) = row.signals_failure_on {
            let (http_status, body) = signal(&router, row.stack, gate, "the-one-host", "FAILURE").await;
            assert_eq!(
                http_status, 200,
                "case {case:?}: expected the FAILURE signal to be accepted, got \
                 {http_status} {body}"
            );
        }
        wait_for_the_transition().await;

        let settled = status_of(&router, row.stack).await;
        assert_eq!(
            settled,
            status::CREATE_FAILED,
            "case {case:?}: expected the suppressed rollback to park the create in \
             CREATE_FAILED, got {settled:?}"
        );

        // The resource the create DID make is still reported by the stack...
        let kept = resource_status_of(&router, row.stack, "Made").await;
        assert_eq!(
            kept,
            resource_status::CREATE_COMPLETE,
            "case {case:?}: expected the resource the create made to stay CREATE_COMPLETE in \
             the stack's resource view, got {kept:?}"
        );
        // ...and the resource that stopped the create reports its own failure.
        let failed = resource_status_of(&router, row.stack, row.failing).await;
        assert_eq!(
            failed,
            resource_status::CREATE_FAILED,
            "case {case:?}: expected {:?} to report CREATE_FAILED, got {failed:?}",
            row.failing
        );
        // ...and it is still there in the service that owns it: nothing was rolled back.
        let held = parameter_value(&router, row.kept).await;
        assert_eq!(
            held,
            Some("made-by-the-create".to_string()),
            "case {case:?}: expected the parameter {:?} the create made to survive, got \
             {held:?}",
            row.kept
        );
    }
}
