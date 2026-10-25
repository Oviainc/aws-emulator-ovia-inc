// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListBlockingInstancesForCapacityTask`, driven over the wire
//! through the same front door as [`crate::site_and_outpost_lifecycle`] (whose harness
//! this suite reuses), starting its capacity tasks through
//! [`crate::capacity_task_lifecycle`]'s own `StartCapacityTask` helper.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The list reports the Amazon EC2 instances RECORDED as running on the Outpost the
//!    identifier names that the capacity task the route names would need stopped: the
//!    ones belonging to the account that initiated the task, in the Outpost's own
//!    recorded order, each as the `BlockingInstance` projection (`InstanceId`,
//!    `AccountId`, `AwsServiceName` and nothing else) — and nothing more: an Outpost
//!    nobody has launched an instance on reports no instance, another account's instance
//!    is not reported, and a member no instance recorded is reported by no member.
//! 2. An instance the task's `InstancesToExclude` names "will not appear in the list of
//!    instances that Amazon Web Services recommends to stop", however it is named: by
//!    instance id, by the id of the account that owns it, or by the name of the Amazon
//!    Web Services service that owns it. The three sub-lists are alternatives, and one
//!    that is absent or empty excludes nothing.
//! 3. A page holds at most `MaxResults` instances and carries a `NextToken` that resumes
//!    exactly where it stopped, so paging from the first token to the last yields every
//!    blocking instance once, in that same order.
//! 4. Both forms the modelled `OutpostIdentifier` pattern
//!    `^(arn:…:outpost/)?op-[a-f0-9]{17}$` accepts name the same Outpost, and the list is
//!    a REPORT: repeating it answers the same thing and changes nothing. An identifier
//!    that names no Outpost (including one whose Outpost was deleted) is 404
//!    `NotFoundException`, as is a `CapacityTaskId` this Outpost holds no task for —
//!    each with the identifier echo of the value AS SUPPLIED; an identifier that is no
//!    identifier at all, a `MaxResults` outside the modelled range, and a `NextToken`
//!    this service did not issue for THIS list are 400 `ValidationException`.
//! 5. An instance recorded as running on an Outpost is a resource still associated with
//!    it, so it blocks that Outpost's decommission (`EC2_INSTANCE`), which an Outpost
//!    running none does not.
//!
//! Where the recorded instances come from: an instance is launched through Amazon EC2 —
//! `RunInstances` against the Outpost — and by no call in THIS API, so each row RECORDS
//! its instances on the stored Outpost through the same store seam
//! [`crate::outpost_assets`] records an Outpost's hardware through, and then asks over
//! the wire: what the list answers about what is recorded is the whole of rules 1 to 3.
//! The instances below are the rows' own data; the emulator is never expected to produce
//! an instance of its own.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    AssetInstance, A_W_S_SERVICE_NAME_VALUES,
};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    BLOCKING_EC2_INSTANCE, DECOMMISSION_STATUS_BLOCKED, DECOMMISSION_STATUS_SKIPPED,
};

use crate::capacity_task_lifecycle::{
    pool, settle_capacity_task, start_ok, ABSENT_CAPACITY_TASK_ID,
};
use crate::site_and_outpost_lifecycle::{
    query_value, refusal, segment, settle, text, Outposts, Refusal, ABSENT_OUTPOST_ID,
    ABSENT_SITE_ID, OUTPOST_ARN_PREFIX,
};

/// The account id every request this emulator serves arrives as, which is therefore the
/// account every capacity task here was initiated by: the one `Outpost$OwnerId` and
/// `Site$AccountId` carry. Read from the engine itself, the same place the handler reads
/// it, so the suite follows that account id rather than pinning a copy of it.
pub(crate) fn own_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// An account that is NOT the one initiating the tasks below — an account the Outpost is
/// shared with. Its instances run on the Outpost and are reported by
/// `ListAssetInstances` ("belonging to all accounts"), never by this list.
pub(crate) const OTHER_ACCOUNT: &str = "210987654321";

