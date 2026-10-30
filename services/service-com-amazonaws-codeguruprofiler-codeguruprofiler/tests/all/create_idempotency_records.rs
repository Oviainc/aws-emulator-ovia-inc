// SPDX-License-Identifier: CC0-1.0

//! What a `CreateProfilingGroup` idempotency record is good for, end to end over HTTP.
//!
//! The rule every case here asserts: the record of an accepted create belongs to the
//! group that create stored and lives exactly as long as that group, so a retry of ANY
//! accepted create replays its own group — whatever creates ran in between, including
//! ones that reused the same `clientToken` for other names — and a later call can
//! neither rewrite nor drop another create's record.
//!
//! Rows differ along what the rule generalises over: how many groups one token created,
//! the order the retries arrive in, the token's form and length (1 and 64 are the
//! `ClientToken` length bounds the model declares), the name's length, the compute
//! platform and the tag count. No row is privileged: every create in a row is retried.

use crate::profiling_group_lifecycle::{Answer, Harness, group_arn};

/// One `CreateProfilingGroup` request body: the name, plus the members a row varies so
/// no two groups of a row are the same record.
fn create_body(name: &str, compute_platform: Option<&str>, tags: &[(&str, &str)]) -> Vec<u8> {
    let mut body = serde_json::Map::new();
    body.insert(
        "profilingGroupName".to_string(),
        serde_json::Value::from(name),
    );
    if let Some(platform) = compute_platform {
        body.insert(
            "computePlatform".to_string(),
            serde_json::Value::from(platform),
        );
    }
    if !tags.is_empty() {
        body.insert(
            "tags".to_string(),
            serde_json::Value::Object(
                tags.iter()
                    .map(|(key, value)| ((*key).to_string(), serde_json::Value::from(*value)))
                    .collect(),
            ),
        );
    }
    serde_json::to_vec(&serde_json::Value::Object(body)).expect("a serializable body")
}

/// One create of a row: the group it names and the request members that make its record
/// distinguishable from its siblings'.
#[derive(Debug)]
struct Create {
    name: String,
    compute_platform: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

impl Create {
    fn new(
        name: impl Into<String>,
        compute_platform: Option<&'static str>,
        tags: &[(&'static str, &'static str)],
    ) -> Self {
        Self {
            name: name.into(),
            compute_platform,
            tags: tags.to_vec(),
        }
    }

    fn body(&self) -> Vec<u8> {
        create_body(&self.name, self.compute_platform, &self.tags)
    }
}

/// One row: a `clientToken` and every create that presents it, in the order they arrive.
#[derive(Debug)]
struct Row {
    label: &'static str,
    token: String,
    creates: Vec<Create>,
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "two names on one token",
            token: "tok-1".to_string(),
            creates: vec![
                Create::new("alpha", None, &[("env", "dev")]),
                Create::new("beta", Some("AWSLambda"), &[]),
            ],
        },
        Row {
            // A single-character token: the `ClientToken` minimum length the model
            // declares (len 1..64, pattern ^[\w-]+$).
            label: "four names on a one-character token",
            token: "t".to_string(),
            creates: vec![
                Create::new("one", Some("Default"), &[]),
                Create::new("two", None, &[("team", "payments"), ("tier", "prod")]),
                Create::new("three", Some("AWSLambda"), &[("only", "one")]),
                Create::new("four", None, &[]),
            ],
        },
        Row {
            // The `ClientToken` maximum length, and the `ProfilingGroupName` bounds
            // (len 1..255) among the names that share it.
            label: "name-length bounds on a 64-character token",
            token: "r".repeat(64),
            creates: vec![
                Create::new("x", Some("AWSLambda"), &[]),
                Create::new("g".repeat(255), Some("Default"), &[("env", "prod")]),
                Create::new("Mixed_Case-Group", None, &[("zz", "")]),
            ],
        },
        Row {
            label: "one name, one token",
            token: "single-create-token".to_string(),
            creates: vec![Create::new("solo", None, &[("env", "test")])],
        },
    ]
}

/// A read's description with the `profilingStatus` container only a READ carries removed
/// — so a describe's payload is comparable with the create's own.
fn without_status(label: &str, answer: &Answer) -> serde_json::Value {
    let mut description = answer.json();
    let removed = description
        .as_object_mut()
        .unwrap_or_else(|| panic!("{label}: expected a JSON object, got nothing readable"))
        .remove("profilingStatus");
    assert!(
        removed.is_some(),
        "{label}: a read must carry profilingStatus, got {description:?}"
    );
    description
}

