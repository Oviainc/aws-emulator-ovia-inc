// SPDX-License-Identifier: CC0-1.0

//! A stack operation that DELETES a resource is not reported complete while the service that
//! owns that resource still holds it.
//!
//! The rule under test: **CloudFormation does not report a stack operation complete while a
//! service that owns one of the resources that operation deleted still holds that resource;
//! the operation stays `*_IN_PROGRESS` until that service reports the resource gone, exactly
//! as a create is held `*_IN_PROGRESS` until that service reports the resource ready.** It is
//! the same relation [`crate::cloudformation_firehose_stream_stabilisation`] pins on the
//! create side, read backwards, and several crates have to agree for it to hold
//! (CloudFormation makes the calls; the owning service decides when the resource is gone), so
//! the suite lives here rather than in any one of them.
//!
//! What this pins is the observation an infrastructure-as-code client makes: the instant a
//! stack operation first reports itself complete, every resource that operation deleted is
//! gone in the service that owned it — so the client's next call about that resource is
//! refused, and a name it freed can be used again.
//!
//! The rows differ along what the rule generalises over: WHICH service owns the deleted
//! resource — Amazon Data Firehose ("When an API request to DeleteDeliveryStream succeeds,
//! the Firehose stream is marked for deletion, and it goes into the DELETING state … Removal
//! of a Firehose stream that is in the DELETING state is a low priority operation for the
//! service") and Amazon Kinesis Data Streams ("After a DeleteStream request, the specified
//! stream is in the DELETING state until Kinesis Data Streams completes the deletion"), the
//! two whose delete this emulator answers with the resource still there — and WHICH operation
//! deletes it: an update that stops declaring the resource, an update that renames it (so the
//! old one is replaced), and the stack's own deletion. Every expected value is computed here
//! from what the row declares, never read back out of the answer being checked.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element reader this test crate writes ONCE
// (`tests/all/query_wire.rs`), and the crate's own SigV4 header builder, which signs for the
// Region this suite's hosts name.
use super::query_wire::{form_encode, opt_element};
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// The Region every request here is signed for, and so the Region the stacks' resources
/// belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// How often the suite asks the stack whether its operation has concluded, and how long it
/// goes on asking. The interval is short because the FIRST completed observation is what the
/// rule is stated about; the budget is long enough that a loaded machine is slower than the
/// spans these services choose without the rule being any different.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The `StackStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// The IAM role every Firehose row's stream delivers under. Firehose resolves a stream's
/// `RoleARN` against IAM before it writes the stream, so the role exists before any stack
/// here is created ([`given_delivery_role`]).
const DELIVERY_ROLE: &str = "removal-stabilisation-role";

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
///
/// IAM is here because Firehose resolves the delivery role before it writes a stream, and
/// Amazon S3 because CloudFormation provisions the bucket type a template may declare beside
/// a stream.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one `awsJson1_1` operation of the service `signing_name` routes to.
async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Create the IAM role every Firehose row's stream delivers under, trusting Firehose.
async fn given_delivery_role(router: &MultiServiceRouter, case: &str) {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "firehose.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string();
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        form_encode(DELIVERY_ROLE),
        form_encode(&trust),
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    assert_eq!(
        status,
        200,
        "case {case:?}: expected HTTP 200 from CreateRole, got {status} with body {}",
        String::from_utf8_lossy(response.body())
    );
}

/// Which service owns the resource a row's stack deletes.
#[derive(Debug, Clone, Copy)]
enum Owner {
    /// Amazon Data Firehose, whose `AWS::KinesisFirehose::DeliveryStream` goes to `DELETING`
    /// on an accepted delete and is removed later.
    Firehose,
    /// Amazon Kinesis Data Streams, whose `AWS::Kinesis::Stream` does the same.
    Kinesis,
}

/// Both owners, which every test below runs over.
const OWNERS: &[Owner] = &[Owner::Firehose, Owner::Kinesis];