/// A second such account, so a row can exclude one foreign account without touching the
/// other. Both satisfy the modelled `AccountId` pattern `^\d{12}$`.
pub(crate) const THIRD_ACCOUNT: &str = "123456789012";

/// The route `ListBlockingInstancesForCapacityTask` binds, for one Outpost identifier
/// form, one capacity task id and one query string.
fn route(outpost_identifier: &str, capacity_task_id: &str, query: &str) -> String {
    format!(
        "/outposts/{}/capacity/{}/blockingInstances{query}",
        segment(outpost_identifier),
        segment(capacity_task_id)
    )
}

/// One recorded instance, spelled out. Every member of the modelled `AssetInstance` but
/// the id is optional, so a row can record an instance whose type, asset, owning account
/// or owning service was never recorded.
pub(crate) fn instance(
    id: &str,
    instance_type: Option<&str>,
    asset_id: Option<&str>,
    account_id: Option<&str>,
    aws_service_name: Option<&str>,
) -> AssetInstance {
    AssetInstance {
        instance_id: Some(id.to_string()),
        instance_type: instance_type.map(str::to_string),
        asset_id: asset_id.map(str::to_string),
        account_id: account_id.map(str::to_string),
        aws_service_name: aws_service_name.map(str::to_string),
    }
}

/// An `InstanceId` of the modelled form `^i-[0-9a-z]+$` (`length(11..32)`), minted from
/// a row's own counter so every recorded instance in a set is a distinct one.
pub(crate) fn instance_id(number: u32) -> String {
    format!("i-{number:017x}")
}

/// The instances every exclusion and paging row below is recorded against: twelve
/// instances in the order they were recorded.
///
/// Between them they span what the rules generalise over. Every value of the modelled
/// `AWSServiceName` enum owns one of the initiating account's instances (the catalog is
/// `ops::types::A_W_S_SERVICE_NAME_VALUES`, read off the model's own enum), so the
/// exclusion rows cover each value of it and not only the ones a report cited; two
/// instances carry no owning service at all, which is what a customer's own instance is;
/// one recorded nothing but its id and its account; two belong to other accounts (one of
/// them owned by a service); and one recorded no owning account at all.
pub(crate) fn running() -> Vec<AssetInstance> {
    let mut instances = vec![
        // The initiating account's own instance, every member recorded, owned by no
        // Amazon Web Services service.
        instance(
            &instance_id(1),
            Some("m5.4xlarge"),
            Some("1000000001"),
            Some(own_account()),
            None,
        ),
        // One that recorded nothing but its id and the account that owns it.
        AssetInstance {
            instance_id: Some(instance_id(2)),
            account_id: Some(own_account().to_string()),
            ..AssetInstance::default()
        },
    ];
    // One instance of the initiating account per value of the modelled `AWSServiceName`
    // enum, so every value of that catalog owns a blocking instance.
    for (step, service) in A_W_S_SERVICE_NAME_VALUES.iter().enumerate() {
        instances.push(instance(
            &instance_id(3 + step as u32),
            Some("m5.xlarge"),
            Some("1000000002"),
            Some(own_account()),
            Some(service),
        ));
    }
    instances.extend([
        // A second instance of the initiating account with no owning service, after the
        // service-owned ones, so a row that excludes by service cannot pass by dropping
        // a suffix of the list.
        instance(
            &instance_id(20),
            Some("c5.24xlarge"),
            Some("1000000003"),
            Some(own_account()),
            None,
        ),
        // Another account's instances: on the Outpost, but not the initiating account's
        // to stop.
        instance(
            &instance_id(21),
            Some("m5.large"),
            Some("1000000003"),
            Some(OTHER_ACCOUNT),
            None,
        ),
        instance(
            &instance_id(22),
            Some("m5.large"),
            Some("1000000003"),
            Some(THIRD_ACCOUNT),
            Some("EC2"),
        ),
        // One whose owning account was never recorded, so it belongs to no account this
        // list can claim it for.
        instance(
            &instance_id(23),
            Some("r5.large"),
            Some("1000000003"),
            None,
            Some("RDS"),
        ),
    ]);
    instances
}

