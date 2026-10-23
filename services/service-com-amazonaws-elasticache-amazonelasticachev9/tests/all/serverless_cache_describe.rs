// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `DescribeServerlessCaches` answers when the
//! `ServerlessCacheName` it is given names no stored serverless cache.
//!
//! The lifecycle of a serverless cache — create, read back, page, refuse — is
//! covered by the `serverless_cache_lifecycle` suite beside this one. What this
//! suite pins is the refusal ITSELF: the HTTP status, the modelled fault code
//! and the message, which ElastiCache builds from the request ("Serverless cache
//! with name <name> not found.") rather than from anything it stores. A message
//! built from the request is a requirement, so it is asserted here over a table
//! of names rather than for one recorded name.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, using the request builder and XML reader the
//! `cache_cluster_lifecycle` suite marks `pub(crate)`; nothing reaches into the
//! service's state struct.

use crate::harness::{params, Emulator};

/// Describe one serverless cache by name.
async fn describe_cache(emulator: &Emulator, name: &str) -> crate::harness::Reply {
    emulator
        .call(
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", name)]),
        )
        .await
}

#[tokio::test]
async fn a_name_no_stored_cache_carries_is_refused_and_the_fault_names_it() {
    let emulator = Emulator::new();
    // One stored cache, so the refusals below are about the NAME and not about
    // an empty store. A cache cluster with a similar id is created too: a
    // provisioned cluster is a different resource, so its id is not a
    // serverless-cache name.
    let created = emulator
        .call(
            "CreateServerlessCache",
            &params(&[("ServerlessCacheName", "stored-cache"), ("Engine", "valkey")]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the stored cache must be created, got {}",
        created.body
    );
    let cluster = emulator
        .call(
            "CreateCacheCluster",
            &params(&[
                ("CacheClusterId", "shadow-cluster"),
                ("CacheNodeType", "cache.t3.micro"),
            ]),
        )
        .await;
    assert_eq!(
        cluster.status, 200,
        "the cache cluster the table needs must be created, got {}",
        cluster.body
    );

    // The axis: every way a name can fail to name a stored serverless cache.
    let cases = [
        ("never created", "no-such-cache"),
        ("a prefix of a stored name", "stored"),
        ("a stored name with a suffix", "stored-cache-2"),
        ("the id of a cache cluster", "shadow-cluster"),
        ("a single character", "x"),
        (
            "a long name",
            "a-serverless-cache-name-that-nobody-ever-created-in-this-account",
        ),
    ];

    for (label, name) in cases {
        let refused = describe_cache(&emulator, name).await;
        assert_eq!(
            refused.status, 404,
            "case {label:?} ({name}): an unknown name must answer 404, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ServerlessCacheNotFoundFault",
            "case {label:?} ({name}): must answer the modelled fault, got {}",
            refused.body
        );
        // The message is built from the request: it names the cache the caller
        // asked for, exactly as asked for.
        let expected = format!("Serverless cache with name {name} not found.");
        assert_eq!(
            refused.text("Error/Message"),
            Some(expected.as_str()),
            "case {label:?} ({name}): the fault must name the requested cache, got {:?}",
            refused.text("Error/Message")
        );
    }

    // A refusal stores nothing and removes nothing: the stored cache is still
    // reported, and still the only one.
    let stored = describe_cache(&emulator, "stored-cache").await;
    assert_eq!(
        stored.status, 200,
        "the stored cache must still be reported after the refusals, got {}",
        stored.body
    );
    let listed = emulator.call("DescribeServerlessCaches", &[]).await;
    assert_eq!(
        listed.all("ServerlessCacheName"),
        vec!["stored-cache"],
        "the refusals must not have changed what is stored, got {:?}",
        listed.all("ServerlessCacheName")
    );
}

#[tokio::test]
async fn the_name_is_matched_case_insensitively_and_the_refusal_echoes_the_request() {
    let emulator = Emulator::new();
    // `ServerlessCacheName` "is stored as a lowercase string", so a lookup in
    // any case finds the cache; the refusal for a name that exists in NO case
    // still quotes the request's own spelling.
    let created = emulator
        .call(
            "CreateServerlessCache",
            &params(&[("ServerlessCacheName", "Mixed-Case-Cache"), ("Engine", "redis")]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the stored cache must be created, got {}",
        created.body
    );

    for lookup in ["Mixed-Case-Cache", "mixed-case-cache", "MIXED-CASE-CACHE"] {
        let found = describe_cache(&emulator, lookup).await;
        assert_eq!(
            found.status, 200,
            "case {lookup}: a lookup in any case must find the cache, got {} / {}",
            found.status, found.body
        );
        assert_eq!(
            found.text("ServerlessCacheName"),
            Some("mixed-case-cache"),
            "case {lookup}: the stored (lowercase) name is what is reported, got {:?}",
            found.text("ServerlessCacheName")
        );
    }

    for missing in ["Absent-Cache", "ABSENT-CACHE"] {
        let refused = describe_cache(&emulator, missing).await;
        assert_eq!(
            refused.status, 404,
            "case {missing}: must answer 404, got {} / {}",
            refused.status, refused.body
        );
        let expected = format!("Serverless cache with name {missing} not found.");
        assert_eq!(
            refused.text("Error/Message"),
            Some(expected.as_str()),
            "case {missing}: the fault quotes the request's own spelling, got {:?}",
            refused.text("Error/Message")
        );
    }
}
