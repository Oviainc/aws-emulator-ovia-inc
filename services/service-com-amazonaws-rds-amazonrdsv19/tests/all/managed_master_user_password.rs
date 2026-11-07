// SPDX-License-Identifier: CC0-1.0

//! The master user password RDS MANAGES in Amazon Web Services Secrets Manager.
//!
//! The RULE under test, asserted over a table of inputs rather than one recorded call:
//! when a request turns `ManageMasterUserPassword` on, RDS generates the master user
//! password, stores it in a secret it owns in Secrets Manager, and reports that secret on
//! the resource as `MasterUserSecret` (`SecretArn`, `SecretStatus`, `KmsKeyId`) — and the
//! only refusal it answers is the documented conflict with a password the request supplies
//! itself. A resource whose password the caller supplied reports no `MasterUserSecret` at
//! all.
//!
//! What follows from that one rule, and is asserted here:
//!
//! * the reported `SecretArn` names a secret this emulator's Secrets Manager really holds,
//!   whose value is the `{"username", "password"}` document an RDS-managed secret holds,
//!   with the master user the request named and a password the engine's own
//!   `MasterUserPassword` rule accepts
//!   ([`a_create_that_manages_the_password_mints_a_secret_that_holds_the_credentials`]);
//! * `SecretStatus` is `creating` while the secret is being created and `active` once the
//!   clock has made it due, and no read moves it;
//! * `KmsKeyId` is the key the secret is encrypted under, as KMS identifies it: the key the
//!   request named, else the Amazon Web Services managed key for Secrets Manager;
//! * every resource gets a secret of its OWN, and the create's answer is what a later
//!   describe reports;
//! * `ModifyDBCluster` / `ModifyDBInstance` turn that management on for a resource created
//!   without it, rotate the secret's password, and turn it off — which deletes the secret —
//!   while a modification one of them REFUSES leaves the secret exactly as it was
//!   ([`turning_management_on_rotating_and_turning_it_off_follow_the_secret`]);
//! * the modelled refusals, each with its own sentence and HTTP 400
//!   ([`the_documented_constraints_are_the_only_refusals`]).
//!
//! The rows differ along what the rule generalises over: the kind of resource (DB cluster,
//! DB instance), the engine family (Aurora MySQL, Aurora PostgreSQL, Neptune, RDS for
//! MySQL, RDS for PostgreSQL — which publish different password length bounds), and
//! whether the request names a KMS key of its own for the secret.
//!
//! Secrets Manager and KMS are MOUNTED beside the RDS service ([`emulator`]), because the
//! handlers call them: the secret a resource reports is a real secret of this emulator,
//! read back through Secrets Manager's own operations, and the key it is encrypted under is
//! a real KMS key.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_rds_amazonrdsv19::state as st;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, expect_error, expect_ok, optional_text, subtrees, text, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents, which every ARN this suite
/// reads is scoped to.
///
/// `pub(crate)` so the sibling suite that drives the same managed-secret path reads the
/// same account rather than writing a second copy of it.
pub(crate) const ACCOUNT: &str = "000000000000";

/// A router carrying the RDS service plus the two services its managed-secret path calls
/// over the in-process callback — Secrets Manager, which holds the secret, and KMS, which
/// identifies the key it is encrypted under — all over ONE shared engine, exactly as the
/// assembled router mounts them.
///
/// `pub(crate)` so the sibling suites that drive the same path mount it the same way rather
/// than standing up a second copy.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests
    // sign nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// A Secrets Manager client over this emulator's own in-process callback, for [`REGION`].
pub(crate) fn secrets_manager(router: &MultiServiceRouter) -> aws_sdk_secretsmanager::Client {
    secrets_manager_in(router, REGION)
}

