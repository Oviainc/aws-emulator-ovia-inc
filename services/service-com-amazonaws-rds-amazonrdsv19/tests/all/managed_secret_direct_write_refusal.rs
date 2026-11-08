// SPDX-License-Identifier: CC0-1.0

//! Who may change the Secrets Manager secret RDS MANAGES for a resource's master user
//! password, driven across the two services over one engine.
//!
//! THE RULE: that secret is a secret RDS owns, so it is changed through RDS and not through
//! Secrets Manager. A caller's own `PutSecretValue` or `DeleteSecret` on it is refused with
//! `InvalidRequestException` and changes nothing, while the two RDS requests that DO change
//! it — `RotateMasterUserPassword`, which stakes a new password, and turning
//! `ManageMasterUserPassword` off, which deletes the secret — still take effect, because
//! RDS makes those calls as its own service principal.
//!
//! The rows differ along what the rule generalises over: the kind of resource whose
//! password is managed (a DB instance, a DB cluster), the engine family, and the key the
//! secret is encrypted under (a customer managed KMS key, and the Amazon Web Services
//! managed key) — the last of which is what a direct write would have to re-encrypt under.
//!
//! Secrets Manager and KMS are mounted beside the RDS service
//! ([`crate::managed_master_user_password::emulator`]), because the secret read back here is
//! a real secret of this emulator.

use aws_sdk_secretsmanager::operation::delete_secret::DeleteSecretError;
use aws_sdk_secretsmanager::operation::put_secret_value::PutSecretValueError;

use crate::db_instances::{call, expect_ok, text, wait_for_settle};
use crate::managed_master_user_password::{
    emulator, held_credentials, kms, master_user_secret, secrets_manager, the_resource,
};

/// Which resource a row manages the password of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Instance,
    Cluster,
}

impl Owner {
    fn create(self) -> &'static str {
        match self {
            Self::Instance => "CreateDBInstance",
            Self::Cluster => "CreateDBCluster",
        }
    }

    fn modify(self) -> &'static str {
        match self {
            Self::Instance => "ModifyDBInstance",
            Self::Cluster => "ModifyDBCluster",
        }
    }

    fn identifier_member(self) -> &'static str {
        match self {
            Self::Instance => "DBInstanceIdentifier",
            Self::Cluster => "DBClusterIdentifier",
        }
    }

    fn element(self) -> &'static str {
        match self {
            Self::Instance => "DBInstance",
            Self::Cluster => "DBCluster",
        }
    }
}

/// One row: the resource whose managed secret a caller tries to write, and whether the
/// secret is encrypted under a key the request named.
struct Row {
    case: &'static str,
    owner: Owner,
    engine: &'static str,
    master_username: &'static str,
    extra: &'static [(&'static str, &'static str)],
    /// Whether the create names a customer managed key for the secret
    /// (`MasterUserSecretKmsKeyId`).
    names_kms_key: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "an RDS for PostgreSQL DB instance under a key of its own",
        owner: Owner::Instance,
        engine: "postgres",
        master_username: "sim2real",
        extra: &[("DBInstanceClass", "db.t4g.micro"), ("AllocatedStorage", "20")],
        names_kms_key: true,
    },
    Row {
        case: "an RDS for MySQL DB instance under the managed key",
        owner: Owner::Instance,
        engine: "mysql",
        master_username: "admin",
        extra: &[("DBInstanceClass", "db.t3.micro"), ("AllocatedStorage", "20")],
        names_kms_key: false,
    },
    Row {
        case: "an Aurora PostgreSQL DB cluster under a key of its own",
        owner: Owner::Cluster,
        engine: "aurora-postgresql",
        master_username: "clusteradmin",
        extra: &[],
        names_kms_key: true,
    },
];

