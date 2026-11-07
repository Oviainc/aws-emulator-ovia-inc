// SPDX-License-Identifier: CC0-1.0

//! How EC2 WORDS the `IncorrectInstanceState` refusal, across the family of operations
//! that act on an instance only from certain lifecycle states.
//!
//! The transport is the crate's shared [`super::ec2_query`] harness, so every call here is
//! an ec2Query request a client could have sent, and every refusal is read out of the
//! error envelope the caller receives rather than out of the store.
//!
//! The rule under test is one sentence: an operation refused because the instance's
//! current state does not admit it answers HTTP 400 with the code
//! `IncorrectInstanceState` and ONE sentence — the instance id the request named, the
//! action its state does not admit, and a closing full stop. The state the instance is
//! actually in is NOT part of the sentence, so the text an operation answers is the same
//! whichever state it refused.
//!
//! Each assertion is driven from a TABLE whose rows differ along what the rule
//! generalises over — the operation, the action it names, whether its request carries a
//! batch of ids or one, and the state the instance is held in — so the recorded
//! terminated-instance `StartInstances` call is a single row of it.

use service_com_amazonaws_ec2_amazonec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, InstanceStateName,
};

use super::ec2_query;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names.
const AMI: &str = "ami-0123456789abcdef0";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One EC2 emulator over its own engine. The engine is kept so an instance can be held in
/// a state no sequence of calls leaves it in long enough to call about.
struct Ec2 {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: ::std::sync::Arc<engine::InMemoryEngine>,
}

impl Ec2 {
    fn new() -> Self {
        let (router, engine) = ec2_query::emulator();
        Self { router, engine }
    }

    async fn call(&self, action: &str, params: Vec<(String, String)>) -> (u16, String) {
        ec2_query::call(&self.router, REGION, action, &params).await
    }

    /// Launch one instance and answer the id this account issued it.
    async fn launch(&self, case: &str) -> String {
        let (status, body) = self
            .call(
                "RunInstances",
                vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from RunInstances, got {status} with body {body}"
        );
        ec2_query::element(&body, "instanceId").unwrap_or_else(|| {
            panic!("case {case:?}: RunInstances must report the instance it minted: {body}")
        })
    }

    /// Hold one instance still in `state`.
    ///
    /// The transient lifecycle states cannot be held across a call through the operations
    /// that start them — the clock moves the instance on whether or not anybody calls — so
    /// the state is written into the store the way the sibling suites seed the branches no
    /// sequence of calls can hold still.
    fn park(&self, case: &str, instance_id: &str, state: InstanceStateName) {
        let store = self.engine.state::<Ec2State>();
        let mut guard = store.lock();
        let instance = guard.instances.get_mut(instance_id).unwrap_or_else(|| {
            panic!("case {case:?}: the store must hold the launched instance {instance_id}")
        });
        instance.state = state;
        instance.pending_transition = None;
    }
}

/// One operation of the family: the ec2Query action, the action its refusal names, how its
/// request carries instance ids, and the states it does not admit.
///
/// Which states each operation admits is that operation's own rule, covered by its own
/// suite; what this table needs of it is one state it refuses, so the WORDING of the
/// refusal can be asserted. The refused states below are the ones each handler leaves to
/// this refusal — `RebootInstances` ignores a terminated instance rather than refusing it,
/// so `terminated` is not one of its rows.
struct Family {
    action: &'static str,
    /// The action the sentence ends with, as that operation names it.
    names_action: &'static str,
    /// `true` when the request carries an `InstanceId.N` list, `false` for the single
    /// `InstanceId` field.
    batched: bool,
    refused: &'static [InstanceStateName],
}

/// The operations whose state check answers this refusal with a request carrying nothing
/// but instance ids.
const FAMILIES: &[Family] = &[
    Family {
        action: "StartInstances",
        names_action: "started",
        batched: true,
        refused: &[
            InstanceStateName::Stopping,
            InstanceStateName::ShuttingDown,
            InstanceStateName::Terminated,
        ],
    },
    Family {
        action: "StopInstances",
        names_action: "stopped",
        batched: true,
        refused: &[
            InstanceStateName::ShuttingDown,
            InstanceStateName::Terminated,
        ],
    },
    Family {
        action: "RebootInstances",
        names_action: "rebooted",
        batched: true,
        refused: &[
            InstanceStateName::Stopped,
            InstanceStateName::Stopping,
            InstanceStateName::ShuttingDown,
        ],
    },
    Family {
        action: "MonitorInstances",
        names_action: "monitored",
        batched: true,
        refused: &[
            InstanceStateName::Stopped,
            InstanceStateName::Stopping,
            InstanceStateName::ShuttingDown,
            InstanceStateName::Terminated,
        ],
    },
    Family {
        action: "UnmonitorInstances",
        names_action: "unmonitored",
        batched: true,
        refused: &[
            InstanceStateName::Stopped,
            InstanceStateName::Stopping,
            InstanceStateName::ShuttingDown,
            InstanceStateName::Terminated,
        ],
    },
    Family {
        action: "SendDiagnosticInterrupt",
        names_action: "sent a diagnostic interrupt",
        batched: false,
        refused: &[
            InstanceStateName::Stopped,
            InstanceStateName::Stopping,
            InstanceStateName::ShuttingDown,
            InstanceStateName::Terminated,
        ],
    },
];

