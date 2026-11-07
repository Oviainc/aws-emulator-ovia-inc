// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal an operation answers when one request is both malformed and names a
//! resource this account does not hold.
//!
//! The rule under test: an operation checks the identifier it was given against the length
//! bound it states for that member BEFORE it resolves the resource, so a value no resource
//! of this service could carry is refused 400 `InvalidParameterValue` in that operation's
//! own words — the member and the bound are the only variable parts of those words and both
//! come from the rule, so no identifier from the request appears in them. The bound is each
//! operation's OWN: one of the rows below states none, resolves the identifier however long
//! it is and reports the resource as absent, which is why the bound and its sentence sit
//! with the operations that state them. The one operation that builds a resource AROUND
//! another resolves that dependency first, so its missing primary is reported 404 before
//! anything is said about the name the new resource would get.
//!
//! The tests are tables. The rows differ along what the rule generalises over — which
//! operation answers, which member carries the identifier, which bound that operation
//! states, how far past the bound the value is, and (for the datastore create) which way
//! the suffix is malformed — and each row is checked at the bound as well as past it, so
//! the id the recorded probe happened to use is one row of many.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, using the harness in [`crate::harness`]; nothing reaches into the service's state
//! struct.

use service_com_amazonaws_elasticache_amazonelasticachev9::service::identifier_length_rules;
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, PAST_TRANSITION};

