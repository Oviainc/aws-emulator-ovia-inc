// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteCustomDBEngineVersion`, the one path a client has to remove
//! a custom DB engine version (CEV) its account minted.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (`crate::db_instances::emulator`), each request is a
//! real awsQuery form POST carrying the SigV4 credential scope the router keys on, and
//! every assertion reads the XML the emulator answered with — so a handler that persisted
//! nothing could not pass them. Every delete is read back through
//! `DescribeDBEngineVersions`, the report a CEV joins.
//!
//! The rules are stated over TABLES spanning what they generalise over rather than over one
//! request:
//!
//! * the delete is driven over the engine FAMILIES a CEV can be created for, each with a
//!   different combination of the optional media members, because the rule is about the
//!   record the account holds and not about the engine it was minted for;
//! * the denial is a row per way a `(Engine, EngineVersion)` can name no CEV the account
//!   holds in the region the request was signed for: a CEV name nothing created, a stored
//!   name under another engine, a CEV of another region, an engine version this service
//!   PUBLISHES rather than one an account minted, an engine no account mints custom
//!   versions of at all, and names at both ends of the length the two members' patterns
//!   admit — every one of them answered with the IDENTICAL sentence, which carries no part
//!   of the request, of the caller's identity or of the account contract, because a denial
//!   discloses nothing about the version it was asked about;
//! * the prerequisite gate is driven over the engine families again for the CEV that has
//!   not finished being created, and over both kinds of record that USE a CEV (an RDS
//!   Custom DB instance and a snapshot taken of one).

use smithy_server_runtime::MultiServiceRouter;

use crate::custom_db_engine_versions::{
    described, strip, KMS_KEY_ID, MANIFEST, STATUS_AVAILABLE, STATUS_CREATING,
};
use crate::db_instances::{
    call_in, emulator, expect_error, expect_ok, optional_text, text, wait_for_settle, REGION,
};

/// The operation under test.
const ACTION: &str = "DeleteCustomDBEngineVersion";

/// The operation that mints the CEVs these tests delete.
const CREATE: &str = "CreateCustomDBEngineVersion";

/// The `Description` every CEV below is created with, so the answer to the delete can be
/// told from a default-filled one.
const DESCRIPTION: &str = "deletable-cev";

/// The account every generated emulator presents. A denial must carry no trace of it.
const ACCOUNT: &str = "000000000000";

/// The principal the caller's credential speaks for: the IAM user this emulator's own
/// long-term credential presents, which is the identity STS reports. A denial must name
/// neither it nor any other caller identity.
const PRINCIPAL: &str = "arn:aws:iam::000000000000:user/emulator";

/// The IAM action a `DeleteCustomDBEngineVersion` request asks for, in this service's own
/// action namespace — another thing a denial must not spell out.
const DELETE_ACTION: &str = "rds:DeleteCustomDBEngineVersion";

/// The sentence a `DeleteCustomDBEngineVersion` this service will not serve is refused
/// with: one fixed text, the same for every such request, with nothing interpolated into
/// it.
///
/// Spelled out here rather than read from the service, so the wording is pinned by the test
/// and not by the code under test.
const DENIAL: &str = "The custom engine version you requested may not exist or you don't \
                      have the required permission.";

/// One `CreateCustomDBEngineVersion` call in `region`, answered with its
/// `<DBEngineVersion>`, `<Image>` stripped.
async fn create_cev(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    engine: &str,
    engine_version: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut params = vec![
        ("Engine".to_string(), engine.to_string()),
        ("EngineVersion".to_string(), engine_version.to_string()),
        ("Description".to_string(), DESCRIPTION.to_string()),
    ];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    let created = expect_ok(case, call_in(router, region, CREATE, &params).await);
    strip(&created, "Image")
}

/// One `DeleteCustomDBEngineVersion` call in `region`.
async fn delete(
    router: &MultiServiceRouter,
    region: &str,
    engine: &str,
    engine_version: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        ACTION,
        &[("Engine", engine), ("EngineVersion", engine_version)],
    )
    .await
}

/// Assert the denial: HTTP 403, the wire code `AccessDenied`, and exactly the [`DENIAL`]
/// sentence — with none of the request, the caller or the account anywhere in it.
fn expect_denied(case: &str, response: (u16, String), region: &str, engine: &str, version: &str) {
    let body = expect_error(case, response, 403, "AccessDenied");
    let message = text(&body, "Message");
    assert_eq!(
        message, DENIAL,
        "case {case:?}: the denial must be the fixed custom-engine-version sentence, got \
         {body}"
    );
    // The equality above already pins the whole text; this scan says WHAT may not appear in
    // it, so a future rewording cannot reintroduce the oracle. Runs of three characters or
    // fewer are skipped: ordinary words in the sentence contain such runs, so they are no
    // evidence of disclosure.
    for leaked in [region, engine, version, PRINCIPAL, ACCOUNT, DELETE_ACTION, "arn:"] {
        if leaked.len() <= 3 {
            continue;
        }
        assert!(
            !message.contains(leaked),
            "case {case:?}: the denial must disclose nothing about the request or the \
             caller, but it carries {leaked:?}: {message}"
        );
    }
}

