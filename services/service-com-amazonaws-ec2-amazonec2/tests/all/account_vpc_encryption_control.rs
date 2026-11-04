// SPDX-License-Identifier: CC0-1.0

//! The account's VPC Encryption Control configuration: what
//! `ModifyAccountVpcEncryptionControl` records and what
//! `DescribeAccountVpcEncryptionControl` reports back out of that one store, driven over
//! the wire the service speaks (`ec2Query`, through [`super::ec2_query`]).
//!
//! The rules each test pins:
//!
//! 1. DEFAULT: an account nothing has modified is reported with the configuration AWS gives
//!    one — `unmanaged`, `default-state`, every excludable resource type `disabled`, managed
//!    by the account and no update timestamp — with every member the result declares and
//!    every enumerated one out of its modelled catalog;
//! 2. ROUND TRIP: a modify records the mode and the per-resource exclusions it names and
//!    NOTHING else, and the configuration a later describe reports is the one that modify
//!    left — transitional while the moves it started are owed, settled once they are;
//! 3. REPORTING ONLY: describing the configuration writes nothing, so repeated reports —
//!    in any Region, the configuration being the account's rather than a Region's — answer
//!    exactly the same words;
//! 4. REFUSAL: a value outside a modelled catalog is refused at the boundary and records
//!    nothing, and a `DryRun` on either operation is answered 412 `DryRunOperation` and
//!    records nothing.
//!
//! Each row of the tables below is a distinct input along what its rule generalises over:
//! which mode is asked for, which and how many resource types are excluded, whether a
//! request asks for what is already in force, and whether the moves it started have had
//! their span.

use std::time::Duration;

use ec2::state::TRANSITION_DELAY;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message};
use super::wire::{Param, child, child_text, document, param};

/// Regions spanning the partitions and the shapes a Region name takes, so nothing asserted
/// below is a property of one Region string.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-1", "us-gov-west-1"];

/// The whole `AccountVpcEncryptionControlMode` catalog
/// (`smithy_query(query="shape",shape="AccountVpcEncryptionControlMode")`).
const MODES: [&str; 3] = ["unmanaged", "attempt-monitor", "attempt-enforce"];

/// The whole `AccountVpcEncryptionControlState` catalog
/// (`smithy_query(query="shape",shape="AccountVpcEncryptionControlState")`).
const STATES: [&str; 5] = [
    "default-state",
    "transitions-in-progress",
    "transitions-partially-successful",
    "transitions-successful",
    "transitions-failed",
];

/// The whole `VpcEncryptionControlExclusionState` catalog
/// (`smithy_query(query="shape",shape="VpcEncryptionControlExclusionState")`).
const EXCLUSION_STATES: [&str; 4] = ["enabling", "enabled", "disabling", "disabled"];

/// The whole `ManagedBy` catalog
/// (`smithy_query(query="shape",shape="ManagedBy")`).
const MANAGED_BY: [&str; 2] = ["account", "declarative-policy"];

/// Every excludable resource type, as the REQUEST member that asks about it is spelled and
/// as the RESULT member that reports it is spelled
/// (`smithy_query(query="operation",operation="ModifyAccountVpcEncryptionControl")` and
/// `smithy_query(query="shape",shape="AccountVpcEncryptionControlExclusions")`).
const EXCLUDABLE: [(&str, &str); 8] = [
    ("InternetGateway", "internetGateway"),
    ("EgressOnlyInternetGateway", "egressOnlyInternetGateway"),
    ("NatGateway", "natGateway"),
    ("VirtualPrivateGateway", "virtualPrivateGateway"),
    ("VpcPeering", "vpcPeering"),
    ("Lambda", "lambda"),
    ("VpcLattice", "vpcLattice"),
    ("ElasticFileSystem", "elasticFileSystem"),
];

/// Long enough past [`TRANSITION_DELAY`] that the moves a modify started are certainly
/// made, with a margin so a slow machine fails nothing.
fn past_the_transitions() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The configuration ONE response carries, read through the XML parser so a member of the
/// configuration is never confused with one of the same name nested in its exclusions.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    state: Option<String>,
    mode: Option<String>,
    managed_by: Option<String>,
    last_update_timestamp: Option<String>,
    /// The exclusion members the response reported, in the order it wrote them.
    exclusions: Vec<(String, String)>,
}