/// What an identifier AT an operation's bound is answered with: the bound did not refuse
/// it, so the answer is about the RESOURCE the identifier names.
#[derive(Debug, Clone, Copy)]
enum AtBound {
    /// The resource was looked up and not held, and this is the HTTP status and awsQuery
    /// code that operation reports that with.
    NotHeld(u16, &'static str),
    /// The create accepted it: an id at the bound is one the service stores.
    Stored,
}

/// One row: an operation that resolves an identifier, the bound it states for it, and the
/// sentence it refuses a longer one with.
#[derive(Debug, Clone, Copy)]
struct IdentifierCase {
    case: &'static str,
    action: &'static str,
    /// The member carrying the identifier under test.
    member: &'static str,
    /// The rest of the request, so the call reaches the identifier rule rather than being
    /// refused for something else it is missing.
    extra: &'static [(&'static str, &'static str)],
    bound: usize,
    /// The WHOLE message a longer identifier is refused with.
    refusal: &'static str,
    at_bound: AtBound,
}

/// The axis: which operation resolves the identifier, which member carries it, which bound
/// that operation states (the replication group's 40, the cluster's 50, the user group's
/// 40) and which of the sentences this service words that bound in it answers.
const IDENTIFIER_CASES: &[IdentifierCase] = &[
    IdentifierCase {
        case: "a settings change to a replication group",
        action: "ModifyReplicationGroup",
        member: "ReplicationGroupId",
        extra: &[("ApplyImmediately", "true")],
        bound: 40,
        refusal: "The parameter ReplicationGroupId is not a valid identifier because it is \
                  longer than 40 characters.",
        at_bound: AtBound::NotHeld(404, "ReplicationGroupNotFoundFault"),
    },
    IdentifierCase {
        case: "a failover test",
        action: "TestFailover",
        member: "ReplicationGroupId",
        extra: &[("NodeGroupId", "0001")],
        bound: 40,
        refusal: "The parameter ReplicationGroupId is not a valid identifier because it is \
                  longer than 40 characters.",
        at_bound: AtBound::NotHeld(404, "ReplicationGroupNotFoundFault"),
    },
    IdentifierCase {
        case: "a replica count decrease",
        action: "DecreaseReplicaCount",
        member: "ReplicationGroupId",
        extra: &[("ApplyImmediately", "true"), ("NewReplicaCount", "0")],
        bound: 40,
        refusal: "Replication Group ID cannot be longer than 40 characters.",
        at_bound: AtBound::NotHeld(400, "InvalidParameterValue"),
    },
    IdentifierCase {
        case: "a migration into a replication group",
        action: "StartMigration",
        member: "ReplicationGroupId",
        extra: &[
            ("CustomerNodeEndpointList.member.1.Address", "10.0.0.1"),
            ("CustomerNodeEndpointList.member.1.Port", "6379"),
        ],
        bound: 50,
        refusal: "Cluster ID cannot be longer than 50 characters.",
        at_bound: AtBound::NotHeld(404, "ReplicationGroupNotFoundFault"),
    },
    IdentifierCase {
        case: "a membership change to a user group",
        action: "ModifyUserGroup",
        member: "UserGroupId",
        extra: &[("UserIdsToAdd.member.1", "default")],
        bound: 40,
        refusal: "User group id length must be less than 40.",
        at_bound: AtBound::NotHeld(404, "UserGroupNotFound"),
    },
    IdentifierCase {
        case: "a user group delete",
        action: "DeleteUserGroup",
        member: "UserGroupId",
        extra: &[],
        bound: 40,
        refusal: "User group id length must be less than 40.",
        at_bound: AtBound::NotHeld(404, "UserGroupNotFound"),
    },
    IdentifierCase {
        case: "the create that would store the id",
        action: "CreateUserGroup",
        member: "UserGroupId",
        extra: &[("Engine", "redis"), ("UserIds.member.1", "default")],
        bound: 40,
        refusal: "User group id length must be less than 40.",
        at_bound: AtBound::Stored,
    },
];

/// A well-formed identifier of exactly `length` characters: it begins with a letter and
/// carries only letters, so the only thing that can be wrong with it is its length.
fn identifier(length: usize) -> String {
    format!("g{}", "a".repeat(length - 1))
}

#[tokio::test]
async fn an_identifier_past_an_operations_bound_is_refused_before_the_lookup() {
    // Where each row's bound comes from: the documented maximum of the member the operation
    // states it for, as this service publishes it. A row that drifted from one of these is
    // a defect, which is why the numbers are checked here and not just written in the
    // table.
    assert_eq!(
        state::REPLICATION_GROUP_ID_MAX_LEN, 40,
        "ReplicationGroupId is documented as at most 40 characters, got {}",
        state::REPLICATION_GROUP_ID_MAX_LEN
    );
    assert_eq!(
        state::CLUSTER_ID_MAX_LEN, 50,
        "CacheClusterId is documented as at most 50 characters, got {}",
        state::CLUSTER_ID_MAX_LEN
    );
    assert_eq!(
        identifier_length_rules::MAX_USER_GROUP_ID_LEN, 40,
        "UserGroupId is documented as at most 40 characters, got {}",
        identifier_length_rules::MAX_USER_GROUP_ID_LEN
    );

    for case in IDENTIFIER_CASES {
        let emulator = Emulator::new();
        // The row's own name, read off the row, so every assertion below says which
        // operation and which member it is about.
        let name = case.case;
        let at_bound = identifier(case.bound);
        // Past the bound by one and by a long way: the sentence states the rule, so it is
        // the same sentence for both and carries neither value.
        let past = [
            ("one character past the bound", identifier(case.bound + 1)),
            ("far past the bound", identifier(case.bound + 17)),
        ];

        for (how, value) in &past {
            let refused = {
                let mut request = params(&[(case.member, value.as_str())]);
                request.extend(params(case.extra));
                emulator.call(case.action, &request).await
            };
            assert_eq!(
                (refused.status, refused.error_code()),
                (400, "InvalidParameterValue"),
                "case {name} / {how}: expected 400 InvalidParameterValue, got {} {} {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.error_message(),
                case.refusal,
                "case {name} / {how}: expected the length rule, got {:?}",
                refused.error_message()
            );
            assert!(
                !refused.error_message().contains(value.as_str()),
                "case {name} / {how}: the rule carries no request value, got {:?}",
                refused.error_message()
            );
        }

        // At the bound the rule does not fire, so the answer is about the RESOURCE the id
        // names — which is what makes the rows above a refusal of the VALUE rather than a
        // report that the resource is absent.
        let answered = {
            let mut request = params(&[(case.member, at_bound.as_str())]);
            request.extend(params(case.extra));
            emulator.call(case.action, &request).await
        };
        match case.at_bound {
            AtBound::NotHeld(status, code) => {
                assert_eq!(
                    (answered.status, answered.error_code()),
                    (status, code),
                    "case {name} / at the bound: expected {status} {code}, got {} {} {}",
                    answered.status,
                    answered.error_code(),
                    answered.body
                );
                assert_ne!(
                    answered.error_message(),
                    case.refusal,
                    "case {name} / at the bound: the lookup must have run, got {:?}",
                    answered.error_message()
                );
            }
            AtBound::Stored => {
                assert_eq!(
                    answered.status, 200,
                    "case {name} / at the bound: the create must store it, got {} {}",
                    answered.status, answered.body
                );
                // Read back through the implemented reader: the id at the bound is one the
                // store holds and every later call can resolve.
                let described = emulator
                    .call(
                        "DescribeUserGroups",
                        &params(&[("UserGroupId", at_bound.as_str())]),
                    )
                    .await;
                assert_eq!(
                    described.status, 200,
                    "case {name} / at the bound: the stored group must be readable, got {} {}",
                    described.status, described.body
                );
                assert_eq!(
                    described.text("UserGroupId"),
                    Some(at_bound.as_str()),
                    "case {name} / at the bound: the read must report the stored id, got {:?}",
                    described.text("UserGroupId")
                );
                // And the refused ids stored nothing: a create the rule refused leaves no
                // group behind for a later read to find.
                for (how, value) in &past {
                    let missing = emulator
                        .call("DescribeUserGroups", &params(&[("UserGroupId", value.as_str())]))
                        .await;
                    assert_eq!(
                        (missing.status, missing.error_code()),
                        (404, "UserGroupNotFound"),
                        "case {name} / {how}: a refused create stores nothing, got {} {}",
                        missing.status,
                        missing.body
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn the_replica_count_increase_states_non_existence_for_the_same_identifier() {
    // The bound is each operation's OWN: the sibling of the decrease above states none, and
    // answers the existence diagnosis with the id the request named — so the rule must not
    // be carried across to it however long the id is. The rows span the bound its sibling
    // states, one character past it, and far past it.
    let emulator = Emulator::new();
    for length in [
        state::REPLICATION_GROUP_ID_MAX_LEN,
        state::REPLICATION_GROUP_ID_MAX_LEN + 1,
        state::REPLICATION_GROUP_ID_MAX_LEN + 17,
    ] {
        let id = identifier(length);
        let refused = emulator
            .call(
                "IncreaseReplicaCount",
                &params(&[
                    ("ReplicationGroupId", id.as_str()),
                    ("ApplyImmediately", "true"),
                    ("NewReplicaCount", "1"),
                ]),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "InvalidParameterValue"),
            "case an id of {length} characters: expected 400 InvalidParameterValue, got {} {} {}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            format!("Replication group {id} does not exist."),
            "case an id of {length} characters: expected the existence diagnosis, got {:?}",
            refused.error_message()
        );
    }
}

/// Create one replication group over the wire, asserting the create itself was accepted.
async fn create_replication_group(emulator: &Emulator, id: &str) {
    let created = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", id),
                ("ReplicationGroupDescription", "the datastore primary"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the primary {id} must be creatable, got {} {}",
        created.status, created.body
    );
}

/// One malformed-suffix row: HOW the suffix is wrong, the suffix itself, and the whole
/// message the create refuses it with once the primary has been resolved.
#[derive(Debug, Clone)]
struct SuffixCase {
    case: &'static str,
    suffix: String,
    refusal: String,
}

/// A suffix nothing is wrong with, so the only thing a create naming it can be refused for
/// is the resource it names.
const WELL_FORMED_SUFFIX: &str = "well-formed";

#[tokio::test]
async fn the_datastore_create_resolves_its_primary_before_it_judges_the_suffix() {
    let emulator = Emulator::new();
    // The bound a suffix gets is what is left of a group identifier after the prefix the
    // service mints and its hyphen, so the row at and past it is derived from the published
    // constants rather than written out.
    let suffix_bound = state::REPLICATION_GROUP_ID_MAX_LEN - state::GLOBAL_ID_PREFIX_LEN - 1;
    let too_long = format!(
        "Global Replication Group ID Suffix cannot be longer than {suffix_bound} characters."
    );
    // The other half of the identifier rule: the FORM every identifier of this service
    // shares, stated once by the service for whichever member broke it.
    let malformed = "The parameter GlobalReplicationGroupIdSuffix is not a valid identifier. \
                     Identifiers must begin with a letter; must contain only ASCII letters, \
                     digits, and hyphens; and must not end with a hyphen or contain two \
                     consecutive hyphens."
        .to_string();
    // The axis: WHICH way the suffix is wrong — one character past the bound, far past it,
    // and each shape the identifier form refuses (a leading digit, a trailing hyphen, two
    // consecutive hyphens).
    let suffixes = [
        SuffixCase {
            case: "one character past the bound",
            suffix: identifier(suffix_bound + 1),
            refusal: too_long.clone(),
        },
        SuffixCase {
            case: "far past the bound",
            suffix: identifier(suffix_bound + 11),
            refusal: too_long.clone(),
        },
        SuffixCase {
            case: "a leading digit",
            suffix: "1-datastore".to_string(),
            refusal: malformed.clone(),
        },
        SuffixCase {
            case: "a trailing hyphen",
            suffix: "datastore-".to_string(),
            refusal: malformed.clone(),
        },
        SuffixCase {
            case: "two consecutive hyphens",
            suffix: "data--store".to_string(),
            refusal: malformed.clone(),
        },
    ];

    let held = "datastore-primary";
    create_replication_group(&emulator, held).await;

    for (case, suffix) in suffixes
        .iter()
        .map(|row| (row.case, row.suffix.as_str()))
        .chain([("a suffix nothing is wrong with", WELL_FORMED_SUFFIX)])
    {
        // A primary this account does not hold is what the create reports FIRST, whatever
        // is wrong with the suffix: the datastore is built around that group, so the
        // dependency it cannot resolve comes before the name of the resource it would
        // create. The id is reported as the request spelled it, case included.
        for primary in ["no-such-primary", "Mixed-Case-Absent", "p"] {
            let refused = emulator
                .call(
                    "CreateGlobalReplicationGroup",
                    &params(&[
                        ("GlobalReplicationGroupIdSuffix", suffix),
                        ("PrimaryReplicationGroupId", primary),
                    ]),
                )
                .await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (404, "ReplicationGroupNotFoundFault"),
                "case {case} / primary {primary}: expected 404 ReplicationGroupNotFoundFault, \
                 got {} {} {}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.error_message(),
                format!("Replication group '{primary}' not found"),
                "case {case} / primary {primary}: expected the not-found template, got {:?}",
                refused.error_message()
            );
        }
    }

    // The same malformed suffixes against the primary this account DOES hold: the primary
    // resolves, so now the suffix is what the answer is about — and the suffix rule runs
    // ahead of every gate the group's own state imposes, so the answer does not depend on
    // whether the clock has settled the primary yet.
    for row in &suffixes {
        let answered = emulator
            .call(
                "CreateGlobalReplicationGroup",
                &params(&[
                    ("GlobalReplicationGroupIdSuffix", row.suffix.as_str()),
                    ("PrimaryReplicationGroupId", held),
                ]),
            )
            .await;
        assert_eq!(
            (answered.status, answered.error_code()),
            (400, "InvalidParameterValue"),
            "case {}: expected 400 InvalidParameterValue, got {} {} {}",
            row.case,
            answered.status,
            answered.error_code(),
            answered.body
        );
        assert_eq!(
            answered.error_message(),
            row.refusal.as_str(),
            "case {}: expected the suffix rule, got {:?}",
            row.case,
            answered.error_message()
        );
    }

    // And once the primary has settled, the well-formed suffix is accepted and the
    // datastore it names is readable: the reordering refuses nothing it used to serve.
    std::thread::sleep(PAST_TRANSITION);
    let created = emulator
        .call(
            "CreateGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupIdSuffix", "settled-create"),
                ("PrimaryReplicationGroupId", held),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the create over a settled primary must be accepted, got {} {}",
        created.status, created.body
    );
    let stored = created
        .text("GlobalReplicationGroupId")
        .expect("the create reports the stored datastore id")
        .to_string();
    let described = emulator
        .call(
            "DescribeGlobalReplicationGroups",
            &params(&[("GlobalReplicationGroupId", stored.as_str())]),
        )
        .await;
    assert_eq!(
        described.status, 200,
        "the stored datastore must be readable, got {} {}",
        described.status, described.body
    );
    assert_eq!(
        described.text("GlobalReplicationGroupId"),
        Some(stored.as_str()),
        "the read must report the datastore the create stored, got {:?}",
        described.text("GlobalReplicationGroupId")
    );
}
