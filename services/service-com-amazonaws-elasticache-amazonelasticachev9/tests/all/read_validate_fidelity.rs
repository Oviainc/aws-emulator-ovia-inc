// SPDX-License-Identifier: CC0-1.0

//! Read and validation fidelity on the implemented ElastiCache surface: what a
//! discovery read refuses, the order a log is reported in, the identifier an
//! encrypted resource stores, the description a blank one carries, and the words
//! a refusal is made of.
//!
//! The rules under test, each one more general than the single call that showed
//! it up:
//!
//! * `DescribeCacheEngineVersions$Engine` names an engine of the published
//!   catalog, so a value that names none is refused rather than answered with an
//!   empty page.
//! * `DescribeEvents` reports the log strictly reverse-chronologically — newest
//!   `Event$Date` first — at every page size, and `Marker` pages through it in
//!   that same order.
//! * a `KmsKeyId` a request names is stored RESOLVED: whichever form KMS accepts
//!   it in (key id, key ARN, alias name, alias ARN), the resource reports the
//!   key's own ARN for the rest of its life.
//! * a resource description a request sends BLANK is stored and reported as a
//!   single space; a non-blank one is kept byte for byte.
//! * `PreferredMaintenanceWindow` is the documented `ddd:hh24:mi-ddd:hh24:mi`
//!   range on every operation that takes one, refused before anything is stored.
//! * a not-found / already-exists refusal carries the service's TEMPLATE for that
//!   error class — the resource named the way its shape spells it, and an
//!   identifier interpolated only where the template has one.
//! * a validation or state-gate refusal states the RULE that was violated, with
//!   the bounds the calling operation actually holds the value to.
//!
//! Every assertion goes over the wire through the same router the emulator
//! mounts, and every resource is created by a real call, so nothing here can pass
//! on a response that was not computed from persisted state. Each suite is a
//! TABLE whose rows differ along what its rule generalises over.

use serde_json::{json, Value};
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{create_available_cluster, create_cluster, describe_cluster};
use crate::harness::{params, Emulator, Reply, PAST_TRANSITION, REGION};

/// The form pairs of one request row, as a table writes them.
type FormPairs = &'static [(&'static str, &'static str)];

/// One refusal row of the template table: the case, the action, its form pairs,
/// and the HTTP status, `awsQuery` code and message it must be answered with.
type TemplateRow = (
    &'static str,
    &'static str,
    FormPairs,
    u16,
    &'static str,
    &'static str,
);

/// One row that just makes a call: the case, the action and its form pairs.
type ActionRow = (&'static str, &'static str, FormPairs);

/// One usage-limit row: the case, the limits it sends, and the message it must
/// be refused with.
type UsageLimitRow = (&'static str, FormPairs, &'static str);

/// Assert `reply` carries exactly `message`.
///
/// A message with trailing whitespace — one built from a request member that was
/// sent empty — is asserted on the raw `<Message>` element, because the XML
/// reader reports text nodes trimmed.
fn assert_message(case: &str, reply: &Reply, message: &str) {
    if message != message.trim_end() {
        let rendered = format!("<Message>{message}</Message>");
        assert!(
            reply.body.contains(&rendered),
            "case {case}: expected {rendered} in the body, got {}",
            reply.body
        );
        return;
    }
    assert_eq!(
        reply.error_message(),
        message,
        "case {case}: expected message {message:?}, got {:?}",
        reply.error_message()
    );
}

/// Assert `reply` is the refusal `(status, code, message)` names, exactly.
fn assert_refusal(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (status, code),
        "case {case}: expected {status} {code}, got {} {}",
        reply.status,
        reply.body
    );
    assert_message(case, reply, message);
}

/// Assert `reply` reports `expected` for the element named `element`.
///
/// A value that is only whitespace — the single space a blank description
/// normalises to — carries no text node the XML reader keeps, so it is asserted
/// on the ELEMENT in the raw body instead.
fn assert_element(case: &str, reply: &Reply, element: &str, expected: &str) {
    if expected.trim().is_empty() {
        let rendered = format!("<{element}>{expected}</{element}>");
        assert!(
            reply.body.contains(&rendered),
            "case {case}: expected {rendered} in the body, got {}",
            reply.body
        );
        return;
    }
    assert_eq!(
        reply.text(element),
        Some(expected),
        "case {case}: expected {element} {expected:?}, got {:?}",
        reply.text(element)
    );
}

/// `CreateReplicationGroup` for `id`, with whatever else `extra` names.
async fn create_replication_group(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "the fidelity suite"),
    ]);
    pairs.extend(params(extra));
    emulator.call("CreateReplicationGroup", &pairs).await
}