/// The `InstancesToExclude` a row's capacity task was started with: "user-specified
/// instances that must not be stopped", by instance id, by owning account or by owning
/// service. A sub-list left `None` is one the request never carried.
#[derive(Debug, Default, Clone)]
struct Exclude {
    instances: Option<Vec<String>>,
    account_ids: Option<Vec<String>>,
    services: Option<Vec<String>>,
}

impl Exclude {
    /// The `InstancesToExclude` member a `StartCapacityTask` request carries for this
    /// row, or `None` when the row excludes nothing at all.
    fn member(&self) -> Option<Value> {
        let mut member = serde_json::Map::new();
        for (name, list) in [
            ("Instances", &self.instances),
            ("AccountIds", &self.account_ids),
            ("Services", &self.services),
        ] {
            if let Some(list) = list {
                member.insert(name.to_string(), json!(list));
            }
        }
        match member.is_empty() {
            true => None,
            false => Some(Value::Object(member)),
        }
    }

    /// Whether this row's exclusions name `instance` through any of the three sub-lists —
    /// rule 2, stated once and applied to every row.
    fn excludes(&self, instance: &AssetInstance) -> bool {
        let names = |list: &Option<Vec<String>>, value: Option<&String>| match (list, value) {
            (Some(list), Some(value)) => list.contains(value),
            _ => false,
        };
        names(&self.instances, instance.instance_id.as_ref())
            || names(&self.account_ids, instance.account_id.as_ref())
            || names(&self.services, instance.aws_service_name.as_ref())
    }
}

/// An exclusion by instance id.
fn by_instances(ids: &[String]) -> Exclude {
    Exclude {
        instances: Some(ids.to_vec()),
        ..Exclude::default()
    }
}

/// An exclusion by the account that owns the instance.
fn by_accounts(accounts: &[&str]) -> Exclude {
    Exclude {
        account_ids: Some(accounts.iter().map(|id| (*id).to_string()).collect()),
        ..Exclude::default()
    }
}

/// An exclusion by the Amazon Web Services service that owns the instance.
fn by_services(services: &[&str]) -> Exclude {
    Exclude {
        services: Some(services.iter().map(|name| (*name).to_string()).collect()),
        ..Exclude::default()
    }
}

/// The instances rule 1 and rule 2 together select out of `recorded` for a task started
/// with `exclude`: the initiating account's own instances, less every one the task
/// excludes, in the order they were recorded. The expectation of every row below is
/// COMPUTED from the rule rather than spelled out.
fn blocking(recorded: &[AssetInstance], exclude: &Exclude) -> Vec<AssetInstance> {
    recorded
        .iter()
        .filter(|instance| {
            instance.account_id.as_deref() == Some(own_account()) && !exclude.excludes(instance)
        })
        .cloned()
        .collect()
}

/// The `InstanceId`s of those instances, in that same order.
fn blocking_ids(recorded: &[AssetInstance], exclude: &Exclude) -> Vec<String> {
    blocking(recorded, exclude)
        .iter()
        .map(|instance| instance.instance_id.clone().unwrap_or_default())
        .collect()
}

/// An Outpost to record instances on, as its `OutpostId`. An Outpost belongs to a site,
/// so the site comes first — the same order a real workflow takes.
async fn an_outpost(outposts: &Outposts, name: &str) -> String {
    let site = outposts
        .create_site(&json!({ "Name": format!("{name}-site") }))
        .await;
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": text(&site, "SiteId") }))
        .await;
    text(&outpost, "OutpostId")
}

/// Record `instances` as the Amazon EC2 instances running on the stored Outpost. See
/// this module's header for why a row records them through the store.
pub(crate) fn record(outposts: &Outposts, outpost_id: &str, instances: &[AssetInstance], case: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .outposts
        .get_mut(outpost_id)
        .unwrap_or_else(|| panic!("case {case:?}: the create persisted the Outpost"));
    record.instances = instances.to_vec();
}

