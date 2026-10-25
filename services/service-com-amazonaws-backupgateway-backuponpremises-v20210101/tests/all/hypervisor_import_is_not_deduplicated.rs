// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ImportHypervisorConfiguration` reached REPEATEDLY with the very
//! same request.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour.
//!
//! The rule under test: no input member of this operation is an identity or an
//! idempotency token, so every accepted import mints its OWN hypervisor record under a
//! freshly minted `HypervisorArn` — repeating one request, however many times and whether
//! one at a time or all at once, leaves one record per call and is never conflated with,
//! or refused because of, a record already stored.
//!
//! The table below spans that class: the number of identical repeats, whether they are
//! dispatched one after another or at the same instant, and which members the repeated
//! body carries (the two required ones alone, the `@sensitive` credentials, a `KmsKeyArn`
//! in both forms its pattern accepts, `Tags`). The sibling suite
//! `hypervisor_import_concurrency` spans the number of RACERS and the host forms;
//! `hypervisor_host_is_not_a_key` spans how a record came to hold a shared host.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::gateway_lifecycle::{
    build_engine, build_router, call, hypervisor_arns_for_host, hypervisor_count, Reply,
};

/// One row: the body every repeat carries, how many repeats there are, and whether they
/// are dispatched at the same instant.
#[derive(Debug)]
struct Row {
    case: &'static str,
    repeats: usize,
    concurrent: bool,
    name: String,
    host: String,
    /// The members beyond the two required ones that every repeat carries, as a JSON
    /// object merged into the request body.
    extras: Value,
}

/// The `ImportHypervisorConfiguration` body for `row`, identical for every repeat.
fn request_body(row: &Row) -> Value {
    let mut body = json!({ "Name": row.name, "Host": row.host });
    for (member, value) in row
        .extras
        .as_object()
        .expect("a row's extra members are a JSON object")
    {
        body[member] = value.clone();
    }
    body
}

/// The tags a body's `Tags` member asks for, as a map.
fn requested_tags(extras: &Value) -> BTreeMap<String, String> {
    extras
        .get("Tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Issue `repeats` copies of `body`, either one after another or all at once, and hand
/// back what each one answered.
async fn issue_repeats(
    router: &Arc<MultiServiceRouter>,
    body: &Value,
    repeats: usize,
    concurrent: bool,
) -> Vec<Reply> {
    if !concurrent {
        let mut replies = Vec::new();
        for _ in 0..repeats {
            replies.push(call(router, "ImportHypervisorConfiguration", body).await);
        }
        return replies;
    }

    let mut tasks = Vec::new();
    for _ in 0..repeats {
        let router = Arc::clone(router);
        let body = body.clone();
        tasks.push(tokio::spawn(async move {
            call(&router, "ImportHypervisorConfiguration", &body).await
        }));
    }
    let mut replies = Vec::new();
    for task in tasks {
        replies.push(task.await.expect("a repeated import task must not panic"));
    }
    replies
}

/// The rows of the table, built here because two of them need a generated host.
///
/// `Host` is `@length(min: 3, max: 128)` with pattern `^.+$` and `Name` is
/// `@length(min: 1, max: 100)` with pattern `^[a-zA-Z0-9-]*$`
/// (`smithy_query(query="operation",operation="ImportHypervisorConfiguration")`), so
/// every name and host below is one the members accept. `KmsKeyArn` is `@length(min: 50,
/// max: 500)` and its pattern admits a `key/` ARN and an `alias/` ARN; `Username` is
/// `@sensitive` with pattern `^[ -\.0-\[\]-~]*[!-\.0-\[\]-~][ -\.0-\[\]-~]*$` and
/// `Password` is `@sensitive` with pattern `^[ -~]+$`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one import, the required members alone",
            repeats: 1,
            concurrent: false,
            name: "hv-once".to_string(),
            host: "198.51.100.11".to_string(),
            extras: json!({}),
        },
        Row {
            case: "two sequential imports of the required members alone",
            repeats: 2,
            concurrent: false,
            name: "hv-twice".to_string(),
            host: "hypervisor.example.com".to_string(),
            extras: json!({}),
        },
        Row {
            case: "two sequential imports carrying the credentials",
            repeats: 2,
            concurrent: false,
            name: "hv-with-credentials".to_string(),
            host: "198.51.100.12".to_string(),
            extras: json!({ "Username": "bgw-user", "Password": "Passw0rd!" }),
        },
        Row {
            case: "three sequential imports carrying a key/ KmsKeyArn and tags",
            repeats: 3,
            concurrent: false,
            name: "hv-with-key".to_string(),
            host: "a.b".to_string(),
            extras: json!({
                "KmsKeyArn": "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "Tags": [{ "Key": "env", "Value": "test" }, { "Key": "owner", "Value": "bgw" }],
            }),
        },
        Row {
            case: "three concurrent imports carrying an alias/ KmsKeyArn",
            repeats: 3,
            concurrent: true,
            name: "hv-with-alias".to_string(),
            host: "the hypervisor box".to_string(),
            extras: json!({
                "KmsKeyArn": "arn:aws:kms:us-west-2:000000000000:alias/backup-gateway-repeated-import",
            }),
        },
        Row {
            case: "five concurrent imports of one whole body",
            repeats: 5,
            concurrent: true,
            name: "hv-five-racers".to_string(),
            host: "racers.hypervisor.example.com".to_string(),
            extras: json!({
                "Username": "bgw-user",
                "Password": "Passw0rd!",
                "Tags": [{ "Key": "env", "Value": "race" }],
            }),
        },
        Row {
            case: "five sequential imports at the 100-character maximum name",
            repeats: 5,
            concurrent: false,
            name: format!("hv-{}", "n".repeat(100 - "hv-".len())),
            host: "198.51.100.13".to_string(),
            extras: json!({}),
        },
        Row {
            case: "four concurrent imports at the 128-character maximum host",
            repeats: 4,
            concurrent: true,
            name: "hv-longest-host".to_string(),
            host: format!("{}.example.com", "h".repeat(128 - ".example.com".len())),
            extras: json!({}),
        },
    ]
}

