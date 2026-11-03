// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Inspector CIS scan configuration slice:
//! `CreateCisScanConfiguration`, `ListCisScanConfigurations` and
//! `DeleteCisScanConfiguration`.
//!
//! The rule under test: a CIS scan configuration is a stored resource — a create mints its
//! ARN and records the scan name, security level, schedule, targets and tags it was sent;
//! a list reports exactly those records, selected by the request's filter criteria,
//! ordered and paged as the request asks; and a delete removes the record, so a second
//! delete of the same ARN is refused as a missing resource.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The signed-request harness this crate's suites share: `router()` builds a router over
// its own engine, `post_in` sends one restJson1 POST signed for a region.
use crate::filters_and_enablement::{post_in, router};

/// The single account id every generated emulator presents
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// A well-formed `CisScanConfigurationArn` this emulator never minted — for the
/// not-found paths, which need an ARN that passes the member's modelled `@pattern` and
/// names no stored configuration.
/// Source: `smithy_query(query="shape", shape="CisScanConfigurationArn")`.
const UNMINTED_ARN: &str = "arn:aws:inspector2:us-east-1:000000000000:owner/000000000000/\
                            cis-configuration/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// `CreateCisScanConfiguration` — `POST /cis/scan-configuration/create`.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Option<String>, Value) {
    post_in(router, region, "/cis/scan-configuration/create", body).await
}

/// `ListCisScanConfigurations` — `POST /cis/scan-configuration/list`.
async fn list(router: &MultiServiceRouter, body: &Value) -> (u16, Option<String>, Value) {
    post_in(router, "us-east-1", "/cis/scan-configuration/list", body).await
}

/// `DeleteCisScanConfiguration` — `POST /cis/scan-configuration/delete`.
async fn delete(router: &MultiServiceRouter, arn: &str) -> (u16, Option<String>, Value) {
    post_in(
        router,
        "us-east-1",
        "/cis/scan-configuration/delete",
        &json!({ "scanConfigurationArn": arn }),
    )
    .await
}

/// Assert `arn` is a `CisScanConfigurationArn` of `region` owned by `owner`, with the
/// hyphenated UUID id segment the real service mints.
///
/// Source: `smithy_query(query="shape", shape="CisScanConfigurationArn")` — pattern
/// `^arn:aws(-us-gov|-cn)?:inspector2:[a-z]{2}(-gov)?-[a-z]+-[0-9]{1}:[0-9]{12}:owner/(o-[a-z0-9]+|[0-9]{12})/cis-configuration/[0-9a-fA-F-]+$`.
fn assert_cis_arn(case: &str, arn: &str, region: &str, owner: &str) {
    let prefix = format!("arn:aws:inspector2:{region}:{owner}:owner/{owner}/cis-configuration/");
    let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}")
    });
    assert!(
        id.chars().all(|char| char.is_ascii_hexdigit() || char == '-'),
        "case {case:?}: id segment outside [0-9a-fA-F-], got {id:?}"
    );
    let groups: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected the hyphenated UUID grouping, got {id:?}"
    );
}

/// The `scanConfigurations` of a successful list answer.
fn reported(case: &str, body: &Value) -> Vec<Value> {
    body["scanConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no scanConfigurations list in {body}"))
        .clone()
}

/// The scan names a list answer reports, in the order it reported them.
fn reported_names(case: &str, body: &Value) -> Vec<String> {
    reported(case, body)
        .iter()
        .map(|configuration| configuration["scanName"].as_str().unwrap_or("").to_string())
        .collect()
}

/// A create body: one row of the table below.
struct Row {
    case: &'static str,
    region: &'static str,
    scan_name: &'static str,
    security_level: &'static str,
    schedule: Value,
    account_ids: Value,
    target_resource_tags: Value,
    tags: Option<Value>,
    /// The `CisTargets$accountIds` the configuration must report: `SELF` resolved to the
    /// calling account, `ALL_ACCOUNTS` to every account of its organization (which, with
    /// no account enabled through it, is that one account), de-duplicated and ordered.
    expected_account_ids: Vec<&'static str>,
}

