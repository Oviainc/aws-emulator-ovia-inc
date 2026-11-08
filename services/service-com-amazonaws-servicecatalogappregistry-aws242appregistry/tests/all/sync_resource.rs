// SPDX-License-Identifier: CC0-1.0

//! What `SyncResource` reconciles, end to end over the wire.
//!
//! The rule under test: a sync compares the AppRegistry system tag the resource carries
//! with what this service's records say the resource is associated with. Where the two
//! already agree there is nothing to do, so the answer is `NO_ACTION` and nothing is
//! written; where they differ a sync is started — the associated application's tag applied,
//! or a tag no association backs removed — and the answer is `START_SYNC`. Either way the
//! answer carries the resource's OWN ARN, as CloudFormation holds it, and the associated
//! application's ARN only where there is an association; a second sync, with nothing left
//! to do, answers `NO_ACTION`.
//!
//! The cases are a table because the rule is general: it holds whichever way the resource
//! is addressed (by stack name or by stack id), whichever specifier the association was
//! recorded under, and whatever the resource already carries (nothing, its own
//! application's tag, or another application's). One recorded request is one row.
//!
//! Two services are mounted on ONE engine, because the behaviour spans them: a
//! `CFN_STACK` resource IS a CloudFormation stack, so the stack the sync reads — its stack
//! id and its tags — comes from CloudFormation's own API, exactly as it does in the
//! production router.

use std::sync::Arc;

use service_com_amazonaws_servicecatalogappregistry_aws242appregistry::state::{
    ComAmazonawsServicecatalogappregistryAws242appregistryState as AppRegistryState,
    APPLICATION_TAG_KEY,
};
use service_com_amazonaws_servicecatalogappregistry_aws242appregistry::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::appregistry_core_loop::{as_str, call, encode_label, error_code, REGION};

/// The resource type a stack is synced as, and the one no resource is
/// (`smithy_query(query="shape", shape="ResourceType")`: `CFN_STACK | RESOURCE_TAG_VALUE`).
const CFN_STACK: &str = "CFN_STACK";
/// See [`CFN_STACK`].
const RESOURCE_TAG_VALUE: &str = "RESOURCE_TAG_VALUE";

/// The `actionTaken` values (`smithy_query(query="shape", shape="SyncAction")`).
const START_SYNC: &str = "START_SYNC";
/// See [`START_SYNC`].
const NO_ACTION: &str = "NO_ACTION";