/// RULE: an import mints one record per call, so repeating the identical request mints a
/// distinct `HypervisorArn` every time and each record reports the members that request
/// carried — nothing is deduplicated and nothing is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeating_one_import_mints_one_record_per_call() {
    for row in rows() {
        // A store of its own per row, so the record count asserted below is exactly what
        // this row wrote.
        let engine = build_engine();
        let router = Arc::new(build_router(&engine));
        let case = row.case;
        let body = request_body(&row);
        let tags = requested_tags(&row.extras);
        let kms_key_arn = row.extras.get("KmsKeyArn").and_then(Value::as_str);

        let replies = issue_repeats(&router, &body, row.repeats, row.concurrent).await;

        let mut arns: Vec<String> = Vec::new();
        for (attempt, reply) in replies.iter().enumerate() {
            assert_eq!(
                (reply.status, reply.code.as_deref()),
                (200, None),
                "case {case:?}: repeat {attempt} of an identical import must answer 200 \
                 with no error code, got {} {:?} {:?}",
                reply.status,
                reply.code,
                reply.body
            );
            arns.push(
                reply
                    .str_at("/HypervisorArn")
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: repeat {attempt}: no HypervisorArn in {:?}",
                            reply.body
                        )
                    })
                    .to_string(),
            );
        }

        let unique: BTreeSet<String> = arns.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            row.repeats,
            "case {case:?}: {} identical imports must mint {} distinct ARNs, got {arns:?}",
            row.repeats,
            row.repeats
        );
        assert_eq!(
            hypervisor_count(&engine),
            row.repeats,
            "case {case:?}: {} identical imports must leave {} records, got {}",
            row.repeats,
            row.repeats,
            hypervisor_count(&engine)
        );
        assert_eq!(
            hypervisor_arns_for_host(&engine, &row.host),
            unique,
            "case {case:?}: every minted record must be stored for the requested host, \
             got {:?}",
            hypervisor_arns_for_host(&engine, &row.host)
        );

        // Every record reports the members the repeated request carried, read back
        // through the implemented reader, and carries the tags that request asked for.
        for arn in &unique {
            let read = call(&router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await;
            let reported = (
                read.str_at("/Hypervisor/Name"),
                read.str_at("/Hypervisor/Host"),
                read.str_at("/Hypervisor/HypervisorArn"),
                read.str_at("/Hypervisor/KmsKeyArn"),
            );
            assert_eq!(
                (read.status, reported),
                (
                    200,
                    (
                        Some(row.name.as_str()),
                        Some(row.host.as_str()),
                        Some(arn.as_str()),
                        kms_key_arn,
                    )
                ),
                "case {case:?}: {arn} must report the requested Name, Host, its own ARN \
                 and the requested KmsKeyArn, got {} {reported:?}",
                read.status
            );

            let listed = call(
                &router,
                "ListTagsForResource",
                &json!({ "ResourceArn": arn }),
            )
            .await;
            assert_eq!(
                (listed.status, listed.tags()),
                (200, tags.clone()),
                "case {case:?}: {arn} must carry the requested tags {tags:?}, got {} {:?}",
                listed.status,
                listed.body
            );
        }
    }
}