/// Create one replication group of `node_groups` shards with
/// `replicas_per_node_group` replicas each, and let the clock settle it at
/// `available`.
async fn available_replication_group(
    emulator: &Emulator,
    id: &str,
    node_groups: i32,
    replicas_per_node_group: i32,
) {
    let created = create_replication_group(
        emulator,
        id,
        &[
            ("CacheNodeType", "cache.m7g.large"),
            ("NumNodeGroups", &node_groups.to_string()),
            ("ReplicasPerNodeGroup", &replicas_per_node_group.to_string()),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {id}: the replication group must be creatable, got {}",
        created.body
    );
    std::thread::sleep(PAST_TRANSITION);
}

/// `DescribeReplicationGroups` for one group.
async fn describe_replication_group(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call(
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await
}

/// `DescribeServerlessCaches` for one cache.
async fn describe_serverless_cache(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", name)]),
        )
        .await
}

// ─────────────────── the engine filter of the version catalog ────────────────

#[tokio::test]
async fn the_engine_filter_names_a_published_engine_or_the_describe_is_refused() {
    let emulator = Emulator::new();

    // Served: every engine the catalog publishes, in the spelling the catalog
    // holds and in another case — the engine name is resolved, not matched
    // byte-wise.
    for engine in state::ENGINES {
        for (case, sent) in [
            (format!("{engine} as published"), engine.to_string()),
            (format!("{engine} upper-cased"), engine.to_uppercase()),
        ] {
            let reply = emulator
                .call(
                    "DescribeCacheEngineVersions",
                    &params(&[("Engine", &sent)]),
                )
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case}: a published engine must be served, got {} {}",
                reply.status, reply.body
            );
            let reported = reply.all("Engine");
            assert!(
                !reported.is_empty() && reported.iter().all(|row| *row == engine),
                "case {case}: must report only {engine}'s versions, got {reported:?}"
            );
        }
    }

    // Refused: a value that names no engine of the catalog. The axis: how it
    // misses — a different engine, a misspelling, a value that is not an engine
    // name at all (a version, a parameter group family), and the empty string.
    let refused = [
        ("an engine this service does not run", "sqlite"),
        ("a misspelling of a published engine", "readis"),
        ("a version in the engine member", "7.1"),
        ("a parameter group family as an engine", "redis7"),
        ("the empty string", ""),
    ];
    for (case, engine) in refused {
        let reply = emulator
            .call(
                "DescribeCacheEngineVersions",
                &params(&[("Engine", engine)]),
            )
            .await;
        assert_refusal(
            case,
            &reply,
            400,
            "InvalidParameterValue",
            &state::invalid_engine_name_message(engine),
        );
    }

    // The refusal is about the engine alone: a published engine stays served
    // when another filter narrows it to nothing.
    let narrowed = emulator
        .call(
            "DescribeCacheEngineVersions",
            &params(&[
                ("Engine", state::DEFAULT_ENGINE),
                ("EngineVersion", "99.99"),
            ]),
        )
        .await;
    assert_eq!(
        narrowed.status, 200,
        "case a published engine with a version it has no row for: must answer an empty page, \
         got {} {}",
        narrowed.status, narrowed.body
    );
    assert!(
        narrowed.all("Engine").is_empty(),
        "case a published engine with a version it has no row for: the page must be empty, got {}",
        narrowed.body
    );
}

// ───────────────────────── the order the log is reported in ──────────────────

#[tokio::test]
async fn the_event_log_is_reported_newest_first_at_every_page_size() {
    let emulator = Emulator::new();
    // Three clusters created in a known order, each recording its creation and
    // then the transition the clock owes it: six events whose order is the order
    // these calls happened in.
    let ids = ["log-first", "log-second", "log-third"];
    for id in ids {
        create_available_cluster(&emulator, id, &[]).await;
    }

    // The axis: the page size the request asks for — none, the smallest
    // `MaxRecords` serves, and the largest — and the source filter.
    let cases: [(&str, Vec<(String, String)>); 4] = [
        ("no page size", Vec::new()),
        ("the smallest page size", params(&[("MaxRecords", "20")])),
        ("the largest page size", params(&[("MaxRecords", "100")])),
        (
            "one source",
            params(&[("SourceIdentifier", "log-second")]),
        ),
    ];
    for (case, pairs) in &cases {
        let reply = emulator.call("DescribeEvents", pairs).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: DescribeEvents must answer 200, got {}",
            reply.body
        );
        let messages = reply.all("Event/Message");
        let dates = reply.all("Event/Date");
        assert!(
            !messages.is_empty(),
            "case {case}: the log holds the events these calls wrote, got {}",
            reply.body
        );
        // The dates render in ISO 8601, whose lexicographic order IS time order,
        // so the order is asserted on the reported values themselves.
        let mut descending = dates.clone();
        descending.sort_unstable();
        descending.reverse();
        assert_eq!(
            dates, descending,
            "case {case}: events must be reported newest first, got {dates:?}"
        );
        // And the newest event really is the most recent change: the last
        // cluster these calls touched.
        let sources = reply.all("Event/SourceIdentifier");
        let newest = sources.first().copied().unwrap_or_default();
        let expected_newest = if case == &"one source" {
            "log-second"
        } else {
            "log-third"
        };
        assert_eq!(
            newest, expected_newest,
            "case {case}: the first event reported must be the most recent one, got {sources:?}"
        );
    }

    // Paging keeps that order: the pages of a walk, concatenated, are the single
    // page — so a client that follows `Marker` never jumps backwards in time.
    let whole = emulator
        .call("DescribeEvents", &params(&[("MaxRecords", "100")]))
        .await;
    let expected: Vec<String> = whole
        .all("Event/Date")
        .into_iter()
        .map(str::to_string)
        .collect();
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut pairs = params(&[("MaxRecords", "20")]);
        if let Some(from) = marker.as_deref() {
            pairs.extend(params(&[("Marker", from)]));
        }
        let page = emulator.call("DescribeEvents", &pairs).await;
        assert_eq!(
            page.status, 200,
            "case page {pages}: DescribeEvents must answer 200, got {}",
            page.body
        );
        walked.extend(page.all("Event/Date").into_iter().map(str::to_string));
        pages += 1;
        assert!(
            pages <= 3,
            "case the walk: {pages} pages for {} events must terminate",
            expected.len()
        );
        match page.marker() {
            Some(token) => marker = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        walked, expected,
        "case the walk: the pages must add up to the whole log, in the same newest-first order"
    );
}

