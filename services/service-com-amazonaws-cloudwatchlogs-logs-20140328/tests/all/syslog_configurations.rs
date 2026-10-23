// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the syslog configuration loop:
//! `PutSyslogConfiguration` / `ListSyslogConfigurations` / `DeleteSyslogConfiguration`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: a syslog configuration is the ingestion one log group accepts
//! through one VPC endpoint, so it is stored per `(log group, vpcEndpointId)` pair on the
//! group `PutSyslogConfiguration` put it on — a put naming a pair already stored updates
//! that configuration rather than adding a second, `DeleteSyslogConfiguration` takes away
//! exactly the one its pair names, and either operation naming no endpoint is refused for
//! that member. `ListSyslogConfigurations` enumerates those configurations — each reported
//! under the ARN of its log group, with the source type the model names and the instant it
//! was created at — ordered by log group and then by endpoint, narrowed by whichever of the
//! two filters the request names, and cut into pages of `maxResults` with a token that
//! resumes the same filtered scan. An identifier that names no log group is refused as a
//! missing resource, and a delete that names no stored configuration as the parameter that
//! selected nothing.
//!
//! The tables below span the classes the rule generalises over (the endpoint id FORM at
//! both published bounds of its pattern and one past the maximum, an absent endpoint id,
//! the COUNT of configurations on one group and across groups, the identifier FORMS a log
//! group is named by, each FILTER and both together, the page SIZE at both published bounds
//! of `maxResults` and one past the maximum, a re-put of a configuration already stored and
//! a re-delete of one already gone); any single configuration an issue happens to mention
//! is one row.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::MAX_SYSLOG_CONFIGURATIONS_PER_GROUP;

use crate::harness::{now_ms, Logs};

/// The code CloudWatch Logs answers for a parameter it will not accept. All three
/// operations under test declare it and none declares `ValidationException`, so it is also
/// the code a modelled-constraint refusal carries here (see
/// `tests/all/refusal_error_codes.rs`).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when the resource an operation names is not there.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// Every value `SyslogConfiguration$sourceType` can carry. Source: the model's
/// `SyslogSourceType` enum, whose only value is `VPCE`.
const SYSLOG_SOURCE_TYPES: [&str; 1] = ["VPCE"];

/// The published bounds on the hexadecimal part of a `vpcEndpointId`. Source: the model's
/// `VpcEndpointId` shape, `@pattern ^vpce-[0-9a-f]{1,64}$`.
const VPC_ENDPOINT_ID_HEX_MIN: usize = 1;
const VPC_ENDPOINT_ID_HEX_MAX: usize = 64;

/// The refusal a request that names no VPC endpoint carries: the service's own wording for
/// the constraint, naming the member and what it requires.
const VPC_ENDPOINT_ID_REQUIRED: &str = "vpcEndpointId is required";

/// The published bounds of one page. Source: the model's
/// `ListSyslogConfigurationsMaxResults` shape, `@range(min: 0, max: 50)` with a default of
/// `0`.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 50;

/// A `vpcEndpointId` whose hexadecimal part is `hex_len` characters long, so a row can vary
/// the id's LENGTH along the pattern that bounds it (see [`VPC_ENDPOINT_ID_HEX_MIN`]). The
/// characters cycle through the whole `[0-9a-f]` class the pattern admits.
fn endpoint(hex_len: usize) -> String {
    let digits: String = "0123456789abcdef".chars().cycle().take(hex_len).collect();
    format!("vpce-{digits}")
}

/// A distinct `vpcEndpointId` for `index`, rendered in hexadecimal so it satisfies the
/// member's pattern (see [`endpoint`]), for a case that needs more distinct endpoints than
/// the pattern has lengths.
fn indexed_endpoint(index: usize) -> String {
    format!("vpce-{index:04x}")
}

