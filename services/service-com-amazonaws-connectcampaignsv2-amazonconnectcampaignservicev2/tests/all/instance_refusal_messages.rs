// SPDX-License-Identifier: CC0-1.0

//! What this service SAYS when a `connectInstanceId` names no instance it can serve,
//! end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handlers together. The rule under test: a 404 `ResourceNotFoundException` names
//! the RESOURCE the request addresses, and every operation addressing that resource
//! answers the one text for it —
//!
//! * the operations whose own resource IS the instance's Campaigns configuration (the
//!   two on `/v2/connect-instance/{id}/config` and the two on its
//!   `/communication-limits`, which are a member of that same configuration) report
//!   that configuration as not found, naming the instance asked about;
//! * the operations that address some OTHER resource and only require the instance to
//!   be usable by Campaigns (`CreateCampaign`, the three instance-integration
//!   operations, `StartInstanceOnboardingJob`) answer the service's FIXED sentence
//!   about the instance, which carries nothing from the request;
//! * the two whose own resource is the onboarding JOB report that job as not found,
//!   naming the instance asked about;
//!
//! so two operations of one class cannot disagree about a request they both refuse.
//!
//! The rows differ along what the rule generalises over: the operation (every
//! instance-scoped one, across all three classes), the form of the instance id (its
//! length bounds, the punctuation its pattern admits, the uuid form a Connect instance
//! id takes) and WHY the instance cannot be served — unknown to Amazon Connect,
//! registered but never onboarded, onboarding still in progress, onboarded only in
//! another region, or its configuration deleted since.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    instance_config_not_found_message, onboarding_job_not_found_message,
    INSTANCE_NOT_FOUND_MESSAGE,
};

use super::common::{call, error_code, onboard_instance, router, Harness};

/// The region every case signs for unless it is about more than one.
const REGION: &str = "us-east-1";

/// A second region, for the row whose instance is onboarded only through another
/// region's endpoint: its configuration is that region's resource, so this one holds
/// none.
const OTHER_REGION: &str = "eu-west-1";

/// Which resource a request addresses, and so which text its 404 carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Refusal {
    /// The instance's Campaigns configuration itself.
    Config,
    /// Another resource, for which the instance only has to be usable by Campaigns.
    Instance,
    /// The instance's onboarding job.
    Job,
}

impl Refusal {
    /// The message this class answers for `instance`, spelled out HERE rather than
    /// taken from the crate, so the assertion pins the wording and not merely the fact
    /// that every handler calls the same helper.
    fn message(self, instance: &str) -> String {
        match self {
            Refusal::Config => format!("ConnectInstanceConfig not found {instance}."),
            Refusal::Instance => {
                "Connect Instance does not exist or is not enabled to use Campaigns.".to_string()
            }
            Refusal::Job => format!("No onboarding job found for Connect instance {instance}"),
        }
    }
}

/// One request to drive against one instance, and the class its 404 belongs to.
struct Op {
    name: &'static str,
    method: &'static str,
    path: String,
    body: Value,
    refusal: Refusal,
}

/// Instance ids spanning `InstanceId` (len 1..256, pattern `^[-_.a-zA-Z0-9]+$`, from
/// `smithy_query(query="operation", operation="GetConnectInstanceConfig")`): the
/// shortest id, the uuid form a Connect instance id takes, every punctuation character
/// the pattern admits, and the longest id the length bound allows.
fn instance_ids() -> Vec<(&'static str, String)> {
    vec![
        ("one character (length minimum)", "a".to_string()),
        (
            "uuid form",
            "7c9e6679-7425-40de-944b-e07fc1f90ae7".to_string(),
        ),
        ("every admitted punctuation", "A-z_0.9".to_string()),
        ("256 characters (length maximum)", "r".repeat(256)),
    ]
}