/// The create requests the round-trip test drives, spanning what the rule generalises
/// over: every `Schedule` union variant and every `CisSecurityLevel` token, each
/// `TargetAccount` form (an account id, `SELF`, `ALL_ACCOUNTS`, and a mix), the
/// `TargetResourceTags` bounds (1 and the modelled maximum of 5 keys, 1 and the maximum
/// of 5 values), tags absent / empty / present, and three regions.
///
/// Sources: `smithy_query(query="shape", shape="Schedule,Day,CisSecurityLevel,TargetAccount,TargetResourceTags,TagValueList")`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one-time schedule, SELF target, no tags",
            region: "us-east-1",
            scan_name: "nightly-benchmark",
            security_level: "LEVEL_1",
            schedule: json!({ "oneTime": {} }),
            account_ids: json!(["SELF"]),
            target_resource_tags: json!({ "Environment": ["production"] }),
            tags: None,
            expected_account_ids: vec![ACCOUNT],
        },
        Row {
            case: "daily schedule, ALL_ACCOUNTS target, tags present",
            region: "eu-west-2",
            scan_name: "daily-level-two",
            security_level: "LEVEL_2",
            schedule: json!({ "daily": { "startTime": { "timeOfDay": "01:30", "timezone": "UTC" } } }),
            account_ids: json!(["ALL_ACCOUNTS"]),
            target_resource_tags: json!({ "Team": ["security", "platform"] }),
            tags: Some(json!({ "owner": "security-team", "costCenter": "4711" })),
            expected_account_ids: vec![ACCOUNT],
        },
        Row {
            case: "weekly schedule, two explicit account ids, five tag keys, empty tags",
            region: "ap-southeast-1",
            scan_name: "weekly-fleet",
            security_level: "LEVEL_1",
            schedule: json!({
                "weekly": {
                    "startTime": { "timeOfDay": "23:59", "timezone": "Europe/Berlin" },
                    "days": ["MON", "THU"],
                }
            }),
            account_ids: json!(["111122223333", "000000000000"]),
            target_resource_tags: json!({
                "Environment": ["production"],
                "Team": ["security"],
                "Stage": ["canary"],
                "Tier": ["frontend"],
                "Owner": ["sec-ops"],
            }),
            tags: Some(json!({})),
            expected_account_ids: vec!["000000000000", "111122223333"],
        },
        Row {
            case: "monthly schedule, one explicit account id, five tag values",
            region: "us-east-1",
            scan_name: "monthly-level-two",
            security_level: "LEVEL_2",
            schedule: json!({
                "monthly": {
                    "startTime": { "timeOfDay": "0:05", "timezone": "America/New_York" },
                    "day": "FRI",
                }
            }),
            account_ids: json!(["444455556666"]),
            target_resource_tags: json!({ "Stage": ["a", "b", "c", "d", "e"] }),
            tags: Some(json!({ "purpose": "cis" })),
            expected_account_ids: vec!["444455556666"],
        },
        Row {
            case: "SELF beside an explicit account id",
            region: "eu-west-2",
            scan_name: "mixed-targets",
            security_level: "LEVEL_1",
            schedule: json!({ "weekly": {
                "startTime": { "timeOfDay": "12:00", "timezone": "UTC" },
                "days": ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"],
            } }),
            account_ids: json!(["SELF", "222233334444"]),
            target_resource_tags: json!({ "Environment": ["staging"] }),
            tags: None,
            expected_account_ids: vec!["000000000000", "222233334444"],
        },
    ]
}

/// The create body of one row.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "scanName": row.scan_name,
        "securityLevel": row.security_level,
        "schedule": row.schedule,
        "targets": {
            "accountIds": row.account_ids,
            "targetResourceTags": row.target_resource_tags,
        },
    });
    if let Some(tags) = &row.tags {
        body["tags"] = tags.clone();
    }
    body
}