// ──────────────────────── the key an encrypted resource stores ───────────────

/// Mint a customer managed key through the mounted KMS, as `(KeyId, Arn)`.
async fn create_key(emulator: &Emulator) -> (String, String) {
    let (status, body) = emulator
        .kms("CreateKey", &json!({ "Description": "the fidelity suite" }))
        .await;
    assert_eq!(status, 200, "kms CreateKey must answer 200, got {body}");
    let metadata = &body["KeyMetadata"];
    (
        metadata["KeyId"]
            .as_str()
            .unwrap_or_else(|| panic!("kms CreateKey reports a KeyId, got {body}"))
            .to_string(),
        metadata["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("kms CreateKey reports the key ARN, got {body}"))
            .to_string(),
    )
}

#[tokio::test]
async fn a_named_kms_key_is_stored_and_reported_as_the_key_arn() {
    let emulator = Emulator::with_kms();
    let (key_id, key_arn) = create_key(&emulator).await;
    let alias = "alias/the-fidelity-suite";
    let (status, body): (u16, Value) = emulator
        .kms(
            "CreateAlias",
            &json!({ "AliasName": alias, "TargetKeyId": key_id }),
        )
        .await;
    assert_eq!(status, 200, "kms CreateAlias must answer 200, got {body}");
    let alias_arn = format!(
        "arn:aws:kms:{REGION}:{}:{alias}",
        engine::emulator_account_id()
    );

    // The axis: the FORM the request names the key in — every one KMS accepts —
    // plus an identifier KMS resolves to nothing, which is stored as sent
    // because none of these operations models a KMS fault to refuse it with.
    let rows: [(&str, String, String); 5] = [
        ("the key id", key_id.clone(), key_arn.clone()),
        ("the key ARN", key_arn.clone(), key_arn.clone()),
        ("an alias name", alias.to_string(), key_arn.clone()),
        ("an alias ARN", alias_arn, key_arn.clone()),
        (
            "an alias no key is behind",
            "alias/no-such-key".to_string(),
            "alias/no-such-key".to_string(),
        ),
    ];
    for (index, (case, named, expected)) in rows.iter().enumerate() {
        // A replication group: the create answers it, and so does every later
        // read of the group.
        let group_id = format!("kms-rg-{index}");
        let created = create_replication_group(
            &emulator,
            &group_id,
            &[
                ("AtRestEncryptionEnabled", "true"),
                ("KmsKeyId", named),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case}: CreateReplicationGroup must answer 200, got {}",
            created.body
        );
        assert_element(case, &created, "KmsKeyId", expected);
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_replication_group(&emulator, &group_id).await;
        assert_eq!(
            described.status, 200,
            "case {case}: DescribeReplicationGroups must answer 200, got {}",
            described.body
        );
        assert_element(
            &format!("{case} read back"),
            &described,
            "KmsKeyId",
            expected,
        );

        // A serverless cache: the same rule, on another resource that takes a
        // `KmsKeyId`.
        let cache_name = format!("kms-cache-{index}");
        let mut pairs = params(&[("ServerlessCacheName", &cache_name), ("Engine", "redis")]);
        pairs.extend(params(&[("KmsKeyId", named)]));
        let cache = emulator.call("CreateServerlessCache", &pairs).await;
        assert_eq!(
            cache.status, 200,
            "case {case}: CreateServerlessCache must answer 200, got {}",
            cache.body
        );
        assert_element(&format!("{case} on a serverless cache"), &cache, "KmsKeyId", expected);
        let read_back = describe_serverless_cache(&emulator, &cache_name).await;
        assert_element(
            &format!("{case} on a serverless cache, read back"),
            &read_back,
            "KmsKeyId",
            expected,
        );
    }
}

// ─────────────────────── the description a blank one carries ─────────────────

#[tokio::test]
async fn a_blank_resource_description_is_stored_as_a_single_space() {
    let emulator = Emulator::new();

    // The axis: the description a request sends — the empty string, whitespace
    // only, and a real description — and the RESOURCE whose description it is.
    let descriptions: [(&str, &str, &str); 3] = [
        ("the empty string", "", state::BLANK_DESCRIPTION),
        ("whitespace only", "   ", state::BLANK_DESCRIPTION),
        ("a real description", "a described resource", "a described resource"),
    ];
    for (index, (case, sent, expected)) in descriptions.iter().enumerate() {
        // A cache parameter group.
        let group = format!("desc-params-{index}");
        let created = emulator
            .call(
                "CreateCacheParameterGroup",
                &params(&[
                    ("CacheParameterGroupName", &group),
                    ("CacheParameterGroupFamily", "redis7"),
                    ("Description", sent),
                ]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case} on a cache parameter group: create must answer 200, got {}",
            created.body
        );
        let described = emulator
            .call(
                "DescribeCacheParameterGroups",
                &params(&[("CacheParameterGroupName", &group)]),
            )
            .await;
        assert_element(
            &format!("{case} on a cache parameter group"),
            &described,
            "Description",
            expected,
        );

        // A cache subnet group, whose description is its own member.
        let subnet_group = format!("desc-subnets-{index}");
        let subnets: Vec<String> = emulator
            .account_subnets()
            .await
            .into_iter()
            .map(|subnet| subnet.id)
            .collect();
        let mut pairs = params(&[
            ("CacheSubnetGroupName", &subnet_group),
            ("CacheSubnetGroupDescription", sent),
        ]);
        for (position, id) in subnets.iter().take(2).enumerate() {
            pairs.push((
                format!("SubnetIds.SubnetIdentifier.{}", position + 1),
                id.clone(),
            ));
        }
        let created = emulator.call("CreateCacheSubnetGroup", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {case} on a cache subnet group: create must answer 200, got {}",
            created.body
        );
        let described = emulator
            .call(
                "DescribeCacheSubnetGroups",
                &params(&[("CacheSubnetGroupName", &subnet_group)]),
            )
            .await;
        assert_element(
            &format!("{case} on a cache subnet group"),
            &described,
            "CacheSubnetGroupDescription",
            expected,
        );

        // A replication group, which also reports it after a modify changed it.
        let group_id = format!("desc-rg-{index}");
        let mut pairs = params(&[("ReplicationGroupId", &group_id)]);
        pairs.extend(params(&[("ReplicationGroupDescription", sent)]));
        let created = emulator.call("CreateReplicationGroup", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {case} on a replication group: create must answer 200, got {}",
            created.body
        );
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_replication_group(&emulator, &group_id).await;
        assert_element(
            &format!("{case} on a replication group"),
            &described,
            "Description",
            expected,
        );
        let modified = emulator
            .call(
                "ModifyReplicationGroup",
                &[
                    ("ReplicationGroupId".to_string(), group_id.clone()),
                    ("ApplyImmediately".to_string(), "true".to_string()),
                    (
                        "ReplicationGroupDescription".to_string(),
                        (*sent).to_string(),
                    ),
                ],
            )
            .await;
        assert_eq!(
            modified.status, 200,
            "case {case} on a modified replication group: modify must answer 200, got {}",
            modified.body
        );
        assert_element(
            &format!("{case} on a modified replication group"),
            &modified,
            "Description",
            expected,
        );
    }
}

// ───────────────────── the maintenance window a create accepts ───────────────

#[tokio::test]
async fn every_operation_that_takes_a_maintenance_window_holds_it_to_the_documented_range() {
    let emulator = Emulator::new();

    // The axis: how the value relates to the documented format
    // `ddd:hh24:mi-ddd:hh24:mi` — a well-formed window (including one that wraps
    // past midnight and one at exactly the documented 60-minute minimum), and
    // the ways a value misses it.
    let served = [
        ("the format's own example", "sun:23:00-mon:01:30"),
        ("exactly the minimum span", "mon:05:00-mon:06:00"),
        ("a window that wraps past midnight", "sat:23:30-sun:00:30"),
        ("the days in another case", "TUE:05:00-TUE:06:00"),
    ];
    for (index, (case, window)) in served.iter().enumerate() {
        let id = format!("window-ok-{index}");
        let created = create_cluster(
            &emulator,
            &id,
            &[("PreferredMaintenanceWindow", window)],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case}: a documented window must be accepted, got {} {}",
            created.status, created.body
        );
        // The window is STORED as the request spelled it: the cluster reports
        // the value back.
        let described = describe_cluster(&emulator, &id).await;
        assert_eq!(
            described.text("PreferredMaintenanceWindow"),
            Some(*window),
            "case {case}: the stored window must be reported, got {}",
            described.body
        );
    }

    let refused = [
        (
            "a value that is not a range at all",
            "not-a-valid-window",
            state::INVALID_MAINTENANCE_WINDOW_MESSAGE.to_string(),
        ),
        (
            "one end only",
            "sun:23:00",
            state::INVALID_MAINTENANCE_WINDOW_MESSAGE.to_string(),
        ),
        (
            "a day no week has",
            "funday:05:00-mon:06:00",
            state::INVALID_MAINTENANCE_WINDOW_MESSAGE.to_string(),
        ),
        (
            "an hour no clock has",
            "mon:24:00-mon:25:30",
            state::INVALID_MAINTENANCE_WINDOW_MESSAGE.to_string(),
        ),
        (
            // Well-formed, but shorter than the documented minimum, which is a
            // rule of its own and states itself.
            "a span under the documented minimum",
            "mon:05:00-mon:05:30",
            format!(
                "The maintenance window must be at least {} minutes.",
                state::MAINTENANCE_WINDOW_MIN_MINUTES
            ),
        ),
        (
            "a window that spans nothing",
            "mon:05:00-mon:05:00",
            format!(
                "The maintenance window must be at least {} minutes.",
                state::MAINTENANCE_WINDOW_MIN_MINUTES
            ),
        ),
    ];
    for (index, (case, window, message)) in refused.iter().enumerate() {
        let id = format!("window-bad-{index}");
        // Every operation that takes the member applies the one rule: the two
        // creates and the two modifies.
        let create_cluster_reply =
            create_cluster(&emulator, &id, &[("PreferredMaintenanceWindow", window)]).await;
        assert_refusal(
            &format!("{case} on CreateCacheCluster"),
            &create_cluster_reply,
            400,
            "InvalidParameterValue",
            message,
        );
        // Nothing was stored: the cluster the refused create named does not
        // exist, which is the not-found template of a cache cluster.
        let described = describe_cluster(&emulator, &id).await;
        assert_refusal(
            &format!("{case}: the refused create stored nothing"),
            &described,
            404,
            "CacheClusterNotFound",
            &state::cache_cluster_not_found_message(&id),
        );

        let create_group_reply = create_replication_group(
            &emulator,
            &format!("window-rg-{index}"),
            &[("PreferredMaintenanceWindow", window)],
        )
        .await;
        assert_refusal(
            &format!("{case} on CreateReplicationGroup"),
            &create_group_reply,
            400,
            "InvalidParameterValue",
            message,
        );
        let described = describe_replication_group(&emulator, &format!("window-rg-{index}")).await;
        assert_refusal(
            &format!("{case}: the refused group create stored nothing"),
            &described,
            404,
            "ReplicationGroupNotFoundFault",
            &state::replication_group_not_found_message(&format!("window-rg-{index}")),
        );
    }
}

