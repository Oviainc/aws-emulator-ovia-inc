// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SENTENCE the create paths of this front door refuse a bad
//! parameter with: the identifier a DB instance or a DB cluster is created under, and the
//! engine a global database cluster is created on.
//!
//! Two rules, each asserted over a TABLE spanning what it generalises over rather than over
//! one request:
//!
//! 1. An identifier outside the identifier grammar is refused by the MEMBER that carried it
//!    and, where that member splits them, by the CLAUSE the value broke:
//!    * a value that is no identifier TOKEN of its member at all — it does not BEGIN WITH A
//!      LETTER, it carries a character outside the letters, digits and hyphens an identifier
//!      is written in, or it is longer than the member's own bound — is refused by the
//!      subject Amazon RDS names that member by, echoing the value exactly as the request
//!      spelled it (`Invalid database identifier:  <value>` for a DB instance's,
//!      `Invalid database cluster identifier:  <value>` for a DB cluster's);
//!    * a DB INSTANCE identifier of that shape which still breaks one of the two HYPHEN
//!      rules (a trailing hyphen, two consecutive hyphens) is refused instead with the one
//!      sentence pair that names the member at fault and states the whole rule, which echoes
//!      NOTHING the request carried — while a DB CLUSTER identifier keeps its subject there
//!      too, which is how the two create paths differ.
//!
//!    Both wordings are the Query API's `InvalidParameterValue` at HTTP 400.
//! 2. A global database cluster's `Engine` / `EngineVersion` that no global database can run
//!    is refused with ONE fixed sentence, which interpolates no request value at all and
//!    names neither the value rejected nor the engines the service would have taken — and
//!    the engines a global database CAN run are still created and read back.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each
//! assertion reads the XML this emulator itself answered with.

use crate::db_instances::{call, emulator, expect_error, expect_ok, text};

/// The Query API error code every refusal below carries: a value outside what its own
/// member accepts.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The sentence pair Amazon RDS refuses a DB instance identifier with where the value is a
/// token of the member's shape and breaks one of the two hyphen rules. Spelled out here so
/// the wording is pinned by the test rather than by the code under test.
fn states_the_rule(member: &str) -> String {
    format!(
        "The parameter {member} is not a valid identifier. Identifiers must begin with a \
         letter; must contain only ASCII letters, digits, and hyphens; and must not end with \
         a hyphen or contain two consecutive hyphens."
    )
}

/// The one sentence a global database cluster's engine refusal carries — fixed text, with
/// no terminal period and nothing from the request in it.
const GLOBAL_ENGINE_REFUSAL: &str =
    "The requested engine version was not found or does not support global functionality";

/// One create path that names an identifier: the operation, the member it carries the
/// identifier in, the subject Amazon RDS names that member by, and the rest of the members
/// the call needs.
struct IdentifierPath {
    action: &'static str,
    member: &'static str,
    subject: &'static str,
    rest: &'static [(&'static str, &'static str)],
    /// The longest identifier the member admits on this path, as its own constraints state
    /// it ("Must contain from 1 to 63 letters, numbers, or hyphens" for a DB instance and
    /// for an Aurora DB cluster).
    max_len: usize,
    /// Whether this member states the RULE for a token that breaks a hyphen rule instead of
    /// answering its subject there, which is what the DB instance identifier does and the DB
    /// cluster identifier does not.
    states_the_rule_for_a_hyphen_rule: bool,
}

/// The two create paths this rule is about: the DB instance's and the DB cluster's, the two
/// members this front door words by a subject of their own.
const IDENTIFIER_PATHS: &[IdentifierPath] = &[
    IdentifierPath {
        action: "CreateDBInstance",
        member: "DBInstanceIdentifier",
        subject: "database identifier",
        rest: &[("Engine", "postgres"), ("DBInstanceClass", "db.m5.large")],
        max_len: 63,
        states_the_rule_for_a_hyphen_rule: true,
    },
    IdentifierPath {
        action: "CreateDBCluster",
        member: "DBClusterIdentifier",
        subject: "database cluster identifier",
        rest: &[("Engine", "aurora-mysql")],
        max_len: 63,
        states_the_rule_for_a_hyphen_rule: false,
    },
];