/// One row of the table below: the case, then the `(Name, Host)` of the first import and
/// of the second.
type SharedMembersRow<'a> = (&'a str, (&'a str, &'a str), (&'a str, &'a str));

/// RULE (the same rule, from the other direction): which members two imports happen to
/// share constrains neither of them — each mints its own record carrying its own members,
/// and the earlier record is left exactly as it was.
#[tokio::test]
async fn which_members_two_imports_share_constrains_neither() {
    let longest_host = format!("{}.example.com", "h".repeat(128 - ".example.com".len()));
    let rows: Vec<SharedMembersRow<'_>> = vec![
        (
            "nothing shared",
            ("hv-alpha", "198.51.100.21"),
            ("hv-beta", "198.51.100.22"),
        ),
        (
            "the host shared",
            ("hv-alpha", "198.51.100.23"),
            ("hv-beta", "198.51.100.23"),
        ),
        (
            "the name shared",
            ("hv-shared-name", "198.51.100.24"),
            ("hv-shared-name", "198.51.100.25"),
        ),
        (
            "both shared",
            ("hv-shared-both", "198.51.100.26"),
            ("hv-shared-both", "198.51.100.26"),
        ),
        (
            // The published minimum of `Host` (3) and of `Name` (1).
            "both shared, at the shortest name and host",
            ("h", "a.b"),
            ("h", "a.b"),
        ),
        (
            "both shared, at the longest host",
            ("hv-longest", longest_host.as_str()),
            ("hv-longest", longest_host.as_str()),
        ),
    ];

    for (case, first, second) in rows {
        let engine = build_engine();
        let router = Arc::new(build_router(&engine));

        let mut arns = Vec::new();
        for (attempt, (name, host)) in [first, second].into_iter().enumerate() {
            let reply = call(
                &router,
                "ImportHypervisorConfiguration",
                &json!({ "Name": name, "Host": host }),
            )
            .await;
            assert_eq!(
                (reply.status, reply.code.as_deref()),
                (200, None),
                "case {case:?}: import {attempt} must answer 200 with no error code, got \
                 {} {:?} {:?}",
                reply.status,
                reply.code,
                reply.body
            );
            arns.push(
                reply
                    .str_at("/HypervisorArn")
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: import {attempt}: no HypervisorArn in {:?}",
                            reply.body
                        )
                    })
                    .to_string(),
            );
        }

        assert_ne!(
            arns[0], arns[1],
            "case {case:?}: the two imports must mint ARNs of their own, got {arns:?}"
        );
        assert_eq!(
            hypervisor_count(&engine),
            2,
            "case {case:?}: two imports must leave two records, got {}",
            hypervisor_count(&engine)
        );

        // Both records are still there afterwards, each reporting its own members.
        for (attempt, (name, host)) in [first, second].into_iter().enumerate() {
            let read = call(
                &router,
                "GetHypervisor",
                &json!({ "HypervisorArn": arns[attempt] }),
            )
            .await;
            let reported = (
                read.str_at("/Hypervisor/Name"),
                read.str_at("/Hypervisor/Host"),
            );
            assert_eq!(
                (read.status, reported),
                (200, (Some(name), Some(host))),
                "case {case:?}: record {attempt} must report its own Name and Host, got \
                 {} {reported:?}",
                read.status
            );
        }
    }
}
