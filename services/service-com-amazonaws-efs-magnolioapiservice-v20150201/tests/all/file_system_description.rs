// SPDX-License-Identifier: CC0-1.0

//! The two facts about a `FileSystemDescription` that no other suite in this crate
//! pins, both from the one serializer every file-system path answers through
//! (`state::FileSystem::describe`).
//!
//! Deliberately narrow: the member set of `FileSystemSize`, the `Encrypted` flag, every
//! `KmsKeyId` form and the durability of an accepted throughput change are already
//! asserted in `mutation_results.rs`, and the protection STATUSES a caller may set in
//! `replication_configuration.rs`. What is left, and lives here, is
//!
//! 1. `FileSystemProtection` is reported on an ordinary file system nobody has touched —
//!    "Replication overwrite protection is ENABLED by default"
//!    (`FileSystemProtectionDescription$ReplicationOverwriteProtection`) — on EVERY path
//!    that reports a file system, over a table of create shapes, because the member does
//!    not depend on any of them; and
//! 2. the key an alias resolves to is a property of the alias, not of one process: a
//!    client may pin the reported `KmsKeyId` across a restart.
//!
//! The harness (`Emulator`, `Reply`, `settle`) is the one the sibling suites drive this
//! service through: the same HTTP path the production entrypoint serves.

use serde_json::{json, Value};

use crate::file_system_lifecycle::{settle, Emulator};

#[tokio::test]
async fn every_file_system_reports_its_replication_overwrite_protection() {
    // The rows vary what the create names — encryption, throughput mode, performance
    // mode, One Zone placement, tags — because the protection member depends on none of
    // it: every file system carries a replication overwrite protection status, and a
    // create that asks for nothing gets the documented default.
    let cases: Vec<(&str, Value)> = vec![
        ("nothing but a creation token", json!({ "CreationToken": "prot-1" })),
        (
            "elastic throughput",
            json!({ "CreationToken": "prot-2", "ThroughputMode": "elastic" }),
        ),
        (
            "provisioned throughput",
            json!({
                "CreationToken": "prot-3",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 64.0,
            }),
        ),
        (
            "maxIO performance mode",
            json!({ "CreationToken": "prot-4", "PerformanceMode": "maxIO" }),
        ),
        (
            "encrypted, One Zone, tagged",
            json!({
                "CreationToken": "prot-5",
                "Encrypted": true,
                "AvailabilityZoneName": "us-east-1b",
                "Tags": [{ "Key": "Name", "Value": "protected" }],
            }),
        ),
    ];

    let expected = json!({ "ReplicationOverwriteProtection": "ENABLED" });
    let efs = Emulator::new();
    for (case, create_body) in cases {
        let created = efs.create_file_system(create_body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        let id = created.string("FileSystemId");
        // An update needs the file system `available`, which the clock owes it.
        settle().await;
        // The two other paths that report a file system: the read, and the 202 of an
        // accepted update, both through the harness's own HTTP sender.
        let read = efs
            .describe_file_systems(&format!("FileSystemId={id}"))
            .await;
        let read = read.get("FileSystems")[0].clone();
        let accepted = efs
            .send(
                "PUT",
                &format!("/2015-02-01/file-systems/{id}"),
                Some(json!({ "ThroughputMode": "elastic" })),
            )
            .await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: UpdateFileSystem expected 202, got {} {:?}",
            accepted.status, accepted.body
        );
        for (path, description) in [
            ("CreateFileSystem", &created.body),
            ("DescribeFileSystems", &read),
            ("UpdateFileSystem", &accepted.body),
        ] {
            assert_eq!(
                description["FileSystemProtection"], expected,
                "case {case:?} on {path}: replication overwrite protection is ENABLED by \
                 default and always reported, got {:?}",
                description["FileSystemProtection"]
            );
        }
        settle().await;
    }
}

/// Create one encrypted file system under `key` and answer the `KmsKeyId` its description
/// reports.
async fn reported_key(efs: &Emulator, token: &str, key: &str) -> String {
    let created = efs
        .create_file_system(json!({
            "CreationToken": token,
            "Encrypted": true,
            "KmsKeyId": key,
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "creating under {key:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.string("KmsKeyId")
}

#[tokio::test]
async fn an_alias_resolves_to_the_same_key_in_a_fresh_emulator() {
    // The alias forms the modelled `KmsKeyId` pattern admits
    // (`smithy_query(query="shape",shape="KmsKeyId")`): a bare `alias/NAME`, a name with
    // a path separator, the EFS default-key alias, and an alias ARN. An alias points at
    // ONE key, so the key it resolves to cannot depend on the process that reports it —
    // a client that pins the returned `KmsKeyId` keeps matching after a restart.
    let aliases = [
        "alias/projectKey1",
        "alias/team/data_at-rest",
        "alias/aws/elasticfilesystem",
        "arn:aws:kms:us-east-1:000000000000:alias/projectKey1",
    ];

    let first_run = Emulator::new();
    let mut keys = Vec::new();
    for (index, alias) in aliases.iter().enumerate() {
        let key = reported_key(&first_run, &format!("alias-{index}"), alias).await;
        assert!(
            key.contains(":key/") && !key.contains(":alias/"),
            "alias {alias:?}: KmsKeyId names the key, never an alias, got {key:?}"
        );
        keys.push(key);
    }

    let restarted = Emulator::new();
    for (index, alias) in aliases.iter().enumerate() {
        let again = reported_key(&restarted, &format!("alias-{index}"), alias).await;
        assert_eq!(
            again, keys[index],
            "alias {alias:?}: a fresh emulator must resolve it to the same key, got \
             {again:?} against {:?}",
            keys[index]
        );
    }
}
