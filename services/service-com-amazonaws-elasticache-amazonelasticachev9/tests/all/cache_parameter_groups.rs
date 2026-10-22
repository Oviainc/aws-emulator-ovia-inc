// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeCacheParameterGroups`.
//!
//! The rule under test: the operation reports the cache parameter groups the
//! account HOLDS — the default group of every family the service publishes,
//! plus any group the store carries — filtered to one group when the request
//! names one, refused with `CacheParameterGroupNotFound` when it names a group
//! the account does not hold, and paged at the documented `MaxRecords` window.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, reusing the harness in
//! [`crate::cache_cluster_lifecycle`]; only the HTTP status and the XML body
//! are inspected. One suite writes a record straight into the store first, and
//! says there why: `CreateCacheParameterGroup` is still a scaffolded 501 owned
//! by its own issue, so writing the record a create would write is the only way
//! to have a non-default group to read back — and the assertion is still on the
//! wire response, so a describe that answered from a constant list rather than
//! from state would fail it.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{params, Emulator, Reply};

/// Describe every group, or just the one `name` names.
async fn describe_groups(emulator: &Emulator, name: Option<&str>) -> Reply {
    let pairs = match name {
        Some(name) => params(&[("CacheParameterGroupName", name)]),
        None => Vec::new(),
    };
    emulator.call("DescribeCacheParameterGroups", &pairs).await
}

/// Every `CacheParameterGroupName` a describe reply reports, in page order.
fn reported_names(reply: &Reply) -> Vec<&str> {
    reply.all("CacheParameterGroups/CacheParameterGroup/CacheParameterGroupName")
}

#[tokio::test]
async fn every_published_family_has_its_default_group_reported() {
    let emulator = Emulator::new();
    let reply = describe_groups(&emulator, None).await;
    assert_eq!(
        reply.status, 200,
        "case describe-all: must succeed, got {} {}",
        reply.status, reply.body
    );

    // The catalog's source: `smithy_query(query="doc",
    // shape="CreateCacheParameterGroupMessage",
    // member="CacheParameterGroupFamily")`, published through
    // `state::PARAMETER_GROUP_FAMILIES`. Every family's default group must be
    // reported, including the ones no cluster in this suite ever names.
    let names = reported_names(&reply);
    for family in state::PARAMETER_GROUP_FAMILIES {
        let expected = format!("default.{family}");
        assert!(
            names.contains(&expected.as_str()),
            "case family {family}: expected {expected} among the reported groups, got {names:?}"
        );
    }
    assert_eq!(
        names.len(),
        state::PARAMETER_GROUP_FAMILIES.len(),
        "case describe-all: expected one group per published family, got {names:?}"
    );

    // A describe is a read: calling it twice reports the same groups.
    let again = describe_groups(&emulator, None).await;
    assert_eq!(
        reported_names(&again),
        names,
        "case describe-twice: a read must not change what the next caller sees"
    );

    // Each reported group carries its family, a description and an ARN.
    let families = reply.all("CacheParameterGroups/CacheParameterGroup/CacheParameterGroupFamily");
    assert_eq!(
        families.len(),
        names.len(),
        "case describe-all: every group must report its family, got {families:?}"
    );
    for (name, family) in names.iter().zip(families.iter()) {
        assert_eq!(
            *name,
            format!("default.{family}"),
            "case group {name}: a default group's name is `default.` plus its family, got {family}"
        );
    }
    let arns = reply.all("CacheParameterGroups/CacheParameterGroup/ARN");
    assert_eq!(
        arns.len(),
        names.len(),
        "case describe-all: every group must report its ARN, got {arns:?}"
    );
    let globals = reply.all("CacheParameterGroups/CacheParameterGroup/IsGlobal");
    assert!(
        globals.iter().all(|flag| *flag == "false"),
        "case describe-all: no group here belongs to a global datastore, got {globals:?}"
    );
}