/// One row of the delete table: a CEV of one engine family, and the members that family's
/// documentation says it is created from.
struct Case {
    /// `Engine`.
    engine: &'static str,
    /// `EngineVersion` — the CEV name, in the form that engine documents.
    engine_version: &'static str,
    /// The create's members beyond `Engine`, `EngineVersion` and `Description`.
    create: &'static [(&'static str, &'static str)],
}

/// The CEVs this suite deletes: one per engine FAMILY `CreateCustomDBEngineVersion$Engine`
/// publishes a build of (RDS Custom for SQL Server and the RDS for SQL Server
/// bring-your-own-media engines), each minted from a different combination of the optional
/// media members, so no assertion below depends on one engine or on one set of members.
const CASES: &[Case] = &[
    Case {
        engine: "sqlserver-se",
        engine_version: "15.00.4435.7.deleted_cev1",
        create: &[
            ("KMSKeyId", KMS_KEY_ID),
            ("Manifest", MANIFEST),
            ("DatabaseInstallationFilesS3BucketName", "my-cev-media"),
        ],
    },
    Case {
        engine: "sqlserver-dev-ee",
        engine_version: "15.00.4385.2.deleted_cev2",
        create: &[("KMSKeyId", KMS_KEY_ID)],
    },
    Case {
        engine: "custom-sqlserver-web",
        engine_version: "15.00.4249.2.deleted_cev3",
        create: &[
            ("KMSKeyId", KMS_KEY_ID),
            ("ImageId", "ami-0abcdef1234567890"),
        ],
    },
    Case {
        engine: "sqlserver-ee",
        engine_version: "16.00.4165.4.deleted_cev4",
        create: &[("Manifest", MANIFEST)],
    },
];

#[tokio::test]
async fn every_engine_family_deletes_the_cev_the_account_holds() {
    let router = emulator();

    for row in CASES {
        let case = format!("{}/{}", row.engine, row.engine_version);
        create_cev(
            &router,
            REGION,
            &case,
            row.engine,
            row.engine_version,
            row.create,
        )
        .await;
        // A CEV is deletable once this service has finished creating it, which is what the
        // clock settles; the refusal before that is its own test below.
        wait_for_settle();

        // The record the report holds right before the delete: the delete answers with the
        // record it removed, so the two have to agree member by member.
        let reported = described(&router, REGION, &case, row.engine, row.engine_version, true)
            .await
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the created CEV must be reported"));

        let answered = strip(
            &expect_ok(&case, delete(&router, REGION, row.engine, row.engine_version).await),
            "Image",
        );
        for member in [
            "Engine",
            "EngineVersion",
            "MajorEngineVersion",
            "DBParameterGroupFamily",
            "DBEngineVersionArn",
            "DBEngineVersionDescription",
            "CreateTime",
            "Status",
        ] {
            assert_eq!(
                optional_text(&answered, member),
                optional_text(&reported, member),
                "case {case:?}: the delete must answer with the record it removed, but \
                 {member} differs: {answered} against {reported}"
            );
        }
        assert_eq!(
            text(&answered, "Status"),
            STATUS_AVAILABLE,
            "case {case:?}: the deleted CEV is answered in the status it was deleted from, \
             got {answered}"
        );
        assert_eq!(
            optional_text(&answered, "CustomDBEngineVersionManifest").as_deref(),
            row.create
                .iter()
                .find(|(member, _)| *member == "Manifest")
                .map(|(_, value)| *value),
            "case {case:?}: the answer carries the manifest the CEV was created with, got \
             {answered}"
        );

        // The record is gone: the report no longer holds it, whatever status is asked for.
        let after = described(&router, REGION, &case, row.engine, row.engine_version, true).await;
        assert!(
            after.is_empty(),
            "case {case:?}: a deleted CEV must not be reported any more, got {after:?}"
        );
        // And the account no longer holds anything of that name, so a second delete is the
        // same denial a name nothing created gets.
        expect_denied(
            &format!("{case}: the second delete"),
            delete(&router, REGION, row.engine, row.engine_version).await,
            REGION,
            row.engine,
            row.engine_version,
        );
    }
}

/// One row of the denial table: a `(Engine, EngineVersion)` that names no CEV the account
/// holds in the region the request is signed for.
struct Unheld {
    case: &'static str,
    region: &'static str,
    engine: &'static str,
    engine_version: &'static str,
}

