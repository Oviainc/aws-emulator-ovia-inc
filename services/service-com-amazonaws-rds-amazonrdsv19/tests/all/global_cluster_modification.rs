// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyGlobalCluster`, and for the two preconditions the
//! `DeleteGlobalCluster` path holds against what a modify left behind.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (through `db_instances::emulator`, the one mounting
//! helper this test target has), and each request is a real awsQuery form POST carrying the
//! SigV4 credential scope the router keys on. The assertions read the XML the emulator
//! answers with, so a handler that computed nothing could not pass them.
//!
//! The rules under test:
//!
//! * a modify writes the settings it names onto the PERSISTED global database and every
//!   later call sees them: a rename moves the record to the new identifier — its ARN, its
//!   endpoint and the identifier its member clusters report follow it, and the old
//!   identifier is then the modelled `GlobalClusterNotFoundFault` — `DeletionProtection`
//!   decides whether the delete is accepted, and `EngineVersion` is resolved through the
//!   engine-version catalogue the create resolves it through;
//! * an engine upgrade is work the service runs, so the global database reports the
//!   transient `modifying` status and the clock owes it `available` a span later, and while
//!   it is in that status another modify and a delete are the modelled
//!   `InvalidGlobalClusterStateFault`; a modify that changes only the global database's own
//!   metadata (its identifier, its deletion protection) has the change in force when it
//!   answers, so the resource stays ready and the next call is accepted;
//! * a request this service refuses persists nothing: an identifier nothing created is the
//!   modelled 404, a rename onto a name the account already holds is the modelled
//!   `GlobalClusterAlreadyExistsFault`, a new identifier outside the rule its member
//!   publishes is `InvalidParameterValue`, and a major-version jump without
//!   `AllowMajorVersionUpgrade` is `InvalidParameterCombination`.
//!
//! The tables below span what those rules generalise over: both engines
//! `CreateGlobalClusterMessage$Engine` publishes, each settable member alone and two
//! together, the identifier in another case, a global database with no member cluster and
//! one with a member, the published 63-character bound of `NewGlobalClusterIdentifier` and
//! one past it, and each shape that member's documented rule refuses.

use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    GLOBAL_CLUSTER_STATUS_AVAILABLE, GLOBAL_CLUSTER_STATUS_MODIFYING, STATUS_TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, text, texts, REGION};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The longest `NewGlobalClusterIdentifier` the member's own rule admits: "Must contain
/// from 1 to 63 letters, numbers, or hyphens."
const MAX_NEW_IDENTIFIER_LEN: usize = 63;

/// A global cluster's ARN, which carries NO region: it is an account-scoped resource.
fn global_cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds::{ACCOUNT}:global-cluster:{identifier}")
}

/// Wait past the instant a status change this service accepted becomes due.
async fn wait_for_settle() {
    tokio::time::sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// `CreateGlobalCluster` with the engine and version a row runs on.
async fn create_global(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    version: &str,
) -> String {
    expect_ok(
        case,
        call(
            router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", identifier),
                ("Engine", engine),
                ("EngineVersion", version),
            ],
        )
        .await,
    )
}

/// `ModifyGlobalCluster` with `extra` beside the identifier it names.
async fn modify(
    router: &MultiServiceRouter,
    identifier: &str,
    extra: &[(&str, String)],
) -> (u16, String) {
    let mut params: Vec<(&str, String)> =
        vec![("GlobalClusterIdentifier", identifier.to_string())];
    params.extend(extra.iter().map(|(name, value)| (*name, value.clone())));
    call(router, "ModifyGlobalCluster", &params).await
}

/// The body `DescribeGlobalClusters` answers for `identifier`, or `None` when the read is
/// the modelled 404.
///
/// The whole body rather than one subtree: the list element and the global cluster's own
/// member list share the `GlobalClusterMember` element name, and a read of one global
/// cluster's own members (`GlobalClusterIdentifier`, `Status`, …) is unambiguous over the
/// body of a single-record report.
async fn described(router: &MultiServiceRouter, identifier: &str) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeGlobalClusters",
        &[("GlobalClusterIdentifier", identifier)],
    )
    .await;
    if status == 404 {
        return None;
    }
    assert_eq!(
        status, 200,
        "reading {identifier} must be a 200 or the modelled 404, got {status} with body {body}"
    );
    assert_eq!(
        texts(&body, "GlobalClusterArn").len(),
        1,
        "reading {identifier} must report exactly one global cluster, got {body}"
    );
    Some(body)
}