/// The body `PutSyslogConfiguration` and `DeleteSyslogConfiguration` take: a log group
/// identifier and a `vpcEndpointId`. The member is optional in the model on both, so `None`
/// sends no such member at all.
fn syslog_body(identifier: &str, vpc_endpoint_id: Option<&str>) -> Value {
    let mut body = json!({"logGroupIdentifier": identifier});
    if let Some(id) = vpc_endpoint_id {
        body["vpcEndpointId"] = json!(id);
    }
    body
}

/// One `ListSyslogConfigurations` page: the configurations it reports and the token it
/// hands back, if any.
async fn list(logs: &Logs, body: &Value) -> (Vec<Value>, Option<String>) {
    let page = logs.ok("ListSyslogConfigurations", body).await;
    let items = page
        .get("syslogConfigurations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {body}: a page must carry syslogConfigurations, got {page}")
        })
        .clone();
    let token = page
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (items, token)
}

/// What identifies one listed configuration: the ARN of the log group it was put on and the
/// VPC endpoint it ingests through.
type Placement = (String, String);

/// One row of the filter table: what it varies, the request, and the configurations the
/// service must report for it, in order.
type FilterRow = (&'static str, Value, Vec<Placement>);

/// The [`Placement`] of one listed configuration.
fn placement(item: &Value) -> Placement {
    let member = |key: &str| {
        item.get(key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a listed configuration carries {key}, got {item}"))
            .to_string()
    };
    (member("logGroupArn"), member("vpcEndpointId"))
}

/// The `createdAt` one listed configuration reports.
fn created_at(item: &Value) -> i64 {
    item.get("createdAt")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("a listed configuration carries createdAt, got {item}"))
}

/// The source type one listed configuration reports, checked against the model's own
/// catalogue.
fn source_type(item: &Value) -> String {
    let reported = item
        .get("sourceType")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a listed configuration carries sourceType, got {item}"));
    assert!(
        SYSLOG_SOURCE_TYPES.contains(&reported),
        "a listed configuration's sourceType is one the model names ({SYSLOG_SOURCE_TYPES:?}), got {reported:?}"
    );
    reported.to_string()
}

/// A created log group.
async fn create_group(logs: &Logs, name: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name}))
        .await;
}

/// A configuration put on a log group is listed back under that group, whichever form the
/// request named the group by, with the endpoint it was put for, the source type the model
/// names, and the instant of the put.
#[tokio::test]
async fn a_put_configuration_is_listed_back_under_its_log_group() {
    let logs = Logs::new();
    // (case, log group, the vpcEndpointId the put names)
    let rows: [(&str, &str, String); 3] = [
        (
            "the shortest endpoint id",
            "/syslog/shortest",
            endpoint(VPC_ENDPOINT_ID_HEX_MIN),
        ),
        (
            "a mid-length endpoint id",
            "/syslog/mid-length",
            endpoint(17),
        ),
        (
            "the longest endpoint id",
            "/syslog/longest",
            endpoint(VPC_ENDPOINT_ID_HEX_MAX),
        ),
    ];
    for (case, group, vpc_endpoint_id) in rows {
        create_group(&logs, group).await;
        let (_wildcard_arn, group_arn) = logs.group_arns(group).await;

        let before = now_ms();
        logs.ok(
            "PutSyslogConfiguration",
            &syslog_body(group, Some(&vpc_endpoint_id)),
        )
        .await;
        let after = now_ms();

        // The name and the ARN name the same log group, so both read the same
        // configuration back. The `LogGroup$arn` form that carries a trailing `:*` is not
        // a third form here: `*` is outside the modelled `LogGroupIdentifier` pattern, so
        // the boundary refuses it before any handler runs (asserted in
        // `a_request_naming_no_log_group_or_no_endpoint_is_refused`).
        for identifier in [group.to_string(), group_arn.clone()] {
            let (items, token) = list(&logs, &json!({"logGroupIdentifier": identifier})).await;
            assert_eq!(
                items.len(),
                1,
                "case {case} by {identifier}: the group carries exactly the one configuration put on it, got {items:?}"
            );
            let item = &items[0];
            assert_eq!(
                placement(item),
                (group_arn.clone(), vpc_endpoint_id.clone()),
                "case {case} by {identifier}: the configuration is reported under its group's ARN and its endpoint, got {item}"
            );
            assert_eq!(
                source_type(item),
                SYSLOG_SOURCE_TYPES[0],
                "case {case} by {identifier}: the source of a syslog configuration is the VPC endpoint it ingests through, got {item}"
            );
            let created = created_at(item);
            assert!(
                (before..=after).contains(&created),
                "case {case} by {identifier}: expected createdAt in {before}..={after}, got {created}"
            );
            assert!(
                token.is_none(),
                "case {case} by {identifier}: one configuration is one page, so no token, got {token:?}"
            );
        }
    }
}

