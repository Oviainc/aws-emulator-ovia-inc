// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAssetInstances`, driven over the wire through the same
//! front door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite
//! reuses), recording its instances through the same store seam
//! [`crate::blocking_instances`] records them through — and reusing that suite's
//! recorded-instance data, so the two lists over one Outpost's instances are asserted
//! against ONE record of what is running on it.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The list reports the Amazon EC2 instances RECORDED as running on the Outpost the
//!    identifier names, belonging to ALL accounts — not only the caller's — in the
//!    Outpost's own recorded order, each as the whole modelled `AssetInstance`
//!    (`InstanceId`, `InstanceType`, `AssetId`, `AccountId`, `AwsServiceName`), and
//!    nothing more: an Outpost nobody has launched an instance on reports no instance,
//!    and a member an instance never recorded is reported by no member.
//! 2. Each `*Filter` is a LIST of accepted values for ONE member of an instance
//!    (`AssetIdFilter` on its `AssetId`, `InstanceTypeFilter` on its `InstanceType`,
//!    `AccountIdFilter` on its `AccountId`, `AwsServiceFilter` on its `AwsServiceName`),
//!    so an instance matches when its value for that member is one of them; several
//!    filters together narrow rather than widen, an absent filter matches everything,
//!    and an instance with no value for a filtered member matches none.
//! 3. A page holds at most `MaxResults` instances and carries a `NextToken` that resumes
//!    exactly where it stopped, so paging from the first token to the last yields every
//!    instance once, in that same recorded order.
//! 4. Both forms the modelled `OutpostIdentifier` pattern
//!    `^(arn:…:outpost/)?op-[a-f0-9]{17}$` accepts name the same Outpost, and the list is
//!    a REPORT: repeating it answers the same thing and changes nothing. An identifier
//!    that names no Outpost (including one whose Outpost was deleted) is 404
//!    `NotFoundException` with the identifier echo of the value AS SUPPLIED; an
//!    identifier that is no identifier at all, a `MaxResults` outside the modelled
//!    range, and a `NextToken` this service did not issue for THIS list are 400
//!    `ValidationException`.
//!
//! Where the recorded instances come from: an instance is launched through Amazon EC2 —
//! `RunInstances` against the Outpost — and by no call in THIS API, so each row RECORDS
//! its instances on the stored Outpost and then asks over the wire. The instances are
//! the rows' own data; the emulator is never expected to produce an instance of its own.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    AssetInstance, A_W_S_SERVICE_NAME_VALUES,
};

use crate::blocking_instances::{
    instance, instance_id, own_account, record, running, OTHER_ACCOUNT, THIRD_ACCOUNT,
};
use crate::site_and_outpost_lifecycle::{
    query_value, refusal, segment, settle, text, Outposts, Refusal, ABSENT_OUTPOST_ID,
    ABSENT_SITE_ID, OUTPOST_ARN_PREFIX,
};

/// The route `ListAssetInstances` binds, for one Outpost identifier form and one query
/// string.
fn route(outpost_identifier: &str, query: &str) -> String {
    format!(
        "/outposts/{}/assetInstances{query}",
        segment(outpost_identifier)
    )
}

/// The `AssetInstances` a page reports, as a list.
fn reported(output: &Value, case: &str) -> Vec<Value> {
    output
        .get("AssetInstances")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an AssetInstances list, got {output}"))
}

/// The `InstanceId`s a page reports, in the order it reported them.
fn reported_ids(output: &Value, case: &str) -> Vec<String> {
    reported(output, case)
        .iter()
        .map(|item| text(item, "InstanceId"))
        .collect()
}

/// The whole `AssetInstance` the wire must carry for a recorded instance: every member
/// it recorded, under the name the model gives that member, and no member it did not
/// record.
fn projection(instance: &AssetInstance) -> Value {
    let mut member = serde_json::Map::new();
    for (name, value) in [
        ("InstanceId", &instance.instance_id),
        ("InstanceType", &instance.instance_type),
        ("AssetId", &instance.asset_id),
        ("AccountId", &instance.account_id),
        ("AwsServiceName", &instance.aws_service_name),
    ] {
        if let Some(value) = value {
            member.insert(name.to_string(), json!(value));
        }
    }
    Value::Object(member)
}

/// Those projections, in recorded order — what rule 1 expects a page covering the whole
/// list to answer.
fn projections(instances: &[AssetInstance]) -> Vec<Value> {
    instances.iter().map(projection).collect()
}