/// RULE: a created CIS scan configuration is a stored record — the list reports exactly
/// the scan name, security level, schedule, targets and tags the create was sent (with
/// `SELF` and `ALL_ACCOUNTS` resolved to account ids, which is what the read shape
/// models), and a delete removes it, so the configuration is gone from the list and a
/// second delete of its ARN is a 404.
#[tokio::test]
async fn a_created_configuration_is_listed_as_stored_and_gone_after_delete() {
    for row in rows() {
        let case = row.case;
        let router = router();

        let (status, error_type, created) =
            create_in(&router, row.region, &create_body(&row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateCisScanConfiguration: expected 200, got {status} / {created} \
             ({error_type:?})"
        );
        let arn = created["scanConfigurationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no scanConfigurationArn in {created}"))
            .to_string();
        assert_cis_arn(case, &arn, row.region, ACCOUNT);

        // The list reports the stored record, member by member.
        let (status, _, listed) = list(&router, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCisScanConfigurations: expected 200, got {status} / {listed}"
        );
        let configurations = reported(case, &listed);
        assert_eq!(
            configurations.len(),
            1,
            "case {case:?}: expected exactly the one created configuration, got {listed}"
        );
        let configuration = &configurations[0];
        assert_eq!(
            configuration["scanConfigurationArn"],
            json!(arn),
            "case {case:?}: expected the created ARN, got {configuration}"
        );
        assert_eq!(
            configuration["ownerId"],
            json!(ACCOUNT),
            "case {case:?}: expected the calling account as ownerId, got {configuration}"
        );
        assert_eq!(
            configuration["scanName"],
            json!(row.scan_name),
            "case {case:?}: expected the scan name as sent, got {configuration}"
        );
        assert_eq!(
            configuration["securityLevel"],
            json!(row.security_level),
            "case {case:?}: expected the security level as sent, got {configuration}"
        );
        assert_eq!(
            configuration["schedule"], row.schedule,
            "case {case:?}: expected the schedule as sent, got {configuration}"
        );
        assert_eq!(
            configuration["targets"]["accountIds"],
            json!(row.expected_account_ids),
            "case {case:?}: expected the resolved target accounts, got {configuration}"
        );
        assert_eq!(
            configuration["targets"]["targetResourceTags"], row.target_resource_tags,
            "case {case:?}: expected the target resource tags as sent, got {configuration}"
        );
        match &row.tags {
            // Absent when the create carried no `tags`, and the map it carried otherwise —
            // an empty one included.
            None => assert_eq!(
                configuration["tags"],
                Value::Null,
                "case {case:?}: expected no tags member, got {configuration}"
            ),
            Some(tags) => assert_eq!(
                configuration["tags"], *tags,
                "case {case:?}: expected the tags as sent, got {configuration}"
            ),
        }
        assert_eq!(
            listed["nextToken"],
            Value::Null,
            "case {case:?}: a single-record answer needs no page token, got {listed}"
        );

        // The delete reports the ARN it removed.
        let (status, error_type, deleted) = delete(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteCisScanConfiguration: expected 200, got {status} / {deleted} \
             ({error_type:?})"
        );
        assert_eq!(
            deleted["scanConfigurationArn"],
            json!(arn),
            "case {case:?}: expected the deleted ARN, got {deleted}"
        );

        // And the record is gone: the list no longer reports it, and deleting it again is
        // a missing resource.
        let (status, _, listed) = list(&router, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCisScanConfigurations after delete: expected 200, got {listed}"
        );
        assert!(
            reported(case, &listed).is_empty(),
            "case {case:?}: expected no configuration after delete, got {listed}"
        );

        for (attempt, candidate) in [("already deleted", arn.as_str()), ("never minted", UNMINTED_ARN)]
        {
            let (status, error_type, body) = delete(&router, candidate).await;
            assert_eq!(
                status, 404,
                "case {case:?}/{attempt}: expected 404, got {status} / {body}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}/{attempt}: expected ResourceNotFoundException, got {error_type:?} \
                 / {body}"
            );
            assert_eq!(
                body["message"],
                json!("Resource not found."),
                "case {case:?}/{attempt}: unexpected refusal message, got {body}"
            );
        }
    }
}

/// Create one configuration per name, over `router`, and answer the ARN each name got.
async fn create_named(router: &MultiServiceRouter, names: &[&str]) -> Vec<(String, String)> {
    let mut created = Vec::new();
    for name in names {
        let body = json!({
            "scanName": name,
            "securityLevel": "LEVEL_1",
            "schedule": { "oneTime": {} },
            "targets": {
                "accountIds": ["SELF"],
                // One tag every configuration carries, and one only this name carries, so
                // a tag filter can select both the whole set and a single record.
                "targetResourceTags": { "Fleet": ["all"], "Stage": [*name] },
            },
        });
        let (status, error_type, response) = create_in(router, "us-east-1", &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateCisScanConfiguration: expected 200, got {status} / {response} \
             ({error_type:?})"
        );
        let arn = response["scanConfigurationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no scanConfigurationArn in {response}"))
            .to_string();
        created.push(((*name).to_string(), arn));
    }
    created
}