/// AppRegistry and CloudFormation over one shared engine, plus the in-process callback a
/// test drives CloudFormation's own API through.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            callback,
            engine,
        }
    }

    /// A CloudFormation client over the same in-process callback the handler uses, so the
    /// stacks a case creates are the stacks the handler reads.
    fn cloudformation(&self) -> aws_sdk_cloudformation::Client {
        let config = aws_sdk_cloudformation::Config::builder()
            .behavior_version(aws_sdk_cloudformation::config::BehaviorVersion::latest())
            .region(aws_sdk_cloudformation::config::Region::new(
                REGION.to_string(),
            ))
            .endpoint_url(format!("https://cloudformation.{REGION}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(aws_sdk_cloudformation::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_cloudformation::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_cloudformation::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_cloudformation::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_cloudformation::Client::from_conf(config)
    }

    /// Create a stack named `stack_name` carrying `tags`, and answer its stack id.
    ///
    /// The template declares a resource type this emulator has no provisioner for, so the
    /// stack is addressable without anything being created in a third service.
    async fn create_stack(
        &self,
        case: &str,
        stack_name: &str,
        tags: &[(String, String)],
    ) -> String {
        let template = serde_json::json!({
            "Resources": {
                "Group": {
                    "Type": "AWS::CloudWatch::Alarm",
                    "Properties": { "AlarmName": format!("{stack_name}-alarm") }
                }
            }
        })
        .to_string();
        let mut request = self
            .cloudformation()
            .create_stack()
            .stack_name(stack_name)
            .template_body(template);
        for (key, value) in tags {
            request = request.tags(
                aws_sdk_cloudformation::types::Tag::builder()
                    .key(key)
                    .value(value)
                    .build(),
            );
        }
        let answer = request.send().await.unwrap_or_else(|error| {
            panic!("case {case:?}: creating stack {stack_name} must succeed, got {error:?}")
        });
        answer
            .stack_id()
            .unwrap_or_else(|| {
                panic!("case {case:?}: CreateStack must answer the stack id of {stack_name}")
            })
            .to_string()
    }

    /// Create an application named `name` and answer its id and ARN.
    async fn create_application(&self, case: &str, name: &str) -> (String, String) {
        let (status, body) = call(
            &self.router,
            "POST",
            "/applications",
            Some(serde_json::json!({ "name": name, "clientToken": format!("token-{name}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: creating application {name} must answer 201, got {status} with {body}"
        );
        (
            as_str(&body, "/application/id"),
            as_str(&body, "/application/arn"),
        )
    }

    /// Record that the resource `specifier` of type `resource_type` is associated with the
    /// application `application_id`.
    ///
    /// Written into the store directly because `AssociateResource` is still a scaffold stub
    /// with an issue of its own; the association IS what that operation persists, and what
    /// the sync under test reads.
    fn associate(&self, case: &str, application_id: &str, resource_type: &str, specifier: &str) {
        let state = self.engine.state::<AppRegistryState>();
        let mut store = state.lock();
        let application = store
            .applications
            .get_mut(application_id)
            .unwrap_or_else(|| panic!("case {case:?}: application {application_id} must be stored"));
        application
            .resources
            .insert((resource_type.to_string(), specifier.to_string()));
    }

    /// `SyncResource` over the wire: `POST /sync/{resourceType}/{resource}`.
    async fn sync(&self, resource_type: &str, resource: &str) -> (u16, serde_json::Value) {
        call(
            &self.router,
            "POST",
            &format!(
                "/sync/{}/{}",
                encode_label(resource_type),
                encode_label(resource)
            ),
            None,
        )
        .await
    }

    /// The `associatedResourceCount` `GetApplication` reports for `application_id`.
    async fn associated_resource_count(&self, case: &str, application_id: &str) -> i64 {
        let (status, body) = call(
            &self.router,
            "GET",
            &format!("/applications/{}", encode_label(application_id)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading application {application_id} must answer 200, got {status} \
             with {body}"
        );
        body.pointer("/associatedResourceCount")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: expected an associatedResourceCount in {body}"))
    }
}

/// How a case addresses the stack: the two forms CloudFormation's own `StackName` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// The stack's name.
    Name,
    /// The stack id, which is the stack's ARN.
    StackId,
}

/// Which application's system tag the stack is created already carrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carried {
    /// No AppRegistry system tag at all.
    Nothing,
    /// The tag of the application the case associates the stack with.
    ItsApplication,
    /// The tag of a DIFFERENT application, which no association backs.
    AnotherApplication,
}

/// One row of the reconciliation table.
#[derive(Debug)]
struct Case {
    /// What this row is called in an assertion.
    name: &'static str,
    /// What the stack already carries under the AppRegistry system tag key.
    carried: Carried,
    /// The specifier the association is recorded under, or `None` for a stack associated
    /// with no application.
    associated_by: Option<Address>,
    /// How the sync request addresses the stack.
    addressed_by: Address,
    /// The action the first sync must report.
    first_action: &'static str,
}

/// The rows: the rule is one, so each row differs only in what it generalises over — what
/// the resource carries, whether and how it is associated, and how it is addressed.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "unassociated and untagged, addressed by stack name",
            carried: Carried::Nothing,
            associated_by: None,
            addressed_by: Address::Name,
            first_action: NO_ACTION,
        },
        Case {
            name: "unassociated and untagged, addressed by stack id",
            carried: Carried::Nothing,
            associated_by: None,
            addressed_by: Address::StackId,
            first_action: NO_ACTION,
        },
        Case {
            name: "associated by name and untagged, addressed by stack name",
            carried: Carried::Nothing,
            associated_by: Some(Address::Name),
            addressed_by: Address::Name,
            first_action: START_SYNC,
        },
        Case {
            name: "associated by name and untagged, addressed by stack id",
            carried: Carried::Nothing,
            associated_by: Some(Address::Name),
            addressed_by: Address::StackId,
            first_action: START_SYNC,
        },
        Case {
            name: "associated by stack id and untagged, addressed by stack name",
            carried: Carried::Nothing,
            associated_by: Some(Address::StackId),
            addressed_by: Address::Name,
            first_action: START_SYNC,
        },
        Case {
            name: "associated and already carrying its own application's tag",
            carried: Carried::ItsApplication,
            associated_by: Some(Address::Name),
            addressed_by: Address::Name,
            first_action: NO_ACTION,
        },
        Case {
            name: "unassociated but carrying another application's tag",
            carried: Carried::AnotherApplication,
            associated_by: None,
            addressed_by: Address::Name,
            first_action: START_SYNC,
        },
        Case {
            name: "associated but carrying another application's tag",
            carried: Carried::AnotherApplication,
            associated_by: Some(Address::Name),
            addressed_by: Address::Name,
            first_action: START_SYNC,
        },
    ]
}

/// A sync reports the action its records and the resource's own tag imply, carries the
/// stack's id as the resource ARN and the application's ARN only where one is associated,
/// and has nothing left to do when it is called again.
#[tokio::test]
async fn a_sync_reconciles_a_stack_with_the_application_it_is_associated_with() {
    for (row, case) in cases().into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = case.name;
        let stack_name = format!("sync-stack-{row}");

        let (application_id, application_arn) = emulator
            .create_application(name, &format!("sync-application-{row}"))
            .await;
        let (_, other_application_arn) = emulator
            .create_application(name, &format!("other-application-{row}"))
            .await;

        let tags = match case.carried {
            Carried::Nothing => Vec::new(),
            Carried::ItsApplication => vec![(
                APPLICATION_TAG_KEY.to_string(),
                application_arn.clone(),
            )],
            Carried::AnotherApplication => vec![(
                APPLICATION_TAG_KEY.to_string(),
                other_application_arn.clone(),
            )],
        };
        let stack_id = emulator.create_stack(name, &stack_name, &tags).await;

        let address = |form: Address| match form {
            Address::Name => stack_name.clone(),
            Address::StackId => stack_id.clone(),
        };
        if let Some(form) = case.associated_by {
            emulator.associate(name, &application_id, CFN_STACK, &address(form));
        }
        let expected_application_arn = case
            .associated_by
            .map(|_| application_arn.clone());

        let (status, body) = emulator.sync(CFN_STACK, &address(case.addressed_by)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from the sync, got {status} with {body}"
        );
        assert_eq!(
            body.pointer("/actionTaken").and_then(serde_json::Value::as_str),
            Some(case.first_action),
            "case {name:?}: expected actionTaken {:?}, got {body}",
            case.first_action
        );
        assert_eq!(
            body.pointer("/resourceArn").and_then(serde_json::Value::as_str),
            Some(stack_id.as_str()),
            "case {name:?}: expected the stack's own id as resourceArn, got {body}"
        );
        assert_eq!(
            body.pointer("/applicationArn")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            expected_application_arn,
            "case {name:?}: expected applicationArn {expected_application_arn:?}, got {body}"
        );

        // Called again, the sync has nothing left to do: the tag it left and the
        // association now agree, whichever of the two the first call changed.
        let (status, again) = emulator.sync(CFN_STACK, &address(case.addressed_by)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from the second sync, got {status} with {again}"
        );
        assert_eq!(
            again.pointer("/actionTaken").and_then(serde_json::Value::as_str),
            Some(NO_ACTION),
            "case {name:?}: expected the second sync to have nothing to do, got {again}"
        );
        assert_eq!(
            again.pointer("/resourceArn"),
            body.pointer("/resourceArn"),
            "case {name:?}: expected the same resourceArn from both syncs, got {again}"
        );
        assert_eq!(
            again.pointer("/applicationArn"),
            body.pointer("/applicationArn"),
            "case {name:?}: expected the same applicationArn from both syncs, got {again}"
        );

        // A sync reconciles tags; it never associates anything. What the application
        // reports as its associated resources is only what was associated with it.
        let expected_count = i64::from(case.associated_by.is_some());
        let count = emulator
            .associated_resource_count(name, &application_id)
            .await;
        assert_eq!(
            count, expected_count,
            "case {name:?}: expected the sync to leave associatedResourceCount at \
             {expected_count}, got {count}"
        );
    }
}