/// The ways a request can name no CEV the account holds. The CEV the rows are written
/// against is `sqlserver-ee`/`16.00.4175.1.held_cev`, created in [`REGION`].
const UNHELD: &[Unheld] = &[
    Unheld {
        case: "a CEV name nothing created",
        region: REGION,
        engine: "sqlserver-ee",
        engine_version: "16.00.4175.1.never_created_cev",
    },
    Unheld {
        case: "a held CEV name under another engine",
        region: REGION,
        engine: "sqlserver-se",
        engine_version: "16.00.4175.1.held_cev",
    },
    Unheld {
        case: "the CEV of another region",
        region: "eu-west-1",
        engine: "sqlserver-ee",
        engine_version: "16.00.4175.1.held_cev",
    },
    Unheld {
        // `postgres` 16.4 is one of the versions this service publishes as reference data
        // through `DescribeDBEngineVersions`; no account minted it, so no account deletes it.
        case: "an engine version this service publishes rather than one an account minted",
        region: REGION,
        engine: "postgres",
        engine_version: "16.4",
    },
    Unheld {
        // An engine outside `CreateCustomDBEngineVersion$Engine`'s published set: the denial
        // is answered before the engine is looked at at all, so this row reads like the rest.
        case: "an engine no account mints custom versions of",
        region: REGION,
        engine: "mysql",
        engine_version: "8.0.35",
    },
    Unheld {
        // `DeleteCustomDBEngineVersion$EngineVersion` is a `CustomEngineVersion`, pattern
        // ^[a-z0-9_.-]{1,60}$ — this name is at that 60-character maximum, and the denial
        // is the same sentence however long the name asked about is.
        case: "a CEV name at the published sixty-character maximum",
        region: REGION,
        engine: "custom-oracle-ee",
        engine_version: "19.cev_name_at_the_published_sixty_character_maximum_length1",
    },
    Unheld {
        // The other end of both patterns: `CustomEngineName` is ^[A-Za-z0-9-]{1,35}$, so a
        // one-character engine and a one-character CEV name are admitted requests too.
        case: "the shortest engine and CEV name the two patterns admit",
        region: REGION,
        engine: "x",
        engine_version: "0",
    },
    Unheld {
        // `CustomEngineName`'s pattern admits upper case, which this service folds before
        // resolving anything with it; the denial still carries no trace of either form.
        case: "a mixed-case engine name",
        region: REGION,
        engine: "Custom-Oracle-EE",
        engine_version: "19.never_created_cev",
    },
    Unheld {
        // A third region's credential scope, so the row differs from the held CEV's region
        // and from the other unheld region alike.
        case: "a region that holds no CEV at all",
        region: "ap-southeast-2",
        engine: "custom-sqlserver-ee",
        engine_version: "15.00.4249.2.other_region_cev",
    },
];

#[tokio::test]
async fn a_request_that_names_no_held_cev_is_denied() {
    let router = emulator();
    create_cev(
        &router,
        REGION,
        "create the CEV",
        "sqlserver-ee",
        "16.00.4175.1.held_cev",
        &[("KMSKeyId", KMS_KEY_ID), ("Manifest", MANIFEST)],
    )
    .await;
    wait_for_settle();

    for row in UNHELD {
        expect_denied(
            row.case,
            delete(&router, row.region, row.engine, row.engine_version).await,
            row.region,
            row.engine,
            row.engine_version,
        );
    }

    // Every refusal above left the CEV that IS held exactly where it was.
    let held = described(
        &router,
        REGION,
        "the held CEV",
        "sqlserver-ee",
        "16.00.4175.1.held_cev",
        true,
    )
    .await;
    assert_eq!(
        held.len(),
        1,
        "a denied delete must have removed nothing, got {held:?}"
    );
    assert_eq!(
        text(&held[0], "DBEngineVersionDescription"),
        DESCRIPTION,
        "a denied delete must have changed nothing, got {:?}",
        held[0]
    );
}

#[tokio::test]
async fn a_cev_that_is_still_being_created_cannot_be_deleted_yet() {
    let router = emulator();

    for row in CASES {
        let case = format!("{}/{}", row.engine, row.engine_version);
        let created = create_cev(
            &router,
            REGION,
            &case,
            row.engine,
            row.engine_version,
            row.create,
        )
        .await;
        assert_eq!(
            text(&created, "Status"),
            STATUS_CREATING,
            "case {case:?}: a created CEV must report that it is still being created, got \
             {created}"
        );

        // A CEV whose own creation has not finished has reached no availability status, so
        // there is nothing to delete yet: the refusal is the modelled state fault, and the
        // record is left exactly as it was.
        expect_error(
            &case,
            delete(&router, REGION, row.engine, row.engine_version).await,
            400,
            "InvalidCustomDBEngineVersionStateFault",
        );
        let still_there =
            described(&router, REGION, &case, row.engine, row.engine_version, true).await;
        assert_eq!(
            still_there.len(),
            1,
            "case {case:?}: the refused delete must have left the CEV stored, got \
             {still_there:?}"
        );

        // Once the creation settles, the identical request is served.
        wait_for_settle();
        expect_ok(
            &format!("{case}: once it has settled"),
            delete(&router, REGION, row.engine, row.engine_version).await,
        );
    }
}