/// Every operation that resolves a `connectInstanceId` against this service's
/// instance-onboarding store, with the request bodies the model requires, as
/// `smithy_query(query="operation", operation="GetConnectInstanceConfig,...")` binds
/// them.
fn instance_scoped_operations(instance: &str) -> Vec<Op> {
    let instance_path = format!("/v2/connect-instance/{instance}");
    vec![
        Op {
            name: "GetConnectInstanceConfig",
            method: "GET",
            path: format!("{instance_path}/config"),
            body: Value::Null,
            refusal: Refusal::Config,
        },
        Op {
            name: "DeleteConnectInstanceConfig",
            method: "DELETE",
            path: format!("{instance_path}/config"),
            body: Value::Null,
            refusal: Refusal::Config,
        },
        Op {
            name: "GetInstanceCommunicationLimits",
            method: "GET",
            path: format!("{instance_path}/communication-limits"),
            body: Value::Null,
            refusal: Refusal::Config,
        },
        Op {
            name: "PutInstanceCommunicationLimits",
            method: "PUT",
            path: format!("{instance_path}/communication-limits"),
            body: json!({"communicationLimitsConfig": {"allChannelSubtypes": {
                "communicationLimitsList": [
                    {"maxCountPerRecipient": 3, "frequency": 1, "unit": "DAY"},
                ],
            }}}),
            refusal: Refusal::Config,
        },
        Op {
            name: "ListConnectInstanceIntegrations",
            method: "GET",
            path: format!("{instance_path}/integrations"),
            body: Value::Null,
            refusal: Refusal::Instance,
        },
        Op {
            name: "PutConnectInstanceIntegration",
            method: "PUT",
            path: format!("{instance_path}/integrations"),
            body: json!({"integrationConfig": {"lambda": {
                "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:refusal-probe",
            }}}),
            refusal: Refusal::Instance,
        },
        Op {
            name: "DeleteConnectInstanceIntegration",
            method: "POST",
            path: format!("{instance_path}/integrations/delete"),
            body: json!({"integrationIdentifier": {"lambda": {
                "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:refusal-probe",
            }}}),
            refusal: Refusal::Instance,
        },
        Op {
            name: "CreateCampaign",
            method: "PUT",
            path: "/v2/campaigns".to_string(),
            body: json!({
                "name": "refusal-probe-campaign",
                "connectInstanceId": instance,
                // `outboundMode` and `defaultOutboundConfig` are both required by the
                // model.
                "channelSubtypeConfig": {"telephony": {
                    "outboundMode": {"agentless": {}},
                    "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"},
                }},
            }),
            refusal: Refusal::Instance,
        },
        Op {
            name: "GetInstanceOnboardingJobStatus",
            method: "GET",
            path: format!("{instance_path}/onboarding"),
            body: Value::Null,
            refusal: Refusal::Job,
        },
        Op {
            name: "DeleteInstanceOnboardingJob",
            method: "DELETE",
            path: format!("{instance_path}/onboarding"),
            body: Value::Null,
            refusal: Refusal::Job,
        },
    ]
}

/// Why the instance under a row cannot be served, and how the harness gets it there.
#[derive(Clone, Copy, Debug)]
enum Setup {
    /// Amazon Connect holds no instance under the id, so nothing was ever onboardable.
    Unknown,
    /// The account's Connect registry holds the instance, but Campaigns was never
    /// asked to onboard it.
    Registered,
    /// Onboarding was started and the clock has NOT yet finished the job, so the
    /// instance holds no configuration yet.
    Onboarding,
    /// The instance is onboarded through the OTHER region's endpoint only.
    OnboardedElsewhere,
    /// The instance was onboarded here and its configuration deleted since.
    ConfigDeleted,
}