/// A stack CloudFormation does not hold is not a resource this service can sync: the
/// modelled 404, whichever form the specifier takes.
#[tokio::test]
async fn syncing_a_stack_cloudformation_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    // One stack exists, so the refusals below are about the specifier and not about an
    // empty CloudFormation.
    let stack_id = emulator
        .create_stack("a stack that does exist", "sync-stack-present", &[])
        .await;

    let absent = [
        ("a name no stack answers to", "sync-stack-absent".to_string()),
        (
            "a stack id no stack answers to",
            format!("{stack_id}-absent"),
        ),
        (
            "a name that differs from a live stack's only in case",
            "SYNC-STACK-PRESENT".to_string(),
        ),
    ];
    for (name, specifier) in absent {
        let (status, body) = emulator.sync(CFN_STACK, &specifier).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404 for {specifier}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected the modelled not-found error for {specifier}, got {body}"
        );
        let message = body
            .pointer("/message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(&specifier),
            "case {name:?}: expected the refusal to name the resource it could not address, \
             got {message:?}"
        );
    }
}

/// A tag value is not a resource whose tags there is anything to reconcile, so a sync of
/// one is refused rather than answered with an action nothing took.
#[tokio::test]
async fn syncing_a_resource_that_is_not_a_cfn_stack_is_refused() {
    let emulator = Emulator::new();
    let stack_id = emulator
        .create_stack("the stack the tag values name", "sync-stack-tagged", &[])
        .await;

    let resources = [
        ("a bare tag value", "sim2real-managed".to_string()),
        ("an ARN as the tag value", stack_id),
        (
            "a tag value that looks like an application id",
            "a".repeat(26),
        ),
    ];
    for (name, resource) in resources {
        let (status, body) = emulator.sync(RESOURCE_TAG_VALUE, &resource).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for {resource}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name:?}: expected the modelled validation error for {resource}, got {body}"
        );
        // The wording is part of the contract: the refusal says which resource type a sync
        // IS supported with, and names no value from the request.
        assert_eq!(
            body.pointer("/message")
                .and_then(serde_json::Value::as_str),
            Some("Sync resource is only supported with resource type CFN_STACK."),
            "case {name:?}: expected the refusal to state the one supported resource type, \
             got {body}"
        );
    }
}