/// Each endpoint is its own configuration on the group, and a put naming an endpoint
/// already configured updates that one rather than adding a second — keeping the instant it
/// was created at.
#[tokio::test]
async fn each_vpc_endpoint_is_one_configuration_and_a_re_put_updates_it() {
    let logs = Logs::new();
    let group = "/syslog/endpoints";
    create_group(&logs, group).await;
    let (_wildcard_arn, group_arn) = logs.group_arns(group).await;

    // In the ASCII order the service reports them.
    let endpoints = [
        endpoint(VPC_ENDPOINT_ID_HEX_MIN),
        endpoint(8),
        endpoint(VPC_ENDPOINT_ID_HEX_MAX),
    ];
    for vpc_endpoint_id in &endpoints {
        logs.ok(
            "PutSyslogConfiguration",
            &syslog_body(group, Some(vpc_endpoint_id)),
        )
        .await;
    }

    let (items, _) = list(&logs, &json!({})).await;
    let expected: Vec<Placement> = endpoints
        .iter()
        .map(|id| (group_arn.clone(), id.clone()))
        .collect();
    assert_eq!(
        items.iter().map(placement).collect::<Vec<_>>(),
        expected,
        "case {} endpoints: each endpoint is one configuration, reported in endpoint order, got {items:?}",
        endpoints.len()
    );
    let created: Vec<i64> = items.iter().map(created_at).collect();

    // A measurable gap, so a `createdAt` restamped by the second put would be visible.
    tokio::time::sleep(Duration::from_millis(5)).await;
    for vpc_endpoint_id in &endpoints {
        logs.ok(
            "PutSyslogConfiguration",
            &syslog_body(group, Some(vpc_endpoint_id)),
        )
        .await;
    }
    let (re_put, _) = list(&logs, &json!({})).await;
    assert_eq!(
        re_put.iter().map(placement).collect::<Vec<_>>(),
        expected,
        "case a re-put of every endpoint: a put updates the configuration stored for that endpoint, got {re_put:?}"
    );
    assert_eq!(
        re_put.iter().map(created_at).collect::<Vec<_>>(),
        created,
        "case a re-put of every endpoint: an update keeps the instant the configuration was created at, got {re_put:?}"
    );
}