/// `StartCapacityTask` on the Outpost with `exclude` as its `InstancesToExclude`,
/// returning the minted `CapacityTaskId`.
async fn a_task(outposts: &Outposts, outpost_id: &str, exclude: &Exclude) -> String {
    let mut request = json!({ "InstancePools": [pool("m5.4xlarge", 1)] });
    if let Some(member) = exclude.member() {
        request["InstancesToExclude"] = member;
    }
    text(
        &start_ok(outposts, outpost_id, &request).await,
        "CapacityTaskId",
    )
}

/// The `BlockingInstances` a page reports, as a list.
fn reported(output: &Value, case: &str) -> Vec<Value> {
    output
        .get("BlockingInstances")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a BlockingInstances list, got {output}"))
}

/// The `InstanceId`s a page reports, in the order it reported them.
fn reported_ids(output: &Value, case: &str) -> Vec<String> {
    reported(output, case)
        .iter()
        .map(|item| text(item, "InstanceId"))
        .collect()
}

/// Rule 1's projection half: one reported instance against the one a row recorded.
fn assert_reported(actual: &Value, expected: &AssetInstance, case: &str) {
    for (member, value) in [
        ("InstanceId", expected.instance_id.as_deref()),
        ("AccountId", expected.account_id.as_deref()),
        ("AwsServiceName", expected.aws_service_name.as_deref()),
    ] {
        assert_eq!(
            actual.get(member).and_then(Value::as_str),
            value,
            "case {case:?}: expected {member} {value:?}, got {actual}"
        );
    }
    // `BlockingInstance` is the narrower projection: it carries neither the instance
    // type nor the asset the instance runs on, both of which the record holds.
    for member in ["InstanceType", "AssetId"] {
        assert!(
            actual.get(member).is_none(),
            "case {case:?}: a BlockingInstance carries no {member}, got {actual}"
        );
    }
}

/// Rules 1 + 4's identifier half: the list answers the initiating account's recorded
/// instances, field for field and in recorded order, through either identifier form.
///
/// The rows differ along everything the rule generalises over: how many instances are
/// recorded (none, one, two, twelve), which members each recorded (all of them, only an
/// id and an account, no owning service, no owning account at all), which account owns
/// each, and which owning service.
#[tokio::test]
async fn the_list_reports_the_recorded_instances_the_task_would_need_stopped() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Vec<AssetInstance>)> = vec![
        ("no instance running on the Outpost", Vec::new()),
        (
            "one instance of the initiating account, every member recorded",
            vec![instance(
                &instance_id(101),
                Some("m5.4xlarge"),
                Some("1000000001"),
                Some(own_account()),
                Some("EC2"),
            )],
        ),
        (
            "one instance that recorded nothing but its id and its account",
            vec![AssetInstance {
                instance_id: Some(instance_id(102)),
                account_id: Some(own_account().to_string()),
                ..AssetInstance::default()
            }],
        ),
        (
            "only another account's instances, so none of them is this task's to stop",
            vec![
                instance(
                    &instance_id(103),
                    Some("m5.large"),
                    Some("1000000002"),
                    Some(OTHER_ACCOUNT),
                    None,
                ),
                instance(
                    &instance_id(104),
                    Some("m5.large"),
                    Some("1000000002"),
                    Some(THIRD_ACCOUNT),
                    Some("ELB"),
                ),
            ],
        ),
        (
            "one instance whose owning account was never recorded",
            vec![instance(
                &instance_id(105),
                Some("m5.large"),
                None,
                None,
                Some("RDS"),
            )],
        ),
        (
            "two of the initiating account's instances among two other accounts'",
            vec![
                instance(
                    &instance_id(106),
                    Some("c5.xlarge"),
                    Some("1000000001"),
                    Some(OTHER_ACCOUNT),
                    None,
                ),
                instance(
                    &instance_id(107),
                    Some("c5.xlarge"),
                    Some("1000000001"),
                    Some(own_account()),
                    Some("EKS"),
                ),
                instance(
                    &instance_id(108),
                    Some("c5.xlarge"),
                    Some("1000000001"),
                    Some(THIRD_ACCOUNT),
                    Some("AWS"),
                ),
                instance(
                    &instance_id(109),
                    Some("c5.xlarge"),
                    None,
                    Some(own_account()),
                    None,
                ),
            ],
        ),
        (
            "every value of the modelled AWSServiceName enum owns one, among the rest",
            running(),
        ),
    ];

    for (case, recorded) in &cases {
        let outpost_id = an_outpost(&outposts, "blocking").await;
        record(&outposts, &outpost_id, recorded, case);
        let task_id = a_task(&outposts, &outpost_id, &Exclude::default()).await;
        let expected = blocking(recorded, &Exclude::default());

        // Rule 4's identifier half: the bare id and the full Outpost ARN name the same
        // Outpost, so both answer the same list.
        for identifier in [
            outpost_id.clone(),
            format!("{OUTPOST_ARN_PREFIX}{outpost_id}"),
        ] {
            let case = &format!("{case} read by {identifier}");
            let reply = outposts
                .ok("GET", &route(&identifier, &task_id, ""), None)
                .await;
            let items = reported(&reply, case);
            assert_eq!(
                items.len(),
                expected.len(),
                "case {case:?}: expected {} blocking instance(s), got {reply}",
                expected.len()
            );
            for (index, (actual, expected)) in items.iter().zip(&expected).enumerate() {
                assert_reported(actual, expected, &format!("{case} (instance {index})"));
            }
            // A page that exhausts the list issues no token to resume from.
            assert!(
                reply.get("NextToken").is_none(),
                "case {case:?}: a page holding every blocking instance issues no NextToken, got \
                 {reply}"
            );
        }
    }
}