#[tokio::test]
async fn a_named_group_is_reported_alone_and_an_unknown_name_is_refused() {
    let emulator = Emulator::new();

    // The axis: the name. A published family's default group, in a case the
    // request did not store it in (the name "is stored as a lowercase string"),
    // and names no account holds — a family-shaped one the catalog does not
    // publish among them.
    let found: [(&str, &str); 4] = [
        ("default.redis7", "default.redis7"),
        ("default.memcached1.4", "default.memcached1.4"),
        ("DEFAULT.VALKEY8", "default.valkey8"),
        ("default.redis6.x", "default.redis6.x"),
    ];
    for (requested, expected) in found {
        let reply = describe_groups(&emulator, Some(requested)).await;
        assert_eq!(
            reply.status, 200,
            "case {requested}: must succeed, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reported_names(&reply),
            vec![expected],
            "case {requested}: expected only {expected}, got {}",
            reply.body
        );
        assert_eq!(
            reply.marker(),
            None,
            "case {requested}: a single-group lookup pages nothing, got {}",
            reply.body
        );
    }

    let missing = [
        "no-such-group",
        // A family the published catalog does not carry, so its default group
        // does not exist either.
        "default.redis9",
        "default.memcached2.0",
        "default.",
    ];
    for name in missing {
        let reply = describe_groups(&emulator, Some(name)).await;
        assert_eq!(
            reply.status, 404,
            "case {name}: must be refused 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "CacheParameterGroupNotFound",
            "case {name}: expected CacheParameterGroupNotFound, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_group_in_the_store_is_reported_and_pages_are_bounded_and_resumable() {
    let emulator = Emulator::new();

    // 25 groups beyond the published defaults, so the store holds more than the
    // smallest page size the service serves. These are written the way
    // `CreateCacheParameterGroup` will write them once its own issue is fixed;
    // until then there is no implemented operation that creates one, and a
    // describe that reported a fixed catalog instead of the store would miss
    // every one of them.
    let custom: Vec<String> = (0..25).map(|index| format!("custom-{index:02}")).collect();
    {
        let store = emulator.store();
        let mut store = store.lock();
        for name in &custom {
            store.cache_parameter_groups.insert(
                name.clone(),
                state::CacheParameterGroupRecord {
                    name: name.clone(),
                    family: "redis7".to_string(),
                    description: format!("group {name}"),
                },
            );
        }
    }
    let total = custom.len() + state::PARAMETER_GROUP_FAMILIES.len();

    // Each row is a page size: the published bounds of "minimum 20; maximum
    // 100", one past each, and the default when the member is absent.
    let cases: [(&str, Option<usize>); 6] = [
        ("20", Some(20)),
        ("21", Some(21)),
        ("100", Some(total.min(100))),
        ("19", None),
        ("101", None),
        ("0", None),
    ];
    for (max_records, accepted) in cases {
        let reply = emulator
            .call(
                "DescribeCacheParameterGroups",
                &params(&[("MaxRecords", max_records)]),
            )
            .await;
        match accepted {
            Some(expected) => {
                assert_eq!(
                    reply.status, 200,
                    "case MaxRecords={max_records}: must be accepted, got {} {}",
                    reply.status, reply.body
                );
                let page = reported_names(&reply);
                assert_eq!(
                    page.len(),
                    expected,
                    "case MaxRecords={max_records}: expected {expected} groups, got {page:?}"
                );
                assert_eq!(
                    reply.marker().is_some(),
                    total > expected,
                    "case MaxRecords={max_records}: a marker is present exactly when more \
                     records remain, got {}",
                    reply.body
                );
            }
            None => {
                assert_eq!(
                    reply.status, 400,
                    "case MaxRecords={max_records}: must be refused 400, got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    "InvalidParameterValue",
                    "case MaxRecords={max_records}: expected InvalidParameterValue, got {}",
                    reply.body
                );
            }
        }
    }

    // A `Marker` this service did not mint is refused rather than silently
    // serving the first page.
    let bogus = emulator
        .call(
            "DescribeCacheParameterGroups",
            &params(&[("Marker", "not-a-marker")]),
        )
        .await;
    assert_eq!(
        bogus.status, 400,
        "case Marker=not-a-marker: must be refused 400, got {} {}",
        bogus.status, bogus.body
    );
    assert_eq!(
        bogus.error_code(),
        "InvalidParameterValue",
        "case Marker=not-a-marker: expected InvalidParameterValue, got {}",
        bogus.body
    );

    // Walking the pages visits every stored group exactly once, in one stable
    // order, and ends without a marker.
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut pairs = params(&[("MaxRecords", "20")]);
        if let Some(token) = marker.as_deref() {
            pairs.extend(params(&[("Marker", token)]));
        }
        let reply = emulator.call("DescribeCacheParameterGroups", &pairs).await;
        assert_eq!(
            reply.status, 200,
            "case page {pages}: must succeed, got {} {}",
            reply.status, reply.body
        );
        seen.extend(reported_names(&reply).into_iter().map(str::to_string));
        pages += 1;
        assert!(
            pages <= 5,
            "case walk: {total} groups at 20 per page must not take more than 5 pages, \
             got {pages}"
        );
        match reply.marker() {
            Some(token) => marker = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen.len(),
        total,
        "case walk: expected every one of the {total} groups exactly once, got {seen:?}"
    );
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        total,
        "case walk: the pages must not repeat a group, got {seen:?}"
    );
    assert_eq!(
        seen, sorted,
        "case walk: the pages must follow one stable order, got {seen:?}"
    );
    for name in &custom {
        assert!(
            seen.contains(name),
            "case walk: expected the stored group {name} to be reported, got {seen:?}"
        );
    }

    // The group a store write added is also reachable by name, and reports what
    // the record holds.
    let one = describe_groups(&emulator, Some("custom-07")).await;
    assert_eq!(
        one.status, 200,
        "case custom-07: must succeed, got {} {}",
        one.status, one.body
    );
    assert_eq!(
        one.text("Description"),
        Some("group custom-07"),
        "case custom-07: the description must come from the stored record, got {}",
        one.body
    );
    assert_eq!(
        one.text("CacheParameterGroupFamily"),
        Some("redis7"),
        "case custom-07: the family must come from the stored record, got {}",
        one.body
    );
}

#[tokio::test]
async fn a_default_group_arn_names_the_region_the_request_was_signed_for() {
    // A group AWS provides exists in every Region, so the ARN a read reports
    // names the Region the request was signed for. The axis: that Region.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::new();
        let reply = emulator
            .call_in(
                region,
                "DescribeCacheParameterGroups",
                &params(&[("CacheParameterGroupName", "default.redis7")]),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {region}: must succeed, got {} {}",
            reply.status, reply.body
        );
        let arn = reply.text("ARN").unwrap_or_default();
        assert!(
            arn.starts_with(&format!("arn:aws:elasticache:{region}:"))
                && arn.ends_with(":parametergroup:default.redis7"),
            "case {region}: expected an elasticache parametergroup ARN in {region}, got {arn}"
        );
    }
}