/// Each filter narrows the enumeration to the configurations it names, and the two together
/// to those satisfying both.
#[tokio::test]
async fn the_filters_select_the_configurations_they_name() {
    let logs = Logs::new();
    let alpha = "/syslog/alpha";
    let beta = "/syslog/beta";
    create_group(&logs, alpha).await;
    create_group(&logs, beta).await;
    let (_alpha_wildcard, alpha_arn) = logs.group_arns(alpha).await;
    let (_beta_wildcard, beta_arn) = logs.group_arns(beta).await;

    // One endpoint both groups ingest through, and one only each group does.
    let shared = endpoint(4);
    let alpha_only = endpoint(12);
    let beta_only = endpoint(2);
    let unconfigured = endpoint(6);
    for (group, id) in [
        (alpha, &shared),
        (alpha, &alpha_only),
        (beta, &shared),
        (beta, &beta_only),
    ] {
        logs.ok("PutSyslogConfiguration", &syslog_body(group, Some(id)))
            .await;
    }

    let at = |arn: &str, id: &str| (arn.to_string(), id.to_string());
    let all: Vec<Placement> = vec![
        at(&alpha_arn, &shared),
        at(&alpha_arn, &alpha_only),
        at(&beta_arn, &beta_only),
        at(&beta_arn, &shared),
    ];
    // (case, request, the configurations it must report, in order)
    let rows: Vec<FilterRow> = vec![
        ("no filter at all", json!({}), all.clone()),
        (
            "by log group name",
            json!({"logGroupIdentifier": alpha}),
            vec![at(&alpha_arn, &shared), at(&alpha_arn, &alpha_only)],
        ),
        (
            "by log group ARN",
            json!({"logGroupIdentifier": alpha_arn}),
            vec![at(&alpha_arn, &shared), at(&alpha_arn, &alpha_only)],
        ),
        (
            "by log group name, the other group",
            json!({"logGroupIdentifier": beta}),
            vec![at(&beta_arn, &beta_only), at(&beta_arn, &shared)],
        ),
        (
            "by an endpoint two groups share",
            json!({"vpcEndpointId": shared}),
            vec![at(&alpha_arn, &shared), at(&beta_arn, &shared)],
        ),
        (
            "by an endpoint one group has",
            json!({"vpcEndpointId": alpha_only}),
            vec![at(&alpha_arn, &alpha_only)],
        ),
        (
            "by both filters together",
            json!({"logGroupIdentifier": beta, "vpcEndpointId": shared}),
            vec![at(&beta_arn, &shared)],
        ),
        (
            "by two filters no one configuration satisfies",
            json!({"logGroupIdentifier": beta, "vpcEndpointId": alpha_only}),
            Vec::new(),
        ),
        (
            "by an endpoint nothing was put for",
            json!({"vpcEndpointId": unconfigured}),
            Vec::new(),
        ),
    ];
    for (case, body, expected) in rows {
        let (items, token) = list(&logs, &body).await;
        assert_eq!(
            items.iter().map(placement).collect::<Vec<_>>(),
            expected,
            "case {case}: expected {expected:?}, got {items:?}"
        );
        assert!(
            token.is_none(),
            "case {case}: the whole selection fits one default page, so no token, got {token:?}"
        );
    }

    // A read reports; it must not change what a later caller sees.
    let (again, _) = list(&logs, &json!({})).await;
    assert_eq!(
        again.iter().map(placement).collect::<Vec<_>>(),
        all,
        "case a second read: listing changes nothing a later caller sees, got {again:?}"
    );
}