impl Reported {
    /// The state reported for the `member` exclusion, or `None` when the response carries
    /// no such member.
    fn exclusion(&self, member: &str) -> Option<&str> {
        self.exclusions
            .iter()
            .find(|(name, _)| name == member)
            .map(|(_, state)| state.as_str())
    }
}

/// The `<accountVpcEncryptionControl>` of a response, as its reported words.
fn reported(case: &str, body: &str) -> Reported {
    let parsed = document(case, body);
    let control = parsed
        .descendants()
        .find(|node| {
            node.is_element() && node.tag_name().name() == "accountVpcEncryptionControl"
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no <accountVpcEncryptionControl> in {body}")
        });
    Reported {
        state: child_text(control, "state"),
        mode: child_text(control, "mode"),
        managed_by: child_text(control, "managedBy"),
        last_update_timestamp: child_text(control, "lastUpdateTimestamp"),
        exclusions: child(control, "exclusions")
            .map(|exclusions| {
                exclusions
                    .children()
                    .filter(roxmltree::Node::is_element)
                    .map(|node| {
                        (
                            node.tag_name().name().to_string(),
                            node.text().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// One `DescribeAccountVpcEncryptionControl` that must succeed, as its reported
/// configuration.
async fn describe(router: &MultiServiceRouter, case: &str, region: &str) -> Reported {
    describe_with(router, case, region, &[]).await
}

/// The same report as [`describe`], with whatever extra parameters the case needs.
async fn describe_with(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    extra: &[Param],
) -> Reported {
    let (status, body) = call(
        router,
        region,
        "DescribeAccountVpcEncryptionControl",
        extra,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAccountVpcEncryptionControl in \
         {region}, got {status} with body {body}"
    );
    reported(case, &body)
}

/// One `ModifyAccountVpcEncryptionControl` that must succeed, as the configuration it
/// answered.
async fn modify(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    params: &[Param],
) -> Reported {
    let (status, body) = call(
        router,
        region,
        "ModifyAccountVpcEncryptionControl",
        params,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyAccountVpcEncryptionControl in \
         {region}, got {status} with body {body}"
    );
    reported(case, &body)
}

/// One modify request: the mode it names (`None` says nothing about the mode) and the
/// exclusion choice it asks for per resource type, named by the request member.
struct Request {
    case: &'static str,
    mode: Option<&'static str>,
    exclusions: &'static [(&'static str, &'static str)],
}

impl Request {
    fn params(&self) -> Vec<Param> {
        let mut params: Vec<Param> = self
            .mode
            .map(|mode| vec![param("Mode", mode)])
            .unwrap_or_default();
        for (member, requested) in self.exclusions {
            params.push(param(member, requested));
        }
        params
    }

    /// The exclusion state this request leaves the resource asked about as `asked_as` in,
    /// once the moves it started have been made: `enabled` where it asked to exclude the
    /// resource, `disabled` where it asked to stop excluding it or said nothing at all (a
    /// fresh account excludes nothing).
    fn settled_exclusion(&self, asked_as: &str) -> &'static str {
        match self
            .exclusions
            .iter()
            .find(|(name, _)| *name == asked_as)
            .map(|(_, requested)| *requested)
        {
            Some("enable") => "enabled",
            _ => "disabled",
        }
    }

    /// Whether this request asks a FRESH account for anything other than the configuration
    /// it already holds — `unmanaged`, nothing excluded. Only such a request transitions
    /// anything.
    fn changes_a_fresh_account(&self) -> bool {
        self.mode.is_some_and(|mode| mode != "unmanaged")
            || self
                .exclusions
                .iter()
                .any(|(_, requested)| *requested == "enable")
    }
}

/// The modify requests the table tests drive, spanning the mode catalog, the number and
/// identity of the resource types excluded, a request that asks for what is already in
/// force and one that asks for nothing at all.
static REQUESTS: [Request; 8] = [
    Request {
        case: "a request that says nothing at all",
        mode: None,
        exclusions: &[],
    },
    Request {
        case: "the mode asked for monitoring",
        mode: Some("attempt-monitor"),
        exclusions: &[],
    },
    Request {
        case: "the mode asked for enforcement",
        mode: Some("attempt-enforce"),
        exclusions: &[],
    },
    Request {
        case: "the mode asked for the unmanaged it already holds",
        mode: Some("unmanaged"),
        exclusions: &[],
    },
    Request {
        case: "one service excluded",
        mode: None,
        exclusions: &[("Lambda", "enable")],
    },
    Request {
        case: "one gateway resource excluded, under enforcement",
        mode: Some("attempt-enforce"),
        exclusions: &[("EgressOnlyInternetGateway", "enable")],
    },
    Request {
        case: "three resource types excluded and one explicitly not",
        mode: Some("attempt-monitor"),
        exclusions: &[
            ("InternetGateway", "enable"),
            ("VpcPeering", "enable"),
            ("ElasticFileSystem", "enable"),
            ("NatGateway", "disable"),
        ],
    },
    Request {
        case: "every resource type excluded",
        mode: Some("attempt-enforce"),
        exclusions: &[
            ("InternetGateway", "enable"),
            ("EgressOnlyInternetGateway", "enable"),
            ("NatGateway", "enable"),
            ("VirtualPrivateGateway", "enable"),
            ("VpcPeering", "enable"),
            ("Lambda", "enable"),
            ("VpcLattice", "enable"),
            ("ElasticFileSystem", "enable"),
        ],
    },
];

/// Every reported configuration must carry each member the result declares, each holding a
/// word out of its modelled catalog.
fn assert_modelled(case: &str, control: &Reported) {
    let state = control.state.as_deref();
    assert!(
        state.is_some_and(|word| STATES.contains(&word)),
        "case {case:?}: expected an AccountVpcEncryptionControlState out of {STATES:?}, \
         got {state:?}"
    );
    let mode = control.mode.as_deref();
    assert!(
        mode.is_some_and(|word| MODES.contains(&word)),
        "case {case:?}: expected an AccountVpcEncryptionControlMode out of {MODES:?}, \
         got {mode:?}"
    );
    let managed_by = control.managed_by.as_deref();
    assert!(
        managed_by.is_some_and(|word| MANAGED_BY.contains(&word)),
        "case {case:?}: expected a ManagedBy out of {MANAGED_BY:?}, got {managed_by:?}"
    );
    if let Some(timestamp) = control.last_update_timestamp.as_deref() {
        assert!(
            as_timestamp(timestamp).is_some(),
            "case {case:?}: expected lastUpdateTimestamp as a timestamp, got {timestamp:?}"
        );
    }
    let reported_members: Vec<&str> = control
        .exclusions
        .iter()
        .map(|(member, _)| member.as_str())
        .collect();
    let declared: Vec<&str> = EXCLUDABLE.iter().map(|(_, reported)| *reported).collect();
    assert_eq!(
        reported_members, declared,
        "case {case:?}: expected the exclusions AccountVpcEncryptionControlExclusions \
         declares, in its own order, got {reported_members:?}"
    );
    for (_, member) in EXCLUDABLE {
        let state = control.exclusion(member);
        assert!(
            state.is_some_and(|word| EXCLUSION_STATES.contains(&word)),
            "case {case:?}: expected {member} as a VpcEncryptionControlExclusionState out \
             of {EXCLUSION_STATES:?}, got {state:?}"
        );
    }
}

/// RULE 1: an account nothing has modified is reported with the configuration AWS gives
/// one, whichever Region it is asked in.
#[tokio::test]
async fn an_unconfigured_account_is_reported_in_its_default_state() {
    for region in REGIONS {
        let router = emulator();
        let case = format!("a fresh account, asked in {region}");
        let control = describe(&router, &case, region).await;

        assert_modelled(&case, &control);
        assert_eq!(
            control.state.as_deref(),
            Some("default-state"),
            "case {case:?}: expected the default state before any modify, got {control:?}"
        );
        assert_eq!(
            control.mode.as_deref(),
            Some("unmanaged"),
            "case {case:?}: expected an unmanaged account before any modify, got {control:?}"
        );
        assert_eq!(
            control.managed_by.as_deref(),
            Some("account"),
            "case {case:?}: expected the configuration to be managed by the account, \
             got {control:?}"
        );
        assert_eq!(
            control.last_update_timestamp, None,
            "case {case:?}: expected no lastUpdateTimestamp on a configuration nothing has \
             updated, got {control:?}"
        );
        for (_, member) in EXCLUDABLE {
            assert_eq!(
                control.exclusion(member),
                Some("disabled"),
                "case {case:?}: expected {member} not excluded before any modify, \
                 got {control:?}"
            );
        }
    }
}

/// RULE 2: the configuration a modify records is the one a later describe reports —
/// transitional while the moves it started are owed, settled once they have had their span —
/// and a modify changes only what its request speaks about.
#[tokio::test]
async fn a_modify_records_the_configuration_a_describe_reports() {
    for request in &REQUESTS {
        let region = REGIONS[0];
        let router = emulator();
        let case = request.case;
        let answered = modify(&router, case, region, &request.params()).await;
        assert_modelled(case, &answered);

        // The moves a modify starts are still owed when it answers, so the configuration it
        // reports carries the transitional words — and a request that asks for what the
        // account already holds transitions nothing.
        let changes = request.changes_a_fresh_account();
        let expected_state = if changes {
            "transitions-in-progress"
        } else {
            "default-state"
        };
        assert_eq!(
            answered.state.as_deref(),
            Some(expected_state),
            "case {case:?}: expected the state {expected_state:?} from the modify itself, \
             got {answered:?}"
        );
        assert_eq!(
            answered.mode.as_deref(),
            Some(request.mode.unwrap_or("unmanaged")),
            "case {case:?}: expected the mode the request named, got {answered:?}"
        );
        assert_eq!(
            answered.last_update_timestamp.is_some(),
            changes,
            "case {case:?}: expected a lastUpdateTimestamp exactly where the request \
             changed the configuration, got {answered:?}"
        );
        for (asked_as, member) in EXCLUDABLE {
            let settled = request.settled_exclusion(asked_as);
            // A newly excluded resource type is `enabling` until the move falls due;
            // everything else is already where the request leaves it.
            let expected = if settled == "enabled" {
                "enabling"
            } else {
                "disabled"
            };
            assert_eq!(
                answered.exclusion(member),
                Some(expected),
                "case {case:?}: expected {member} {expected:?} from the modify itself, \
                 got {answered:?}"
            );
        }

        // The configuration the modify answered is the configuration the store holds: a
        // describe reads it back, and once the moves have had their span every exclusion
        // reports its settled word and the transitions are successful.
        let read_back = describe(&router, case, region).await;
        assert_eq!(
            read_back, answered,
            "case {case:?}: expected a describe to read back the configuration the modify \
             answered ({answered:?}), got {read_back:?}"
        );

        tokio::time::sleep(past_the_transitions()).await;
        let settled = describe(&router, case, region).await;
        assert_modelled(case, &settled);
        let expected_state = if changes {
            "transitions-successful"
        } else {
            "default-state"
        };
        assert_eq!(
            settled.state.as_deref(),
            Some(expected_state),
            "case {case:?}: expected the state {expected_state:?} once the moves had their \
             span, got {settled:?}"
        );
        assert_eq!(
            settled.mode, answered.mode,
            "case {case:?}: expected the mode to stay where the modify put it, \
             got {settled:?}"
        );
        assert_eq!(
            settled.last_update_timestamp, answered.last_update_timestamp,
            "case {case:?}: expected the update timestamp to stay where the modify put it, \
             got {settled:?}"
        );
        for (asked_as, member) in EXCLUDABLE {
            let expected = request.settled_exclusion(asked_as);
            assert_eq!(
                settled.exclusion(member),
                Some(expected),
                "case {case:?}: expected {member} {expected:?} once the moves had their \
                 span, got {settled:?}"
            );
        }
    }
}

/// RULE 2, continued: a second modify changes only the members it names — the exclusion it
/// takes back goes `disabling` and then `disabled`, and every other choice the account holds
/// is left exactly where the first modify put it.
#[tokio::test]
async fn a_later_modify_leaves_what_it_does_not_name_alone() {
    for (member, reported_member) in EXCLUDABLE {
        let region = REGIONS[1];
        let router = emulator();
        let case = format!("excluding {member} and then taking it back");

        // Two resource types excluded and a mode set, so the second modify has something
        // beside its own member to leave alone. The kept one is the resource type at the
        // mirrored place in the catalog, which is never the one under test.
        let position = EXCLUDABLE
            .iter()
            .position(|(name, _)| *name == member)
            .expect("a member of the excludable catalog");
        let kept = EXCLUDABLE[EXCLUDABLE.len() - 1 - position];
        let first = modify(
            &router,
            &case,
            region,
            &[
                param("Mode", "attempt-enforce"),
                param(member, "enable"),
                param(kept.0, "enable"),
            ],
        )
        .await;
        assert_modelled(&case, &first);
        tokio::time::sleep(past_the_transitions()).await;

        let second = modify(&router, &case, region, &[param(member, "disable")]).await;
        assert_modelled(&case, &second);
        assert_eq!(
            second.exclusion(reported_member),
            Some("disabling"),
            "case {case:?}: expected {reported_member} to be coming back into enforcement, \
             got {second:?}"
        );
        assert_eq!(
            second.mode.as_deref(),
            Some("attempt-enforce"),
            "case {case:?}: expected the mode the second modify did not name to be left \
             alone, got {second:?}"
        );
        if kept.1 != reported_member {
            assert_eq!(
                second.exclusion(kept.1),
                Some("enabled"),
                "case {case:?}: expected the exclusion the second modify did not name to be \
                 left alone, got {second:?}"
            );
        }
        tokio::time::sleep(past_the_transitions()).await;
        let settled = describe(&router, &case, region).await;
        assert_eq!(
            settled.exclusion(reported_member),
            Some("disabled"),
            "case {case:?}: expected {reported_member} back inside enforcement once the \
             move had its span, got {settled:?}"
        );
        assert_eq!(
            settled.state.as_deref(),
            Some("transitions-successful"),
            "case {case:?}: expected the transitions to have succeeded, got {settled:?}"
        );
        assert!(
            settled.last_update_timestamp.is_some(),
            "case {case:?}: expected the second modify to date the configuration's update, \
             got {settled:?}"
        );
    }
}

/// RULE 3: describing the configuration writes nothing, and the configuration is the
/// ACCOUNT's — so repeated reports, in every Region, answer exactly the same words.
#[tokio::test]
async fn repeated_reports_in_any_region_answer_the_same_configuration() {
    for request in &REQUESTS {
        let router = emulator();
        let case = request.case;
        modify(&router, case, REGIONS[2], &request.params()).await;
        // Compared only once the moves are made: a configuration mid-transition is
        // deliberately a function of the clock, which the round-trip test above pins.
        tokio::time::sleep(past_the_transitions()).await;
        let first = describe(&router, case, REGIONS[0]).await;

        for region in REGIONS {
            for round in 0..3 {
                let case = format!("{case}, read {round} in {region}");
                let again = describe(&router, &case, region).await;
                assert_eq!(
                    again, first,
                    "case {case:?}: expected the same configuration as {first:?}, \
                     got {again:?}"
                );
            }
        }
    }
}

/// RULE 4: a value outside a modelled catalog is refused — the modelled enums are refused
/// by the framework's boundary validator, before the handler runs, so the code is the
/// boundary's `ValidationError` — and the refused request records nothing, whichever member
/// carried the value.
#[tokio::test]
async fn a_value_outside_a_modelled_catalog_is_refused_and_records_nothing() {
    let mut cases: Vec<(String, Vec<Param>)> = vec![(
        "a mode outside the modelled catalog".to_string(),
        vec![param("Mode", "attempt-everything")],
    )];
    for (member, _) in EXCLUDABLE {
        cases.push((
            format!("an exclusion state {member} does not take"),
            vec![param("Mode", "attempt-monitor"), param(member, "maybe")],
        ));
    }

    for (case, params) in cases {
        let region = REGIONS[3];
        let router = emulator();
        let before = describe(&router, &case, region).await;

        let (status, body) = call(
            &router,
            region,
            "ModifyAccountVpcEncryptionControl",
            &params,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a value outside the catalog, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected the refusal of a value outside the catalog, got {body}"
        );

        let after = describe(&router, &case, region).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected a refused modify to record nothing, leaving \
             {before:?}, got {after:?}"
        );
    }
}

/// RULE 4, continued: the readings the handler canonicalises its enumerated members
/// through accept exactly their modelled catalog and nothing else — the `None` each answers
/// outside it is what the handler's own refusals key on, behind the boundary refusal above.
#[test]
fn the_enumerated_members_are_read_against_their_modelled_catalogs() {
    use ec2::ops::describe_account_vpc_encryption_control::account_configuration::{
        account_vpc_encryption_control_mode, exclusion_state_input,
    };

    for mode in MODES {
        assert_eq!(
            account_vpc_encryption_control_mode(mode),
            Some(mode),
            "case {mode:?}: expected a mode of the catalog to be read as itself"
        );
    }
    for outside in [
        "attempt-everything",
        "Unmanaged",
        "unmanaged ",
        "",
        "attempt-monitor,attempt-enforce",
    ] {
        assert_eq!(
            account_vpc_encryption_control_mode(outside),
            None,
            "case {outside:?}: expected a value outside the mode catalog to be read as none"
        );
    }

    for (requested, expected) in [("enable", Some(true)), ("disable", Some(false))] {
        assert_eq!(
            exclusion_state_input(requested),
            expected,
            "case {requested:?}: expected the exclusion input to be read as {expected:?}"
        );
    }
    for outside in ["maybe", "Enable", "enabled", "disabled", ""] {
        assert_eq!(
            exclusion_state_input(outside),
            None,
            "case {outside:?}: expected a value outside the exclusion input catalog to be \
             read as none"
        );
    }
}

/// RULE 4, continued: `DryRun` on either operation is answered 412 `DryRunOperation` with
/// the wording EC2 answers a permitted caller, reports nothing and records nothing — and
/// `DryRun=false` is an ordinary call.
#[tokio::test]
async fn a_dry_run_is_refused_and_changes_nothing() {
    for request in &REQUESTS {
        let region = REGIONS[0];
        let router = emulator();
        let case = request.case;
        let before = describe(&router, case, region).await;

        for (action, params) in [
            (
                "DescribeAccountVpcEncryptionControl",
                vec![param("DryRun", "true")],
            ),
            ("ModifyAccountVpcEncryptionControl", {
                let mut params = request.params();
                params.push(param("DryRun", "true"));
                params
            }),
        ] {
            let (status, body) = call(&router, region, action, &params).await;
            assert_eq!(
                status, 412,
                "case {case:?}: expected HTTP 412 from a DryRun {action}, \
                 got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "DryRunOperation",
                "case {case:?}: expected DryRunOperation from {action}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                "Request would have succeeded, but DryRun flag is set.",
                "case {case:?}: expected the wording EC2 answers a permitted caller from \
                 {action}, got {body}"
            );
        }

        let after = describe(&router, case, region).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected a DryRun to leave the configuration at {before:?}, \
             got {after:?}"
        );

        // `DryRun=false` is an ordinary request, so it records what the plain one would.
        let mut params = request.params();
        params.push(param("DryRun", "false"));
        let answered = modify(&router, case, region, &params).await;
        assert_modelled(case, &answered);
        assert_eq!(
            answered.mode.as_deref(),
            Some(request.mode.unwrap_or("unmanaged")),
            "case {case:?}: expected a DryRun=false modify to record the mode it named, \
             got {answered:?}"
        );
        tokio::time::sleep(past_the_transitions()).await;
        let explicit = describe_with(&router, case, region, &[param("DryRun", "false")]).await;
        let plain = describe(&router, case, region).await;
        assert_eq!(
            explicit, plain,
            "case {case:?}: expected DryRun=false to report what a plain describe reports \
             ({plain:?}), got {explicit:?}"
        );
        for (asked_as, member) in EXCLUDABLE {
            let expected = request.settled_exclusion(asked_as);
            assert_eq!(
                plain.exclusion(member),
                Some(expected),
                "case {case:?}: expected {member} {expected:?} after the DryRun=false \
                 modify, got {plain:?}"
            );
        }
    }
}