#[tokio::test]
async fn every_create_record_survives_later_creates_reusing_its_client_token() {
    for row in rows() {
        let harness = Harness::new();
        let Row {
            label,
            token,
            creates,
        } = row;

        // Every create of the row presents the SAME token, in order.
        let mut created: Vec<serde_json::Value> = Vec::new();
        for create in &creates {
            let answer = harness
                .send(
                    "POST",
                    &format!("/profilingGroups?clientToken={token}"),
                    "application/json",
                    create.body(),
                )
                .await;
            assert_eq!(
                answer.status, 201,
                "case {label}/{create:?}: a create of a free name must answer 201, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let description = answer.json();
            assert_eq!(
                description.get("arn").and_then(serde_json::Value::as_str),
                Some(group_arn(&create.name).as_str()),
                "case {label}/{create:?}: wrong arn, got {description:?}"
            );
            created.push(description);
        }

        // The decisive assertion: retry EVERY create of the row — in the reverse of the
        // order they arrived, so the first create, whose record the later ones were
        // supposed to have overwritten, is retried too. Each retry must replay its own
        // group, byte for byte.
        for (create, first) in creates.iter().zip(created.iter()).rev() {
            let replay = harness
                .send(
                    "POST",
                    &format!("/profilingGroups?clientToken={token}"),
                    "application/json",
                    create.body(),
                )
                .await;
            assert_eq!(
                replay.status, 201,
                "case {label}/{create:?}: retrying a create on its own clientToken must \
                 replay it with 201, got {} {}",
                replay.status,
                String::from_utf8_lossy(&replay.body)
            );
            assert_eq!(
                &replay.json(),
                first,
                "case {label}/{create:?}: the retry must replay the group that create \
                 stored, got {}",
                String::from_utf8_lossy(&replay.body)
            );
        }

        // A create of one of those names under ANOTHER token is a different create of a
        // name that is taken: 409, and it leaves the records alone.
        for (index, create) in creates.iter().enumerate() {
            let conflict = harness
                .send(
                    "POST",
                    &format!("/profilingGroups?clientToken=other-{index}"),
                    "application/json",
                    create.body(),
                )
                .await;
            assert_eq!(
                conflict.status, 409,
                "case {label}/{create:?}: another token on a taken name must answer 409, \
                 got {} {}",
                conflict.status,
                String::from_utf8_lossy(&conflict.body)
            );
            assert_eq!(
                conflict.error_code(),
                "ConflictException",
                "case {label}/{create:?}: expected ConflictException, got {}",
                String::from_utf8_lossy(&conflict.body)
            );
        }

        // Each group reads back as the record its create stored, and each retry still
        // replays it after the refusals above.
        for (create, first) in creates.iter().zip(created.iter()) {
            let described = harness.describe(&create.name).await;
            assert_eq!(
                described.status, 200,
                "case {label}/{create:?}: DescribeProfilingGroup must answer 200, got {} {}",
                described.status,
                String::from_utf8_lossy(&described.body)
            );
            assert_eq!(
                &without_status(label, &described),
                first,
                "case {label}/{create:?}: describe must report the record the create \
                 stored, got {}",
                String::from_utf8_lossy(&described.body)
            );
            let replay = harness
                .send(
                    "POST",
                    &format!("/profilingGroups?clientToken={token}"),
                    "application/json",
                    create.body(),
                )
                .await;
            assert_eq!(
                &replay.json(),
                first,
                "case {label}/{create:?}: a refused create must not disturb a stored \
                 record, got {}",
                String::from_utf8_lossy(&replay.body)
            );
        }
    }
}

#[tokio::test]
async fn deleting_one_group_takes_only_its_own_create_record() {
    for row in rows() {
        let harness = Harness::new();
        let Row {
            label,
            token,
            creates,
        } = row;

        let mut created: Vec<serde_json::Value> = Vec::new();
        for create in &creates {
            let answer = harness
                .send(
                    "POST",
                    &format!("/profilingGroups?clientToken={token}"),
                    "application/json",
                    create.body(),
                )
                .await;
            assert_eq!(
                answer.status, 201,
                "case {label}/{create:?}: create must succeed, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            created.push(answer.json());
        }

        // Delete the FIRST group of the row: the record of the create that stored it goes
        // with it, and no other create's record does.
        let deleted = &creates[0];
        let delete = harness
            .send(
                "DELETE",
                &format!("/profilingGroups/{}", deleted.name),
                "application/json",
                Vec::new(),
            )
            .await;
        assert_eq!(
            delete.status, 204,
            "case {label}/{deleted:?}: DeleteProfilingGroup must answer 204, got {} {}",
            delete.status,
            String::from_utf8_lossy(&delete.body)
        );

        // Every OTHER create of the row still replays its own group.
        for (create, first) in creates.iter().zip(created.iter()).skip(1) {
            let replay = harness
                .send(
                    "POST",
                    &format!("/profilingGroups?clientToken={token}"),
                    "application/json",
                    create.body(),
                )
                .await;
            assert_eq!(
                &replay.json(),
                first,
                "case {label}/{create:?}: deleting another group must not drop this \
                 create's record, got {}",
                String::from_utf8_lossy(&replay.body)
            );
        }

        // The deleted group's own record is gone, so the same request stores a FRESH
        // group rather than replaying the one that was removed.
        let recreated = harness
            .send(
                "POST",
                &format!("/profilingGroups?clientToken={token}"),
                "application/json",
                deleted.body(),
            )
            .await;
        assert_eq!(
            recreated.status, 201,
            "case {label}/{deleted:?}: re-creating a deleted group must answer 201, got {} {}",
            recreated.status,
            String::from_utf8_lossy(&recreated.body)
        );
        let described = harness.describe(&deleted.name).await;
        assert_eq!(
            described.status, 200,
            "case {label}/{deleted:?}: the re-created group must be readable, got {} {}",
            described.status,
            String::from_utf8_lossy(&described.body)
        );
        assert_eq!(
            without_status(label, &described),
            recreated.json(),
            "case {label}/{deleted:?}: describe must report the re-created record, got {}",
            String::from_utf8_lossy(&described.body)
        );
    }
}