/// A `logGroupIdentifier` that names no log group is refused as a missing resource; a
/// request that names no VPC endpoint, or a value outside a modelled constraint, is refused
/// for that member.
#[tokio::test]
async fn a_request_naming_no_log_group_or_no_endpoint_is_refused() {
    let logs = Logs::new();
    let existing = "/syslog/existing";
    create_group(&logs, existing).await;
    let (wildcard_arn, group_arn) = logs.group_arns(existing).await;
    logs.ok(
        "PutSyslogConfiguration",
        &syslog_body(existing, Some(&endpoint(3))),
    )
    .await;

    // (case, the identifier that names no log group)
    let absent = [
        (
            "a name no group was created under",
            "/syslog/never-created".to_string(),
        ),
        (
            "an ARN no group was created under",
            group_arn.replace("existing", "never-created"),
        ),
    ];
    for (case, identifier) in &absent {
        for operation in [
            "ListSyslogConfigurations",
            "PutSyslogConfiguration",
            "DeleteSyslogConfiguration",
        ] {
            let body = syslog_body(identifier, Some(&endpoint(3)));
            let code = logs.err(operation, &body).await;
            assert_eq!(
                code, RESOURCE_NOT_FOUND,
                "case {case} on {operation}: expected {RESOURCE_NOT_FOUND}, got {code}"
            );
        }
    }

    // A put and a delete that name no endpoint are refused for that member, in the
    // service's own wording.
    for operation in ["PutSyslogConfiguration", "DeleteSyslogConfiguration"] {
        let (code, message) = logs.refusal(operation, &syslog_body(existing, None)).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case no vpcEndpointId on {operation}: expected {INVALID_PARAMETER}, got {code}"
        );
        assert_eq!(
            message, VPC_ENDPOINT_ID_REQUIRED,
            "case no vpcEndpointId on {operation}: expected {VPC_ENDPOINT_ID_REQUIRED:?}, got {message:?}"
        );
    }

    // (case, operation, body) — each violates one modelled constraint.
    let refused: Vec<(&str, &str, Value)> = vec![
        (
            "the wildcard ARN form, whose `*` is outside the LogGroupIdentifier pattern",
            "ListSyslogConfigurations",
            json!({"logGroupIdentifier": wildcard_arn}),
        ),
        (
            "an endpoint id one character past the pattern's maximum",
            "ListSyslogConfigurations",
            json!({"vpcEndpointId": endpoint(VPC_ENDPOINT_ID_HEX_MAX + 1)}),
        ),
        (
            "an endpoint id one character past the pattern's maximum, on the put",
            "PutSyslogConfiguration",
            syslog_body(existing, Some(&endpoint(VPC_ENDPOINT_ID_HEX_MAX + 1))),
        ),
        (
            "an endpoint id one character past the pattern's maximum, on the delete",
            "DeleteSyslogConfiguration",
            syslog_body(existing, Some(&endpoint(VPC_ENDPOINT_ID_HEX_MAX + 1))),
        ),
        (
            "a maxResults one past the published maximum",
            "ListSyslogConfigurations",
            json!({"maxResults": MAX_RESULTS_MAX + 1}),
        ),
        (
            "a maxResults one below the published minimum",
            "ListSyslogConfigurations",
            json!({"maxResults": MAX_RESULTS_MIN - 1}),
        ),
        (
            "a nextToken this service never minted",
            "ListSyslogConfigurations",
            json!({"nextToken": "not-a-token"}),
        ),
    ];
    for (case, operation, body) in refused {
        let code = logs.err(operation, &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code}"
        );
    }

    // Nothing above stored or removed anything: the one configuration put at the top is
    // still the whole of this account's state.
    let (items, _) = list(&logs, &json!({})).await;
    assert_eq!(
        items.iter().map(placement).collect::<Vec<_>>(),
        vec![(group_arn, endpoint(3))],
        "case after every refusal: a refused request changes nothing, got {items:?}"
    );
}

