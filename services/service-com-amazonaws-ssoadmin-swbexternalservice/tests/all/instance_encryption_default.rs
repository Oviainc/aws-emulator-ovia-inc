// SPDX-License-Identifier: CC0-1.0

//! `DescribeInstance$EncryptionConfigurationDetails` end to end over the `awsJson1_1`
//! wire.
//!
//! The rule under test: the encryption configuration is a property every IAM Identity
//! Center instance HAS from the moment it exists — data at rest is encrypted under the
//! service's own default key until a caller opts into a key of its own — so
//! `DescribeInstance` reports it for every instance it reports at all, as
//! `KeyType: AWS_OWNED_KMS_KEY` with encryption `ENABLED` and no `KmsKeyArn` naming it
//! (`EncryptionConfiguration$KmsKeyArn`: "Cannot be specified when KeyType is
//! AWS_OWNED_KMS_KEY"), and an `UpdateInstance` that carries an `EncryptionConfiguration`
//! REPLACES that configuration rather than bringing one into being.
//!
//! The table spans what the rule generalises over — not which caller wrote something, but
//! which instance is being read: the instance the account already holds, one
//! `CreateInstance` minted (read both while its activation is still owed and once it has
//! fallen due, so the two `InstanceStatus` values of a live instance are both covered),
//! and an organization instance, which is enabled outside this API. The second test walks
//! the configuration through the key types the model's `KmsKeyType` enum names
//! (`AWS_OWNED_KMS_KEY | CUSTOMER_MANAGED_KEY`, from
//! `smithy_query(query="shape",shape="KmsKeyType")`) and back, asserting the member is
//! never absent along the way.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    ENCRYPTION_UPDATE_DELAY, KEY_TYPE_AWS_OWNED, KEY_TYPE_CUSTOMER_MANAGED, KMS_KEY_STATUS_ENABLED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::{empty_account_router, hold_activation};
use crate::ssoadmin_permission_sets::{call, organization_router, router, seeded_instance_arn};

/// A customer managed key ARN in the hyphenated-UUID spelling the model's `KmsKeyArn`
/// pattern admits (`smithy_query(query="shape",shape="EncryptionConfiguration")`).
const KEY_ARN_UUID: &str =
    "arn:aws:kms:us-east-1:123456789012:key/0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";

/// Which instance a row reads, and in which `InstanceStatus`.
#[derive(Debug, Clone, Copy)]
enum Subject {
    /// The instance this account already holds when a request first touches the store.
    HeldInstance,
    /// An instance `CreateInstance` minted, read while the clock still owes it its
    /// activation: `CREATE_IN_PROGRESS`.
    CreatedWhileCreating,
    /// The same created instance, once that activation has fallen due: `ACTIVE`.
    CreatedOnceActive,
    /// An organization instance, which is enabled through AWS Organizations rather than
    /// through this API.
    OrganizationInstance,
}

impl Subject {
    /// The `InstanceStatus` a row's subject must report, which is what makes the two
    /// created rows different reads of one rule.
    fn status(self) -> &'static str {
        match self {
            Subject::CreatedWhileCreating => "CREATE_IN_PROGRESS",
            _ => "ACTIVE",
        }
    }
}

/// A router over the subject, and the subject's `InstanceArn`.
async fn subject(subject: Subject) -> (MultiServiceRouter, String) {
    match subject {
        Subject::HeldInstance => {
            let router = router();
            let arn = seeded_instance_arn(&router).await;
            (router, arn)
        }
        Subject::CreatedWhileCreating | Subject::CreatedOnceActive => {
            let (router, engine) = empty_account_router();
            let (status, body) = call(&router, "CreateInstance", &json!({"Name": "Fresh"})).await;
            assert_eq!(
                status, 200,
                "case {subject:?}: CreateInstance expected 200, got {status} {body}"
            );
            let arn = body["InstanceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {subject:?}: no InstanceArn in {body}"))
                .to_string();
            // Only the CLOCK's instant is moved, so the status the service owes the
            // instance can be read on both sides of it without the test sleeping; the
            // transition itself is still the service's own.
            hold_activation(
                &engine,
                match subject {
                    Subject::CreatedOnceActive => SystemTime::now() - Duration::from_secs(1),
                    _ => SystemTime::now() + Duration::from_secs(3_600),
                },
            );
            (router, arn)
        }
        Subject::OrganizationInstance => organization_router(),
    }
}

/// The `DescribeInstance` body for `arn`, which must answer 200.
async fn describe(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(
        status, 200,
        "DescribeInstance expected 200 for {arn}, got {status} {body}"
    );
    body
}