/// Rule 2: every exclusion row, with the expectation computed from the recorded
/// instances rather than spelled out — so each value of the modelled `AWSServiceName`
/// catalog, and each recorded instance id, is covered.
///
/// One Outpost per row, because only ONE capacity task can be active on an Outpost at a
/// time: each row records the same instances and starts its own task with its own
/// exclusions.
#[tokio::test]
async fn an_excluded_instance_is_absent_from_the_list_however_it_was_named() {
    let outposts = Outposts::new();
    let recorded = running();
    let own_ids: Vec<String> = blocking_ids(&recorded, &Exclude::default());

    let mut cases: Vec<(String, Exclude)> = vec![
        ("no InstancesToExclude at all".to_string(), Exclude::default()),
        (
            "every sub-list present but empty, so nothing is excluded".to_string(),
            Exclude {
                instances: Some(Vec::new()),
                account_ids: Some(Vec::new()),
                services: Some(Vec::new()),
            },
        ),
        (
            "one instance id".to_string(),
            by_instances(&own_ids[..1]),
        ),
        (
            "three instance ids at once".to_string(),
            by_instances(&own_ids[1..4]),
        ),
        (
            "every one of the account's instances by id".to_string(),
            by_instances(&own_ids),
        ),
        (
            "an instance id no recorded instance carries".to_string(),
            by_instances(&[instance_id(999)]),
        ),
        (
            "the initiating account itself, so every one of its instances".to_string(),
            by_accounts(&[own_account()]),
        ),
        (
            "another account, whose instances this list never reported anyway".to_string(),
            by_accounts(&[OTHER_ACCOUNT, THIRD_ACCOUNT]),
        ),
        (
            "two owning services at once".to_string(),
            by_services(&["EC2", "RDS"]),
        ),
        (
            "every owning service the model declares".to_string(),
            by_services(A_W_S_SERVICE_NAME_VALUES),
        ),
        (
            "an instance id and an owning service together, so the union of both".to_string(),
            Exclude {
                instances: Some(own_ids[..2].to_vec()),
                services: Some(vec!["ELB".to_string()]),
                ..Exclude::default()
            },
        ),
        (
            "all three sub-lists at once".to_string(),
            Exclude {
                instances: Some(vec![own_ids[0].clone()]),
                account_ids: Some(vec![THIRD_ACCOUNT.to_string()]),
                services: Some(vec!["EKS".to_string(), "ROUTE53".to_string()]),
            },
        ),
    ];
    // One row per value of the catalog the model declares, so no value goes untested: an
    // instance is excluded exactly when its owning service is one of the named ones.
    for service in A_W_S_SERVICE_NAME_VALUES {
        cases.push((
            format!("the one owning service {service}"),
            by_services(&[service]),
        ));
    }
    // One row per recorded instance of the account, so no recorded id goes untested.
    for id in &own_ids {
        cases.push((
            format!("the one instance id {id}"),
            by_instances(std::slice::from_ref(id)),
        ));
    }

    for (case, exclude) in &cases {
        let outpost_id = an_outpost(&outposts, "excluded").await;
        record(&outposts, &outpost_id, &recorded, case);
        let task_id = a_task(&outposts, &outpost_id, exclude).await;

        let reply = outposts
            .ok("GET", &route(&outpost_id, &task_id, ""), None)
            .await;
        let wanted = blocking_ids(&recorded, exclude);
        assert_eq!(
            &reported_ids(&reply, case),
            &wanted,
            "case {case:?}: the list must omit exactly the excluded instances, got {reply}"
        );
    }
}

