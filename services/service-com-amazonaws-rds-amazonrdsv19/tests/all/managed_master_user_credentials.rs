// SPDX-License-Identifier: CC0-1.0

//! What the RDS-managed master user password means ON THE ENGINE, and whose secret it is.
//!
//! `managed_master_user_password.rs` beside this file asserts the CONTROL PLANE of
//! `ManageMasterUserPassword`: the secret a resource reports, its status, its key and the
//! credentials Secrets Manager holds under it. This suite asserts the two halves of the
//! same rule that are not visible from there:
//!
//! 1. THE ENGINE takes the managed password and only it. Turning the management on
//!    installs the password RDS generated as the instance's master credential, so a
//!    session opens with the password the secret holds and the password the instance was
//!    created with is refused with MySQL's own `ER_ACCESS_DENIED_ERROR`; rotating the
//!    secret moves that boundary again, and turning the management off hands it to the
//!    password the request carries
//!    ([`the_engine_admits_the_managed_password_and_refuses_the_one_it_replaced`]).
//! 2. THE SECRET BELONGS TO THE INSTANCE RDS MINTED IT FOR. An instance that inherits
//!    another's master credential — a read replica, an instance restored from a snapshot —
//!    reports no `MasterUserSecret` of its own, and deleting the OWNER takes the secret
//!    with it, while deleting an inheriting instance takes nothing
//!    ([`a_managed_secret_belongs_to_the_instance_it_was_minted_for`]).
//!
//! The rows span the lifecycle stages the rule generalises over (created managed, turned
//! on, rotated, turned off) and the two ways an instance can inherit a credential.
//!
//! Secrets Manager and KMS are MOUNTED beside the RDS service, because the handlers call
//! them: the password a session is opened with here is read back out of the secret the
//! emulated Secrets Manager holds, over its own `GetSecretValue`, and the engine endpoint
//! is the instance's own listener on this machine.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_engine_endpoint::{
    available_instance, endpoint_of, mysql_login, MySqlSession, Refusal, Transport, DATABASE,
    MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::{call, expect_ok, text, wait_for_settle};
// The router that mounts RDS beside Secrets Manager and KMS, the Secrets Manager client
// over this emulator's own callback, and the `<MasterUserSecret>` reader all belong to the
// CONTROL-PLANE suite of this same rule: this one drives the engine side through them
// rather than standing up a second copy.
use crate::managed_master_user_password::{emulator, master_user_secret, secrets_manager};

/// `MasterUserSecret$SecretStatus` once the secret can be used (`smithy_query(query="shape",
/// shape="MasterUserSecret")`: "active - The secret is available for normal use and
/// rotation").
const ACTIVE: &str = "active";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The password a managed secret holds, read back through Secrets Manager's own
/// `GetSecretValue`.
///
/// Only the password: the username half is what the control-plane suite beside this one
/// asserts, and what this suite does with the value is open a session with it.
async fn managed_password(
    case: &str,
    router: &MultiServiceRouter,
    secret_arn: &str,
) -> Result<String, String> {
    let answer = secrets_manager(router)
        .get_secret_value()
        .secret_id(secret_arn)
        .send()
        .await
        .map_err(|error| {
            aws_smithy_types::error::display::DisplayErrorContext(&error).to_string()
        })?;
    let document = answer
        .secret_string
        .unwrap_or_else(|| panic!("case {case:?}: the managed secret must hold a SecretString"));
    let parsed: Value = serde_json::from_str(&document).unwrap_or_else(|error| {
        panic!("case {case:?}: the managed secret must hold a JSON document, got {error}")
    });
    Ok(parsed
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the managed secret must carry a password"))
        .to_string())
}

/// The `SecretArn` an envelope's `MasterUserSecret` publishes, where it carries one.
fn published_secret_arn(case: &str, envelope: &str) -> Option<String> {
    master_user_secret(case, envelope).map(|element| text(&element, "SecretArn"))
}

/// `DescribeDBInstances` for one instance.
async fn describe(case: &str, router: &MultiServiceRouter, identifier: &str) -> String {
    expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => identifier],
        )
        .await,
    )
}

/// `ModifyDBInstance` with `extra` on top of the instance it names, which must succeed.
async fn modify(
    case: &str,
    router: &MultiServiceRouter,
    identifier: &str,
    extra: &[(&str, String)],
) -> String {
    let mut request: Vec<(&str, String)> = params!["DBInstanceIdentifier" => identifier].to_vec();
    request.extend(extra.iter().cloned());
    expect_ok(case, call(router, "ModifyDBInstance", &request).await)
}

/// A MySQL session the master user opens with `password`, which must be admitted.
async fn admitted_login(case: &str, address: &str, port: u16, password: &str) -> MySqlSession {
    match mysql_login(
        address,
        port,
        Transport::Plaintext,
        MASTER_USER,
        password,
        Some(DATABASE),
    )
    .await
    {
        Ok(session) => session,
        Err(refusal) => {
            panic!("case {case:?}: this password must open a session, got {refusal:?}")
        }
    }
}