// ───────────────── the templates a not-found / already-exists carries ────────

#[tokio::test]
async fn the_not_found_and_already_exists_refusals_carry_the_service_templates() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "tmpl-cluster", &[]).await;
    let created = emulator
        .call(
            "CreateCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", "tmpl-params"),
                ("CacheParameterGroupFamily", "redis7"),
                ("Description", "the fidelity suite"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the parameter group must be creatable, got {}",
        created.body
    );
    available_replication_group(&emulator, "tmpl-rg", 1, 1).await;
    let snapshot = emulator
        .call(
            "CreateSnapshot",
            &params(&[
                ("SnapshotName", "tmpl-snap"),
                ("CacheClusterId", "tmpl-cluster"),
            ]),
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "the snapshot must be creatable, got {}",
        snapshot.body
    );
    std::thread::sleep(PAST_TRANSITION);

    // The axis: the resource class, the operation that could not resolve (or
    // could not take) its name, and whether the service's template interpolates
    // that name at all. The `ghost-*` ids below are the ones a template WITHOUT
    // an identifier must not mention.
    let rows: [TemplateRow; 12] = [
        (
            "a service update the catalogue does not publish, on the describe",
            "DescribeServiceUpdates",
            &[("ServiceUpdateName", "ghost-update")],
            404,
            "ServiceUpdateNotFoundFault",
            "ServiceUpdate ghost-update not found.",
        ),
        (
            "the same update on a batch update action",
            "BatchApplyUpdateAction",
            &[
                ("ServiceUpdateName", "ghost-update"),
                ("CacheClusterIds.member.1", "tmpl-cluster"),
            ],
            404,
            "ServiceUpdateNotFoundFault",
            "ServiceUpdate ghost-update not found.",
        ),
        (
            "a cache cluster this account does not hold, on the describe",
            "DescribeCacheClusters",
            &[("CacheClusterId", "ghost-cluster")],
            404,
            "CacheClusterNotFound",
            "CacheCluster not found: ghost-cluster",
        ),
        (
            "the same cluster on a reboot",
            "RebootCacheCluster",
            &[
                ("CacheClusterId", "ghost-cluster"),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
            ],
            404,
            "CacheClusterNotFound",
            "CacheCluster not found: ghost-cluster",
        ),
        (
            "a cache parameter group name already taken",
            "CreateCacheParameterGroup",
            &[
                ("CacheParameterGroupName", "tmpl-params"),
                ("CacheParameterGroupFamily", "redis7"),
                ("Description", "a second group of the same name"),
            ],
            400,
            "CacheParameterGroupAlreadyExists",
            "Parameter group tmpl-params already exists",
        ),
        (
            "a family default's name, also already taken",
            "CreateCacheParameterGroup",
            &[
                ("CacheParameterGroupName", "default.redis7"),
                ("CacheParameterGroupFamily", "redis7"),
                ("Description", "a group over a family default"),
            ],
            400,
            "CacheParameterGroupAlreadyExists",
            "Parameter group default.redis7 already exists",
        ),
        (
            "a Global datastore this account does not hold, on the describe",
            "DescribeGlobalReplicationGroups",
            &[("GlobalReplicationGroupId", "ghost-datastore")],
            404,
            "GlobalReplicationGroupNotFoundFault",
            state::GLOBAL_REPLICATION_GROUP_NOT_FOUND_MESSAGE,
        ),
        (
            "the same datastore on a modify",
            "ModifyGlobalReplicationGroup",
            &[
                ("GlobalReplicationGroupId", "ghost-datastore"),
                ("ApplyImmediately", "true"),
            ],
            404,
            "GlobalReplicationGroupNotFoundFault",
            state::GLOBAL_REPLICATION_GROUP_NOT_FOUND_MESSAGE,
        ),
        (
            "a replication group this account does not hold, on the describe",
            "DescribeReplicationGroups",
            &[("ReplicationGroupId", "ghost-group")],
            404,
            "ReplicationGroupNotFoundFault",
            "ReplicationGroup ghost-group not found.",
        ),
        (
            "the same group on a failover test",
            "TestFailover",
            &[
                ("ReplicationGroupId", "ghost-group"),
                ("NodeGroupId", "0001"),
            ],
            404,
            "ReplicationGroupNotFoundFault",
            "ReplicationGroup ghost-group not found.",
        ),
        (
            "a replication group id already taken",
            "CreateReplicationGroup",
            &[
                ("ReplicationGroupId", "tmpl-rg"),
                ("ReplicationGroupDescription", "a second group of that name"),
            ],
            400,
            "ReplicationGroupAlreadyExists",
            state::REPLICATION_GROUP_ALREADY_EXISTS_MESSAGE,
        ),
        (
            "a snapshot name already taken",
            "CreateSnapshot",
            &[
                ("SnapshotName", "tmpl-snap"),
                ("CacheClusterId", "tmpl-cluster"),
            ],
            400,
            "SnapshotAlreadyExistsFault",
            state::SNAPSHOT_ALREADY_EXISTS_MESSAGE,
        ),
    ];
    for (case, action, pairs, status, code, message) in rows {
        let reply = emulator.call(action, &params(pairs)).await;
        assert_refusal(case, &reply, status, code, message);
    }

    // The second rule the templates carry: where the service's template has no
    // identifier, the refusal names none — whatever the request asked for.
    for (case, action, pairs, message) in [
        (
            "the Global datastore template",
            "DescribeGlobalReplicationGroups",
            &[("GlobalReplicationGroupId", "ghost-datastore")][..],
            state::GLOBAL_REPLICATION_GROUP_NOT_FOUND_MESSAGE,
        ),
        (
            "the duplicate replication group template",
            "CreateReplicationGroup",
            &[
                ("ReplicationGroupId", "tmpl-rg"),
                ("ReplicationGroupDescription", "a second group of that name"),
            ][..],
            state::REPLICATION_GROUP_ALREADY_EXISTS_MESSAGE,
        ),
        (
            "the duplicate snapshot template",
            "CopySnapshot",
            &[
                ("SourceSnapshotName", "tmpl-snap"),
                ("TargetSnapshotName", "tmpl-snap"),
            ][..],
            state::SNAPSHOT_ALREADY_EXISTS_MESSAGE,
        ),
    ] {
        let reply = emulator.call(action, &params(pairs)).await;
        assert_eq!(
            reply.error_message(),
            message,
            "case {case}: expected message {message:?}, got {:?}",
            reply.error_message()
        );
        for (_, value) in pairs {
            assert!(
                !reply.error_message().contains(value),
                "case {case}: the template names no identifier, yet the refusal carries \
                 {value:?}: {:?}",
                reply.error_message()
            );
        }
    }
}