/// RULE: every create mints a NEW configuration with an ARN of its own — the ARN
/// identifies the record, so two configurations never share one, not even under the same
/// scan name.
#[tokio::test]
async fn every_create_mints_a_distinct_configuration() {
    let router = router();
    // Rows differ along how many creates carry the SAME scan name: a repeated name is a
    // second configuration, not a second write to the first.
    let created = create_named(&router, &["same", "same", "other", "same"]).await;

    let arns: BTreeSet<&str> = created.iter().map(|(_, arn)| arn.as_str()).collect();
    assert_eq!(
        arns.len(),
        created.len(),
        "expected one ARN per create, got {created:?}"
    );

    let (status, _, listed) = list(&router, &json!({ "maxResults": 100 })).await;
    assert_eq!(status, 200, "ListCisScanConfigurations: got {listed}");
    assert_eq!(
        reported_names("distinct creates", &listed),
        vec!["other", "same", "same", "same"],
        "expected every created configuration, ordered by scan name, got {listed}"
    );
}

/// RULE: a list answer is ordered by the requested key and order, and paged by
/// `maxResults` — walking the pages visits every stored configuration exactly once, in
/// that order, and the last page carries no token.
#[tokio::test]
async fn pages_walk_every_configuration_in_the_requested_order() {
    let names = ["delta", "alpha", "echo", "bravo", "charlie"];
    let router = router();
    let created = create_named(&router, &names).await;

    let mut by_name: Vec<&str> = names.to_vec();
    by_name.sort_unstable();
    // The ARN order is the order the ARNs sort in, whatever order the names were created
    // in.
    let mut by_arn: Vec<&str> = created.iter().map(|(name, _)| name.as_str()).collect();
    by_arn.sort_by_key(|name| {
        created
            .iter()
            .find(|(created_name, _)| created_name == name)
            .map(|(_, arn)| arn.clone())
            .unwrap_or_default()
    });

    // Rows differ along the sort key, the sort order (named and left out) and the page
    // size, including the model's bounds (1 and 100).
    let orders: [(&str, Option<&str>, Vec<&str>); 6] = [
        ("SCAN_NAME", None, by_name.clone()),
        ("SCAN_NAME", Some("ASC"), by_name.clone()),
        ("SCAN_NAME", Some("DESC"), by_name.iter().rev().copied().collect()),
        ("SCAN_CONFIGURATION_ARN", None, by_arn.clone()),
        ("SCAN_CONFIGURATION_ARN", Some("ASC"), by_arn.clone()),
        (
            "SCAN_CONFIGURATION_ARN",
            Some("DESC"),
            by_arn.iter().rev().copied().collect(),
        ),
    ];

    for (sort_by, sort_order, expected) in orders {
        for page_size in [1_u64, 2, 4, 5, 100] {
            let case = format!("{sort_by}/{sort_order:?} in pages of {page_size}");
            let mut walked: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            // One page per stored configuration is the most a correct walk needs.
            for _ in 0..=names.len() {
                let mut body = json!({ "sortBy": sort_by, "maxResults": page_size });
                if let Some(order) = sort_order {
                    body["sortOrder"] = json!(order);
                }
                if let Some(token) = &next_token {
                    body["nextToken"] = json!(token);
                }
                let (status, error_type, listed) = list(&router, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} / {listed} ({error_type:?})"
                );
                let page = reported_names(&case, &listed);
                assert!(
                    page.len() <= page_size as usize,
                    "case {case:?}: page over maxResults, got {listed}"
                );
                walked.extend(page);
                next_token = listed["nextToken"].as_str().map(str::to_string);
                if next_token.is_none() {
                    break;
                }
            }
            assert_eq!(
                next_token, None,
                "case {case:?}: the walk did not end, got a token after {walked:?}"
            );
            assert_eq!(
                walked, expected,
                "case {case:?}: expected {expected:?}, got {walked:?}"
            );
        }
    }
}