/// One value under test: which clause of the grammar it breaks, and whether that clause is
/// one of the two HYPHEN rules — the clauses the DB instance identifier states the rule for.
struct ValueCase {
    case: &'static str,
    value: String,
    hyphen_rule: bool,
}

/// The values the table runs over, spanning every clause of the grammar in both directions
/// of the split: six that are no identifier token of their member at all (a leading digit, a
/// leading hyphen, a mixed-case value whose case has to survive the echo, two characters
/// outside the alphabet, and one character past the member's own bound) and two that are
/// tokens breaking a hyphen rule (a trailing hyphen and two consecutive hyphens).
fn value_cases(max_len: usize) -> Vec<ValueCase> {
    vec![
        ValueCase {
            case: "a leading digit",
            value: "1emu-create-wording".to_string(),
            hyphen_rule: false,
        },
        ValueCase {
            case: "a leading hyphen",
            value: "-emu-create-wording".to_string(),
            hyphen_rule: false,
        },
        ValueCase {
            case: "a mixed-case value with a leading digit",
            value: "9EMU-Create-Wording".to_string(),
            hyphen_rule: false,
        },
        ValueCase {
            case: "an underscore, outside the alphabet",
            value: "emu_create_wording".to_string(),
            hyphen_rule: false,
        },
        ValueCase {
            case: "a multi-byte character",
            value: "emu-create-café".to_string(),
            hyphen_rule: false,
        },
        ValueCase {
            case: "one character past the length bound",
            value: format!("e{}", "a".repeat(max_len)),
            hyphen_rule: false,
        },
        ValueCase {
            case: "a trailing hyphen",
            value: "emu-create-wording-".to_string(),
            hyphen_rule: true,
        },
        ValueCase {
            case: "two consecutive hyphens",
            value: "emu--create-wording".to_string(),
            hyphen_rule: true,
        },
    ]
}

#[tokio::test]
async fn an_identifier_refusal_is_worded_by_the_clause_it_broke_and_the_member_it_names() {
    let router = emulator();

    for path in IDENTIFIER_PATHS {
        for row in value_cases(path.max_len) {
            let case = format!("{} with {}", path.action, row.case);
            let mut params: Vec<(&str, &str)> = vec![(path.member, row.value.as_str())];
            params.extend(path.rest.iter().copied());
            let body = expect_error(
                &case,
                call(&router, path.action, &params).await,
                400,
                INVALID_PARAMETER_VALUE,
            );
            let message = text(&body, "Message");
            if row.hyphen_rule && path.states_the_rule_for_a_hyphen_rule {
                let expected = states_the_rule(path.member);
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
                assert!(
                    !message.contains(row.value.as_str()),
                    "case {case:?}: the refusal must echo nothing the request carried, got \
                     {message:?}"
                );
            } else {
                // The two spaces after the colon are part of that wording.
                let expected = format!("Invalid {}:  {}", path.subject, row.value);
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
            }
        }

        // The same member, a value inside the grammar: the path still serves it, so the
        // refusals above are the clause's doing and not a guard refusing everything.
        let accepted = format!("emu-create-wording-{}", path.member.to_ascii_lowercase());
        let mut params: Vec<(&str, &str)> = vec![(path.member, accepted.as_str())];
        params.extend(path.rest.iter().copied());
        expect_ok(
            &format!("{} with a well-formed identifier", path.action),
            call(&router, path.action, &params).await,
        );
    }
}

/// One global-cluster create under test: what it names, and the member that names it.
struct GlobalEngineCase {
    case: &'static str,
    engine: &'static str,
    engine_version: Option<&'static str>,
}