/// Rule 3: for EVERY page size, paging the list from the first `NextToken` to the last
/// yields exactly the blocking instances, once each, in recorded order — with and
/// without exclusions.
///
/// The page sizes span one instance per page, a size that leaves a short last page, an
/// exact divisor of the count, the count itself, more than the count, and the modelled
/// `MaxResults1000` maximum, so a token that duplicated or skipped an instance at any
/// boundary would show up.
#[tokio::test]
async fn paging_covers_every_blocking_instance_exactly_once() {
    let outposts = Outposts::new();
    let recorded = running();

    let excludes: Vec<(&str, Exclude)> = vec![
        ("no exclusion", Exclude::default()),
        ("two services excluded", by_services(&["EC2", "EKS"])),
    ];

    for (exclusion_case, exclude) in &excludes {
        let outpost_id = an_outpost(&outposts, "paging").await;
        record(&outposts, &outpost_id, &recorded, exclusion_case);
        let task_id = a_task(&outposts, &outpost_id, exclude).await;
        let expected = blocking_ids(&recorded, exclude);

        for size in [1usize, 2, 3, 5, 9, 11, 1000] {
            let case = &format!("{exclusion_case}, page size {size}");
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let query = match &token {
                    Some(token) => {
                        format!("?MaxResults={size}&NextToken={}", query_value(token))
                    }
                    None => format!("?MaxResults={size}"),
                };
                let page = outposts
                    .ok("GET", &route(&outpost_id, &task_id, &query), None)
                    .await;
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
                "case {case:?}: paging must yield every blocking instance once, in recorded order"
            );
        }
    }
}

/// Rule 4's report half: listing a task's blocking instances changes nothing — not the
/// list it answers, not the Outpost, not the recorded instances, not another Outpost's
/// list.
#[tokio::test]
async fn listing_blocking_instances_changes_nothing_a_later_call_sees() {
    let outposts = Outposts::new();
    let recorded = running();
    let busy = an_outpost(&outposts, "busy").await;
    let idle = an_outpost(&outposts, "idle").await;
    record(&outposts, &busy, &recorded, "busy");
    let busy_task = a_task(&outposts, &busy, &Exclude::default()).await;
    let idle_task = a_task(&outposts, &idle, &Exclude::default()).await;

    // Past the Outpost lifecycle transition the clock owes a just-created Outpost, so the
    // Outpost snapshots below differ only if THIS read changed one.
    settle();
    let before = outposts.get_outpost(&busy).await;

    let mut answers = Vec::new();
    for pass in ["first read", "second read", "third read"] {
        answers.push((
            pass,
            outposts.ok("GET", &route(&busy, &busy_task, ""), None).await,
        ));
        // Another Outpost's instances are its own: listing one never fills in another.
        let other = outposts
            .ok("GET", &route(&idle, &idle_task, ""), None)
            .await;
        assert_eq!(
            other.get("BlockingInstances"),
            Some(&json!([])),
            "case {pass:?}: an Outpost running no instance reports no blocking instance, got \
             {other}"
        );
    }
    let (_, first) = &answers[0];
    for (pass, answer) in &answers {
        assert_eq!(
            answer, first,
            "case {pass:?}: every listing of one task's blocking instances must answer the same"
        );
    }

    let after = outposts.get_outpost(&busy).await;
    assert_eq!(
        after, before,
        "listing blocking instances must not change the Outpost itself"
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
        "listing blocking instances must leave the recorded instances alone: expected \
         {expected_held:?}, got {held:?}"
    );
}