/// A Secrets Manager client over this emulator's own in-process callback — the same seam
/// the RDS handlers create the secret over — addressing `region`, where the secret of a
/// resource in that region lives. `pub(crate)` for the same reason as [`emulator`].
pub(crate) fn secrets_manager_in(
    router: &MultiServiceRouter,
    region: &str,
) -> aws_sdk_secretsmanager::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_secretsmanager::Config::builder()
        .behavior_version(aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .region(aws_sdk_secretsmanager::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://secretsmanager.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_secretsmanager::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    aws_sdk_secretsmanager::Client::from_conf(config)
}

/// A KMS client over the same callback, for [`REGION`].
pub(crate) fn kms(router: &MultiServiceRouter) -> aws_sdk_kms::Client {
    kms_in(router, REGION)
}

/// A KMS client over the same callback, addressing `region` — where the key encrypting the
/// secret of a resource in that region lives. `pub(crate)` for the same reason as
/// [`emulator`].
pub(crate) fn kms_in(router: &MultiServiceRouter, region: &str) -> aws_sdk_kms::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_kms::Config::builder()
        .behavior_version(aws_sdk_kms::config::BehaviorVersion::latest())
        .region(aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_kms::Client::from_conf(config)
}

/// The credentials a managed secret holds, read back out of Secrets Manager.
pub(crate) struct HeldCredentials {
    pub(crate) username: Option<String>,
    pub(crate) password: String,
}

/// Read the secret `arn` holds, for a secret in [`REGION`].
pub(crate) async fn held_credentials(
    case: &str,
    router: &MultiServiceRouter,
    arn: &str,
) -> HeldCredentials {
    held_credentials_in(case, router, REGION, arn).await
}

/// Read the secret `arn` holds in `region`, through Secrets Manager's own
/// `GetSecretValue`.
pub(crate) async fn held_credentials_in(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
) -> HeldCredentials {
    let answer = secrets_manager_in(router, region)
        .get_secret_value()
        .secret_id(arn)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: Secrets Manager holds no secret at {arn}: {error}")
        });
    let held = answer
        .secret_string()
        .unwrap_or_else(|| panic!("case {case:?}: the secret at {arn} holds no string value"));
    let document: Value = serde_json::from_str(held)
        .unwrap_or_else(|error| panic!("case {case:?}: the secret at {arn} is not JSON: {error}"));
    let password = document
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the secret at {arn} carries no password: {document}")
        })
        .to_string();
    HeldCredentials {
        username: document
            .get("username")
            .and_then(Value::as_str)
            .map(str::to_string),
        password,
    }
}

/// The one `<MasterUserSecret>` subtree of a resource envelope, or `None` when the
/// resource reports none.
///
/// `pub(crate)` for the same reason as [`emulator`].
pub(crate) fn master_user_secret(case: &str, xml: &str) -> Option<String> {
    let found = subtrees(xml, "MasterUserSecret");
    assert!(
        found.len() <= 1,
        "case {case:?}: expected at most one MasterUserSecret, got {} in {xml}",
        found.len()
    );
    found.into_iter().next()
}

/// The one resource subtree (`DBCluster`, `DBInstance`) of `xml`.
pub(crate) fn the_resource(case: &str, xml: &str, element: &str) -> String {
    let found = subtrees(xml, element);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one {element}, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// Which resource a row creates, and the operations that create and read it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    Cluster,
    Instance,
}

impl Resource {
    fn create(self) -> &'static str {
        match self {
            Self::Cluster => "CreateDBCluster",
            Self::Instance => "CreateDBInstance",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Cluster => "DescribeDBClusters",
            Self::Instance => "DescribeDBInstances",
        }
    }

    fn identifier_member(self) -> &'static str {
        match self {
            Self::Cluster => "DBClusterIdentifier",
            Self::Instance => "DBInstanceIdentifier",
        }
    }

    fn element(self) -> &'static str {
        match self {
            Self::Cluster => "DBCluster",
            Self::Instance => "DBInstance",
        }
    }
}

/// One row of the managed-password table.
struct ManagedCase {
    case: &'static str,
    resource: Resource,
    engine: &'static str,
    master_username: &'static str,
    /// The parameters this row adds beyond the identifier, the engine and the master
    /// username.
    extra: &'static [(&'static str, &'static str)],
    /// Whether the request names a customer managed KMS key for the secret
    /// (`MasterUserSecretKmsKeyId`).
    names_kms_key: bool,
}