/// One row of the metadata-modify table: what the request changes, and what the global
/// database reports afterwards.
struct MetadataCase {
    case: &'static str,
    engine: &'static str,
    /// The version the global database is created on, an entry of the published catalogue.
    version: &'static str,
    identifier: &'static str,
    /// How the modify SPELLS the identifier it names: "this parameter isn't
    /// case-sensitive".
    named_as: &'static str,
    /// The `NewGlobalClusterIdentifier` the modify asks for, if any.
    rename_to: Option<&'static str>,
    /// What the rename is stored and reported as: "this value is stored as a lowercase
    /// string".
    stored_as: &'static str,
    /// The `DeletionProtection` the modify asks for, if any.
    protection: Option<bool>,
}

/// The metadata modifies this suite drives.
///
/// The rows differ along the engine, which settable member the request carries (the
/// identifier alone, the protection alone, both at once), the case the identifier is named
/// in, and the case of the new identifier.
const METADATA_CASES: &[MetadataCase] = &[
    MetadataCase {
        case: "aurora-mysql, renamed",
        engine: "aurora-mysql",
        version: "5.7.mysql_aurora.2.11.6",
        identifier: "emu-mgc-mysql",
        named_as: "emu-mgc-mysql",
        rename_to: Some("emu-mgc-mysql-renamed"),
        stored_as: "emu-mgc-mysql-renamed",
        protection: None,
    },
    MetadataCase {
        case: "aurora-postgresql, deletion protection turned on",
        engine: "aurora-postgresql",
        version: "15.8",
        identifier: "emu-mgc-pg",
        named_as: "EMU-MGC-PG",
        rename_to: None,
        stored_as: "emu-mgc-pg",
        protection: Some(true),
    },
    MetadataCase {
        case: "aurora-postgresql, renamed in MIXED case and protected at once",
        engine: "aurora-postgresql",
        version: "16.6",
        identifier: "emu-mgc-both",
        named_as: "emu-mgc-both",
        rename_to: Some("EMU-MGC-Both-Renamed"),
        stored_as: "emu-mgc-both-renamed",
        protection: Some(true),
    },
];