// ────────────── the rule a validation / state-gate refusal states ────────────

#[tokio::test]
async fn a_reboot_states_the_states_it_is_accepted_in() {
    let emulator = Emulator::new();

    // Accepted: every state `RebootCacheCluster` serves. A cluster being
    // snapshotted is one of them, which is what the refusal's own list says.
    create_available_cluster(&emulator, "reboot-ready", &[]).await;
    let accepted = emulator
        .call(
            "RebootCacheCluster",
            &params(&[
                ("CacheClusterId", "reboot-ready"),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
            ]),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case an available cluster: the reboot must be accepted, got {}",
        accepted.body
    );

    create_available_cluster(&emulator, "reboot-snapshotting", &[]).await;
    let snapshot = emulator
        .call(
            "CreateSnapshot",
            &params(&[
                ("SnapshotName", "reboot-snap"),
                ("CacheClusterId", "reboot-snapshotting"),
            ]),
        )
        .await;
    assert_eq!(
        snapshot.status, 200,
        "the snapshot must be creatable, got {}",
        snapshot.body
    );
    let while_snapshotting = emulator
        .call(
            "RebootCacheCluster",
            &params(&[
                ("CacheClusterId", "reboot-snapshotting"),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
            ]),
        )
        .await;
    assert_eq!(
        while_snapshotting.status, 200,
        "case a cluster being snapshotted: the reboot must be accepted, because that is one of \
         the states the refusal names, got {}",
        while_snapshotting.body
    );

    // Refused: a state that is in none of them. The axis: which state the
    // cluster is in — still being created, and already rebooting.
    let created = create_cluster(&emulator, "reboot-creating", &[]).await;
    assert_eq!(
        created.status, 200,
        "the cluster must be creatable, got {}",
        created.body
    );
    let while_creating = emulator
        .call(
            "RebootCacheCluster",
            &params(&[
                ("CacheClusterId", "reboot-creating"),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
            ]),
        )
        .await;
    assert_refusal(
        "a cluster still being created",
        &while_creating,
        400,
        "InvalidCacheClusterState",
        &state::reboot_state_refusal("reboot-creating", state::STATUS_CREATING),
    );
    let twice = emulator
        .call(
            "RebootCacheCluster",
            &params(&[
                ("CacheClusterId", "reboot-snapshotting"),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
            ]),
        )
        .await;
    assert_refusal(
        "a cluster already rebooting",
        &twice,
        400,
        "InvalidCacheClusterState",
        &state::reboot_state_refusal("reboot-snapshotting", state::STATUS_REBOOTING_NODES),
    );
    // The refusal states the rule: every state it accepts is named.
    for accepted_state in state::REBOOTABLE_CLUSTER_STATES {
        assert!(
            twice.error_message().contains(accepted_state),
            "case a cluster already rebooting: the refusal must name the accepted state \
             {accepted_state:?}, got {:?}",
            twice.error_message()
        );
    }
}

