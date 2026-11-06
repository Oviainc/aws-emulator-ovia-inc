// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyCustomDBEngineVersion`, the one path a client has over a
//! custom DB engine version (CEV) it already created.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (`crate::db_instances::emulator`), each request is a
//! real awsQuery form POST carrying the SigV4 credential scope the router keys on, and
//! every assertion reads the XML the emulator answered with — so a handler that persisted
//! nothing could not pass them. Every change is read back through
//! `DescribeDBEngineVersions`, the report a CEV joins.
//!
//! The rules are stated over TABLES spanning what they generalise over rather than over one
//! request:
//!
//! * the refusal while a CEV is still being created is driven over the engine FAMILIES a
//!   CEV can be created for and over every combination of the two members the operation
//!   takes, because the rule is about the CEV's state and not about what the request asks
//!   to change;
//! * the status changes are driven over every ORDERED PAIR of the statuses the modelled
//!   `CustomEngineVersionStatus` enum publishes, which is what "you can change any status
//!   to any status" ranges over, the identity pairs included;
//! * the denials are a row per way a `(Engine, EngineVersion)` can name no CEV the account
//!   holds: an unknown CEV name, a stored name under another engine, a CEV stored in
//!   another region, and an engine version this service PUBLISHES rather than one an
//!   account minted;
//! * the report gate is driven over every status one CEV passes through — the status it is
//!   created in and each value the enum publishes — because what `IncludeAll` withholds is
//!   the engine versions a caller cannot use.

use service_com_amazonaws_rds_amazonrdsv19::ops::types::CUSTOM_ENGINE_VERSION_STATUS_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::custom_db_engine_versions::{
    described, strip, KMS_KEY_ID, MANIFEST, STATUS_AVAILABLE, STATUS_CREATING,
};
use crate::db_instances::{
    call_in, emulator, expect_error, expect_ok, optional_text, text, wait_for_settle, REGION,
};

/// The operation under test.
const ACTION: &str = "ModifyCustomDBEngineVersion";

/// The operation that mints the CEVs these tests modify.
const CREATE: &str = "CreateCustomDBEngineVersion";

/// The `Description` every CEV below is created with, so a refusal that changed nothing is
/// observable. Written without spaces because the test harness posts form values unescaped.
const ORIGINAL_DESCRIPTION: &str = "original-description";

/// Create one CEV in `region` and return the `<DBEngineVersion>` the create answered with,
/// its `<Image>` stripped.
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
        ("Description".to_string(), ORIGINAL_DESCRIPTION.to_string()),
    ];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    let created = expect_ok(case, call_in(router, region, CREATE, &params).await);
    strip(&created, "Image")
}

/// One `ModifyCustomDBEngineVersion` call in `region`.
async fn modify(
    router: &MultiServiceRouter,
    region: &str,
    engine: &str,
    engine_version: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![
        ("Engine".to_string(), engine.to_string()),
        ("EngineVersion".to_string(), engine_version.to_string()),
    ];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    call_in(router, region, ACTION, &params).await
}

/// The one `<DBEngineVersion>` the report holds for `(engine, engine_version)`, with its
/// `<Image>` stripped. `IncludeAll` is set, so the entry is read whatever status the CEV is
/// in and this helper never hides a record the modify was supposed to have left alone.
async fn stored(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    engine: &str,
    engine_version: &str,
) -> String {
    let entries = described(router, region, case, engine, engine_version, true).await;
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: exactly one CEV must be stored for {engine}/{engine_version}, got \
         {entries:?}"
    );
    entries.into_iter().next().expect("one entry, checked above")
}

/// One row of the refusal table: a CEV of one engine family, the members its create needs,
/// and the modification attempted against it while it is still being created.
struct Attempt {
    /// `Engine`.
    engine: &'static str,
    /// `EngineVersion` — the CEV name, in the form that engine documents.
    engine_version: &'static str,
    /// The create's members beyond `Engine`, `EngineVersion` and `Description`.
    create: &'static [(&'static str, &'static str)],
    /// What the modify asks to change.
    modify: &'static [(&'static str, &'static str)],
}