#[tokio::test]
async fn a_metadata_modify_is_in_force_when_it_answers_and_every_later_call_sees_it() {
    for row in METADATA_CASES {
        let router = emulator();
        create_global(&router, row.case, row.identifier, row.engine, row.version).await;

        let mut extra: Vec<(&str, String)> = Vec::new();
        if let Some(new_identifier) = row.rename_to {
            extra.push(("NewGlobalClusterIdentifier", new_identifier.to_string()));
        }
        if let Some(protection) = row.protection {
            extra.push(("DeletionProtection", protection.to_string()));
        }
        let modified = expect_ok(row.case, modify(&router, row.named_as, &extra).await);

        // 1. The answer reports the global database as the modify left it: the stored
        //    identifier, the ARN and endpoint built from it, and the protection asked for.
        //    Nothing is being provisioned for a metadata change, so it is READY.
        for (member, expected) in [
            ("GlobalClusterIdentifier", row.stored_as.to_string()),
            ("GlobalClusterArn", global_cluster_arn(row.stored_as)),
            ("Status", GLOBAL_CLUSTER_STATUS_AVAILABLE.to_string()),
            (
                "DeletionProtection",
                row.protection.unwrap_or(false).to_string(),
            ),
            ("EngineVersion", row.version.to_string()),
        ] {
            let actual = text(&modified, member);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {member} {expected:?}, got {actual:?}",
                row.case
            );
        }
        assert!(
            text(&modified, "Endpoint").starts_with(&format!("{}.", row.stored_as)),
            "case {:?}: the endpoint must follow the identifier the modify left, got {}",
            row.case,
            text(&modified, "Endpoint")
        );

        // 2. The report reads the same record back under the stored identifier, and the
        //    identifier a rename moved away from names nothing any more.
        let read_back = described(&router, row.stored_as)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the modified record must be reported", row.case));
        for member in [
            "GlobalClusterIdentifier",
            "GlobalClusterArn",
            "Status",
            "DeletionProtection",
            "EngineVersion",
            "GlobalClusterResourceId",
        ] {
            assert_eq!(
                text(&read_back, member),
                text(&modified, member),
                "case {:?}: the report and the modify must agree on {member}, got {read_back}",
                row.case
            );
        }
        if row.rename_to.is_some() {
            assert!(
                described(&router, row.identifier).await.is_none(),
                "case {:?}: the identifier the rename moved away from must not be found",
                row.case
            );
        }

        // 3. `DeletionProtection` decides the delete: while it is on the delete is refused
        //    and the record stays, and a modify that turns it off is accepted straight
        //    after the first one — a metadata change leaves nothing in flight.
        if row.protection == Some(true) {
            expect_error(
                row.case,
                call(
                    &router,
                    "DeleteGlobalCluster",
                    &[("GlobalClusterIdentifier", row.stored_as)],
                )
                .await,
                400,
                "InvalidParameterCombination",
            );
            assert!(
                described(&router, row.stored_as).await.is_some(),
                "case {:?}: a refused delete must leave the global database in place",
                row.case
            );
            expect_ok(
                row.case,
                modify(
                    &router,
                    row.stored_as,
                    &[("DeletionProtection", "false".to_string())],
                )
                .await,
            );
        }
        let deleted = expect_ok(
            row.case,
            call(
                &router,
                "DeleteGlobalCluster",
                &[("GlobalClusterIdentifier", row.stored_as)],
            )
            .await,
        );
        assert_eq!(
            text(&deleted, "GlobalClusterArn"),
            global_cluster_arn(row.stored_as),
            "case {:?}: the delete must answer with the global cluster it removed, got {deleted}",
            row.case
        );
        assert!(
            described(&router, row.stored_as).await.is_none(),
            "case {:?}: the deleted global database must not be found",
            row.case
        );
    }
}

#[tokio::test]
async fn a_rename_re_points_the_member_clusters_at_the_identifier_it_left() {
    let router = emulator();
    create_global(
        &router,
        "the global database",
        "emu-mgc-members",
        "aurora-mysql",
        "8.0.mysql_aurora.3.10.3",
    )
    .await;
    expect_ok(
        "the member cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-mgc-member-cluster"),
                ("Engine", "aurora-mysql"),
                ("EngineVersion", "8.0.mysql_aurora.3.10.3"),
                ("GlobalClusterIdentifier", "emu-mgc-members"),
            ],
        )
        .await,
    );

    let renamed = expect_ok(
        "the rename",
        modify(
            &router,
            "emu-mgc-members",
            &[(
                "NewGlobalClusterIdentifier",
                "emu-mgc-members-renamed".to_string(),
            )],
        )
        .await,
    );
    // The members move with the record: the global database still reports the one member
    // cluster, and the cluster reports the identifier the global database now has, so
    // neither record names a resource the other does not hold.
    assert_eq!(
        texts(&renamed, "DBClusterArn"),
        vec![format!(
            "arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-mgc-member-cluster"
        )],
        "the rename must keep the global database's member, got {renamed}"
    );
    let member = expect_ok(
        "the member after the rename",
        call(
            &router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "emu-mgc-member-cluster")],
        )
        .await,
    );
    assert_eq!(
        text(&member, "GlobalClusterIdentifier"),
        "emu-mgc-members-renamed",
        "the member must report the identifier the global database was renamed to, got {member}"
    );

    // And the delete of a global database that still holds that member is refused, naming
    // it by its ARN, as the state fault's own sentence does.
    let refused = expect_error(
        "a delete with a member attached",
        call(
            &router,
            "DeleteGlobalCluster",
            &[("GlobalClusterIdentifier", "emu-mgc-members-renamed")],
        )
        .await,
        400,
        "InvalidGlobalClusterStateFault",
    );
    assert_eq!(
        text(&refused, "Message"),
        format!(
            "Global Cluster {} is not empty",
            global_cluster_arn("emu-mgc-members-renamed")
        ),
        "the refusal must be the fault's own sentence about the global cluster, got {refused}"
    );
}