/// The rows: both resources that report `MasterUserSecret` from a create, five engine
/// families (whose `MasterUserPassword` length bounds differ — see
/// `st::master_user_password_len`), and both the default and a named encryption key.
const MANAGED_CASES: &[ManagedCase] = &[
    ManagedCase {
        case: "an Aurora MySQL DB cluster on the default encryption key",
        resource: Resource::Cluster,
        engine: "aurora-mysql",
        master_username: "dbadmin",
        extra: &[("EnableHttpEndpoint", "true")],
        names_kms_key: false,
    },
    ManagedCase {
        case: "an Aurora PostgreSQL DB cluster with a key of its own",
        resource: Resource::Cluster,
        engine: "aurora-postgresql",
        master_username: "pgadmin",
        extra: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "2.0"),
        ],
        names_kms_key: true,
    },
    ManagedCase {
        case: "a Neptune DB cluster, another engine family of the same cluster type",
        resource: Resource::Cluster,
        engine: "neptune",
        master_username: "neptuneadmin",
        extra: &[],
        names_kms_key: false,
    },
    ManagedCase {
        case: "an RDS for MySQL DB instance on the default encryption key",
        resource: Resource::Instance,
        engine: "mysql",
        master_username: "mysqladmin",
        extra: &[("DBInstanceClass", "db.t3.medium")],
        names_kms_key: false,
    },
    ManagedCase {
        case: "an RDS for PostgreSQL DB instance with a key of its own",
        resource: Resource::Instance,
        engine: "postgres",
        master_username: "pgadmin",
        extra: &[("DBInstanceClass", "db.m5.large"), ("AllocatedStorage", "20")],
        names_kms_key: true,
    },
];

#[tokio::test]
async fn a_create_that_manages_the_password_mints_a_secret_that_holds_the_credentials() {
    let router = emulator();
    // A key of the account's own, minted through KMS itself, for the rows that name one.
    let key = kms(&router)
        .create_key()
        .description("the key a managed master user secret is encrypted under")
        .send()
        .await
        .expect("KMS mints a customer managed key");
    let key_metadata = key
        .key_metadata()
        .expect("a minted key reports its metadata");
    let key_id = key_metadata.key_id().to_string();
    let key_arn = key_metadata
        .arn()
        .expect("a minted key reports its ARN")
        .to_string();

    let mut minted_arns: Vec<String> = Vec::new();
    for (index, row) in MANAGED_CASES.iter().enumerate() {
        let identifier = format!("emu-managed-{index}");
        let mut params: Vec<(&str, String)> = vec![
            (row.resource.identifier_member(), identifier.clone()),
            ("Engine", row.engine.to_string()),
            ("MasterUsername", row.master_username.to_string()),
            ("ManageMasterUserPassword", "true".to_string()),
        ];
        params.extend(
            row.extra
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );
        if row.names_kms_key {
            params.push(("MasterUserSecretKmsKeyId", key_id.clone()));
        }
        let created = expect_ok(
            row.case,
            call(&router, row.resource.create(), &params).await,
        );
        let envelope = the_resource(row.case, &created, row.resource.element());

        // The resource reports the secret RDS manages for it.
        let secret = master_user_secret(row.case, &envelope).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the resource to report MasterUserSecret, got {envelope}",
                row.case
            )
        });
        let secret_arn = text(&secret, "SecretArn");
        assert!(
            secret_arn.starts_with(&format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:")),
            "case {:?}: expected a Secrets Manager secret ARN in the request's region, got \
             {secret_arn:?}",
            row.case
        );
        assert_eq!(
            text(&secret, "SecretStatus"),
            st::MASTER_USER_SECRET_STATUS_CREATING,
            "case {:?}: a secret this call just created is still being created, got {secret}",
            row.case
        );
        // `KmsKeyId` is the key the secret is encrypted under, as KMS identifies it.
        let reported_key = text(&secret, "KmsKeyId");
        if row.names_kms_key {
            assert_eq!(
                reported_key, key_arn,
                "case {:?}: expected the ARN of the key the request named, got \
                 {reported_key:?}",
                row.case
            );
        } else {
            assert!(
                reported_key.starts_with(&format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/")),
                "case {:?}: expected the ARN of the Amazon Web Services managed key for \
                 Secrets Manager, got {reported_key:?}",
                row.case
            );
        }

        // The ARN names a secret Secrets Manager really holds, whose value is the
        // credentials document, with a password the engine's own rule accepts.
        let held = held_credentials(row.case, &router, &secret_arn).await;
        assert_eq!(
            held.username.as_deref(),
            Some(row.master_username),
            "case {:?}: expected the secret to hold the master user the request named, got \
             {:?}",
            row.case,
            held.username
        );
        assert_eq!(
            st::check_master_user_password(row.engine, &held.password),
            Ok(()),
            "case {:?}: the generated password must satisfy MasterUserPassword for engine \
             {}, and it has {} characters",
            row.case,
            row.engine,
            held.password.chars().count()
        );
        assert!(
            !minted_arns.contains(&secret_arn),
            "case {:?}: every resource gets a secret of its own, but {secret_arn} was \
             already reported for another",
            row.case
        );
        minted_arns.push(secret_arn.clone());

        // The create's answer IS what the describe reports, and the status the clock owed
        // has settled by then.
        wait_for_settle();
        let described = expect_ok(
            row.case,
            call(
                &router,
                row.resource.describe(),
                &[(row.resource.identifier_member(), identifier.as_str())],
            )
            .await,
        );
        let described_secret = master_user_secret(
            row.case,
            &the_resource(row.case, &described, row.resource.element()),
        )
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the stored resource to report MasterUserSecret, got \
                 {described}",
                row.case
            )
        });
        assert_eq!(
            text(&described_secret, "SecretArn"),
            secret_arn,
            "case {:?}: expected the describe to report the secret the create minted, got \
             {described_secret}",
            row.case
        );
        assert_eq!(
            text(&described_secret, "KmsKeyId"),
            reported_key,
            "case {:?}: expected the describe to report the same encryption key, got \
             {described_secret}",
            row.case
        );
        assert_eq!(
            text(&described_secret, "SecretStatus"),
            st::MASTER_USER_SECRET_STATUS_ACTIVE,
            "case {:?}: expected the secret to be active once its span has passed, got \
             {described_secret}",
            row.case
        );
        // And the credentials are unchanged by having been read.
        let reread = held_credentials(row.case, &router, &secret_arn).await;
        assert_eq!(
            reread.password, held.password,
            "case {:?}: reading a resource must not change the password its secret holds",
            row.case
        );
    }
}