/// The page walk: a token resumes the same filtered scan, every configuration is reported
/// exactly once and in order, and the last page carries no token.
#[tokio::test]
async fn pagination_walks_every_configuration_exactly_once() {
    let logs = Logs::new();
    // Two groups, so a page boundary can fall inside one group and between two.
    let groups = ["/syslog/page-a", "/syslog/page-b"];
    let per_group = [4usize, 3];
    for (group, count) in groups.iter().zip(per_group) {
        create_group(&logs, group).await;
        for index in 0..count {
            logs.ok(
                "PutSyslogConfiguration",
                &syslog_body(group, Some(&endpoint(index + 1))),
            )
            .await;
        }
    }
    let total: usize = per_group.iter().sum();

    // `maxResults` left at its modelled default returns the whole set, since it is under
    // the published maximum, and so does sending that default explicitly.
    let (all, token) = list(&logs, &json!({})).await;
    assert_eq!(
        all.len(),
        total,
        "case maxResults unset: the whole set fits one default page, got {all:?}"
    );
    assert!(
        token.is_none(),
        "case maxResults unset: the last page carries no token, got {token:?}"
    );
    let expected: Vec<Placement> = all.iter().map(placement).collect();
    let mut sorted = expected.clone();
    sorted.sort();
    assert_eq!(
        expected, sorted,
        "case maxResults unset: the enumeration is ordered by log group and then by endpoint, got {expected:?}"
    );
    let (defaulted, _) = list(&logs, &json!({"maxResults": MAX_RESULTS_MIN})).await;
    assert_eq!(
        defaulted.iter().map(placement).collect::<Vec<_>>(),
        expected,
        "case maxResults {MAX_RESULTS_MIN}: the modelled default is the same page as sending none, got {defaulted:?}"
    );

    for page_size in [1usize, 2, 3, total, usize::try_from(MAX_RESULTS_MAX).unwrap()] {
        let mut seen: Vec<Placement> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({"maxResults": page_size});
            if let Some(token) = &next {
                body["nextToken"] = json!(token);
            }
            let (items, token) = list(&logs, &body).await;
            pages += 1;
            assert!(
                items.len() <= page_size,
                "case page size {page_size}, page {pages}: a page holds at most maxResults, got {items:?}"
            );
            if token.is_some() {
                assert_eq!(
                    items.len(),
                    page_size,
                    "case page size {page_size}, page {pages}: a page with a successor is full, got {items:?}"
                );
            }
            seen.extend(items.iter().map(placement));
            match token {
                Some(token) => next = Some(token),
                None => break,
            }
            assert!(
                pages <= total,
                "case page size {page_size}: the walk must terminate within {total} pages, seen {seen:?}"
            );
        }
        assert_eq!(
            seen, expected,
            "case page size {page_size}: the walk reports every configuration exactly once and in order, got {seen:?}"
        );
    }

    // A token is a position within ONE filtered scan, so a continuation that changes a
    // filter is refused rather than silently answering a different enumeration.
    let (_first, token) = list(&logs, &json!({"maxResults": 1})).await;
    let token = token.expect("a page followed by another carries a token");
    let code = logs
        .err(
            "ListSyslogConfigurations",
            &json!({"maxResults": 1, "nextToken": token, "logGroupIdentifier": groups[1]}),
        )
        .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case a token continued with a different filter: expected {INVALID_PARAMETER}, got {code}"
    );
}

/// A configuration lives on the log group it was put on, so deleting that group takes its
/// configurations with it and leaves every other group's alone.
#[tokio::test]
async fn deleting_a_log_group_takes_its_configurations_with_it() {
    let logs = Logs::new();
    let deleted = "/syslog/deleted";
    let kept = "/syslog/kept";
    create_group(&logs, deleted).await;
    create_group(&logs, kept).await;
    let (_kept_wildcard, kept_arn) = logs.group_arns(kept).await;
    for (group, id) in [
        (deleted, endpoint(2)),
        (deleted, endpoint(3)),
        (kept, endpoint(2)),
    ] {
        logs.ok("PutSyslogConfiguration", &syslog_body(group, Some(&id)))
            .await;
    }
    let (before, _) = list(&logs, &json!({})).await;
    assert_eq!(
        before.len(),
        3,
        "case before the delete: both groups' configurations are reported, got {before:?}"
    );

    logs.ok("DeleteLogGroup", &json!({"logGroupName": deleted}))
        .await;

    let (after, _) = list(&logs, &json!({})).await;
    assert_eq!(
        after.iter().map(placement).collect::<Vec<_>>(),
        vec![(kept_arn, endpoint(2))],
        "case after the delete: only the surviving group's configuration is reported, got {after:?}"
    );
    let code = logs
        .err(
            "ListSyslogConfigurations",
            &json!({"logGroupIdentifier": deleted}),
        )
        .await;
    assert_eq!(
        code, RESOURCE_NOT_FOUND,
        "case the deleted group: expected {RESOURCE_NOT_FOUND}, got {code}"
    );

    // A group recreated under the same name starts with no configuration.
    create_group(&logs, deleted).await;
    let (recreated, _) = list(&logs, &json!({"logGroupIdentifier": deleted})).await;
    assert!(
        recreated.is_empty(),
        "case a recreated log group: it carries no configuration, got {recreated:?}"
    );
}