/// The modifications refused while the CEV is still being created.
///
/// One row per engine FAMILY the operation serves (the RDS Custom for SQL Server engines
/// and the RDS for SQL Server bring-your-own-media engines), each attempting a
/// DIFFERENT combination of the two members `ModifyCustomDBEngineVersion` takes — the
/// mutable `Description` alone, a `Status` alone, both together, and neither — because the
/// rule is about the state of the CEV rather than about what is being asked for.
const ATTEMPTS: &[Attempt] = &[
    Attempt {
        engine: "sqlserver-ee",
        engine_version: "16.00.4175.1.immutable_cev1",
        create: &[("KMSKeyId", KMS_KEY_ID), ("Manifest", MANIFEST)],
        modify: &[("Description", "changed-while-creating")],
    },
    Attempt {
        engine: "sqlserver-dev-ee",
        engine_version: "16.00.4175.1.immutable_cev2",
        create: &[("KMSKeyId", KMS_KEY_ID)],
        modify: &[("Status", "inactive")],
    },
    Attempt {
        engine: "custom-sqlserver-web",
        engine_version: "15.00.4249.2.immutable_cev3",
        create: &[
            ("KMSKeyId", KMS_KEY_ID),
            ("ImageId", "ami-0abcdef1234567890"),
        ],
        modify: &[
            ("Description", "changed-while-creating"),
            ("Status", "inactive-except-restore"),
        ],
    },
    Attempt {
        engine: "sqlserver-ee",
        engine_version: "16.00.4165.4.immutable_cev4",
        create: &[("Manifest", MANIFEST)],
        modify: &[],
    },
];

#[tokio::test]
async fn a_cev_that_is_still_being_created_cannot_be_modified() {
    let router = emulator();

    for attempt in ATTEMPTS {
        let case = format!("{}/{}", attempt.engine, attempt.engine_version);
        let created = create_cev(
            &router,
            REGION,
            &case,
            attempt.engine,
            attempt.engine_version,
            attempt.create,
        )
        .await;
        // The create is what puts the CEV in a state nothing can be modified in: its own
        // answer says so, so the refusal below is explained by a status the caller can see.
        assert_eq!(
            text(&created, "Status"),
            STATUS_CREATING,
            "case {case:?}: a created CEV must report that it is still being created, got \
             {created}"
        );

        // A CEV that has not reached an availability status is in none of the statuses this
        // operation moves a CEV between, so every modification of it is refused — even one
        // that only touches the mutable description.
        expect_error(
            &case,
            modify(
                &router,
                REGION,
                attempt.engine,
                attempt.engine_version,
                attempt.modify,
            )
            .await,
            400,
            "InvalidCustomDBEngineVersionStateFault",
        );

        // A refused modify changes nothing: the stored description is still the one the
        // create gave the CEV.
        let entry = stored(
            &router,
            REGION,
            &case,
            attempt.engine,
            attempt.engine_version,
        )
        .await;
        assert_eq!(
            text(&entry, "DBEngineVersionDescription"),
            ORIGINAL_DESCRIPTION,
            "case {case:?}: the refused modify must have left the description alone, got \
             {entry}"
        );
    }
}