/// The rows of the global-cluster table.
///
/// `CreateGlobalCluster$Engine` publishes the engines a global database can run ("Valid
/// Values: aurora-mysql | aurora-postgresql"), so the rows span what lies outside that
/// list: the RDS engines with a cluster form, the two sibling control planes' engines this
/// one front door also serves, the legacy Aurora engine name, a published value in the
/// wrong case, a value that names no engine at all — and, on an engine that IS in the list,
/// an `EngineVersion` that is no engine version at all.
const GLOBAL_ENGINE_CASES: &[GlobalEngineCase] = &[
    GlobalEngineCase {
        case: "a Multi-AZ DB cluster engine",
        engine: "mysql",
        engine_version: None,
    },
    GlobalEngineCase {
        case: "the other Multi-AZ DB cluster engine",
        engine: "postgres",
        engine_version: None,
    },
    GlobalEngineCase {
        case: "the Amazon DocumentDB engine",
        engine: "docdb",
        engine_version: None,
    },
    GlobalEngineCase {
        case: "the Amazon Neptune engine",
        engine: "neptune",
        engine_version: None,
    },
    GlobalEngineCase {
        case: "the legacy Aurora engine name",
        engine: "aurora",
        engine_version: None,
    },
    GlobalEngineCase {
        case: "a published engine in the wrong case",
        engine: "Aurora-MySQL",
        engine_version: None,
    },
    GlobalEngineCase {
        case: "a value that names no engine at all",
        engine: "not-an-engine",
        engine_version: None,
    },
    GlobalEngineCase {
        case: "an engine that can run a global database, with a version that is none of it",
        engine: "aurora-postgresql",
        engine_version: Some("not-a-version"),
    },
];

#[tokio::test]
async fn a_global_cluster_engine_refusal_carries_one_fixed_sentence() {
    let router = emulator();

    for (index, row) in GLOBAL_ENGINE_CASES.iter().enumerate() {
        let identifier = format!("emu-global-wording-{index}");
        let mut params: Vec<(&str, &str)> = vec![
            ("GlobalClusterIdentifier", identifier.as_str()),
            ("Engine", row.engine),
        ];
        if let Some(version) = row.engine_version {
            params.push(("EngineVersion", version));
        }
        let body = expect_error(
            row.case,
            call(&router, "CreateGlobalCluster", &params).await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, GLOBAL_ENGINE_REFUSAL,
            "case {:?}: expected {GLOBAL_ENGINE_REFUSAL:?}, got {message:?}",
            row.case
        );
        // The sentence is value-free: neither what the request named nor the engines this
        // service would have taken appear in it.
        for leaked in [row.engine, row.engine_version.unwrap_or("aurora-mysql")] {
            assert!(
                !message.contains(leaked),
                "case {:?}: the refusal must carry no request value, got {message:?}",
                row.case
            );
        }
        // The refused create stored nothing: the global cluster it would have created is
        // not there to be reported.
        expect_error(
            row.case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", identifier.as_str())],
            )
            .await,
            404,
            "GlobalClusterNotFoundFault",
        );
    }

    // Both engines the member publishes are created and read back, so the refusals above
    // are the value's doing rather than this path refusing every create.
    for (index, engine) in ["aurora-mysql", "aurora-postgresql"].iter().enumerate() {
        let identifier = format!("emu-global-ok-{index}");
        expect_ok(
            engine,
            call(
                &router,
                "CreateGlobalCluster",
                &[
                    ("GlobalClusterIdentifier", identifier.as_str()),
                    ("Engine", engine),
                ],
            )
            .await,
        );
        let report = expect_ok(
            engine,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", identifier.as_str())],
            )
            .await,
        );
        assert_eq!(
            text(&report, "GlobalClusterIdentifier"),
            identifier,
            "case {engine:?}: the created global cluster must be reported back, got {report}"
        );
        assert_eq!(
            text(&report, "Engine"),
            *engine,
            "case {engine:?}: the reported engine must be the one created, got {report}"
        );
    }
}