impl Owner {
    /// How a stack name and a resource name of this row are told apart from the other row's.
    fn label(self) -> &'static str {
        match self {
            Self::Firehose => "firehose",
            Self::Kinesis => "kinesis",
        }
    }

    /// The `AWS::<service>::<resource>` type a template declares the resource under.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Firehose => "AWS::KinesisFirehose::DeliveryStream",
            Self::Kinesis => "AWS::Kinesis::Stream",
        }
    }

    /// The `Properties` a declaration of `name` carries: the name the resource is created
    /// under, and the least this owner's own create needs beside it.
    fn properties(self, name: &str) -> Value {
        match self {
            Self::Firehose => json!({
                "DeliveryStreamName": name,
                "ExtendedS3DestinationConfiguration": {
                    "BucketARN": format!("arn:aws:s3:::{name}-data"),
                    "RoleARN": format!("arn:aws:iam::{ACCOUNT_ID}:role/{DELIVERY_ROLE}"),
                }
            }),
            Self::Kinesis => json!({ "Name": name, "ShardCount": 1 }),
        }
    }

    /// What the owning service answers about `name` right now: `(whether it still holds the
    /// resource, the answer it said so with)`.
    ///
    /// A refusal is that service answering that it holds no such resource, which is what the
    /// rule is stated about; a 200 is the resource still being there, whatever state it
    /// reports.
    async fn read(self, router: &MultiServiceRouter, name: &str) -> (bool, Value) {
        let (status, body) = match self {
            Self::Firehose => {
                json_rpc(
                    router,
                    "firehose",
                    "Firehose_20150804.DescribeDeliveryStream",
                    &json!({ "DeliveryStreamName": name }),
                )
                .await
            }
            Self::Kinesis => {
                json_rpc(
                    router,
                    "kinesis",
                    "Kinesis_20131202.DescribeStreamSummary",
                    &json!({ "StreamName": name }),
                )
                .await
            }
        };
        (status == 200, body)
    }
}

/// A template declaring one resource per `(logical id, name)` pair, of `owner`'s type.
fn template(owner: Owner, declared: &[(&str, &str)]) -> String {
    let mut resources = serde_json::Map::new();
    for (logical_id, name) in declared {
        resources.insert(
            (*logical_id).to_string(),
            json!({
                "Type": owner.resource_type(),
                "Properties": owner.properties(name),
            }),
        );
    }
    json!({ "Resources": Value::Object(resources) }).to_string()
}

/// Create the stack and answer the id CloudFormation minted for it, which every read below
/// addresses it by — a stack whose deletion has completed is no longer addressable by name
/// ("Deleted stacks: you must specify the unique stack ID").
async fn create_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    body: &str,
) -> String {
    let (status, answer) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", body)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answer}"
    );
    opt_element(&answer, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a StackId, got {answer}"))
}

/// The `StackStatus` the stack reports right now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks for {stack:?}, got {status} \
         with body {body}"
    );
    (
        opt_element(&body, "StackStatus").unwrap_or_default(),
        body,
    )
}