/// One row of the supplied-password table: the create, and the resource it answers with.
struct SuppliedPasswordCase {
    case: &'static str,
    action: &'static str,
    identifier_member: &'static str,
    element: &'static str,
    /// The parameters the row carries beyond the identifier, a supplied
    /// `MasterUserPassword` among them.
    carried: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_resource_whose_password_the_caller_supplied_reports_no_managed_secret() {
    let router = emulator();
    let rows: [SuppliedPasswordCase; 2] = [
        SuppliedPasswordCase {
            case: "a DB cluster created with MasterUserPassword",
            action: "CreateDBCluster",
            identifier_member: "DBClusterIdentifier",
            element: "DBCluster",
            carried: &[
                ("Engine", "aurora-mysql"),
                ("MasterUsername", "dbadmin"),
                ("MasterUserPassword", "a-strong-password"),
            ],
        },
        SuppliedPasswordCase {
            case: "a DB instance created with MasterUserPassword",
            action: "CreateDBInstance",
            identifier_member: "DBInstanceIdentifier",
            element: "DBInstance",
            carried: &[
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.t3.medium"),
                ("MasterUsername", "mysqladmin"),
                ("MasterUserPassword", "a-strong-password"),
            ],
        },
    ];
    for (index, row) in rows.iter().enumerate() {
        let identifier = format!("emu-unmanaged-{index}");
        let mut params: Vec<(&str, String)> =
            vec![(row.identifier_member, identifier.clone())];
        params.extend(
            row.carried
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );
        let created = expect_ok(row.case, call(&router, row.action, &params).await);
        let envelope = the_resource(row.case, &created, row.element);
        assert_eq!(
            master_user_secret(row.case, &envelope),
            None,
            "case {:?}: a resource whose password the caller supplied manages none, so it \
             reports no MasterUserSecret: {envelope}",
            row.case
        );
    }
}

/// The sentence a request that asks RDS to manage the password AND carries one is refused
/// with, written out here rather than read from the implementation: the wording is part of
/// what this suite pins, so a drift away from it must fail.
const MANAGE_CONFLICT_SENTENCE: &str = "You can't manage the master user password with \
     Amazon Web Services Secrets Manager if MasterUserPassword is specified.";

/// The sentence a request naming a key for a secret it manages none of is refused with.
const KEY_WITHOUT_MANAGEMENT_SENTENCE: &str = "You can't specify MasterUserSecretKmsKeyId \
     without also specifying ManageMasterUserPassword as true.";

/// The sentence a request that stops the management without naming a password instead is
/// refused with.
const PASSWORD_REQUIRED_SENTENCE: &str = "You must specify MasterUserPassword when you stop \
     managing the master user password with Amazon Web Services Secrets Manager.";

/// The sentence a rotation of a secret the resource does not have is refused with, for each
/// resource kind the member names.
fn rotate_without_secret_sentence(resource: &str) -> String {
    format!(
        "You can't rotate the master user password because the master user password for this \
         {resource} isn't managed by RDS in Amazon Web Services Secrets Manager."
    )
}

/// One row of the refusal table: the call, and the refusal it must be answered with.
struct RefusalCase {
    case: &'static str,
    action: &'static str,
    params: &'static [(&'static str, &'static str)],
    /// The sentence the refusal carries.
    message: &'static str,
}

/// The refusals the documented constraints state — every one an HTTP 400 with the Query
/// API code for parameters that cannot hold together, never a 501 and never an error code
/// RDS does not answer.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "a cluster create that manages the password AND carries one",
        action: "CreateDBCluster",
        params: &[
            ("DBClusterIdentifier", "emu-refused-cluster"),
            ("Engine", "aurora-mysql"),
            ("MasterUsername", "dbadmin"),
            ("MasterUserPassword", "a-strong-password"),
            ("ManageMasterUserPassword", "true"),
        ],
        message: MANAGE_CONFLICT_SENTENCE,
    },
    RefusalCase {
        case: "an instance create that manages the password AND carries one",
        action: "CreateDBInstance",
        params: &[
            ("DBInstanceIdentifier", "emu-refused-instance"),
            ("Engine", "mysql"),
            ("DBInstanceClass", "db.t3.medium"),
            ("MasterUsername", "mysqladmin"),
            ("MasterUserPassword", "a-strong-password"),
            ("ManageMasterUserPassword", "true"),
        ],
        message: MANAGE_CONFLICT_SENTENCE,
    },
    RefusalCase {
        case: "a cluster create naming a secret key without managing the password",
        action: "CreateDBCluster",
        params: &[
            ("DBClusterIdentifier", "emu-refused-key-cluster"),
            ("Engine", "aurora-mysql"),
            ("MasterUsername", "dbadmin"),
            ("MasterUserPassword", "a-strong-password"),
            ("MasterUserSecretKmsKeyId", "alias/aws/secretsmanager"),
        ],
        message: KEY_WITHOUT_MANAGEMENT_SENTENCE,
    },
    RefusalCase {
        case: "an instance create naming a secret key without managing the password",
        action: "CreateDBInstance",
        params: &[
            ("DBInstanceIdentifier", "emu-refused-key-instance"),
            ("Engine", "mysql"),
            ("DBInstanceClass", "db.t3.medium"),
            ("MasterUsername", "mysqladmin"),
            ("MasterUserPassword", "a-strong-password"),
            ("MasterUserSecretKmsKeyId", "alias/aws/secretsmanager"),
        ],
        message: KEY_WITHOUT_MANAGEMENT_SENTENCE,
    },
];