#[tokio::test]
async fn an_engine_upgrade_runs_for_a_span_and_the_requests_that_status_refuses_are_refused() {
    // The rows differ along the engine and the form the requested version takes: an exact
    // catalogue version of the same major version, and a bare major version the catalogue
    // resolves to that major version's own default.
    // (case, engine, created on, requested, the version the upgrade lands on)
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        (
            "aurora-mysql, an exact later build of the same major version",
            "aurora-mysql",
            "8.0.mysql_aurora.3.05.2",
            "8.0.mysql_aurora.3.08.0",
            "8.0.mysql_aurora.3.08.0",
        ),
        (
            "aurora-postgresql, a bare major version",
            "aurora-postgresql",
            "16.4",
            "16",
            "16.6",
        ),
    ];

    for &(case, engine, created_on, requested, landed_on) in rows {
        let router = emulator();
        let identifier = "emu-mgc-upgrade";
        create_global(&router, case, identifier, engine, created_on).await;

        let upgrading = expect_ok(
            case,
            modify(
                &router,
                identifier,
                &[("EngineVersion", requested.to_string())],
            )
            .await,
        );
        // The upgrade is accepted, not finished: the global database reports the version it
        // is moving to and the transient status, which is the state its own fault names.
        assert_eq!(
            text(&upgrading, "EngineVersion"),
            landed_on,
            "case {case:?}: the upgrade must resolve the version through the catalogue, got \
             {upgrading}"
        );
        assert_eq!(
            text(&upgrading, "Status"),
            GLOBAL_CLUSTER_STATUS_MODIFYING,
            "case {case:?}: an engine upgrade is work, so the global database reports the \
             transient status, got {upgrading}"
        );
        let polled = described(&router, identifier)
            .await
            .expect("the global database is still reported while it upgrades");
        assert_eq!(
            text(&polled, "Status"),
            GLOBAL_CLUSTER_STATUS_MODIFYING,
            "case {case:?}: a read inside the span sees the upgrade running, got {polled}"
        );
        // While it runs, another modify and a delete are the modelled state fault, and
        // neither changes anything.
        for action in ["ModifyGlobalCluster", "DeleteGlobalCluster"] {
            expect_error(
                case,
                call(
                    &router,
                    action,
                    &[
                        ("GlobalClusterIdentifier", identifier),
                        ("DeletionProtection", "true"),
                    ],
                )
                .await,
                400,
                "InvalidGlobalClusterStateFault",
            );
        }
        assert_eq!(
            text(
                &described(&router, identifier)
                    .await
                    .expect("still reported"),
                "DeletionProtection"
            ),
            "false",
            "case {case:?}: a refused modify must change nothing"
        );

        // Once the span the clock owes has passed, the global database is available on the
        // version it was upgraded to and both calls are accepted again.
        wait_for_settle().await;
        let settled = described(&router, identifier)
            .await
            .expect("the upgraded global database is reported");
        assert_eq!(
            text(&settled, "Status"),
            GLOBAL_CLUSTER_STATUS_AVAILABLE,
            "case {case:?}: the clock owes the global database its available status, got \
             {settled}"
        );
        assert_eq!(
            text(&settled, "EngineVersion"),
            landed_on,
            "case {case:?}: the upgraded version must persist, got {settled}"
        );
        expect_ok(
            case,
            call(
                &router,
                "DeleteGlobalCluster",
                &[("GlobalClusterIdentifier", identifier)],
            )
            .await,
        );
    }
}