/// The four `*Filter` members of one request, each a list of accepted values for one
/// member of an instance. A list left empty is one the request never carried.
#[derive(Debug, Default, Clone)]
struct Filters {
    asset_ids: Vec<String>,
    instance_types: Vec<String>,
    account_ids: Vec<String>,
    services: Vec<String>,
}

impl Filters {
    /// The query string these filters travel in: each modelled `@httpQuery` list member
    /// is repeated once per accepted value, which is what an AWS SDK puts on the wire.
    fn query(&self) -> String {
        let mut params: Vec<String> = Vec::new();
        for (name, values) in [
            ("AssetIdFilter", &self.asset_ids),
            ("InstanceTypeFilter", &self.instance_types),
            ("AccountIdFilter", &self.account_ids),
            ("AwsServiceFilter", &self.services),
        ] {
            for value in values {
                params.push(format!("{name}={}", query_value(value)));
            }
        }
        match params.is_empty() {
            true => String::new(),
            false => format!("?{}", params.join("&")),
        }
    }

    /// Whether every filter accepts `instance` — rule 2, stated once and applied to
    /// every row, so each row's expectation is COMPUTED from the rule rather than
    /// spelled out.
    fn accepts(&self, instance: &AssetInstance) -> bool {
        let names = |filter: &Vec<String>, value: Option<&String>| match filter.is_empty() {
            true => true,
            false => value.is_some_and(|value| filter.contains(value)),
        };
        names(&self.asset_ids, instance.asset_id.as_ref())
            && names(&self.instance_types, instance.instance_type.as_ref())
            && names(&self.account_ids, instance.account_id.as_ref())
            && names(&self.services, instance.aws_service_name.as_ref())
    }
}

/// The accepted values of one filter, as the strings a request carries.
fn values(accepted: &[&str]) -> Vec<String> {
    accepted.iter().map(|value| (*value).to_string()).collect()
}

/// The instances `filters` select out of `recorded`, in recorded order.
fn selected(recorded: &[AssetInstance], filters: &Filters) -> Vec<AssetInstance> {
    recorded
        .iter()
        .filter(|instance| filters.accepts(instance))
        .cloned()
        .collect()
}

/// The `InstanceId`s of those instances, in that same order.
fn selected_ids(recorded: &[AssetInstance], filters: &Filters) -> Vec<String> {
    selected(recorded, filters)
        .iter()
        .map(|instance| instance.instance_id.clone().unwrap_or_default())
        .collect()
}

/// One Outpost to record instances on, as its `(OutpostId, OutpostArn)` — the two
/// identifier forms every route that takes an Outpost accepts.
async fn an_outpost(outposts: &Outposts, name: &str) -> (String, String) {
    outposts.site_and_outpost(name).await
}

/// Rules 1 + 4's identifier half: the list answers every recorded instance, whichever
/// account owns it, field for field and in recorded order, through either identifier
/// form.
///
/// The rows differ along everything the rule generalises over: how many instances are
/// recorded (none, one, two, twelve), which members each recorded (all of them, only an
/// id, only an id and an account, no owning service, no owning account at all), which
/// account owns each (the caller's own, two others, none), and which owning service.
#[tokio::test]
async fn the_list_reports_every_recorded_instance_whoever_owns_it() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Vec<AssetInstance>)> = vec![
        ("no instance running on the Outpost", Vec::new()),
        (
            "one instance of the caller's own account, every member recorded",
            vec![instance(
                &instance_id(301),
                Some("m5.4xlarge"),
                Some("1000000001"),
                Some(own_account()),
                Some("EC2"),
            )],
        ),
        (
            "one instance that recorded nothing but its id",
            vec![AssetInstance {
                instance_id: Some(instance_id(302)),
                ..AssetInstance::default()
            }],
        ),
        (
            "only another account's instances, which this list reports all the same",
            vec![
                instance(
                    &instance_id(303),
                    Some("m5.large"),
                    Some("1000000002"),
                    Some(OTHER_ACCOUNT),
                    None,
                ),
                instance(
                    &instance_id(304),
                    Some("r5.large"),
                    Some("1000000002"),
                    Some(THIRD_ACCOUNT),
                    Some("RDS"),
                ),
            ],
        ),
        (
            "one instance whose owning account was never recorded",
            vec![instance(
                &instance_id(305),
                Some("c5.24xlarge"),
                Some("1000000003"),
                None,
                Some("ELB"),
            )],
        ),
        (
            "every recorded instance: three accounts, one with none, and every owning \
             service the model declares",
            running(),
        ),
    ];

    for (case, recorded) in &cases {
        let (outpost_id, outpost_arn) = an_outpost(&outposts, "asset-instances").await;
        record(&outposts, &outpost_id, recorded, case);

        for identifier in [&outpost_id, &outpost_arn] {
            let case = &format!("{case} read by {identifier}");
            let reply = outposts.ok("GET", &route(identifier, ""), None).await;
            assert_eq!(
                reported(&reply, case),
                projections(recorded),
                "case {case:?}: the list must answer every recorded instance, field for field \
                 and in recorded order, got {reply}"
            );
            // A page that exhausts the list issues no token to resume from.
            assert!(
                reply.get("NextToken").is_none(),
                "case {case:?}: a page holding every instance issues no NextToken, got {reply}"
            );
        }
    }
}