/// Assert `details` is the service's default encryption configuration: the Amazon Web
/// Services owned key, already in effect, named by no ARN and explaining no failure.
fn assert_aws_owned_default(case: &str, details: &Value) {
    assert_eq!(
        details["KeyType"].as_str(),
        Some(KEY_TYPE_AWS_OWNED),
        "case {case:?}: an instance nobody has opted into a key of its own is encrypted \
         under the AWS owned key, got {details:?}"
    );
    assert_eq!(
        details["EncryptionStatus"].as_str(),
        Some(KMS_KEY_STATUS_ENABLED),
        "case {case:?}: that default is in effect already, so encryption is enabled, got \
         {details:?}"
    );
    assert_eq!(
        details["KmsKeyArn"],
        Value::Null,
        "case {case:?}: no ARN names an AWS owned key, got {details:?}"
    );
    assert_eq!(
        details["EncryptionStatusReason"],
        Value::Null,
        "case {case:?}: no key change failed, so no EncryptionStatusReason may be \
         reported, got {details:?}"
    );
}

/// The rule, over the whole table: whichever instance `DescribeInstance` reports, and in
/// whichever status, it reports that instance's encryption configuration — the AWS owned
/// key default, enabled — without any caller having configured one.
#[tokio::test]
async fn every_instance_reports_the_aws_owned_key_default() {
    for case in [
        Subject::HeldInstance,
        Subject::CreatedWhileCreating,
        Subject::CreatedOnceActive,
        Subject::OrganizationInstance,
    ] {
        let label = format!("{case:?}");
        let (router, arn) = subject(case).await;
        let body = describe(&router, &arn).await;
        assert_eq!(
            body["Status"].as_str(),
            Some(case.status()),
            "case {label:?}: expected InstanceStatus {}, got {:?}",
            case.status(),
            body["Status"]
        );
        assert_ne!(
            body["EncryptionConfigurationDetails"],
            Value::Null,
            "case {label:?}: every instance is encrypted, so its configuration must be \
             reported, got {body}"
        );
        assert_aws_owned_default(&label, &body["EncryptionConfigurationDetails"]);
    }
}

/// The same rule under writes: the default is REPLACED by an `UpdateInstance` that opts
/// the instance into a key of its own and is restored by one that opts back out, and no
/// step along the way — including an update that touches another member entirely — leaves
/// the instance reporting no encryption configuration at all.
#[tokio::test]
async fn an_update_replaces_the_default_rather_than_creating_it() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;

    let before = describe(&router, &arn).await["EncryptionConfigurationDetails"].clone();
    assert_aws_owned_default("before any update", &before);

    // (case, the UpdateInstance request to send, the (KeyType, KmsKeyArn) the instance
    //  must report once the change has fallen due)
    let rows: [(&str, Value, &str, Option<&str>); 4] = [
        (
            "an update that carries another member only",
            json!({"InstanceArn": arn, "Name": "Renamed"}),
            KEY_TYPE_AWS_OWNED,
            None,
        ),
        (
            "opting into a customer managed key",
            json!({
                "InstanceArn": arn,
                "EncryptionConfiguration": {
                    "KeyType": KEY_TYPE_CUSTOMER_MANAGED,
                    "KmsKeyArn": KEY_ARN_UUID,
                },
            }),
            KEY_TYPE_CUSTOMER_MANAGED,
            Some(KEY_ARN_UUID),
        ),
        (
            "opting back out to the AWS owned key",
            json!({
                "InstanceArn": arn,
                "EncryptionConfiguration": {"KeyType": KEY_TYPE_AWS_OWNED},
            }),
            KEY_TYPE_AWS_OWNED,
            None,
        ),
        (
            "a further unrelated update",
            json!({"InstanceArn": arn, "Name": "RenamedAgain"}),
            KEY_TYPE_AWS_OWNED,
            None,
        ),
    ];

    for (case, request, expected_key_type, expected_key_arn) in rows {
        let (status, body) = call(&router, "UpdateInstance", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateInstance expected 200, got {status} {body}"
        );
        // A key change is asynchronous, so wait the service's own span out before reading
        // the settled configuration. A blocking wait, not `tokio::time` (the workspace's
        // tokio carries no `time` feature): this test drives one call at a time.
        std::thread::sleep(ENCRYPTION_UPDATE_DELAY + Duration::from_millis(100));

        let details = describe(&router, &arn).await["EncryptionConfigurationDetails"].clone();
        assert_ne!(
            details,
            Value::Null,
            "case {case:?}: the instance is still encrypted, so its configuration must be \
             reported"
        );
        assert_eq!(
            details["KeyType"].as_str(),
            Some(expected_key_type),
            "case {case:?}: expected KeyType {expected_key_type}, got {details:?}"
        );
        assert_eq!(
            details["KmsKeyArn"].as_str(),
            expected_key_arn,
            "case {case:?}: expected KmsKeyArn {expected_key_arn:?}, got {details:?}"
        );
        assert_eq!(
            details["EncryptionStatus"].as_str(),
            Some(KMS_KEY_STATUS_ENABLED),
            "case {case:?}: a change that has fallen due reports enabled, got {details:?}"
        );
        if expected_key_type == KEY_TYPE_AWS_OWNED {
            assert_aws_owned_default(case, &details);
        }
    }
}
