// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of this service's refusals.
//!
//! The rule under test: a refusal of this front door carries the message the API itself
//! words for that class of refusal — its fixed text exactly, with only the parts the
//! request or the service's own state supply interpolated. Each class below is asserted
//! over a TABLE spanning what the rule generalises over (the clause of the rule that was
//! broken, the member it was broken on, the operation that answers it), never over one
//! request:
//!
//! * an identifier outside the identifier rule is refused with the ONE sentence pair that
//!   names the member and states the whole rule, echoing nothing of the value;
//! * a `SourceType` that names no event source type is refused with the sentence that names
//!   every source type the front door accepts;
//! * a `DBSubnetGroupName` already taken is refused with the sentence about that name;
//! * a `DBParameterGroupFamily` that names no family is refused with the sentence that names
//!   the parameter and the value, and enumerates nothing;
//! * a parameter name the family does not publish is refused as a parameter that could not
//!   be found;
//! * a reset blocked by an unapplied change is refused with the sentence that names the
//!   PARAMETER it is waiting on.
//!
//! Every call goes over the wire through the harness in `cluster_lifecycle.rs`, so the
//! assertions read the XML the emulator itself answered with.

use service_com_amazonaws_docdb_amazonrdsv19::state;

use crate::cluster_lifecycle::{text, Docdb};

/// A DocumentDB cluster parameter group family, which publishes a parameter catalog.
const FAMILY: &str = "docdb5.0";

/// Two subnets of the 17-character id form: the smallest list a subnet group can be made
/// of.
const TWO_SUBNETS: [(&str, &str); 2] = [
    ("SubnetIds.SubnetIdentifier.1", "subnet-0123456789abcdef0"),
    ("SubnetIds.SubnetIdentifier.2", "subnet-0fedcba987654321f"),
];

/// Call `action` and require a refusal with `status` and `<Code>`; answer the `<Message>`.
async fn refusal_message(
    docdb: &Docdb,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
    status: u16,
    code: &str,
) -> String {
    let reply = docdb.call(action, params).await;
    assert_eq!(
        (reply.status, text(&reply.xml, "Code").as_str()),
        (status, code),
        "case {case:?}: expected {action} to answer {status} {code}, got {} with {}",
        reply.status,
        reply.xml
    );
    text(&reply.xml, "Message")
}

/// Call `action` and require a refusal at `status`, answering the `<Message>`, without
/// saying which `<Code>` it carries.
///
/// The identifier refusals below are read this way because this suite is about the WORDING
/// of a refusal: the error channel those refusals travel on is the one
/// `cluster_lifecycle.rs` pins for every input rule of this service view, and the two must
/// not be pinned in two places.
async fn refusal_message_at(
    docdb: &Docdb,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
    status: u16,
) -> String {
    let reply = docdb.call(action, params).await;
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected {action} to answer {status}, got {} with {}",
        reply.status, reply.xml
    );
    text(&reply.xml, "Message")
}

/// One identifier refusal: the operation that answers it, the member at fault, the value
/// that breaks the rule and the rest of a request that would otherwise be accepted.
struct IdentifierCase {
    case: &'static str,
    action: &'static str,
    member: &'static str,
    value: &'static str,
    rest: &'static [(&'static str, &'static str)],
}