/// The refusal the engine answers a login with `password` with, which must be refused.
async fn refused_login(case: &str, address: &str, port: u16, password: &str) -> Refusal {
    match mysql_login(
        address,
        port,
        Transport::Plaintext,
        MASTER_USER,
        password,
        Some(DATABASE),
    )
    .await
    {
        Ok(_) => panic!("case {case:?}: this password must no longer open a session"),
        Err(refusal) => refusal,
    }
}

/// MySQL's own access-denied refusal for `address`: `ER_ACCESS_DENIED_ERROR`, whose
/// message names the account the login was for and that a password was sent.
fn access_denied(address: &str) -> (String, String) {
    (
        "1045".to_string(),
        format!("Access denied for user '{MASTER_USER}'@'{address}' (using password: YES)"),
    )
}

/// The managed password is the one the engine admits, and every password it replaces stops
/// opening a session — through the whole lifecycle: created managed, turned on for an
/// instance that had its own password, rotated, and turned off again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_admits_the_managed_password_and_refuses_the_one_it_replaced() {
    let router = emulator();

    // (1) An instance CREATED with the management on: the password its secret holds opens
    // a session, and the instance never had another.
    let case = "mysql, managed from the create";
    let created = expect_ok(
        case,
        call(
            &router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-mmupe-created",
                "Engine" => "mysql",
                "DBInstanceClass" => "db.t3.micro",
                "AllocatedStorage" => 20,
                "MasterUsername" => MASTER_USER,
                "ManageMasterUserPassword" => true,
                "DBName" => DATABASE,
            ],
        )
        .await,
    );
    let (created_address, created_port) = endpoint_of(&created);
    let created_secret = published_secret_arn(case, &created)
        .unwrap_or_else(|| panic!("case {case:?}: the create must publish a secret: {created}"));
    wait_for_settle();
    let from_secret = managed_password(case, &router, &created_secret)
        .await
        .expect("the published secret must be readable");
    let mut session = admitted_login(case, &created_address, created_port, &from_secret).await;
    let identity = session
        .query("SELECT CURRENT_USER()")
        .await
        .expect("the identity query runs");
    assert_eq!(
        identity.only(),
        Some(format!("{MASTER_USER}@%").as_str()),
        "case {case:?}: the session must be the master user's account, got {identity:?}"
    );
    // A password nobody granted this instance is refused, so the login above was the
    // secret's password and not an unauthenticated session.
    let refused = refused_login(case, &created_address, created_port, MASTER_PASSWORD).await;
    assert_eq!(
        (refused.code.clone(), refused.message.clone()),
        access_denied(&created_address),
        "case {case:?}: another password must be refused with the engine's own \
         access-denied error, got {refused:?}"
    );

    // (2) An instance created with its OWN password, whose management is then TURNED ON:
    // the generated password opens a session and the created one stops working — the
    // transition this issue is about.
    let case = "mysql, management turned on";
    let identifier = "emu-mmupe-turned-on";
    let (address, port) = available_instance(&router, identifier, "mysql", &[]).await;
    admitted_login(case, &address, port, MASTER_PASSWORD).await;
    let answered = modify(
        case,
        &router,
        identifier,
        &params![
            "ManageMasterUserPassword" => true,
            "ApplyImmediately" => true,
        ],
    )
    .await;
    let secret_arn = published_secret_arn(case, &answered)
        .unwrap_or_else(|| panic!("case {case:?}: the modify must publish a secret: {answered}"));
    wait_for_settle();
    let managed = managed_password(case, &router, &secret_arn)
        .await
        .expect("the published secret must be readable");
    assert_ne!(
        managed, MASTER_PASSWORD,
        "case {case:?}: the managed password must not be the password the instance was \
         created with"
    );
    admitted_login(case, &address, port, &managed).await;
    let refused = refused_login(case, &address, port, MASTER_PASSWORD).await;
    assert_eq!(
        (refused.code.clone(), refused.message.clone()),
        access_denied(&address),
        "case {case:?}: the password the managed secret replaced must be refused with the \
         engine's own access-denied error, got {refused:?}"
    );

    // (3) ROTATED: the secret holds a new password, which is the one the engine takes.
    let case = "mysql, secret rotated";
    modify(
        case,
        &router,
        identifier,
        &params![
            "RotateMasterUserPassword" => true,
            "ApplyImmediately" => true,
        ],
    )
    .await;
    wait_for_settle();
    let rotated = managed_password(case, &router, &secret_arn)
        .await
        .expect("the rotated secret must be readable");
    assert_ne!(
        rotated, managed,
        "case {case:?}: a rotation must put a new password in the secret"
    );
    admitted_login(case, &address, port, &rotated).await;
    refused_login(case, &address, port, &managed).await;

    // (4) TURNED OFF: "Amazon RDS deletes the secret and uses the new password for the
    // master user specified by MasterUserPassword" — so that password, and not the one the
    // deleted secret held, is what opens a session.
    let case = "mysql, management turned off";
    let own_password = "emu-Own-Password-7";
    let answered = modify(
        case,
        &router,
        identifier,
        &params![
            "ManageMasterUserPassword" => false,
            "MasterUserPassword" => own_password,
            "ApplyImmediately" => true,
        ],
    )
    .await;
    assert_eq!(
        published_secret_arn(case, &answered),
        None,
        "case {case:?}: an instance that no longer manages its password publishes no \
         MasterUserSecret, got {answered}"
    );
    // The modification leaves the instance `modifying`; it is back up a moment later,
    // which is when a client can open a session on it again.
    wait_for_settle();
    admitted_login(case, &address, port, own_password).await;
    refused_login(case, &address, port, &rotated).await;
}