/// Rule 2: every filter row, with the expectation computed from the recorded instances
/// rather than spelled out — so each value of the modelled `AWSServiceName` catalog, and
/// each recorded asset id, instance type and owning account, is covered.
#[tokio::test]
async fn filters_narrow_to_the_instances_that_match_every_one_of_them() {
    let outposts = Outposts::new();
    let recorded = running();
    let (outpost_id, _) = an_outpost(&outposts, "asset-instance-filters").await;
    record(&outposts, &outpost_id, &recorded, "filters");

    let mut cases: Vec<(String, Filters)> = vec![
        ("no filter at all".to_string(), Filters::default()),
        (
            "two asset ids, so the union of both".to_string(),
            Filters {
                asset_ids: values(&["1000000001", "1000000003"]),
                ..Filters::default()
            },
        ),
        (
            "an asset id no instance runs on".to_string(),
            Filters {
                asset_ids: values(&["9999999999"]),
                ..Filters::default()
            },
        ),
        (
            "two instance types, so the union of both".to_string(),
            Filters {
                instance_types: values(&["m5.large", "r5.large"]),
                ..Filters::default()
            },
        ),
        (
            "an instance type nothing runs".to_string(),
            Filters {
                instance_types: values(&["t3.nano"]),
                ..Filters::default()
            },
        ),
        (
            "the two accounts the Outpost is shared with".to_string(),
            Filters {
                account_ids: values(&[OTHER_ACCOUNT, THIRD_ACCOUNT]),
                ..Filters::default()
            },
        ),
        (
            "an account with no instance on the Outpost".to_string(),
            Filters {
                account_ids: values(&["999999999999"]),
                ..Filters::default()
            },
        ),
        (
            "two owning services, so the union of both".to_string(),
            Filters {
                services: values(&["EC2", "EKS"]),
                ..Filters::default()
            },
        ),
        (
            "every owning service at once, so every instance that recorded one".to_string(),
            Filters {
                services: A_W_S_SERVICE_NAME_VALUES
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
                ..Filters::default()
            },
        ),
        (
            "an account and an owning service together narrow rather than widen".to_string(),
            Filters {
                account_ids: values(&[own_account()]),
                services: values(&["RDS"]),
                ..Filters::default()
            },
        ),
        (
            "an account and an owning service that belong to different instances".to_string(),
            Filters {
                account_ids: values(&[OTHER_ACCOUNT]),
                services: values(&["ROUTE53"]),
                ..Filters::default()
            },
        ),
        (
            "an asset id and an instance type together".to_string(),
            Filters {
                asset_ids: values(&["1000000003"]),
                instance_types: values(&["m5.large"]),
                ..Filters::default()
            },
        ),
        (
            "all four filters at once".to_string(),
            Filters {
                asset_ids: values(&["1000000002"]),
                instance_types: values(&["m5.xlarge"]),
                account_ids: values(&[own_account()]),
                services: values(&["ELASTICACHE"]),
            },
        ),
    ];
    // One row per value of the catalog the model declares for `AwsServiceFilter`, so no
    // value goes untested: an instance matches the filter exactly when it recorded that
    // value, and one that recorded no owning service matches none. The catalog is
    // `ops::types::A_W_S_SERVICE_NAME_VALUES`, read off the model's own enum.
    for service in A_W_S_SERVICE_NAME_VALUES {
        cases.push((
            format!("the one owning service {service}"),
            Filters {
                services: values(&[service]),
                ..Filters::default()
            },
        ));
    }
    // One row per asset id, instance type and owning account the data actually recorded,
    // so every value present in the list is filtered on at least once.
    for asset_id in recorded.iter().filter_map(|i| i.asset_id.clone()) {
        cases.push((
            format!("the one asset id {asset_id}"),
            Filters {
                asset_ids: vec![asset_id],
                ..Filters::default()
            },
        ));
    }
    for instance_type in recorded.iter().filter_map(|i| i.instance_type.clone()) {
        cases.push((
            format!("the one instance type {instance_type}"),
            Filters {
                instance_types: vec![instance_type],
                ..Filters::default()
            },
        ));
    }
    for account_id in recorded.iter().filter_map(|i| i.account_id.clone()) {
        cases.push((
            format!("the one account {account_id}"),
            Filters {
                account_ids: vec![account_id],
                ..Filters::default()
            },
        ));
    }

    for (case, filters) in &cases {
        let query = filters.query();
        let reply = outposts.ok("GET", &route(&outpost_id, &query), None).await;
        assert_eq!(
            reported(&reply, case),
            projections(&selected(&recorded, filters)),
            "case {case:?} ({query}): the list must answer only the matching instances, got \
             {reply}"
        );
    }
}