/// One row of the refusal table: the case, the identifier the modify names, the request
/// members it carries, and the HTTP status and wire error code it is refused with.
type RefusalRow = (&'static str, &'static str, Params, u16, &'static str);

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

#[tokio::test]
async fn a_modify_this_service_refuses_persists_nothing() {
    let router = emulator();
    create_global(
        &router,
        "the global database",
        "emu-mgc-refused",
        "aurora-mysql",
        "8.0.mysql_aurora.3.08.0",
    )
    .await;
    create_global(
        &router,
        "the name a rename collides with",
        "emu-mgc-taken",
        "aurora-mysql",
        "8.0.mysql_aurora.3.08.0",
    )
    .await;

    // The longest new identifier the rule admits, and one character past it.
    let longest = format!("e{}", "a".repeat(MAX_NEW_IDENTIFIER_LEN - 1));
    let too_long = format!("e{}", "a".repeat(MAX_NEW_IDENTIFIER_LEN));

    // (case, the identifier named, the members carried, status, code)
    let refusals: &[RefusalRow] = &[
        (
            "an identifier nothing created",
            "emu-mgc-absent",
            vec![("DeletionProtection", "true".to_string())],
            404,
            "GlobalClusterNotFoundFault",
        ),
        (
            "a rename onto a name the account already holds",
            "emu-mgc-refused",
            vec![("NewGlobalClusterIdentifier", "emu-mgc-taken".to_string())],
            400,
            "GlobalClusterAlreadyExistsFault",
        ),
        (
            "a new identifier that does not begin with a letter",
            "emu-mgc-refused",
            vec![("NewGlobalClusterIdentifier", "1-emu-mgc".to_string())],
            400,
            "InvalidParameterValue",
        ),
        (
            "a new identifier that ends with a hyphen",
            "emu-mgc-refused",
            vec![("NewGlobalClusterIdentifier", "emu-mgc-".to_string())],
            400,
            "InvalidParameterValue",
        ),
        (
            "a new identifier with two consecutive hyphens",
            "emu-mgc-refused",
            vec![("NewGlobalClusterIdentifier", "emu--mgc".to_string())],
            400,
            "InvalidParameterValue",
        ),
        (
            "a new identifier one character past the published bound",
            "emu-mgc-refused",
            vec![("NewGlobalClusterIdentifier", too_long.clone())],
            400,
            "InvalidParameterValue",
        ),
        (
            "a major version jump without AllowMajorVersionUpgrade",
            "emu-mgc-refused",
            vec![("EngineVersion", "5.7.mysql_aurora.2.11.6".to_string())],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, named, extra, status, code) in refusals {
        expect_error(case, modify(&router, named, extra).await, *status, code);
        // The record the refused request named is exactly as it was.
        let unchanged = described(&router, "emu-mgc-refused")
            .await
            .expect("the global database is still reported");
        for (member, expected) in [
            ("GlobalClusterIdentifier", "emu-mgc-refused"),
            ("EngineVersion", "8.0.mysql_aurora.3.08.0"),
            ("DeletionProtection", "false"),
            ("Status", GLOBAL_CLUSTER_STATUS_AVAILABLE),
        ] {
            assert_eq!(
                text(&unchanged, member),
                expected,
                "case {case:?}: a refused modify must leave {member} as it was, got {unchanged}"
            );
        }
    }

    // The new identifier AT the published bound is admitted, which is what makes the row
    // past it a bound rather than a blanket refusal — and a major version jump IS accepted
    // once `AllowMajorVersionUpgrade` enables it.
    expect_ok(
        "the longest new identifier the rule admits",
        modify(
            &router,
            "emu-mgc-refused",
            &[("NewGlobalClusterIdentifier", longest.clone())],
        )
        .await,
    );
    let jumped = expect_ok(
        "a major version jump the request allows",
        modify(
            &router,
            &longest,
            &[
                ("EngineVersion", "5.7.mysql_aurora.2.11.6".to_string()),
                ("AllowMajorVersionUpgrade", "true".to_string()),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&jumped, "EngineVersion"),
        "5.7.mysql_aurora.2.11.6",
        "an allowed major version jump must be applied, got {jumped}"
    );
}