/// The rows span the clauses of the identifier rule — a leading digit, a character outside
/// the alphabet, a trailing hyphen, two consecutive hyphens and the empty value — across
/// three operations and three identifier members, so the sentence is the one answer to the
/// whole rule rather than to one clause of it.
const IDENTIFIER_CASES: &[IdentifierCase] = &[
    IdentifierCase {
        case: "a cluster identifier starting with a digit",
        action: "CreateDBCluster",
        member: "DBClusterIdentifier",
        value: "1emu-wording-cluster",
        rest: &[("Engine", "docdb")],
    },
    IdentifierCase {
        case: "a cluster identifier carrying an underscore",
        action: "CreateDBCluster",
        member: "DBClusterIdentifier",
        value: "emu_wording_cluster",
        rest: &[("Engine", "docdb")],
    },
    IdentifierCase {
        case: "a cluster identifier ending in a hyphen",
        action: "CreateDBCluster",
        member: "DBClusterIdentifier",
        value: "emu-wording-cluster-",
        rest: &[("Engine", "docdb")],
    },
    IdentifierCase {
        case: "a cluster identifier with two consecutive hyphens",
        action: "CreateDBCluster",
        member: "DBClusterIdentifier",
        value: "emu--wording-cluster",
        rest: &[("Engine", "docdb")],
    },
    IdentifierCase {
        case: "an empty cluster identifier",
        action: "CreateDBCluster",
        member: "DBClusterIdentifier",
        value: "",
        rest: &[("Engine", "docdb")],
    },
    IdentifierCase {
        case: "an instance identifier outside the alphabet",
        action: "CreateDBInstance",
        member: "DBInstanceIdentifier",
        value: "emu.wording.instance",
        rest: &[
            ("DBClusterIdentifier", "emu-wording-host"),
            ("DBInstanceClass", "db.r5.large"),
            ("Engine", "docdb"),
        ],
    },
    IdentifierCase {
        case: "a final snapshot identifier starting with a hyphen",
        action: "DeleteDBCluster",
        member: "FinalDBSnapshotIdentifier",
        value: "-emu-wording-snap",
        rest: &[
            ("DBClusterIdentifier", "emu-wording-host"),
            ("SkipFinalSnapshot", "false"),
        ],
    },
];