/// Rule 4's refusal half: an Outpost or a capacity task that names nothing is 404
/// `NotFoundException`; an identifier that is no identifier at all, a `MaxResults`
/// outside the modelled range, and a `NextToken` this service did not issue for this
/// list are 400 `ValidationException`.
#[tokio::test]
async fn an_absent_outpost_or_task_is_not_found_and_bad_input_is_refused() {
    let outposts = Outposts::new();
    let live = an_outpost(&outposts, "live").await;
    record(&outposts, &live, &running(), "live");
    let live_task = a_task(&outposts, &live, &Exclude::default()).await;

    // A task of ANOTHER Outpost: a real task, reachable under its own Outpost's route
    // only.
    let elsewhere = an_outpost(&outposts, "elsewhere").await;
    let elsewhere_task = a_task(&outposts, &elsewhere, &Exclude::default()).await;

    // An Outpost that existed, had instances recorded on it and a task started, and was
    // then deleted: both went with it, so the route answers as it does for one that
    // never was. The task is let FINISH first — an Outpost whose capacity task is still
    // outstanding cannot be deleted at all (`crate::delete_conflicts`).
    let deleted = an_outpost(&outposts, "deleted").await;
    record(&outposts, &deleted, &running(), "deleted");
    let deleted_task = a_task(&outposts, &deleted, &Exclude::default()).await;
    settle_capacity_task();
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
            route(ABSENT_OUTPOST_ID, &live_task, ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "the ARN form of an Outpost that names nothing",
            "GET",
            route(
                &format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}"),
                &live_task,
                "",
            ),
            404,
            "NotFoundException",
        ),
        refusal(
            "an Outpost that was deleted",
            "GET",
            route(&deleted, &deleted_task, ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "a well-formed capacity task id that names nothing",
            "GET",
            route(&live, ABSENT_CAPACITY_TASK_ID, ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "a capacity task of another Outpost",
            "GET",
            route(&live, &elsewhere_task, ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "a label that is no Outpost identifier at all",
            "GET",
            route("not-an-outpost-id", &live_task, ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a site id in place of the Outpost identifier",
            "GET",
            route(ABSENT_SITE_ID, &live_task, ""),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost id whose suffix is not hex",
            "GET",
            route("op-zzzzzzzzzzzzzzzzz", &live_task, ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a capacity task id whose suffix is not hex",
            "GET",
            route(&live, "cap-zzzzzzzzzzzzzzzzz", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a capacity task id one character short of the modelled length",
            "GET",
            route(&live, "cap-0000000000000000", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost id in place of the capacity task id",
            "GET",
            route(&live, ABSENT_OUTPOST_ID, ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a ListOutposts token replayed on this list",
            "GET",
            route(
                &live,
                &live_task,
                &format!("?NextToken={}", query_value(&other_token)),
            ),
            400,
            "ValidationException",
        ),
        refusal(
            "a ListAssets token replayed on this list",
            "GET",
            route(
                &live,
                &live_task,
                &format!("?NextToken={}", query_value("1##assets")),
            ),
            400,
            "ValidationException",
        ),
        refusal(
            "a token this service never issued at all",
            "GET",
            route(&live, &live_task, "?NextToken=1%23%23nonsense"),
            400,
            "ValidationException",
        ),
        refusal(
            "MaxResults below the modelled minimum of 1",
            "GET",
            route(&live, &live_task, "?MaxResults=0"),
            400,
            "ValidationException",
        ),
        refusal(
            "MaxResults above the modelled maximum of 1000",
            "GET",
            route(&live, &live_task, "?MaxResults=1001"),
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
    let live = an_outpost(&outposts, "echo").await;
    let live_task = a_task(&outposts, &live, &Exclude::default()).await;
    let elsewhere = an_outpost(&outposts, "echo-elsewhere").await;
    let elsewhere_task = a_task(&outposts, &elsewhere, &Exclude::default()).await;
    let absent_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");

    let cases: Vec<(&str, String, String)> = vec![
        (
            "an absent Outpost named by bare id",
            route(ABSENT_OUTPOST_ID, &live_task, ""),
            format!("outpostIdentifier={ABSENT_OUTPOST_ID}"),
        ),
        (
            "an absent Outpost named by ARN",
            route(&absent_arn, &live_task, ""),
            format!("outpostIdentifier={absent_arn}"),
        ),
        (
            "an absent capacity task",
            route(&live, ABSENT_CAPACITY_TASK_ID, ""),
            format!("capacityTaskId={ABSENT_CAPACITY_TASK_ID}"),
        ),
        (
            "a capacity task of another Outpost",
            route(&live, &elsewhere_task, ""),
            format!("capacityTaskId={elsewhere_task}"),
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

/// Rule 5: an instance recorded as running on an Outpost is a resource still associated
/// with it, so `StartOutpostDecommission` reports it as `EC2_INSTANCE` blocking — and an
/// Outpost running none is not blocked by one.
///
/// The rows differ along what the rule generalises over: no instance, one instance of
/// the initiating account, and only another account's instances — an instance blocks the
/// decommission whoever owns it, because it is the Outpost it runs on that is being
/// decommissioned.
#[tokio::test]
async fn an_instance_running_on_an_outpost_blocks_its_decommission() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Vec<AssetInstance>, bool)> = vec![
        ("no instance running", Vec::new(), false),
        (
            "one instance of the initiating account",
            vec![instance(
                &instance_id(201),
                Some("m5.4xlarge"),
                Some("1000000001"),
                Some(own_account()),
                None,
            )],
            true,
        ),
        (
            "only another account's instance",
            vec![instance(
                &instance_id(202),
                Some("m5.large"),
                Some("1000000001"),
                Some(OTHER_ACCOUNT),
                Some("EC2"),
            )],
            true,
        ),
        ("every recorded instance", running(), true),
    ];

    for (case, recorded, blocked) in &cases {
        let outpost_id = an_outpost(&outposts, "decommission").await;
        record(&outposts, &outpost_id, recorded, case);

        let answer = outposts
            .ok(
                "POST",
                &format!("/outposts/{}/decommission", segment(&outpost_id)),
                Some(&json!({ "ValidateOnly": true })),
            )
            .await;
        let (expected_status, expected_blocking) = match blocked {
            true => (
                DECOMMISSION_STATUS_BLOCKED,
                Some(json!([BLOCKING_EC2_INSTANCE])),
            ),
            // `ValidateOnly` asks for the verdict alone, so a call nothing blocks
            // starts nothing and answers `SKIPPED`.
            false => (DECOMMISSION_STATUS_SKIPPED, None),
        };
        assert_eq!(
            text(&answer, "Status"),
            expected_status,
            "case {case:?}: expected Status {expected_status:?}, got {answer}"
        );
        assert_eq!(
            answer.get("BlockingResourceTypes"),
            expected_blocking.as_ref(),
            "case {case:?}: expected BlockingResourceTypes {expected_blocking:?}, got {answer}"
        );
    }
}