#[tokio::test]
async fn the_documented_constraints_are_the_only_refusals() {
    let router = emulator();
    for row in REFUSAL_CASES {
        let body = expect_error(
            row.case,
            call(&router, row.action, row.params).await,
            400,
            "InvalidParameterCombination",
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, row.message,
            "case {:?}: expected the documented sentence, got {message:?}",
            row.case
        );
        // A refused create stored nothing — and minted no secret, since the identifier
        // the request named resolves to no resource at all.
        let (identifier_member, identifier) = row
            .params
            .iter()
            .find(|(name, _)| name.ends_with("Identifier"))
            .expect("every row names its resource");
        expect_error(
            row.case,
            call(
                &router,
                if row.action == "CreateDBCluster" {
                    "DescribeDBClusters"
                } else {
                    "DescribeDBInstances"
                },
                &[(*identifier_member, *identifier)],
            )
            .await,
            404,
            if row.action == "CreateDBCluster" {
                "DBClusterNotFoundFault"
            } else {
                "DBInstanceNotFound"
            },
        );
    }
}

#[tokio::test]
async fn turning_management_on_rotating_and_turning_it_off_follow_the_secret() {
    let router = emulator();
    let case = "a DB cluster whose password management is turned on, rotated and turned off";
    expect_ok(
        case,
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-managed-lifecycle"),
                ("Engine", "aurora-mysql"),
                ("MasterUsername", "dbadmin"),
                ("MasterUserPassword", "the-callers-own-password"),
            ],
        )
        .await,
    );
    wait_for_settle();

    // Rotating a secret a resource does not have is refused: rotating "is valid only if
    // the master user password is managed by RDS in Amazon Web Services Secrets Manager".
    let refused = expect_error(
        case,
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-managed-lifecycle"),
                ("RotateMasterUserPassword", "true"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    assert_eq!(
        text(&refused, "Message"),
        rotate_without_secret_sentence("DB cluster"),
        "case {case:?}: expected the documented sentence, got {refused}"
    );
    // Turning management "off" for a cluster that manages nothing changes nothing and is
    // accepted: the requirement to name a password instead is conditioned on the cluster
    // ALREADY managing one.
    let unmanaged = expect_ok(
        case,
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-managed-lifecycle"),
                ("ManageMasterUserPassword", "false"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    assert_eq!(
        master_user_secret(case, &the_resource(case, &unmanaged, "DBCluster")),
        None,
        "case {case:?}: a cluster that manages no password still reports no MasterUserSecret: \
         {unmanaged}"
    );

    // Turning management ON mints the secret and the cluster reports it from then on.
    let modified = expect_ok(
        case,
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-managed-lifecycle"),
                ("ManageMasterUserPassword", "true"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    let secret = master_user_secret(case, &the_resource(case, &modified, "DBCluster"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a MasterUserSecret after turning management on: {modified}")
        });
    let secret_arn = text(&secret, "SecretArn");
    assert_eq!(
        text(&secret, "SecretStatus"),
        st::MASTER_USER_SECRET_STATUS_CREATING,
        "case {case:?}: a secret this call just created is still being created: {secret}"
    );
    let first = held_credentials(case, &router, &secret_arn).await;
    assert_eq!(
        st::check_master_user_password("aurora-mysql", &first.password),
        Ok(()),
        "case {case:?}: the generated password must satisfy MasterUserPassword, and it has \
         {} characters",
        first.password.chars().count()
    );
    wait_for_settle();

    // NOW that the cluster manages the password, stopping that management without naming a
    // password to use instead is refused: "you must specify MasterUserPassword".
    let refused = expect_error(
        case,
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-managed-lifecycle"),
                ("ManageMasterUserPassword", "false"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    assert_eq!(
        text(&refused, "Message"),
        PASSWORD_REQUIRED_SENTENCE,
        "case {case:?}: expected the documented sentence, got {refused}"
    );
    let kept = held_credentials(case, &router, &secret_arn).await;
    assert_eq!(
        kept.password, first.password,
        "case {case:?}: that refusal must leave the secret exactly as it was"
    );

    // A ROTATION stakes another password in the same secret, and the secret reports
    // `rotating` until it settles.
    let rotated = expect_ok(
        case,
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-managed-lifecycle"),
                ("RotateMasterUserPassword", "true"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    let rotating = master_user_secret(case, &the_resource(case, &rotated, "DBCluster"))
        .unwrap_or_else(|| panic!("case {case:?}: expected a MasterUserSecret: {rotated}"));
    assert_eq!(
        text(&rotating, "SecretArn"),
        secret_arn,
        "case {case:?}: a rotation stakes a new password in the SAME secret: {rotating}"
    );
    assert_eq!(
        text(&rotating, "SecretStatus"),
        st::MASTER_USER_SECRET_STATUS_ROTATING,
        "case {case:?}: expected the secret to report the rotation under way: {rotating}"
    );
    let second = held_credentials(case, &router, &secret_arn).await;
    assert_ne!(
        second.password, first.password,
        "case {case:?}: a rotation must leave the secret holding a NEW password"
    );
    assert_eq!(
        st::check_master_user_password("aurora-mysql", &second.password),
        Ok(()),
        "case {case:?}: the rotated password must satisfy MasterUserPassword, and it has {} \
         characters",
        second.password.chars().count()
    );
    assert_eq!(
        second.username, first.username,
        "case {case:?}: a rotation changes the password, not the master user"
    );
    wait_for_settle();
    let described = expect_ok(
        case,
        call(
            &router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "emu-managed-lifecycle")],
        )
        .await,
    );
    let settled = master_user_secret(case, &the_resource(case, &described, "DBCluster"))
        .unwrap_or_else(|| panic!("case {case:?}: expected a MasterUserSecret: {described}"));
    assert_eq!(
        text(&settled, "SecretStatus"),
        st::MASTER_USER_SECRET_STATUS_ACTIVE,
        "case {case:?}: expected the rotated secret to be active again: {settled}"
    );

    // A modification this operation REFUSES must leave the secret exactly as it was: the
    // rename below collides with another cluster of the region, so the call is answered
    // 400 — and the secret it would otherwise have deleted still holds the credentials and
    // is still what the cluster reports.
    expect_ok(
        case,
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-managed-taken"),
                ("Engine", "aurora-mysql"),
                ("MasterUsername", "dbadmin"),
                ("MasterUserPassword", "another-callers-own-password"),
            ],
        )
        .await,
    );
    wait_for_settle();
    expect_error(
        case,
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-managed-lifecycle"),
                ("NewDBClusterIdentifier", "emu-managed-taken"),
                ("ManageMasterUserPassword", "false"),
                ("MasterUserPassword", "a-password-this-call-never-takes"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
        400,
        "DBClusterAlreadyExistsFault",
    );
    let after_refusal = held_credentials(case, &router, &secret_arn).await;
    assert_eq!(
        after_refusal.password, second.password,
        "case {case:?}: a refused modification must not touch the secret the cluster manages"
    );
    let still_reported = expect_ok(
        case,
        call(
            &router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "emu-managed-lifecycle")],
        )
        .await,
    );
    assert_eq!(
        master_user_secret(case, &the_resource(case, &still_reported, "DBCluster"))
            .map(|secret| text(&secret, "SecretArn")),
        Some(secret_arn.clone()),
        "case {case:?}: a refused modification leaves the cluster reporting the secret it \
         still manages: {still_reported}"
    );

    // Turning management OFF leaves the cluster with no managed secret, and RDS deleted
    // the one it had: the credentials it held can no longer be read.
    let released = expect_ok(
        case,
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-managed-lifecycle"),
                ("ManageMasterUserPassword", "false"),
                ("MasterUserPassword", "the-callers-own-password-again"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    assert_eq!(
        master_user_secret(case, &the_resource(case, &released, "DBCluster")),
        None,
        "case {case:?}: a cluster that no longer manages its password reports no \
         MasterUserSecret: {released}"
    );
    let gone = secrets_manager(&router)
        .get_secret_value()
        .secret_id(&secret_arn)
        .send()
        .await;
    assert!(
        gone.is_err(),
        "case {case:?}: RDS deletes the secret when management is turned off, so {secret_arn} \
         must no longer answer with credentials"
    );
}

#[tokio::test]
async fn turning_management_on_for_a_db_instance_mints_its_own_secret() {
    let router = emulator();
    let case = "a DB instance whose password management is turned on and then rotated";
    expect_ok(
        case,
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-managed-instance"),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.m5.large"),
                ("MasterUsername", "pgadmin"),
                ("MasterUserPassword", "the-callers-own-password"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let modified = expect_ok(
        case,
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-managed-instance"),
                ("ManageMasterUserPassword", "true"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    let secret = master_user_secret(case, &the_resource(case, &modified, "DBInstance"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a MasterUserSecret after turning management on: {modified}")
        });
    let secret_arn = text(&secret, "SecretArn");
    let held = held_credentials(case, &router, &secret_arn).await;
    assert_eq!(
        held.username.as_deref(),
        Some("pgadmin"),
        "case {case:?}: expected the secret to hold the instance's master user, got {:?}",
        held.username
    );
    assert_eq!(
        st::check_master_user_password("postgres", &held.password),
        Ok(()),
        "case {case:?}: the generated password must satisfy MasterUserPassword for postgres, \
         and it has {} characters",
        held.password.chars().count()
    );
    wait_for_settle();

    let rotated = expect_ok(
        case,
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-managed-instance"),
                ("RotateMasterUserPassword", "true"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    let rotating = master_user_secret(case, &the_resource(case, &rotated, "DBInstance"))
        .unwrap_or_else(|| panic!("case {case:?}: expected a MasterUserSecret: {rotated}"));
    assert_eq!(
        text(&rotating, "SecretArn"),
        secret_arn,
        "case {case:?}: a rotation stakes a new password in the SAME secret: {rotating}"
    );
    let second = held_credentials(case, &router, &secret_arn).await;
    assert_ne!(
        second.password, held.password,
        "case {case:?}: a rotation must leave the secret holding a NEW password"
    );

    // Rotating an instance that does not manage its password is refused, as it is for a
    // cluster: one rule, both resources.
    expect_ok(
        case,
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-unmanaged-instance"),
                ("Engine", "postgres"),
                ("DBInstanceClass", "db.m5.large"),
                ("MasterUsername", "pgadmin"),
                ("MasterUserPassword", "the-callers-own-password"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let refused = expect_error(
        case,
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-unmanaged-instance"),
                ("RotateMasterUserPassword", "true"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    assert_eq!(
        text(&refused, "Message"),
        rotate_without_secret_sentence("DB instance"),
        "case {case:?}: expected the documented sentence, got {refused}"
    );
}

#[tokio::test]
async fn a_tenant_database_that_manages_its_password_reports_its_own_secret() {
    let router = emulator();
    let case = "a tenant database of an Oracle container database";
    // A multi-tenant DB instance is what holds tenant databases
    // (`st::is_multi_tenant_engine`).
    expect_ok(
        case,
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-managed-cdb"),
                ("Engine", "oracle-ee-cdb"),
                ("DBInstanceClass", "db.m5.large"),
                ("MasterUsername", "oracleadmin"),
                ("MasterUserPassword", "the-callers-own-password"),
                ("MultiTenant", "true"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let created = expect_ok(
        case,
        call(
            &router,
            "CreateTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-managed-cdb"),
                ("TenantDBName", "emutenant"),
                ("MasterUsername", "tenantadmin"),
                ("ManageMasterUserPassword", "true"),
            ],
        )
        .await,
    );
    let envelope = the_resource(case, &created, "TenantDatabase");
    let secret = master_user_secret(case, &envelope).unwrap_or_else(|| {
        panic!("case {case:?}: expected the tenant database to report MasterUserSecret: {envelope}")
    });
    let held = held_credentials(case, &router, &text(&secret, "SecretArn")).await;
    assert_eq!(
        held.username.as_deref(),
        Some("tenantadmin"),
        "case {case:?}: expected the secret to hold the tenant database's master user, got \
         {:?}",
        held.username
    );
    assert_eq!(
        st::check_master_user_password("oracle-ee-cdb", &held.password),
        Ok(()),
        "case {case:?}: the generated password must satisfy MasterUserPassword for an Oracle \
         engine, and it has {} characters",
        held.password.chars().count()
    );
    // And the modelled conflict is refused here too.
    let refused = expect_error(
        case,
        call(
            &router,
            "CreateTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-managed-cdb"),
                ("TenantDBName", "emurefused"),
                ("MasterUsername", "tenantadmin"),
                ("MasterUserPassword", "a-strong-password"),
                ("ManageMasterUserPassword", "true"),
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    assert_eq!(
        text(&refused, "Message"),
        MANAGE_CONFLICT_SENTENCE,
        "case {case:?}: expected the documented sentence, got {refused}"
    );
    assert_eq!(
        optional_text(&refused, "TenantDatabase"),
        None,
        "case {case:?}: a refused create answers no tenant database: {refused}"
    );
}