#[tokio::test]
async fn a_replica_count_refusal_states_the_bound_its_own_operation_holds() {
    let emulator = Emulator::new();
    available_replication_group(&emulator, "count-rg", 1, 1).await;

    // The axis: the operation (whose own floor the bound is), and which side of
    // the range the requested count falls on.
    let rows: [(&str, &str, &str, i32); 6] = [
        (
            "an increase past the maximum",
            "IncreaseReplicaCount",
            "6",
            state::INCREASE_REPLICA_COUNT_MIN,
        ),
        (
            "an increase to no replicas at all",
            "IncreaseReplicaCount",
            "0",
            state::INCREASE_REPLICA_COUNT_MIN,
        ),
        (
            "an increase to a negative count",
            "IncreaseReplicaCount",
            "-1",
            state::INCREASE_REPLICA_COUNT_MIN,
        ),
        (
            "a decrease past the maximum",
            "DecreaseReplicaCount",
            "6",
            state::DECREASE_REPLICA_COUNT_MIN,
        ),
        (
            "a decrease to a negative count",
            "DecreaseReplicaCount",
            "-1",
            state::DECREASE_REPLICA_COUNT_MIN,
        ),
        (
            "a decrease far past the maximum",
            "DecreaseReplicaCount",
            "50",
            state::DECREASE_REPLICA_COUNT_MIN,
        ),
    ];
    for (case, action, count, minimum) in rows {
        let reply = emulator
            .call(
                action,
                &params(&[
                    ("ReplicationGroupId", "count-rg"),
                    ("NewReplicaCount", count),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_refusal(
            case,
            &reply,
            400,
            "InvalidParameterValue",
            &state::replica_count_range_message(minimum),
        );
    }

    // A count inside the range is not refused on the bound: the decrease to the
    // count the group already has is the NO-OP refusal, which states that rule
    // instead.
    for (case, action) in [
        ("a decrease to the count it already has", "DecreaseReplicaCount"),
        ("an increase to the count it already has", "IncreaseReplicaCount"),
    ] {
        let reply = emulator
            .call(
                action,
                &params(&[
                    ("ReplicationGroupId", "count-rg"),
                    ("NewReplicaCount", "1"),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_refusal(
            case,
            &reply,
            400,
            "NoOperationFault",
            state::REPLICA_COUNT_UNCHANGED_MESSAGE,
        );
    }
}

#[tokio::test]
async fn a_replication_group_modify_gate_states_the_state_it_requires() {
    let emulator = Emulator::new();
    available_replication_group(&emulator, "gate-rg", 1, 1).await;
    // A replica change leaves the group `modifying` until the clock settles it,
    // which is the state every further modification is refused from.
    let started = emulator
        .call(
            "IncreaseReplicaCount",
            &params(&[
                ("ReplicationGroupId", "gate-rg"),
                ("NewReplicaCount", "2"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "the replica increase must be accepted, got {}",
        started.body
    );

    // The axis: which modification runs into the gate.
    let rows: [ActionRow; 2] = [
        (
            "a further increase",
            "IncreaseReplicaCount",
            &[
                ("ReplicationGroupId", "gate-rg"),
                ("NewReplicaCount", "3"),
                ("ApplyImmediately", "true"),
            ],
        ),
        (
            "a decrease",
            "DecreaseReplicaCount",
            &[
                ("ReplicationGroupId", "gate-rg"),
                ("NewReplicaCount", "1"),
                ("ApplyImmediately", "true"),
            ],
        ),
    ];
    for (case, action, pairs) in rows {
        let reply = emulator.call(action, &params(pairs)).await;
        assert_refusal(
            case,
            &reply,
            400,
            "InvalidReplicationGroupState",
            &state::replication_group_modify_state_refusal(),
        );
        // The template states the requirement, not this group's own state.
        assert!(
            !reply.error_message().contains("gate-rg")
                && !reply.error_message().contains(state::STATUS_MODIFYING),
            "case {case}: the refusal states the requirement alone, got {:?}",
            reply.error_message()
        );
    }
}

#[tokio::test]
async fn a_shard_and_usage_limit_refusal_states_the_rule_it_broke() {
    let emulator = Emulator::new();

    // `NumCacheClusters` counts the clusters of the ONE shard a cluster-mode
    // disabled group has, so it cannot be combined with a multi-shard topology.
    // The axis: how many shards the request asks for beside it.
    for (case, shards) in [
        ("two shards", "2"),
        ("ten shards", "10"),
    ] {
        let reply = create_replication_group(
            &emulator,
            &format!("shards-{shards}"),
            &[
                ("CacheNodeType", "cache.m7g.large"),
                ("NumCacheClusters", "2"),
                ("NumNodeGroups", shards),
            ],
        )
        .await;
        assert_refusal(
            case,
            &reply,
            400,
            "InvalidParameterCombination",
            state::NUM_CACHE_CLUSTERS_ONE_SHARD_MESSAGE,
        );
    }

    // A serverless usage limit states its member, its bounds and the value that
    // removes the limit — and nothing of the value that missed them. The axis:
    // the member, and which bound the value is outside.
    let rows: [UsageLimitRow; 4] = [
        (
            "a negative data storage maximum",
            &[
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Maximum", "-5"),
            ],
            "Invalid value for DataStorage limit. Must be between 1 and 5000. You can set the \
             value to zero to remove the limit.",
        ),
        (
            "a data storage maximum one past the published bound",
            &[
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Maximum", "5001"),
            ],
            "Invalid value for DataStorage limit. Must be between 1 and 5000. You can set the \
             value to zero to remove the limit.",
        ),
        (
            "a data storage minimum one past the published bound",
            &[
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Minimum", "5001"),
            ],
            "Invalid value for DataStorage limit. Must be between 1 and 5000. You can set the \
             value to zero to remove the limit.",
        ),
        (
            "a negative ECPU minimum",
            &[("CacheUsageLimits.ECPUPerSecond.Minimum", "-1")],
            "Invalid value for ECPUPerSecond limit. Must be at least 1. You can set the value to \
             zero to remove the limit.",
        ),
    ];
    for (index, (case, limits, message)) in rows.iter().enumerate() {
        let name = format!("limit-cache-{index}");
        let mut pairs = params(&[("ServerlessCacheName", &name), ("Engine", "redis")]);
        pairs.extend(params(limits));
        let reply = emulator.call("CreateServerlessCache", &pairs).await;
        assert_refusal(case, &reply, 400, "InvalidParameterValue", message);
        // Nothing was created: the refusal happened before any state was
        // written.
        let described = describe_serverless_cache(&emulator, &name).await;
        assert_eq!(
            described.status, 404,
            "case {case}: the refused create must store nothing, got {} {}",
            described.status, described.body
        );
    }

    // The published bounds themselves are served, so the message's range is the
    // range the handler holds the value to.
    for (case, limits) in [
        (
            "the smallest data storage bound",
            &[
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Minimum", "1"),
                ("CacheUsageLimits.DataStorage.Maximum", "1"),
            ][..],
        ),
        (
            "the largest data storage bound",
            &[
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Maximum", "5000"),
            ][..],
        ),
        (
            "a removed data storage limit",
            &[
                ("CacheUsageLimits.DataStorage.Unit", "GB"),
                ("CacheUsageLimits.DataStorage.Maximum", "0"),
            ][..],
        ),
    ] {
        let name = format!("limit-ok-{}", case.replace(' ', "-"));
        let mut pairs = params(&[("ServerlessCacheName", &name), ("Engine", "redis")]);
        pairs.extend(params(limits));
        let reply = emulator.call("CreateServerlessCache", &pairs).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: a published bound must be served, got {} {}",
            reply.status, reply.body
        );
    }
}