/// Rule 3: for EVERY page size, paging the list from the first `NextToken` to the last
/// yields exactly the recorded instances, once each, in recorded order — unfiltered and
/// under a filter alike.
///
/// The page sizes span one instance per page, a size that leaves a short last page, an
/// exact divisor of the count, the count itself, more than the count, and the modelled
/// `MaxResults1000` maximum, so a token that duplicated or skipped an instance at any
/// boundary would show up.
#[tokio::test]
async fn paging_covers_every_recorded_instance_exactly_once() {
    let outposts = Outposts::new();
    let recorded = running();
    let (outpost_id, _) = an_outpost(&outposts, "asset-instance-paging").await;
    record(&outposts, &outpost_id, &recorded, "paging");

    let filters: Vec<(&str, Filters)> = vec![
        ("no filter", Filters::default()),
        (
            "filtered to the caller's own account",
            Filters {
                account_ids: values(&[own_account()]),
                ..Filters::default()
            },
        ),
        (
            "filtered to two owning services",
            Filters {
                services: values(&["EC2", "ELB"]),
                ..Filters::default()
            },
        ),
    ];

    for (filter_case, filter) in &filters {
        let expected = selected_ids(&recorded, filter);
        let filter_query = filter.query().replacen('?', "", 1);
        let prefix = match filter_query.is_empty() {
            true => String::new(),
            false => format!("{filter_query}&"),
        };

        for size in [1usize, 2, 3, 5, 12, 13, 1000] {
            let case = &format!("{filter_case}, page size {size}");
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let query = match &token {
                    Some(token) => format!(
                        "?{prefix}MaxResults={size}&NextToken={}",
                        query_value(token)
                    ),
                    None => format!("?{prefix}MaxResults={size}"),
                };
                let page = outposts.ok("GET", &route(&outpost_id, &query), None).await;
                let page_ids = reported_ids(&page, case);
                assert!(
                    page_ids.len() <= size,
                    "case {case:?}: a page holds at most MaxResults instances, got {page}"
                );
                collected.extend(page_ids);
                pages += 1;
                assert!(
                    pages <= recorded.len() + 1,
                    "case {case:?}: paging must terminate, got {collected:?}"
                );
                match page.get("NextToken").and_then(Value::as_str) {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                collected, expected,
                "case {case:?}: paging must yield every matching instance once, in recorded order"
            );
        }
    }
}

/// Rule 4's report half: listing an Outpost's asset instances changes nothing — not the
/// list it answers, not the Outpost, not the recorded instances, not another Outpost's
/// list.
#[tokio::test]
async fn listing_asset_instances_changes_nothing_a_later_call_sees() {
    let outposts = Outposts::new();
    let recorded = running();
    let (busy, busy_arn) = an_outpost(&outposts, "busy-instances").await;
    let (idle, _) = an_outpost(&outposts, "idle-instances").await;
    record(&outposts, &busy, &recorded, "busy");

    // Past the Outpost lifecycle transition the clock owes a just-created Outpost, so the
    // Outpost snapshots below differ only if THIS read changed one.
    settle();
    let before = outposts.get_outpost(&busy).await;

    let mut answers = Vec::new();
    for pass in ["first read", "second read", "third read"] {
        answers.push((pass, outposts.ok("GET", &route(&busy, ""), None).await));
        // The ARN form names the same Outpost, so it must answer the same list.
        answers.push((pass, outposts.ok("GET", &route(&busy_arn, ""), None).await));
        // Another Outpost's instances are its own: listing one never fills in another.
        let other = outposts.ok("GET", &route(&idle, ""), None).await;
        assert_eq!(
            other.get("AssetInstances"),
            Some(&json!([])),
            "case {pass:?}: an Outpost running no instance reports no asset instance, got {other}"
        );
    }
    let (_, first) = &answers[0];
    for (pass, answer) in &answers {
        assert_eq!(
            answer, first,
            "case {pass:?}: every listing of one Outpost's asset instances must answer the same"
        );
    }

    let after = outposts.get_outpost(&busy).await;
    assert_eq!(
        after, before,
        "listing asset instances must not change the Outpost itself"
    );
    let held: Vec<Option<String>> = outposts.store().lock().outposts[&busy]
        .instances
        .iter()
        .map(|instance| instance.instance_id.clone())
        .collect();
    let expected_held: Vec<Option<String>> = recorded
        .iter()
        .map(|instance| instance.instance_id.clone())
        .collect();
    assert_eq!(
        held, expected_held,
        "listing asset instances must leave the recorded instances alone: expected \
         {expected_held:?}, got {held:?}"
    );
}