#[tokio::test]
async fn a_cluster_joins_a_group_the_account_holds_and_nothing_else() {
    let emulator = Emulator::new();

    // The axis: the `CacheParameterGroupName` a create names. A published
    // family's default group is accepted and reported back under the name it is
    // stored as; a name no group carries is refused with the modelled
    // not-found, by the same lookup `DescribeCacheParameterGroups` answers
    // from.
    let accepted: [(&str, &str, &str); 3] = [
        ("joins-redis7", "default.redis7", "default.redis7"),
        ("joins-redis6", "default.redis6.x", "default.redis6.x"),
        ("joins-upper", "DEFAULT.REDIS7", "default.redis7"),
    ];
    for (id, requested, stored) in accepted {
        let created = emulator
            .call(
                "CreateCacheCluster",
                &params(&[
                    ("CacheClusterId", id),
                    ("CacheParameterGroupName", requested),
                ]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {requested}: create must succeed, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("CacheParameterGroup/CacheParameterGroupName"),
            Some(stored),
            "case {requested}: expected the cluster to join {stored}, got {}",
            created.body
        );
        // The group the cluster joined is one this account holds.
        let described = describe_groups(&emulator, Some(stored)).await;
        assert_eq!(
            described.status, 200,
            "case {requested}: {stored} must be a group the account holds, got {} {}",
            described.status, described.body
        );
    }

    for requested in ["default.redis9", "nope"] {
        let refused = emulator
            .call(
                "CreateCacheCluster",
                &params(&[
                    ("CacheClusterId", "rejected"),
                    ("CacheParameterGroupName", requested),
                ]),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {requested}: create must be refused 404, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "CacheParameterGroupNotFound",
            "case {requested}: expected CacheParameterGroupNotFound, got {}",
            refused.body
        );
    }

    // A create that names NO parameter group joins the default group of its
    // engine version's family, resolved through the same stored map — so an
    // engine version no published family covers has no group to join and is
    // refused rather than the cluster reporting a group the account does not
    // hold.
    let derived: [(&str, &str, Option<&str>); 4] = [
        ("derived-redis6", "6.2", Some("default.redis6.x")),
        ("derived-redis7", "7.1", Some("default.redis7")),
        ("derived-redis28", "2.8.24", Some("default.redis2.8")),
        ("derived-unsupported", "9.9", None),
    ];
    for (id, version, expected) in derived {
        let created = emulator
            .call(
                "CreateCacheCluster",
                &params(&[("CacheClusterId", id), ("EngineVersion", version)]),
            )
            .await;
        match expected {
            Some(group) => {
                assert_eq!(
                    created.status, 200,
                    "case EngineVersion={version}: create must succeed, got {} {}",
                    created.status, created.body
                );
                assert_eq!(
                    created.text("CacheParameterGroup/CacheParameterGroupName"),
                    Some(group),
                    "case EngineVersion={version}: expected the cluster to join {group}, got {}",
                    created.body
                );
                let described = describe_groups(&emulator, Some(group)).await;
                assert_eq!(
                    described.status, 200,
                    "case EngineVersion={version}: {group} must be a group the account holds, \
                     got {} {}",
                    described.status, described.body
                );
            }
            None => {
                assert_eq!(
                    created.status, 400,
                    "case EngineVersion={version}: create must be refused 400, got {} {}",
                    created.status, created.body
                );
                assert_eq!(
                    created.error_code(),
                    "InvalidParameterValue",
                    "case EngineVersion={version}: expected InvalidParameterValue, got {}",
                    created.body
                );
            }
        }
    }
}