#[tokio::test]
async fn an_identifier_outside_the_rule_is_refused_with_the_fixed_sentence() {
    let docdb = Docdb::new();

    for row in IDENTIFIER_CASES {
        let mut params: Vec<(&str, &str)> = vec![(row.member, row.value)];
        params.extend(row.rest.iter().copied());
        let message = refusal_message_at(&docdb, row.case, row.action, &params, 400).await;
        let expected = state::not_a_valid_identifier_message(row.member);
        assert_eq!(
            message, expected,
            "case {:?}: expected {expected:?}, got {message:?}",
            row.case
        );
        // The sentence states the rule; it never echoes the value that broke it.
        assert!(
            row.value.is_empty() || !message.contains(row.value),
            "case {:?}: the sentence must not echo the rejected value, got {message:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_event_source_type_the_front_door_does_not_accept_names_the_ones_it_does() {
    let docdb = Docdb::new();

    // Values spanning the ways a `SourceType` can name no event source type: a name nothing
    // publishes, a published name in the wrong case, a near miss of a published one, and the
    // empty string.
    for value in [
        "not-a-source-type",
        "DB-Instance",
        "db-instances",
        "rds-cluster",
        "",
    ] {
        let case = format!("SourceType {value:?}");
        let message = refusal_message(
            &docdb,
            &case,
            "DescribeEventCategories",
            &[("SourceType", value)],
            400,
            "InvalidParameterValue",
        )
        .await;
        assert_eq!(
            message,
            state::invalid_event_source_type_message(),
            "case {case:?}: expected the fixed sentence, got {message:?}"
        );
        // Every source type the front door accepts is named, single-quoted, in the order it
        // names them — including the ones no category map is published for.
        let mut at = 0;
        for source_type in ::service_common::rds_event_source_types::EVENT_SOURCE_TYPES {
            let quoted = format!("'{source_type}'");
            let found = message[at..].find(&quoted).unwrap_or_else(|| {
                panic!("case {case:?}: expected the sentence to name {quoted} next, got {message:?}")
            });
            at += found + quoted.len();
        }
        assert!(
            value.is_empty()
                || ::service_common::rds_event_source_types::EVENT_SOURCE_TYPES.contains(&value)
                || !message.contains(value),
            "case {case:?}: the sentence must not echo the rejected value, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_subnet_group_name_already_taken_is_refused_as_the_api_words_it() {
    let docdb = Docdb::new();

    // The sentence is about the name the REQUEST asked for, so it quotes that name in the
    // spelling the request carried, whichever case the group was created under.
    for (case, created_as, taken_as) in [
        ("the same name", "emu-wording-subnets", "emu-wording-subnets"),
        (
            "the same name in another case",
            "emu-wording-subnets-2",
            "EMU-Wording-Subnets-2",
        ),
    ] {
        let mut params: Vec<(&str, &str)> = vec![
            ("DBSubnetGroupName", created_as),
            ("DBSubnetGroupDescription", "the group under test"),
        ];
        params.extend(TWO_SUBNETS);
        docdb.ok(case, "CreateDBSubnetGroup", &params).await;

        let mut again: Vec<(&str, &str)> = vec![
            ("DBSubnetGroupName", taken_as),
            ("DBSubnetGroupDescription", "the group under test"),
        ];
        again.extend(TWO_SUBNETS);
        let message = refusal_message(
            &docdb,
            case,
            "CreateDBSubnetGroup",
            &again,
            400,
            "DBSubnetGroupAlreadyExists",
        )
        .await;
        let expected = format!("The DB subnet group '{taken_as}' already exists.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_parameter_group_family_the_engine_does_not_publish_is_refused_naming_it() {
    let docdb = Docdb::new();

    // Rows spanning the forms a `docdb`-prefixed family can take without naming a family
    // the engine has: a version it never published, a version past the published ones, a
    // malformed version, the bare engine name, and one in another case.
    for (index, family) in [
        "docdb1.0",
        "docdb9.9",
        "docdb5",
        "docdb",
        "DocDB4.5",
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("emu-wording-family-{index}");
        let message = refusal_message(
            &docdb,
            family,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", name.as_str()),
                ("DBParameterGroupFamily", family),
                ("Description", "the group under test"),
            ],
            400,
            "InvalidParameterValue",
        )
        .await;
        let expected = format!("ParameterGroupFamily {family} is not a valid parameter group family");
        assert_eq!(
            message, expected,
            "case {family:?}: expected {expected:?}, got {message:?}"
        );
    }

    // Every family the engine DOES publish is accepted, and so is another engine's family:
    // this catalog publishes nothing for one of those, and refusing it would invent a
    // constraint. The published list is the catalog's own, never a list written here.
    for (index, family) in state::DOCDB_PARAMETER_GROUP_FAMILIES
        .iter()
        .copied()
        .chain(["aurora-postgresql15", "neptune1.3"])
        .enumerate()
    {
        let name = format!("emu-wording-ok-family-{index}");
        docdb
            .ok(
                family,
                "CreateDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", name.as_str()),
                    ("DBParameterGroupFamily", family),
                    ("Description", "the group under test"),
                ],
            )
            .await;
    }
}

#[tokio::test]
async fn a_parameter_the_family_does_not_publish_is_refused_as_not_found() {
    let docdb = Docdb::new();

    let group = "emu-wording-parameters";
    docdb
        .ok(
            "the group under test",
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", group),
                ("DBParameterGroupFamily", FAMILY),
                ("Description", "the group under test"),
            ],
        )
        .await;

    // Names spanning the ways a parameter name can name nothing the family publishes: a
    // name nothing has, a published name in the wrong case, a near miss of a published one,
    // and a published name of another engine's catalog.
    let published = state::published_parameter(FAMILY, "tls").expect("the family publishes tls");
    for name in [
        "not_a_real_parameter",
        "TLS",
        "tls_version",
        "max_connections",
    ] {
        // Both calls that name a parameter answer the same refusal: the value half of the
        // rule does not apply to a reset, but the name half does.
        for (action, params) in [
            (
                "ModifyDBClusterParameterGroup",
                vec![
                    ("DBClusterParameterGroupName", group),
                    ("Parameters.Parameter.1.ParameterName", name),
                    ("Parameters.Parameter.1.ParameterValue", "enabled"),
                ],
            ),
            (
                "ResetDBClusterParameterGroup",
                vec![
                    ("DBClusterParameterGroupName", group),
                    ("Parameters.Parameter.1.ParameterName", name),
                ],
            ),
        ] {
            let case = format!("{action} naming {name:?}");
            let message = refusal_message(
                &docdb,
                &case,
                action,
                &params,
                400,
                "InvalidParameterValue",
            )
            .await;
            let expected = format!("Could not find parameter with name: {name}");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // A value outside the ones a published parameter admits is the OTHER refusal: it names
    // the value, the parameter and the values that parameter publishes.
    let message = refusal_message(
        &docdb,
        "a value outside the published set",
        "ModifyDBClusterParameterGroup",
        &[
            ("DBClusterParameterGroupName", group),
            ("Parameters.Parameter.1.ParameterName", "tls"),
            ("Parameters.Parameter.1.ParameterValue", "not-a-tls-value"),
        ],
        400,
        "InvalidParameterValue",
    )
    .await;
    let expected = format!(
        "Invalid parameter value: not-a-tls-value for: tls allowed values are: {}",
        published.allowed_values
    );
    assert_eq!(
        message, expected,
        "case \"a value outside the published set\": expected {expected:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_reset_blocked_by_an_unapplied_change_names_the_parameter() {
    let docdb = Docdb::new();

    let group = "emu-wording-pending";
    docdb
        .ok(
            "the group under test",
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", group),
                ("DBParameterGroupFamily", FAMILY),
                ("Description", "the group under test"),
            ],
        )
        .await;

    // Both ways a change becomes pending: the caller asks for the reboot, or the parameter's
    // own apply type requires one. The refusal names the parameter either way.
    for (case, name, value, apply_method) in [
        (
            "a dynamic change the caller deferred to the reboot",
            "profiler",
            "enabled",
            Some(state::APPLY_METHOD_PENDING_REBOOT),
        ),
        (
            "a static parameter, which can only take effect on a reboot",
            "tls",
            "disabled",
            None,
        ),
    ] {
        let mut params: Vec<(&str, &str)> = vec![
            ("DBClusterParameterGroupName", group),
            ("Parameters.Parameter.1.ParameterName", name),
            ("Parameters.Parameter.1.ParameterValue", value),
        ];
        if let Some(apply_method) = apply_method {
            params.push(("Parameters.Parameter.1.ApplyMethod", apply_method));
        }
        docdb
            .ok(case, "ModifyDBClusterParameterGroup", &params)
            .await;

        // Whichever parameter the group is waiting on, the sentence names THAT parameter
        // and says nothing about the group or about a reboot.
        for (reset_case, reset_params) in [
            (
                format!("{case}: a reset of everything"),
                vec![
                    ("DBClusterParameterGroupName", group),
                    ("ResetAllParameters", "true"),
                ],
            ),
            (
                format!("{case}: a reset of that parameter"),
                vec![
                    ("DBClusterParameterGroupName", group),
                    ("Parameters.Parameter.1.ParameterName", name),
                ],
            ),
        ] {
            let message = refusal_message(
                &docdb,
                &reset_case,
                "ResetDBClusterParameterGroup",
                &reset_params,
                400,
                "InvalidDBParameterGroupState",
            )
            .await;
            let expected = state::parameter_has_pending_changes_message(name);
            assert_eq!(
                message, expected,
                "case {reset_case:?}: expected {expected:?}, got {message:?}"
            );
        }

        // Clear the pending change the direct way the service offers: a reboot of the
        // cluster that carries the group is out of scope here, so the group is replaced.
        docdb
            .ok(
                case,
                "DeleteDBClusterParameterGroup",
                &[("DBClusterParameterGroupName", group)],
            )
            .await;
        docdb
            .ok(
                case,
                "CreateDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", group),
                    ("DBParameterGroupFamily", FAMILY),
                    ("Description", "the group under test"),
                ],
            )
            .await;
    }
}