/// One row of the in-use table: what USES the CEV, and the calls that put it to that use.
struct InUse {
    case: &'static str,
    engine: &'static str,
    engine_version: &'static str,
    instance: &'static str,
    /// Whether a snapshot of that instance is taken, and the instance then deleted — so the
    /// CEV is held by the SNAPSHOT alone when the delete arrives.
    snapshot: Option<&'static str>,
}

/// Both kinds of record this service holds that USE a custom engine version: "the CEV must
/// not be associated with an RDS Custom DB instance, RDS Custom instance snapshot, or
/// automated backup of your RDS Custom instance".
const IN_USE: &[InUse] = &[
    InUse {
        case: "an RDS Custom DB instance running the CEV",
        engine: "sqlserver-ee",
        engine_version: "16.00.4175.1.in_use_cev",
        instance: "emu-cev-delete-instance",
        snapshot: None,
    },
    InUse {
        case: "a snapshot taken of an instance that ran the CEV",
        engine: "sqlserver-se",
        engine_version: "15.00.4435.7.snapshot_cev",
        instance: "emu-cev-delete-snapshotted",
        snapshot: Some("emu-cev-delete-snapshot"),
    },
];

#[tokio::test]
async fn a_cev_in_use_cannot_be_deleted() {
    let router = emulator();

    for row in IN_USE {
        let case = row.case;
        create_cev(
            &router,
            REGION,
            case,
            row.engine,
            row.engine_version,
            &[("KMSKeyId", KMS_KEY_ID), ("Manifest", MANIFEST)],
        )
        .await;
        wait_for_settle();

        expect_ok(
            &format!("{case}: create the instance"),
            call_in(
                &router,
                REGION,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", row.instance),
                    ("Engine", row.engine),
                    ("EngineVersion", row.engine_version),
                    ("DBInstanceClass", "db.m5.large"),
                ],
            )
            .await,
        );
        wait_for_settle();

        if let Some(snapshot) = row.snapshot {
            expect_ok(
                &format!("{case}: snapshot the instance"),
                call_in(
                    &router,
                    REGION,
                    "CreateDBSnapshot",
                    &[
                        ("DBSnapshotIdentifier", snapshot),
                        ("DBInstanceIdentifier", row.instance),
                    ],
                )
                .await,
            );
            // The instance goes away, so what holds the CEV in this row is the snapshot
            // alone. The delete is accepted at once and the instance leaves the store when
            // the removal the clock owes falls due.
            expect_ok(
                &format!("{case}: delete the instance"),
                call_in(
                    &router,
                    REGION,
                    "DeleteDBInstance",
                    &[
                        ("DBInstanceIdentifier", row.instance),
                        ("SkipFinalSnapshot", "true"),
                    ],
                )
                .await,
            );
            wait_for_settle();
        }

        expect_error(
            case,
            delete(&router, REGION, row.engine, row.engine_version).await,
            400,
            "InvalidCustomDBEngineVersionStateFault",
        );
        let still_there =
            described(&router, REGION, case, row.engine, row.engine_version, true).await;
        assert_eq!(
            still_there.len(),
            1,
            "case {case:?}: the refused delete must have left the CEV stored, got \
             {still_there:?}"
        );
        assert_eq!(
            text(&still_there[0], "Status"),
            STATUS_AVAILABLE,
            "case {case:?}: the refused delete must have left the CEV in the status it is \
             used under, got {:?}",
            still_there[0]
        );
    }

    // Releasing the CEV is what makes it deletable: the instance of the first row goes away
    // and nothing else holds that CEV, so the identical delete is then served.
    let row = &IN_USE[0];
    expect_ok(
        "release the CEV",
        call_in(
            &router,
            REGION,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", row.instance),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "the CEV once nothing uses it",
        delete(&router, REGION, row.engine, row.engine_version).await,
    );
    let after = described(
        &router,
        REGION,
        "the released CEV",
        row.engine,
        row.engine_version,
        true,
    )
    .await;
    assert!(
        after.is_empty(),
        "a deleted CEV must not be reported any more, got {after:?}"
    );
}