/// The sentence Secrets Manager refuses a direct WRITE to a secret RDS manages with: fixed
/// text naming the owning service by the name AWS publishes for it and by its service id.
fn write_refusal_sentence() -> String {
    "This secret is managed by Amazon RDS (service ID: rds), and you must use that service \
     to update it. For more information, see \
     https://docs.aws.amazon.com/secretsmanager/latest/userguide/service-linked-secrets.html."
        .to_string()
}

/// The sentence it refuses a DELETE of one with: fixed text naming the owning service by
/// its service id.
fn delete_refusal_sentence() -> String {
    "Operation is not allowed on secret owned by rds".to_string()
}

#[tokio::test]
async fn a_managed_master_user_secret_is_written_through_rds_and_not_through_secrets_manager() {
    let router = emulator();
    let key = kms(&router)
        .create_key()
        .description("the key an RDS-managed master user secret is encrypted under")
        .send()
        .await
        .expect("KMS mints a key")
        .key_metadata()
        .expect("a minted key reports its metadata")
        .arn()
        .expect("a minted key reports its ARN")
        .to_string();

    for (index, row) in ROWS.iter().enumerate() {
        let identifier = format!("emu-owned-write-{index}");
        let mut params: Vec<(&str, String)> = vec![
            (row.owner.identifier_member(), identifier.clone()),
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
            params.push(("MasterUserSecretKmsKeyId", key.clone()));
        }
        let created = expect_ok(row.case, call(&router, row.owner.create(), &params).await);
        let envelope = the_resource(row.case, &created, row.owner.element());
        let secret = master_user_secret(row.case, &envelope).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the resource to report MasterUserSecret, got {envelope}",
                row.case
            )
        });
        let secret_arn = text(&secret, "SecretArn");
        // The secret settles into service, as the status the clock owes it.
        wait_for_settle();
        let minted = held_credentials(row.case, &router, &secret_arn).await;

        // 1. A caller's own write of the secret is refused, and the secret still holds the
        //    password RDS generated.
        let refused = secrets_manager(&router)
            .put_secret_value()
            .secret_id(&secret_arn)
            .client_request_token("a-callers-own-version-token-000001")
            .secret_string("{\"username\":\"sim2real\",\"password\":\"x\"}")
            .send()
            .await
            .err()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected PutSecretValue on the managed secret to be refused",
                    row.case
                )
            });
        let service_error = refused.into_service_error();
        let message = match &service_error {
            PutSecretValueError::InvalidRequestException(refusal) => {
                refusal.message().unwrap_or_default().to_string()
            }
            other => panic!(
                "case {:?}: expected InvalidRequestException from PutSecretValue, got {other:?}",
                row.case
            ),
        };
        assert_eq!(
            message,
            write_refusal_sentence(),
            "case {:?}: expected the service-linked-secret sentence, got {message:?}",
            row.case
        );

        // 2. A caller's own delete of it is refused too, with the sentence AWS words for
        //    that call.
        let refused = secrets_manager(&router)
            .delete_secret()
            .secret_id(&secret_arn)
            .force_delete_without_recovery(true)
            .send()
            .await
            .err()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected DeleteSecret on the managed secret to be refused",
                    row.case
                )
            });
        let service_error = refused.into_service_error();
        let message = match &service_error {
            DeleteSecretError::InvalidRequestException(refusal) => {
                refusal.message().unwrap_or_default().to_string()
            }
            other => panic!(
                "case {:?}: expected InvalidRequestException from DeleteSecret, got {other:?}",
                row.case
            ),
        };
        assert_eq!(
            message,
            delete_refusal_sentence(),
            "case {:?}: expected the owned-secret sentence, got {message:?}",
            row.case
        );

        // Both refusals were inert: the secret still holds exactly what RDS stored.
        let after_refusals = held_credentials(row.case, &router, &secret_arn).await;
        assert_eq!(
            after_refusals.password, minted.password,
            "case {:?}: a refused direct write must leave the managed credentials alone",
            row.case
        );
        assert_eq!(
            after_refusals.username, minted.username,
            "case {:?}: a refused direct write must leave the managed credentials alone",
            row.case
        );

        // 3. The way the password IS changed: through RDS, which rotates the secret it owns.
        let rotated = expect_ok(
            row.case,
            call(
                &router,
                row.owner.modify(),
                &[
                    (row.owner.identifier_member(), identifier.as_str()),
                    ("RotateMasterUserPassword", "true"),
                    ("ApplyImmediately", "true"),
                ],
            )
            .await,
        );
        let rotated_secret = master_user_secret(row.case, &the_resource(row.case, &rotated, row.owner.element()))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the rotated resource to still report its secret",
                    row.case
                )
            });
        assert_eq!(
            text(&rotated_secret, "SecretArn"),
            secret_arn,
            "case {:?}: a rotation stakes a new version of the SAME secret",
            row.case
        );
        let after_rotation = held_credentials(row.case, &router, &secret_arn).await;
        assert_ne!(
            after_rotation.password, minted.password,
            "case {:?}: expected the rotation through RDS to stake a new password",
            row.case
        );
        assert_eq!(
            after_rotation.username, minted.username,
            "case {:?}: a rotation keeps the master user it is for",
            row.case
        );

        // 4. And the way it is DELETED: through RDS, which releases the secret it owns when
        //    a request turns that management off. The secret goes rather than entering a
        //    recovery window — nobody but RDS could take it back out of one — so Secrets
        //    Manager stops holding it altogether, which is what lets the same resource have
        //    a managed secret minted for it again.
        wait_for_settle();
        let released = expect_ok(
            row.case,
            call(
                &router,
                row.owner.modify(),
                &[
                    (row.owner.identifier_member(), identifier.as_str()),
                    ("ManageMasterUserPassword", "false"),
                    ("MasterUserPassword", "a-password-of-the-callers-own"),
                    ("ApplyImmediately", "true"),
                ],
            )
            .await,
        );
        assert_eq!(
            master_user_secret(row.case, &the_resource(row.case, &released, row.owner.element())),
            None,
            "case {:?}: a resource that manages no password reports no MasterUserSecret",
            row.case
        );
        // The removal of a secret deleted without a recovery window is a short span away
        // (Secrets Manager "performs the actual deletion with an asynchronous background
        // process"), so the read is taken until it reports the secret gone rather than once.
        let mut last = None;
        for _ in 0..40 {
            match secrets_manager(&router)
                .describe_secret()
                .secret_id(&secret_arn)
                .send()
                .await
            {
                Err(error) => {
                    let service_error = error.into_service_error();
                    assert!(
                        service_error.is_resource_not_found_exception(),
                        "case {:?}: expected the released secret to be gone, got \
                         {service_error:?}",
                        row.case
                    );
                    last = None;
                    break;
                }
                Ok(described) => {
                    assert!(
                        described.deleted_date().is_some(),
                        "case {:?}: a released secret is at least marked deleted, got \
                         {described:?}",
                        row.case
                    );
                    last = Some(described);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        assert!(
            last.is_none(),
            "case {:?}: expected Secrets Manager to stop holding the released secret, still \
             describes {last:?}",
            row.case
        );

        // Which is what lets the SAME resource manage a password again: the name the secret
        // carries is the resource's own, so a secret still occupying it would refuse.
        let managed_again = expect_ok(
            row.case,
            call(
                &router,
                row.owner.modify(),
                &[
                    (row.owner.identifier_member(), identifier.as_str()),
                    ("ManageMasterUserPassword", "true"),
                    ("ApplyImmediately", "true"),
                ],
            )
            .await,
        );
        let minted_again = master_user_secret(
            row.case,
            &the_resource(row.case, &managed_again, row.owner.element()),
        )
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a managed secret to be minted again, got {managed_again}",
                row.case
            )
        });
        let held_again = held_credentials(row.case, &router, &text(&minted_again, "SecretArn")).await;
        assert_eq!(
            held_again.username, minted.username,
            "case {:?}: the new managed secret holds the resource's master user",
            row.case
        );
    }
}