/// Poll `stack` until it first reports `wanted`, and answer that observation.
///
/// Used for the stack's CREATE, which has nothing deleted to wait for: every row's resource
/// is created before the operation under test deletes it.
async fn wait_for(router: &MultiServiceRouter, case: &str, stack: &str, wanted: &str) {
    for _ in 0..POLLS {
        let (status, body) = stack_status(router, case, stack).await;
        if status == wanted {
            return;
        }
        assert!(
            status.ends_with("_IN_PROGRESS"),
            "case {case:?}: expected the stack to be working towards {wanted}, got {status} \
             in {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (status, body) = stack_status(router, case, stack).await;
    panic!("case {case:?}: expected the stack to reach {wanted}, got {status} in {body}");
}

/// THE RULE: poll `stack` until it first reports `wanted`, and assert that the service owning
/// `deleted` no longer holds that resource.
///
/// The owning service is read AFTER that first terminal observation, which is the sound
/// direction here: a resource that service has let go of stays gone, so one it still holds
/// when asked a moment later was still there when the operation concluded. Reading it first
/// would instead fail the row whose removal fell due in the gap between the two reads, which
/// is no violation of the rule at all.
async fn settles_only_once_gone(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    in_progress: &str,
    wanted: &str,
    owner: Owner,
    deleted: &str,
) {
    for _ in 0..POLLS {
        let (status, body) = stack_status(router, case, stack).await;
        if status == wanted {
            let (still_held, read) = owner.read(router, deleted).await;
            assert!(
                !still_held,
                "case {case:?}: the stack reported {wanted} while {} still held {deleted:?} \
                 ({read}); stack {body}",
                owner.label()
            );
            return;
        }
        assert_eq!(
            status, in_progress,
            "case {case:?}: expected the stack to be {in_progress} until {deleted:?} is gone \
             in {}, got {status} in {body}",
            owner.label()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (status, body) = stack_status(router, case, stack).await;
    panic!("case {case:?}: expected the stack to reach {wanted}, got {status} in {body}");
}

/// An update that STOPS DECLARING a resource: the stack is not `UPDATE_COMPLETE` before the
/// service that owned that resource has removed it — and the resource the update goes on
/// declaring is untouched.
#[tokio::test]
async fn an_update_that_drops_a_resource_completes_only_once_that_resource_is_gone() {
    let router = build_router();
    given_delivery_role(&router, "the delivery role every Firehose row needs").await;

    for owner in OWNERS {
        let owner = *owner;
        let case = format!("an update that stops declaring a resource owned by {}", owner.label());
        let case = case.as_str();
        let stack_name = format!("removal-drop-{}", owner.label());
        let kept = format!("removal-drop-kept-{}", owner.label());
        let dropped = format!("removal-drop-gone-{}", owner.label());

        let stack = create_stack(
            &router,
            case,
            &stack_name,
            &template(owner, &[("Kept", &kept), ("Dropped", &dropped)]),
        )
        .await;
        wait_for(&router, case, &stack, CREATE_COMPLETE).await;

        let (status, body) = cloudformation(
            &router,
            "UpdateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template(owner, &[("Kept", &kept)])),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
        );

        settles_only_once_gone(
            &router,
            case,
            &stack,
            UPDATE_IN_PROGRESS,
            UPDATE_COMPLETE,
            owner,
            &dropped,
        )
        .await;

        let (still_held, read) = owner.read(&router, &kept).await;
        assert!(
            still_held,
            "case {case:?}: expected {kept:?} to be untouched by the update, got {read}"
        );
    }
}

/// An update that RENAMES a resource, so the old one is replaced: the stack is not
/// `UPDATE_COMPLETE` before the service that owned the OLD resource has removed it, and the
/// new one is there.
#[tokio::test]
async fn an_update_that_replaces_a_resource_completes_only_once_the_old_one_is_gone() {
    let router = build_router();
    given_delivery_role(&router, "the delivery role every Firehose row needs").await;

    for owner in OWNERS {
        let owner = *owner;
        let case = format!("an update that renames a resource owned by {}", owner.label());
        let case = case.as_str();
        let stack_name = format!("removal-replace-{}", owner.label());
        let before = format!("removal-replace-before-{}", owner.label());
        let after = format!("removal-replace-after-{}", owner.label());

        let stack = create_stack(
            &router,
            case,
            &stack_name,
            &template(owner, &[("Stream", &before)]),
        )
        .await;
        wait_for(&router, case, &stack, CREATE_COMPLETE).await;

        let (status, body) = cloudformation(
            &router,
            "UpdateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template(owner, &[("Stream", &after)])),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
        );

        settles_only_once_gone(
            &router,
            case,
            &stack,
            UPDATE_IN_PROGRESS,
            UPDATE_COMPLETE,
            owner,
            &before,
        )
        .await;

        let (still_held, read) = owner.read(&router, &after).await;
        assert!(
            still_held,
            "case {case:?}: expected the replacement {after:?} to exist once the update \
             completed, got {read}"
        );
    }
}

/// The stack's own DELETION: it is not `DELETE_COMPLETE` before the service that owned the
/// resource it deleted has removed it.
#[tokio::test]
async fn a_stacks_deletion_completes_only_once_its_resources_are_gone() {
    let router = build_router();
    given_delivery_role(&router, "the delivery role every Firehose row needs").await;

    for owner in OWNERS {
        let owner = *owner;
        let case = format!("a stack deletion that removes a resource owned by {}", owner.label());
        let case = case.as_str();
        let stack_name = format!("removal-delete-{}", owner.label());
        let deleted = format!("removal-delete-stream-{}", owner.label());

        let stack = create_stack(
            &router,
            case,
            &stack_name,
            &template(owner, &[("Stream", &deleted)]),
        )
        .await;
        wait_for(&router, case, &stack, CREATE_COMPLETE).await;

        let (status, body) =
            cloudformation(&router, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );

        settles_only_once_gone(
            &router,
            case,
            &stack,
            DELETE_IN_PROGRESS,
            DELETE_COMPLETE,
            owner,
            &deleted,
        )
        .await;
    }
}