/// A log group takes on at most this store's ceiling of syslog configurations: once it
/// holds that many, a put that would ADD another endpoint is refused, while a put under an
/// endpoint it already holds still updates in place, and freeing one makes room again.
#[tokio::test]
async fn a_log_group_holds_at_most_the_stores_ceiling_of_configurations() {
    let logs = Logs::new();
    let group = "/syslog/capped";
    create_group(&logs, group).await;
    for index in 0..MAX_SYSLOG_CONFIGURATIONS_PER_GROUP {
        logs.ok(
            "PutSyslogConfiguration",
            &syslog_body(group, Some(&indexed_endpoint(index))),
        )
        .await;
    }
    // The ceiling is above one page, so count the group by walking the pages.
    let mut counted = 0usize;
    let mut next: Option<String> = None;
    loop {
        let mut body = json!({"logGroupIdentifier": group, "maxResults": MAX_RESULTS_MAX});
        if let Some(token) = &next {
            body["nextToken"] = json!(token);
        }
        let (items, token) = list(&logs, &body).await;
        counted += items.len();
        match token {
            Some(token) => next = Some(token),
            None => break,
        }
        assert!(
            counted <= MAX_SYSLOG_CONFIGURATIONS_PER_GROUP,
            "case a group filled to the ceiling: the walk must not report more than were put, got {counted}"
        );
    }
    assert_eq!(
        counted, MAX_SYSLOG_CONFIGURATIONS_PER_GROUP,
        "case a group filled to the ceiling: every put was stored, got {counted}"
    );

    // One more endpoint is refused, in a message naming the ceiling.
    let beyond = indexed_endpoint(MAX_SYSLOG_CONFIGURATIONS_PER_GROUP);
    let (code, message) = logs
        .refusal("PutSyslogConfiguration", &syslog_body(group, Some(&beyond)))
        .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case one endpoint past the ceiling: expected {INVALID_PARAMETER}, got {code}"
    );
    assert!(
        message.contains(&MAX_SYSLOG_CONFIGURATIONS_PER_GROUP.to_string()),
        "case one endpoint past the ceiling: the refusal names the ceiling, got {message:?}"
    );

    // A put under an endpoint the group already holds is an update, so the ceiling does
    // not refuse it.
    logs.ok(
        "PutSyslogConfiguration",
        &syslog_body(group, Some(&indexed_endpoint(0))),
    )
    .await;

    // Freeing one makes room for the endpoint that was refused.
    logs.ok(
        "DeleteSyslogConfiguration",
        &syslog_body(group, Some(&indexed_endpoint(0))),
    )
    .await;
    logs.ok("PutSyslogConfiguration", &syslog_body(group, Some(&beyond)))
        .await;
    let (stored, _) = list(
        &logs,
        &json!({"logGroupIdentifier": group, "vpcEndpointId": beyond}),
    )
    .await;
    assert_eq!(
        stored.len(),
        1,
        "case after freeing one: the endpoint that was refused is stored now, got {stored:?}"
    );
}