/// Put `instance` into the state `setup` describes, in a freshly built router.
async fn set_up(setup: Setup, instance: &str) -> Harness {
    let router = router();
    match setup {
        Setup::Unknown => {}
        Setup::Registered => router.register_connect_instance(REGION, instance),
        Setup::Onboarding => {
            router.register_connect_instance(REGION, instance);
            // Started and deliberately NOT waited for: the job is due one
            // ONBOARDING_JOB_DURATION out, so the requests below run while it is still
            // IN_PROGRESS and the instance holds no configuration.
            let (status, body) = call(
                &router,
                REGION,
                "PUT",
                &format!("/v2/connect-instance/{instance}/onboarding"),
                &json!({"encryptionConfig": {"enabled": false}}),
            )
            .await;
            assert_eq!(
                status, 200,
                "setup {setup:?}: expected the job to start, got {status} {body}"
            );
        }
        Setup::OnboardedElsewhere => onboard_instance(&router, OTHER_REGION, instance).await,
        Setup::ConfigDeleted => {
            onboard_instance(&router, REGION, instance).await;
            let (status, body) = call(
                &router,
                REGION,
                "DELETE",
                &format!("/v2/connect-instance/{instance}/config"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                status, 200,
                "setup {setup:?}: expected the configuration to be deleted, got {status} {body}"
            );
        }
    }
    router
}

#[tokio::test]
async fn every_instance_scoped_refusal_carries_its_resources_one_text() {
    for setup in [
        Setup::Unknown,
        Setup::Registered,
        Setup::Onboarding,
        Setup::OnboardedElsewhere,
        Setup::ConfigDeleted,
    ] {
        for (id_case, instance) in instance_ids() {
            let router = set_up(setup, &instance).await;
            // What each class of refusal was observed to say, so the rows can be
            // checked for AGREEMENT as well as for wording.
            let mut said: BTreeMap<Refusal, Vec<(&'static str, String)>> = BTreeMap::new();

            for op in instance_scoped_operations(&instance) {
                // A job that has STARTED is a job: while it is in progress the two
                // onboarding-job operations are answering about a record they hold,
                // not refusing a resource they do not, so this row is not theirs.
                if op.refusal == Refusal::Job && matches!(setup, Setup::Onboarding) {
                    continue;
                }
                let case = format!("{} / {id_case} / {setup:?}", op.name);
                let (status, body) = call(&router, REGION, op.method, &op.path, &op.body).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected HTTP 404 for an instance this region cannot \
                     serve, got {status} {body}"
                );
                assert!(
                    error_code(&body).ends_with("ResourceNotFoundException"),
                    "case {case:?}: expected ResourceNotFoundException, got {:?}",
                    error_code(&body)
                );
                let message = body
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("<no message>")
                    .to_string();
                assert_eq!(
                    message,
                    op.refusal.message(&instance),
                    "case {case:?}: expected the {:?} class text, got {message:?}",
                    op.refusal
                );
                said.entry(op.refusal).or_default().push((op.name, message));
            }

            // The invariant itself: within one class, every operation said the same
            // thing about the same request.
            for (refusal, answers) in said {
                let case = format!("{refusal:?} / {id_case} / {setup:?}");
                let (first_op, first) = &answers[0];
                for (other_op, other) in &answers[1..] {
                    assert_eq!(
                        first, other,
                        "case {case:?}: {first_op} and {other_op} refuse the same request, \
                         so they must say the same thing: got {first:?} and {other:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn the_instance_class_text_carries_nothing_from_the_request() {
    // The fixed sentence names no instance, so the SAME bytes come back whichever id
    // was asked about — which is what makes it a constant and not a template.
    let mut answers: Vec<String> = Vec::new();
    // Only the ids long enough that finding one inside the answer means the answer
    // really named it: a one-character id occurs in an English sentence by chance.
    let ids = instance_ids()
        .into_iter()
        .filter(|(_, id)| id.chars().count() > 3);
    for (id_case, instance) in ids {
        let router = set_up(Setup::Registered, &instance).await;
        let (status, body) = call(
            &router,
            REGION,
            "GET",
            &format!("/v2/connect-instance/{instance}/integrations"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {id_case:?}: expected HTTP 404, got {status} {body}"
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
            .to_string();
        assert!(
            !message.contains(instance.as_str()),
            "case {id_case:?}: the fixed text carries nothing from the request, yet it \
             names the instance: {message:?}"
        );
        answers.push(message);
    }
    for (index, message) in answers.iter().enumerate() {
        assert_eq!(
            message, &answers[0],
            "case {index}: every id gets the same fixed text, got {message:?} and {:?}",
            answers[0]
        );
    }
}

#[tokio::test]
async fn the_shared_texts_are_the_ones_the_handlers_compose() {
    // The crate's helpers are what the handlers call, so the wording pinned above is
    // pinned at its single source too.
    for (id_case, instance) in instance_ids() {
        assert_eq!(
            instance_config_not_found_message(&instance),
            Refusal::Config.message(&instance),
            "case {id_case:?}: the configuration text drifted from what the wire carries"
        );
        assert_eq!(
            onboarding_job_not_found_message(&instance),
            Refusal::Job.message(&instance),
            "case {id_case:?}: the onboarding-job text drifted from what the wire carries"
        );
    }
    assert_eq!(
        INSTANCE_NOT_FOUND_MESSAGE,
        Refusal::Instance.message("any-instance"),
        "the instance text drifted from what the wire carries"
    );
}

#[tokio::test]
async fn an_instance_this_region_can_serve_is_not_refused_at_all() {
    // The positive control: with the instance onboarded HERE, the same reads succeed,
    // so the 404s above are the instance lookup failing and not the operations being
    // broken.
    let instance = "served-instance";
    let router = router();
    onboard_instance(&router, REGION, instance).await;

    for (what, path) in [
        ("GetConnectInstanceConfig", "config"),
        ("GetInstanceCommunicationLimits", "communication-limits"),
        ("ListConnectInstanceIntegrations", "integrations"),
        ("GetInstanceOnboardingJobStatus", "onboarding"),
    ] {
        let (status, body) = call(
            &router,
            REGION,
            "GET",
            &format!("/v2/connect-instance/{instance}/{path}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {what:?}: expected HTTP 200 for an onboarded instance, got {status} {body}"
        );
    }
}