/// RULE: the filter criteria select which stored configurations a list reports — the
/// filters inside one criterion are ORed, the criteria ANDed, and a comparison means what
/// `CisStringComparison` / `TagComparison` say it means.
#[tokio::test]
async fn filter_criteria_select_which_configurations_are_reported() {
    let names = ["alpha", "bravo", "charlie"];
    let router = router();
    let created = create_named(&router, &names).await;
    let arn_of = |name: &str| {
        created
            .iter()
            .find(|(created_name, _)| created_name == name)
            .map(|(_, arn)| arn.clone())
            .unwrap_or_else(|| panic!("no ARN created for {name:?}"))
    };

    // Rows differ along which criterion is filtered, the comparison token, how many
    // filters one criterion carries, and whether two criteria are combined.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "EQUALS one scan name",
            json!({ "scanNameFilters": [{ "comparison": "EQUALS", "value": "alpha" }] }),
            vec!["alpha"],
        ),
        (
            "PREFIX over scan names",
            json!({ "scanNameFilters": [{ "comparison": "PREFIX", "value": "cha" }] }),
            vec!["charlie"],
        ),
        (
            "NOT_EQUALS one scan name",
            json!({ "scanNameFilters": [{ "comparison": "NOT_EQUALS", "value": "alpha" }] }),
            vec!["bravo", "charlie"],
        ),
        (
            "two scan-name filters are ORed",
            json!({ "scanNameFilters": [
                { "comparison": "EQUALS", "value": "alpha" },
                { "comparison": "EQUALS", "value": "charlie" },
            ] }),
            vec!["alpha", "charlie"],
        ),
        (
            "a scan name no configuration carries",
            json!({ "scanNameFilters": [{ "comparison": "EQUALS", "value": "delta" }] }),
            vec![],
        ),
        (
            "EQUALS one configuration ARN",
            json!({ "scanConfigurationArnFilters": [
                { "comparison": "EQUALS", "value": arn_of("bravo") },
            ] }),
            vec!["bravo"],
        ),
        (
            "PREFIX over configuration ARNs",
            json!({ "scanConfigurationArnFilters": [
                { "comparison": "PREFIX", "value": "arn:aws:inspector2:us-east-1:" },
            ] }),
            vec!["alpha", "bravo", "charlie"],
        ),
        (
            "a target resource tag only one configuration carries",
            json!({ "targetResourceTagFilters": [
                { "comparison": "EQUALS", "key": "Stage", "value": "bravo" },
            ] }),
            vec!["bravo"],
        ),
        (
            "a target resource tag every configuration carries",
            json!({ "targetResourceTagFilters": [
                { "comparison": "EQUALS", "key": "Fleet", "value": "all" },
            ] }),
            vec!["alpha", "bravo", "charlie"],
        ),
        (
            "a target resource tag value nothing carries",
            json!({ "targetResourceTagFilters": [
                { "comparison": "EQUALS", "key": "Fleet", "value": "none" },
            ] }),
            vec![],
        ),
        (
            "two criteria are ANDed",
            json!({
                "scanNameFilters": [{ "comparison": "PREFIX", "value": "a" }],
                "targetResourceTagFilters": [
                    { "comparison": "EQUALS", "key": "Stage", "value": "charlie" },
                ],
            }),
            vec![],
        ),
        (
            "two criteria both satisfied",
            json!({
                "scanNameFilters": [{ "comparison": "PREFIX", "value": "c" }],
                "targetResourceTagFilters": [
                    { "comparison": "EQUALS", "key": "Fleet", "value": "all" },
                ],
            }),
            vec!["charlie"],
        ),
    ];

    for (case, filter_criteria, expected) in cases {
        let (status, error_type, listed) = list(
            &router,
            &json!({ "filterCriteria": filter_criteria, "maxResults": 100 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {listed} ({error_type:?})"
        );
        assert_eq!(
            reported_names(case, &listed),
            expected,
            "case {case:?}: expected {expected:?}, got {listed}"
        );
    }
}

/// RULE: a `nextToken` this service did not mint names no position, so the request is
/// refused with the operation's own 400 `ValidationException` rather than answered from
/// the start of the list.
#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let router = router();
    create_named(&router, &["alpha", "bravo"]).await;

    // Rows differ along HOW the token is wrong: not hex at all, hex of text that is no
    // position, hex of a position whose ARN is another service's, and hex of a position
    // with no ARN half.
    let tokens = [
        ("not hexadecimal", "not-a-token".to_string()),
        ("hex of arbitrary text", hex::encode("nonsense")),
        (
            "hex of another service's ARN",
            hex::encode("alpha\narn:aws:s3:::bucket/key"),
        ),
        ("hex of a bare scan name", hex::encode("alpha")),
    ];
    for (case, token) in tokens {
        let (status, error_type, body) = list(
            &router,
            &json!({ "maxResults": 1, "nextToken": token.clone() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for token {token:?}, got {status} / {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {error_type:?} / {body}"
        );
    }

    // The control: a token this service DID mint resumes the walk.
    let (status, _, first) = list(&router, &json!({ "maxResults": 1 })).await;
    assert_eq!(status, 200, "first page: got {first}");
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a page token on a truncated answer, got {first}"))
        .to_string();
    let (status, _, second) = list(&router, &json!({ "maxResults": 1, "nextToken": token })).await;
    assert_eq!(status, 200, "second page: got {second}");
    assert_eq!(
        reported_names("minted token", &second),
        vec!["bravo"],
        "expected the page after the first, got {second}"
    );
}