/// `DeleteSyslogConfiguration` takes away exactly the configuration its `vpcEndpointId`
/// names on the group its identifier resolves to, leaves that group's other endpoints
/// ingesting, and refuses a delete that names a configuration the group does not carry.
#[tokio::test]
async fn delete_removes_the_configuration_its_endpoint_names() {
    let logs = Logs::new();
    let group = "/syslog/deletable";
    let other = "/syslog/untouched";
    create_group(&logs, group).await;
    create_group(&logs, other).await;
    let (_wildcard_arn, group_arn) = logs.group_arns(group).await;
    let (_other_wildcard, other_arn) = logs.group_arns(other).await;
    logs.ok(
        "PutSyslogConfiguration",
        &syslog_body(other, Some(&endpoint(5))),
    )
    .await;

    // The identifier FORMS name the same group, so a delete through either takes the
    // configuration away; each round re-puts the endpoints so the next form has something
    // to delete.
    for identifier in [group.to_string(), group_arn.clone()] {
        let endpoints = [endpoint(1), endpoint(9), endpoint(16)];
        for vpc_endpoint_id in &endpoints {
            logs.ok(
                "PutSyslogConfiguration",
                &syslog_body(&identifier, Some(vpc_endpoint_id)),
            )
            .await;
        }

        // Delete them one at a time: after each, the group carries exactly the endpoints
        // not deleted yet.
        for (deleted, vpc_endpoint_id) in endpoints.iter().enumerate() {
            let body = syslog_body(&identifier, Some(vpc_endpoint_id));
            logs.ok("DeleteSyslogConfiguration", &body).await;

            let (left, _) = list(&logs, &json!({"logGroupIdentifier": identifier})).await;
            let expected: Vec<Placement> = endpoints[deleted + 1..]
                .iter()
                .map(|id| (group_arn.clone(), id.clone()))
                .collect();
            assert_eq!(
                left.iter().map(placement).collect::<Vec<_>>(),
                expected,
                "case {identifier} after deleting {vpc_endpoint_id}: the endpoints not deleted yet are untouched, got {left:?}"
            );

            // Deleting the same configuration again names one that is no longer there.
            let (code, message) = logs.refusal("DeleteSyslogConfiguration", &body).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {identifier} deleting {vpc_endpoint_id} twice: expected {INVALID_PARAMETER}, got {code}"
            );
            assert!(
                message.contains(vpc_endpoint_id.as_str()),
                "case {identifier} deleting {vpc_endpoint_id} twice: the refusal names the endpoint it was built from, got {message:?}"
            );
        }
    }

    // One configuration back on the group, so the refusal below has a stored configuration
    // it does NOT name.
    logs.ok(
        "PutSyslogConfiguration",
        &syslog_body(group, Some(&endpoint(2))),
    )
    .await;
    let code = logs
        .err(
            "DeleteSyslogConfiguration",
            &syslog_body(group, Some(&endpoint(7))),
        )
        .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case an endpoint never configured on a group that has another: expected {INVALID_PARAMETER}, got {code}"
    );

    // Every refusal above left both groups' configurations exactly as they were.
    let (items, _) = list(&logs, &json!({})).await;
    assert_eq!(
        items.iter().map(placement).collect::<Vec<_>>(),
        vec![
            (group_arn, endpoint(2)),
            (other_arn, endpoint(5)),
        ],
        "case after every refused delete: nothing was removed, got {items:?}"
    );

    // A put after a delete creates the configuration again, with the instant of that put.
    let first_created = created_at(&items[0]);
    logs.ok(
        "DeleteSyslogConfiguration",
        &syslog_body(group, Some(&endpoint(2))),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let before = now_ms();
    logs.ok(
        "PutSyslogConfiguration",
        &syslog_body(group, Some(&endpoint(2))),
    )
    .await;
    let after = now_ms();
    let (recreated, _) = list(&logs, &json!({"logGroupIdentifier": group})).await;
    assert_eq!(
        recreated.len(),
        1,
        "case a put after a delete: the configuration is back, got {recreated:?}"
    );
    let created = created_at(&recreated[0]);
    assert!(
        (before..=after).contains(&created),
        "case a put after a delete: it is a new configuration, expected createdAt in {before}..={after}, got {created} (the deleted one was created at {first_created})"
    );
}