/// The secret belongs to the instance RDS minted it for: an instance that inherits that
/// instance's master credential reports none of its own and deleting it deletes none,
/// while deleting the OWNER takes the secret with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_managed_secret_belongs_to_the_instance_it_was_minted_for() {
    let router = emulator();
    let case = "mysql, a managed source and the instances that inherit its credential";
    let source = "emu-mmupe-source";
    let (address, port) = available_instance(&router, source, "mysql", &[]).await;
    let answered = modify(
        case,
        &router,
        source,
        &params![
            "ManageMasterUserPassword" => true,
            "ApplyImmediately" => true,
        ],
    )
    .await;
    let secret_arn = published_secret_arn(case, &answered)
        .unwrap_or_else(|| panic!("case {case:?}: the modify must publish a secret: {answered}"));
    wait_for_settle();
    let managed = managed_password(case, &router, &secret_arn)
        .await
        .expect("the source's published secret must be readable");

    // The two ways an instance inherits another's credential: a read replica of it, and an
    // instance restored from a snapshot of it. Neither manages a secret of its own.
    let replica = "emu-mmupe-replica";
    let created = expect_ok(
        case,
        call(
            &router,
            "CreateDBInstanceReadReplica",
            &params![
                "DBInstanceIdentifier" => replica,
                "SourceDBInstanceIdentifier" => source,
            ],
        )
        .await,
    );
    assert_eq!(
        published_secret_arn(case, &created),
        None,
        "case {case:?}: a read replica must publish no MasterUserSecret of its own, got \
         {created}"
    );
    expect_ok(
        case,
        call(
            &router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => source,
                "DBSnapshotIdentifier" => "emu-mmupe-snapshot",
            ],
        )
        .await,
    );
    wait_for_settle();
    let restored = expect_ok(
        case,
        call(
            &router,
            "RestoreDBInstanceFromDBSnapshot",
            &params![
                "DBInstanceIdentifier" => "emu-mmupe-restored",
                "DBSnapshotIdentifier" => "emu-mmupe-snapshot",
            ],
        )
        .await,
    );
    assert_eq!(
        published_secret_arn(case, &restored),
        None,
        "case {case:?}: an instance restored from a snapshot must publish no \
         MasterUserSecret of its own, got {restored}"
    );
    wait_for_settle();
    for inheriting in [replica, "emu-mmupe-restored"] {
        let described = describe(case, &router, inheriting).await;
        assert_eq!(
            published_secret_arn(case, &described),
            None,
            "case {case:?}: the describe must report no MasterUserSecret for {inheriting}, \
             got {described}"
        );
    }

    // Deleting an inheriting instance deletes no secret: the source still publishes its
    // own, the secret still holds its password, and the source's engine still admits it.
    expect_ok(
        case,
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => replica,
                "SkipFinalSnapshot" => true,
            ],
        )
        .await,
    );
    assert_eq!(
        managed_password(case, &router, &secret_arn).await.as_ref(),
        Ok(&managed),
        "case {case:?}: deleting a read replica must leave the source's secret holding \
         the password it held"
    );
    let described_source = describe(case, &router, source).await;
    let source_secret = master_user_secret(case, &described_source)
        .unwrap_or_else(|| panic!("case {case:?}: the source must still publish its own secret"));
    assert_eq!(
        (
            text(&source_secret, "SecretArn"),
            text(&source_secret, "SecretStatus")
        ),
        (secret_arn.clone(), ACTIVE.to_string()),
        "case {case:?}: the source must still publish its own secret, active, got \
         {described_source}"
    );
    admitted_login(case, &address, port, &managed).await;

    // Deleting the OWNER takes the secret with it: nothing can read the credential of a
    // database that is going away.
    expect_ok(
        case,
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => source,
                "SkipFinalSnapshot" => true,
            ],
        )
        .await,
    );
    let error = managed_password(case, &router, &secret_arn)
        .await
        .expect_err("the deleted instance's secret must no longer serve its value");
    assert!(
        error.contains("marked for deletion"),
        "case {case:?}: Secrets Manager must report the secret as deleted, got {error}"
    );
}