/// Rule 4's refusal half: an Outpost that names nothing is 404 `NotFoundException`; an
/// identifier that is no identifier at all, a `MaxResults` outside the modelled range,
/// and a `NextToken` this service did not issue for this list are 400
/// `ValidationException`.
#[tokio::test]
async fn an_absent_outpost_is_not_found_and_bad_input_is_refused() {
    let outposts = Outposts::new();
    let (live, _) = an_outpost(&outposts, "live-instances").await;
    record(&outposts, &live, &running(), "live");

    // An Outpost that existed and had instances recorded on it, and was then deleted:
    // they went with it, so the route answers as it does for one that never was.
    let (deleted, _) = an_outpost(&outposts, "deleted-instances").await;
    record(&outposts, &deleted, &running(), "deleted");
    outposts
        .ok("DELETE", &format!("/outposts/{deleted}"), None)
        .await;

    // A token this service issued for ANOTHER list is not a boundary of this one.
    let outposts_page = outposts.ok("GET", "/outposts?MaxResults=1", None).await;
    let other_token = outposts_page
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or("1##outposts")
        .to_string();

    let cases: Vec<Refusal> = vec![
        refusal(
            "a well-formed Outpost id that names nothing",
            "GET",
            route(ABSENT_OUTPOST_ID, ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "the ARN form of an Outpost that names nothing",
            "GET",
            route(&format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}"), ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "an Outpost that was deleted",
            "GET",
            route(&deleted, ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "a label that is no Outpost identifier at all",
            "GET",
            route("not-an-outpost-id", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a site id in place of the Outpost identifier",
            "GET",
            route(ABSENT_SITE_ID, ""),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost id whose suffix is not hex",
            "GET",
            route("op-zzzzzzzzzzzzzzzzz", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a ListOutposts token replayed on this list",
            "GET",
            route(&live, &format!("?NextToken={}", query_value(&other_token))),
            400,
            "ValidationException",
        ),
        refusal(
            "a ListAssets token replayed on this list",
            "GET",
            route(&live, &format!("?NextToken={}", query_value("1##assets"))),
            400,
            "ValidationException",
        ),
        refusal(
            "a blocking-instances token replayed on this list",
            "GET",
            route(
                &live,
                &format!("?NextToken={}", query_value("1##blockingInstances")),
            ),
            400,
            "ValidationException",
        ),
        refusal(
            "a token this service never issued at all",
            "GET",
            route(&live, "?NextToken=1%23%23nonsense"),
            400,
            "ValidationException",
        ),
        refusal(
            "MaxResults below the modelled minimum of 1",
            "GET",
            route(&live, "?MaxResults=0"),
            400,
            "ValidationException",
        ),
        refusal(
            "MaxResults above the modelled maximum of 1000",
            "GET",
            route(&live, "?MaxResults=1001"),
            400,
            "ValidationException",
        ),
    ];

    outposts.expect_refusals(&cases).await;
}

/// Rule 4's message half: a not-found refusal is the identifier ECHO
/// `<member>=<value as supplied>` — of the member the identifier arrived in, and of the
/// ARN form when that is what the caller sent.
#[tokio::test]
async fn a_not_found_refusal_echoes_the_identifier_the_request_supplied() {
    let outposts = Outposts::new();
    let absent_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");

    let cases: Vec<(&str, String, String)> = vec![
        (
            "an absent Outpost named by bare id",
            route(ABSENT_OUTPOST_ID, ""),
            format!("outpostIdentifier={ABSENT_OUTPOST_ID}"),
        ),
        (
            "an absent Outpost named by ARN",
            route(&absent_arn, ""),
            format!("outpostIdentifier={absent_arn}"),
        ),
    ];

    for (case, path, expected) in &cases {
        let reply = outposts.call("GET", path, None).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = reply
            .body
            .get("Message")
            .or_else(|| reply.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the identifier echo {expected:?}, got {actual:?}"
        );
    }
}