impl Family {
    /// The fields one request of this operation carries for the ids it names.
    fn fields(&self, instance_ids: &[String]) -> Vec<(String, String)> {
        if self.batched {
            instance_ids
                .iter()
                .enumerate()
                .map(|(index, id)| p(&format!("InstanceId.{}", index + 1), id.clone()))
                .collect()
        } else {
            assert_eq!(
                instance_ids.len(),
                1,
                "case {:?}: this operation names exactly one instance",
                self.action
            );
            vec![p("InstanceId", instance_ids[0].clone())]
        }
    }

    /// The whole sentence this operation's refusal must carry for `instance_id`.
    fn message(&self, instance_id: &str) -> String {
        format!(
            "The instance '{instance_id}' is not in a state from which it can be {}.",
            self.names_action
        )
    }
}

/// The rule, over every (operation, refused state) pair of the table: HTTP 400, the code,
/// and the one sentence — with the state the instance is in nowhere in it.
#[tokio::test]
async fn a_refusal_names_the_instance_and_the_action_and_no_state() {
    for family in FAMILIES {
        for state in family.refused {
            let case = format!("{} on an instance in {}", family.action, state.as_str());
            let ec2 = Ec2::new();
            let instance_id = ec2.launch(&case).await;
            ec2.park(&case, &instance_id, *state);

            let (status, body) = ec2
                .call(
                    family.action,
                    family.fields(std::slice::from_ref(&instance_id)),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            let code = ec2_query::error_code(&body);
            assert_eq!(
                code, "IncorrectInstanceState",
                "case {case:?}: expected IncorrectInstanceState, got {code:?} in {body}"
            );
            let message = ec2_query::error_message(&body);
            assert_eq!(
                message,
                family.message(&instance_id),
                "case {case:?}: expected {:?}, got {message:?}",
                family.message(&instance_id)
            );
            assert!(
                !message.contains(state.as_str()),
                "case {case:?}: the refusal must not report the state the instance is in, \
                 got {message:?}"
            );
        }
    }
}

/// The same rule read as an invariant: for ONE operation and ONE instance, the text is
/// byte-identical whichever of its refused states the instance is held in, so nothing in
/// the sentence is read out of the store.
#[tokio::test]
async fn the_refusal_text_does_not_vary_with_the_state_it_refused() {
    for family in FAMILIES {
        let case = format!("{} across every state it refuses", family.action);
        let ec2 = Ec2::new();
        let instance_id = ec2.launch(&case).await;
        let expected = family.message(&instance_id);

        for state in family.refused {
            ec2.park(&case, &instance_id, *state);
            let (status, body) = ec2
                .call(
                    family.action,
                    family.fields(std::slice::from_ref(&instance_id)),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: in {} the call must be refused, got {status} with body {body}",
                state.as_str()
            );
            let message = ec2_query::error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: in {} the text must still be {expected:?}, got {message:?}",
                state.as_str()
            );
        }
    }
}

/// The id in the sentence is the one the REQUEST named for the instance that was refused,
/// not the first id of the batch: a batch naming an instance that is up before the one in
/// a refused state is refused for the SECOND id.
#[tokio::test]
async fn a_batch_is_refused_for_the_id_whose_state_does_not_admit_it() {
    for family in FAMILIES.iter().filter(|family| family.batched) {
        let Some(state) = family.refused.first() else {
            continue;
        };
        let case = format!(
            "{} on a batch whose second instance is {}",
            family.action,
            state.as_str()
        );
        let ec2 = Ec2::new();
        // Still `pending`, which every operation of this family admits: the batch is
        // refused for the other instance alone.
        let up = ec2.launch(&case).await;
        let parked = ec2.launch(&case).await;
        ec2.park(&case, &parked, *state);

        let (status, body) = ec2
            .call(family.action, family.fields(&[up.clone(), parked.clone()]))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let message = ec2_query::error_message(&body);
        assert_eq!(
            message,
            family.message(&parked),
            "case {case:?}: expected {:?}, got {message:?}",
            family.message(&parked)
        );
        assert!(
            !message.contains(&up),
            "case {case:?}: the refusal must name the refused instance and not {up}, got \
             {message:?}"
        );
    }
}