#[tokio::test]
async fn a_settled_cev_moves_between_every_published_status() {
    let router = emulator();
    let engine = "sqlserver-ee";
    let engine_version = "16.00.4175.1.status_cev";
    let created = create_cev(
        &router,
        REGION,
        "create the CEV",
        engine,
        engine_version,
        &[("KMSKeyId", KMS_KEY_ID), ("Manifest", MANIFEST)],
    )
    .await;
    let arn = text(&created, "DBEngineVersionArn");
    let create_time = text(&created, "CreateTime");

    // Nothing sleeps inside the emulator: the CEV reaches `available` on its own, and only
    // then is it a CEV this operation can change.
    wait_for_settle();

    // "You can change any status to any status": every ordered pair of the statuses the
    // modelled `CustomEngineVersionStatus` enum publishes, the identity pairs included.
    // The source of the list is the model itself, not a copy of it.
    for from in CUSTOM_ENGINE_VERSION_STATUS_VALUES {
        for to in CUSTOM_ENGINE_VERSION_STATUS_VALUES {
            let case = format!("{from} -> {to}");
            let settled = expect_ok(
                &format!("{case}: reach {from}"),
                modify(&router, REGION, engine, engine_version, &[("Status", from)]).await,
            );
            assert_eq!(
                text(&strip(&settled, "Image"), "Status"),
                *from,
                "case {case:?}: the CEV must first report the status it was moved to, got \
                 {settled}"
            );

            let answered = strip(
                &expect_ok(
                    &case,
                    modify(&router, REGION, engine, engine_version, &[("Status", to)]).await,
                ),
                "Image",
            );
            assert_eq!(
                text(&answered, "Status"),
                *to,
                "case {case:?}: the modify must answer with the status it was given, got \
                 {answered}"
            );
            // The core attributes of a CEV are fixed when it is created, so a status change
            // moves nothing else.
            assert_eq!(
                text(&answered, "DBEngineVersionArn"),
                arn,
                "case {case:?}: a modified CEV is the same record, so its ARN must not move, \
                 got {answered}"
            );
            assert_eq!(
                text(&answered, "CreateTime"),
                create_time,
                "case {case:?}: a modified CEV keeps the creation time of the record, got \
                 {answered}"
            );
            assert_eq!(
                optional_text(&answered, "CustomDBEngineVersionManifest").as_deref(),
                Some(MANIFEST),
                "case {case:?}: a status change must not touch the stored manifest, got \
                 {answered}"
            );
            assert_eq!(
                text(&answered, "DBEngineVersionDescription"),
                ORIGINAL_DESCRIPTION,
                "case {case:?}: a request that names no Description must leave the stored \
                 one alone, got {answered}"
            );

            // The record is what the report reads, so the status the modify answered with
            // has to be the one a later caller sees.
            let entry = stored(&router, REGION, &case, engine, engine_version).await;
            assert_eq!(
                text(&entry, "Status"),
                *to,
                "case {case:?}: the reported status must be the stored one, got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn each_member_the_request_names_is_the_only_one_that_changes() {
    let router = emulator();
    let engine = "sqlserver-se";
    let engine_version = "16.00.4175.1.described_cev";
    create_cev(
        &router,
        REGION,
        "create the CEV",
        engine,
        engine_version,
        &[("KMSKeyId", KMS_KEY_ID), ("Manifest", MANIFEST)],
    )
    .await;
    wait_for_settle();

    /// One row: what a modify names, and the description and status the CEV carries after
    /// it. The rows run in order against the one CEV, so each row's expectation is the
    /// previous row's state plus what this row asked for.
    struct Row {
        case: &'static str,
        params: &'static [(&'static str, &'static str)],
        description: &'static str,
        status: &'static str,
    }

    let rows = [
        Row {
            case: "neither member",
            params: &[],
            description: ORIGINAL_DESCRIPTION,
            status: STATUS_AVAILABLE,
        },
        Row {
            case: "the description alone",
            params: &[("Description", "edited-description")],
            description: "edited-description",
            status: STATUS_AVAILABLE,
        },
        Row {
            case: "the status alone",
            params: &[("Status", "inactive")],
            description: "edited-description",
            status: "inactive",
        },
        Row {
            case: "both members at once",
            params: &[
                ("Description", "edited-again"),
                ("Status", "inactive-except-restore"),
            ],
            description: "edited-again",
            status: "inactive-except-restore",
        },
        Row {
            case: "the description alone, back to an available CEV",
            params: &[("Status", STATUS_AVAILABLE)],
            description: "edited-again",
            status: STATUS_AVAILABLE,
        },
    ];

    for row in rows {
        let case = row.case;
        let answered = strip(
            &expect_ok(
                case,
                modify(&router, REGION, engine, engine_version, row.params).await,
            ),
            "Image",
        );
        assert_eq!(
            text(&answered, "DBEngineVersionDescription"),
            row.description,
            "case {case:?}: the description must be the one the requests named, got {answered}"
        );
        assert_eq!(
            text(&answered, "Status"),
            row.status,
            "case {case:?}: the status must be the one the requests named, got {answered}"
        );

        let entry = stored(&router, REGION, case, engine, engine_version).await;
        assert_eq!(
            text(&entry, "DBEngineVersionDescription"),
            row.description,
            "case {case:?}: the reported description must be the stored one, got {entry}"
        );
        assert_eq!(
            text(&entry, "Status"),
            row.status,
            "case {case:?}: the reported status must be the stored one, got {entry}"
        );
    }
}

/// One row of the denial table: a `(Engine, EngineVersion)` that names no CEV the
/// account holds in the region the request is signed for.
struct Missing {
    case: &'static str,
    region: &'static str,
    engine: &'static str,
    engine_version: &'static str,
}

/// The ways a request can name no held CEV. The CEV the rows are written against is
/// `sqlserver-ee`/`16.00.4175.1.found_cev`, created in [`REGION`].
const MISSING: &[Missing] = &[
    Missing {
        case: "a CEV name nothing created",
        region: REGION,
        engine: "sqlserver-ee",
        engine_version: "16.00.4175.1.never_created_cev",
    },
    Missing {
        case: "a stored CEV name under another engine",
        region: REGION,
        engine: "sqlserver-se",
        engine_version: "16.00.4175.1.found_cev",
    },
    Missing {
        case: "the CEV of another region",
        region: "eu-west-1",
        engine: "sqlserver-ee",
        engine_version: "16.00.4175.1.found_cev",
    },
    Missing {
        case: "an engine version this service publishes rather than one an account minted",
        region: REGION,
        engine: "postgres",
        engine_version: "16.4",
    },
];

/// The sentence a request this service will not serve against a custom engine version is
/// refused with: one fixed text, the same for every such request, with nothing
/// interpolated into it.
///
/// Spelled out here rather than read from the service, so the wording is pinned by the test
/// and not by the code under test.
const DENIAL: &str = "The custom engine version you requested may not exist or you don't \
                      have the required permission.";

/// A `(Engine, EngineVersion)` that names no CEV the account holds is a resource the
/// account cannot act on, so the modification is withheld before the pair is resolved: HTTP
/// 403 `AccessDenied` with the one fixed sentence, which discloses nothing about the named
/// version — the same answer `DeleteCustomDBEngineVersion` gives.
#[tokio::test]
async fn a_request_that_names_no_held_cev_is_denied() {
    let router = emulator();
    create_cev(
        &router,
        REGION,
        "create the CEV",
        "sqlserver-ee",
        "16.00.4175.1.found_cev",
        &[("KMSKeyId", KMS_KEY_ID), ("Manifest", MANIFEST)],
    )
    .await;
    wait_for_settle();

    for row in MISSING {
        let case = row.case;
        let body = expect_error(
            case,
            modify(
                &router,
                row.region,
                row.engine,
                row.engine_version,
                &[("Description", "changed-by-a-lost-request")],
            )
            .await,
            403,
            "AccessDenied",
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, DENIAL,
            "case {case:?}: the denial must be the fixed custom-engine-version sentence, got \
             {body}"
        );
        // The equality above pins the whole text; this scan says WHAT may not appear in it,
        // so a future rewording cannot reintroduce the existence oracle.
        for leaked in [row.region, row.engine, row.engine_version] {
            assert!(
                !message.contains(leaked),
                "case {case:?}: the denial must disclose nothing about the request, but it \
                 carries {leaked:?}: {message}"
            );
        }
    }

    // The CEV that WAS stored is untouched by every refusal above.
    let entry = stored(
        &router,
        REGION,
        "the stored CEV",
        "sqlserver-ee",
        "16.00.4175.1.found_cev",
    )
    .await;
    assert_eq!(
        text(&entry, "DBEngineVersionDescription"),
        ORIGINAL_DESCRIPTION,
        "a refused modify must have changed no other record, got {entry}"
    );
}

#[tokio::test]
async fn the_default_report_follows_a_cev_being_created_and_withholds_an_unusable_one() {
    let router = emulator();
    let engine = "sqlserver-ee";
    let engine_version = "16.00.4175.1.reported_cev";
    create_cev(
        &router,
        REGION,
        "create the CEV",
        engine,
        engine_version,
        &[("KMSKeyId", KMS_KEY_ID), ("Manifest", MANIFEST)],
    )
    .await;

    // A CEV the service has not finished creating has reached no availability status at
    // all, and is reported without `IncludeAll`: that is what lets the caller its create
    // answered follow the CEV until it reaches the status it is usable in.
    let being_created = described(
        &router,
        REGION,
        "being created",
        engine,
        engine_version,
        false,
    )
    .await;
    assert_eq!(
        being_created.len(),
        1,
        "a CEV that is still being created must be reported to the account that created \
         it, got {being_created:?}"
    );

    wait_for_settle();

    // Once it has an availability status, the gate is the one `IncludeAll` documents:
    // "the default is to list only available engine versions", so the two statuses a
    // caller cannot create an instance from are reported only when asked for. One row per
    // status the modelled `CustomEngineVersionStatus` enum publishes.
    for status in CUSTOM_ENGINE_VERSION_STATUS_VALUES {
        expect_ok(
            status,
            modify(&router, REGION, engine, engine_version, &[("Status", status)]).await,
        );

        let by_default = described(&router, REGION, status, engine, engine_version, false).await;
        assert_eq!(
            by_default.len(),
            usize::from(*status == STATUS_AVAILABLE),
            "case {status:?}: the default report must carry only the available CEV, got \
             {by_default:?}"
        );

        let asked_for_all = described(&router, REGION, status, engine, engine_version, true).await;
        assert_eq!(
            asked_for_all.len(),
            1,
            "case {status:?}: IncludeAll must report the CEV whatever status it is in, got \
             {asked_for_all:?}"
        );
        assert_eq!(
            text(&asked_for_all[0], "Status"),
            *status,
            "case {status:?}: the reported status must be the stored one, got {:?}",
            asked_for_all[0]
        );
    }
}

#[tokio::test]
async fn a_cev_in_use_keeps_the_status_it_is_used_under() {
    let router = emulator();
    let engine = "sqlserver-ee";
    let engine_version = "16.00.4175.1.in_use_cev";
    create_cev(
        &router,
        REGION,
        "create the CEV",
        engine,
        engine_version,
        &[("KMSKeyId", KMS_KEY_ID), ("Manifest", MANIFEST)],
    )
    .await;
    wait_for_settle();

    // An RDS Custom DB instance running the CEV is what puts it in use.
    expect_ok(
        "create an instance on the CEV",
        call_in(
            &router,
            REGION,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-cev-in-use"),
                ("Engine", engine),
                ("EngineVersion", engine_version),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );

    // "To change the availability status of the CEV, it must not currently be in use by an
    // RDS Custom instance, snapshot, or automated backup."
    expect_error(
        "a status change while in use",
        modify(
            &router,
            REGION,
            engine,
            engine_version,
            &[("Status", "inactive")],
        )
        .await,
        400,
        "InvalidCustomDBEngineVersionStateFault",
    );
    let entry = stored(&router, REGION, "in use", engine, engine_version).await;
    assert_eq!(
        text(&entry, "Status"),
        STATUS_AVAILABLE,
        "the refused status change must have left the CEV in the status it is used under, \
         got {entry}"
    );

    // Only the availability status is gated on the CEV being in use: the description is
    // not a property anything running the CEV depends on.
    let answered = strip(
        &expect_ok(
            "a description change while in use",
            modify(
                &router,
                REGION,
                engine,
                engine_version,
                &[("Description", "edited-while-in-use")],
            )
            .await,
        ),
        "Image",
    );
    assert_eq!(
        text(&answered, "DBEngineVersionDescription"),
        "edited-while-in-use",
        "the description of a CEV in use must still be editable, got {answered}"
    );
    assert_eq!(
        text(&answered, "Status"),
        STATUS_AVAILABLE,
        "a description change must not move the status, got {answered}"
    );

    // And a status change the CEV is already in asks for nothing, so it is not refused.
    let unchanged = strip(
        &expect_ok(
            "the status it already has, while in use",
            modify(
                &router,
                REGION,
                engine,
                engine_version,
                &[("Status", STATUS_AVAILABLE)],
            )
            .await,
        ),
        "Image",
    );
    assert_eq!(
        text(&unchanged, "Status"),
        STATUS_AVAILABLE,
        "a status change to the status the CEV already has must be served, got {unchanged}"
    );
}
