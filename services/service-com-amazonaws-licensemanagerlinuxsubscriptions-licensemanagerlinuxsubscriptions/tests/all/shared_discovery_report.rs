// SPDX-License-Identifier: CC0-1.0

//! The two rules BOTH discovery lists report under, driven through each of them: the
//! pass-materialization gate and the way several `Filters` combine.
//!
//! `ListLinuxSubscriptions` and `ListLinuxSubscriptionInstances` report from the ONE store
//! of discovered instances — the first aggregates the records the second projects — so
//! neither of these two decisions can belong to one list:
//!
//! * EVERY report materializes the discovery pass the account's settings ask for, the
//!   request CONTINUING a walk included: the pass is owed by the clock rather than by the
//!   shape of the request, so no cursor buys a report the inventory of an earlier instant;
//! * several filters select what satisfies ALL of them, which makes a multi-filter selection
//!   exactly the intersection of what each of those filters selects on its own — and a
//!   request carrying no filter at all selects everything.
//!
//! Each rule is asserted through BOTH lists over a table of inputs, so neither list can
//! answer it differently from the other.
//!
//! The gate rows observe the pass over the wire rather than by inspecting the store: what a
//! pass holds is the inventory of the Regions the account's settings discover NOW, so a
//! record held for a Region those settings do not name is one the next pass drops. Such a
//! record is placed between the first page of a walk and its continuation, and whether the
//! list still reports it is exactly whether a pass ran.

use serde_json::{json, Value};

use crate::harness::{
    call, enable_discovery, harness, seed_discovered_instance, Harness, Reply, REGION,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::discovery::{
    SUBSCRIPTION_TYPE_BYOS, SUBSCRIPTION_TYPE_INCLUDED, SUBSCRIPTION_TYPE_MARKETPLACE,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::DiscoveredInstanceRecord;

/// `POST /subscription/ListLinuxSubscriptions` and its sibling list (`OPERATIONS.md`).
const SUBSCRIPTIONS: &str = "/subscription/ListLinuxSubscriptions";
const INSTANCES: &str = "/subscription/ListLinuxSubscriptionInstances";

/// A second Region, so a record can be held for a Region the account's settings do not
/// name. `REGION` is the one the calls are signed for.
const OTHER_REGION: &str = "us-east-1";

/// One of the two lists, with everything that differs between them: where it is reached,
/// which output member carries its page, and how one of its elements is identified.
///
/// `pub(crate)` because a walk of either list is a rule of its own
/// (`tests/all/walk_under_a_discovery_pass.rs`), and the two lists are described once here
/// rather than copied into a second suite.
pub(crate) struct List {
    /// The operation, named in every assertion this list drives.
    pub(crate) label: &'static str,
    pub(crate) path: &'static str,
    /// The output member carrying the page's entries.
    pub(crate) key: &'static str,
    /// How an entry of a reported page is identified.
    pub(crate) reported: fn(&Value, &str) -> String,
    /// The same identity for a record held in the store: an instance list reports one
    /// element per record, a subscription list one per distinct (name, type) pair.
    pub(crate) stored: fn(&DiscoveredInstanceRecord) -> String,
    /// The multi-filter rows this list's conjunction is asserted over; a list's filters can
    /// only name the `Filter$Name` values it documents.
    pub(crate) conjunctions: fn() -> Vec<Conjunction>,
}

pub(crate) const LISTS: &[List] = &[
    List {
        label: "ListLinuxSubscriptions",
        path: SUBSCRIPTIONS,
        key: "Subscriptions",
        reported: reported_subscription,
        stored: stored_subscription,
        conjunctions: subscription_conjunctions,
    },
    List {
        label: "ListLinuxSubscriptionInstances",
        path: INSTANCES,
        key: "Instances",
        reported: reported_instance,
        stored: stored_instance,
        conjunctions: instance_conjunctions,
    },
];

/// A reported `Subscription` is the (name, type) pair it carries: the same name can be
/// licensed in more than one way, so neither member identifies one on its own.
fn reported_subscription(entry: &Value, case: &str) -> String {
    format!(
        "{}|{}",
        member(entry, "Name", case),
        member(entry, "Type", case)
    )
}

fn stored_subscription(record: &DiscoveredInstanceRecord) -> String {
    format!("{}|{}", record.subscription_name, record.subscription_type)
}

/// A reported `Instance` is its `InstanceID`, which is also the key the store holds it
/// under.
fn reported_instance(entry: &Value, case: &str) -> String {
    member(entry, "InstanceID", case)
}

fn stored_instance(record: &DiscoveredInstanceRecord) -> String {
    record.instance_id.clone()
}

/// A string member of a reported entry.
fn member(entry: &Value, name: &str, case: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {entry:?}"))
        .to_string()
}

/// One page of `list`: the identities it reported, and the cursor that reaches the next
/// page.
pub(crate) async fn page(
    h: &Harness,
    list: &List,
    body: &Value,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let reply = call(h, list.path, body).await;
    let entries = reply
        .ok(case)
        .get(list.key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no {} in {:?}", list.key, reply.body))
        .clone();
    let identities = entries
        .iter()
        .map(|entry| (list.reported)(entry, case))
        .collect();
    (identities, next_token(&reply, case))
}

/// The `NextToken` a page carries, if it carries one.
fn next_token(reply: &Reply, case: &str) -> Option<String> {
    reply
        .ok(case)
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One discovered instance a row places in the store, stating the members these two rules
/// select, page and group on. [`Seed::record`] leaves every other member of the record as
/// discovery leaves it for an instance no third-party subscription provider was involved in.
#[derive(Debug, Clone, Copy)]
struct Seed {
    instance_id: &'static str,
    account_id: &'static str,
    ami_id: &'static str,
    instance_type: &'static str,
    status: &'static str,
    region: &'static str,
    usage_operation: &'static str,
    product_codes: &'static [&'static str],
    /// `None` is a record that leaves the member UNSET, which no filter can select on.
    dual_subscription: Option<&'static str>,
    subscription_name: &'static str,
    subscription_type: &'static str,
}

impl Seed {
    fn record(&self) -> DiscoveredInstanceRecord {
        DiscoveredInstanceRecord {
            instance_id: self.instance_id.to_string(),
            account_id: self.account_id.to_string(),
            ami_id: self.ami_id.to_string(),
            instance_type: self.instance_type.to_string(),
            status: self.status.to_string(),
            region: self.region.to_string(),
            usage_operation: self.usage_operation.to_string(),
            product_codes: self
                .product_codes
                .iter()
                .map(|code| (*code).to_string())
                .collect(),
            dual_subscription: self.dual_subscription.map(str::to_string),
            subscription_name: self.subscription_name.to_string(),
            subscription_type: self.subscription_type.to_string(),
            ..DiscoveredInstanceRecord::default()
        }
    }
}

/// Place `seeds` in the store, with the account's settings that make them discovered
/// data: discovery enabled for exactly the Regions they were found in, because what both
/// lists report is the inventory of the Regions the settings discover NOW.
///
/// Discovery reads the account's EC2 inventory and no operation of this service writes a
/// discovered instance, so a suite whose rule is about what the two lists do with those
/// records places them through the harness's seeding seam — the same `engine.state::<…>()`
/// the handlers read.
async fn discover(h: &Harness, seeds: &[Seed]) {
    let mut regions: Vec<&str> = seeds.iter().map(|seed| seed.region).collect();
    regions.sort_unstable();
    regions.dedup();
    enable_discovery(h, &regions).await;
    for seed in seeds {
        seed_discovered_instance(h, seed.record());
    }
}

/// The records a gate row starts from: four instances in the ONE Region the account
/// discovers, each using a different subscription, so both lists hold four elements over
/// which a walk can be paged.
const RETAINED: &[Seed] = &[
    Seed {
        instance_id: "i-001",
        account_id: "000000000000",
        ami_id: "ami-0001",
        instance_type: "t3.micro",
        status: "running",
        region: REGION,
        usage_operation: "RunInstances:0010",
        product_codes: &["pc-rhel"],
        dual_subscription: Some("No"),
        subscription_name: "RHEL",
        subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
    },
    Seed {
        instance_id: "i-002",
        account_id: "000000000000",
        ami_id: "ami-0002",
        instance_type: "t3.small",
        status: "running",
        region: REGION,
        usage_operation: "RunInstances:0010",
        product_codes: &["pc-rhel-sap"],
        dual_subscription: Some("No"),
        subscription_name: "RHEL for SAP",
        subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
    },
    Seed {
        instance_id: "i-003",
        account_id: "000000000000",
        ami_id: "ami-0003",
        instance_type: "m5.large",
        status: "running",
        region: REGION,
        usage_operation: "RunInstances:000g",
        product_codes: &["pc-suse"],
        dual_subscription: None,
        subscription_name: "SUSE Linux Enterprise Server",
        subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
    },
    Seed {
        instance_id: "i-004",
        account_id: "000000000000",
        ami_id: "ami-0004",
        instance_type: "c6g.medium",
        status: "running",
        region: REGION,
        usage_operation: "RunInstances:0g00",
        product_codes: &["pc-suse-sap"],
        dual_subscription: None,
        subscription_name: "SUSE Linux Enterprise Server for SAP",
        subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
    },
];

/// The record a pass DROPS: it is held for a Region the account's settings do not name, and
/// a pass holds data for the Regions it discovers and no others.
///
/// It sorts LAST in both lists — after `i-004` by instance id, and after every subscription
/// of [`RETAINED`] by name — so a walk continuing from any page of those four reaches it if
/// and only if it is still in the store.
const DROPPED: Seed = Seed {
    instance_id: "i-900",
    account_id: "000000000000",
    ami_id: "ami-0009",
    instance_type: "t3.nano",
    status: "running",
    region: OTHER_REGION,
    usage_operation: "RunInstances:0g00",
    product_codes: &["pc-ubuntu"],
    dual_subscription: None,
    subscription_name: "Ubuntu Pro",
    subscription_type: SUBSCRIPTION_TYPE_MARKETPLACE,
};

/// The identities `seeds` are reported under in `list`, each distinct element once, in a
/// total order — so a page can be compared against them whatever order the list reports in
/// (which this suite's rules are not about: `tests/all/linux_subscriptions.rs` pins it).
fn identities(list: &List, seeds: &[Seed]) -> Vec<String> {
    let mut all: Vec<String> = seeds
        .iter()
        .map(|seed| (list.stored)(&seed.record()))
        .collect();
    all.sort();
    all.dedup();
    all
}

#[tokio::test]
async fn both_discovery_lists_materialize_a_pass_on_every_report() {
    for list in LISTS {
        // The rows differ along the page size, so the walk whose continuation requests are
        // observed here is one, two and three pages long.
        for page_size in [1_usize, 2, 3] {
            let case = &format!("{}: a walk in pages of {page_size}", list.label);
            let h = harness();
            discover(&h, RETAINED).await;

            let (mut walked, mut cursor) =
                page(&h, list, &json!({ "MaxResults": page_size }), case).await;
            assert!(
                cursor.is_some(),
                "case {case:?}: a page of {page_size} of four elements must be truncated and \
                 carry a NextToken, got {walked:?}"
            );

            // Placed between the first page of the walk and its continuation: every pass
            // drops it, and every report runs a pass — so no page of the walk reports it.
            seed_discovered_instance(&h, DROPPED.record());
            let dropped_identity = (list.stored)(&DROPPED.record());

            while let Some(token) = cursor {
                let (reported, next) = page(
                    &h,
                    list,
                    &json!({ "MaxResults": page_size, "NextToken": token }),
                    case,
                )
                .await;
                assert!(
                    !reported.contains(&dropped_identity),
                    "case {case:?}: a request continuing a walk must run a pass too, which \
                     drops {dropped_identity:?}, got {reported:?}"
                );
                walked.extend(reported);
                cursor = next;
            }
            // And the walk still reported every record the store held throughout it, once
            // each: what keeps it whole under a pass is its cursor, not a frozen store.
            let expected = identities(list, RETAINED);
            let mut covered = walked.clone();
            covered.sort();
            assert_eq!(
                covered, expected,
                "case {case:?}: the walk must report {expected:?} once each, got {walked:?}"
            );

            // The same of a request that BEGINS a walk: the record is placed again, and the
            // pass that request runs holds the inventory of the Regions the settings
            // discover now — so the same element is gone.
            seed_discovered_instance(&h, DROPPED.record());
            let (first, token) = page(&h, list, &json!({}), case).await;
            assert!(
                !first.contains(&dropped_identity),
                "case {case:?}: a request beginning a walk must run a pass, which drops \
                 {dropped_identity:?}, got {first:?}"
            );
            let mut reported = first.clone();
            reported.sort();
            assert_eq!(
                reported, expected,
                "case {case:?}: after the pass the list must report {expected:?}, got {first:?}"
            );
            assert_eq!(
                token, None,
                "case {case:?}: a complete page carries no NextToken, got {token:?}"
            );
        }
    }
}

/// One multi-filter selection: the filters a request carries together.
pub(crate) struct Conjunction {
    /// What this row is about, named in every assertion it drives.
    label: &'static str,
    filters: Vec<Value>,
    /// Whether this row's filters each select something the others do not, so the
    /// conjunction has to be non-empty and strictly smaller than every one of them: a row
    /// that only ever selects nothing would satisfy the intersection identity vacuously.
    narrowing: bool,
}

/// One filter, with its `Operator` left absent when `operator` is `None` (which the service
/// reads as equality).
fn filter(name: &str, operator: Option<&str>, values: &[&str]) -> Value {
    let mut filter = json!({ "Name": name, "Values": values });
    if let Some(operator) = operator {
        filter["Operator"] = json!(operator);
    }
    filter
}

/// `ListLinuxSubscriptions` documents ONE `Filter$Name` ("The valid names for this filter
/// are: Subscription"), so its rows combine filters addressing that one member — including
/// two that cannot both hold, and one row carrying no filter at all.
fn subscription_conjunctions() -> Vec<Conjunction> {
    vec![
        Conjunction {
            label: "no filter at all",
            filters: Vec::new(),
            narrowing: false,
        },
        Conjunction {
            label: "one filter",
            filters: vec![filter("Subscription", Some("Contains"), &["S"])],
            narrowing: false,
        },
        Conjunction {
            label: "a substring and an exclusion",
            filters: vec![
                filter("Subscription", Some("Contains"), &["R"]),
                filter("Subscription", Some("NotEqual"), &["RHEL"]),
            ],
            narrowing: true,
        },
        Conjunction {
            label: "a substring and the exclusion of another name",
            filters: vec![
                filter("Subscription", Some("Contains"), &["S"]),
                filter(
                    "Subscription",
                    Some("NotEqual"),
                    &["SUSE Linux Enterprise Server"],
                ),
            ],
            narrowing: true,
        },
        Conjunction {
            label: "an equality and an exclusion of the same value",
            filters: vec![
                filter("Subscription", Some("Equal"), &["RHEL"]),
                filter("Subscription", Some("NotEqual"), &["RHEL"]),
            ],
            narrowing: false,
        },
        Conjunction {
            label: "two equalities that cannot both hold",
            filters: vec![
                filter("Subscription", Some("Equal"), &["RHEL"]),
                filter("Subscription", Some("Equal"), &["Ubuntu Pro"]),
            ],
            narrowing: false,
        },
        Conjunction {
            label: "three filters, the operator of one absent",
            filters: vec![
                filter("Subscription", Some("Contains"), &["R"]),
                filter("Subscription", None, &["RHEL", "RHEL for SAP"]),
                filter("Subscription", Some("NotEqual"), &["RHEL"]),
            ],
            narrowing: true,
        },
    ]
}

/// `ListLinuxSubscriptionInstances` documents nine `Filter$Name` values, so its rows combine
/// filters addressing DIFFERENT members — a scalar one, the list member `ProductCode`, and
/// one left unset on some records — under each of the three operators.
fn instance_conjunctions() -> Vec<Conjunction> {
    vec![
        Conjunction {
            label: "no filter at all",
            filters: Vec::new(),
            narrowing: false,
        },
        Conjunction {
            label: "one filter",
            filters: vec![filter("Region", Some("Contains"), &["us-west"])],
            narrowing: false,
        },
        Conjunction {
            label: "a Region and a Status",
            filters: vec![
                filter("Region", Some("Contains"), &["us-west"]),
                filter("Status", Some("Equal"), &["running"]),
            ],
            narrowing: true,
        },
        Conjunction {
            label: "an instance type, a state and an account",
            filters: vec![
                filter("InstanceType", Some("Contains"), &["t"]),
                filter("Status", Some("Equal"), &["running"]),
                filter("AccountID", None, &["000000000000"]),
            ],
            narrowing: true,
        },
        Conjunction {
            label: "a list member beside a scalar one",
            filters: vec![
                filter("ProductCode", Some("Contains"), &["pc-"]),
                filter("Status", Some("Equal"), &["running"]),
            ],
            narrowing: true,
        },
        Conjunction {
            label: "a member some records leave unset",
            filters: vec![
                filter("DualSubscription", Some("Equal"), &["Yes"]),
                filter("Status", Some("Equal"), &["running"]),
            ],
            narrowing: true,
        },
        Conjunction {
            label: "two Regions that cannot both hold",
            filters: vec![
                filter("Region", Some("Equal"), &[REGION]),
                filter("Region", Some("Equal"), &[OTHER_REGION]),
            ],
            narrowing: false,
        },
        Conjunction {
            label: "four filters over four members",
            filters: vec![
                filter("AmiID", Some("Contains"), &["ami-"]),
                filter("InstanceID", Some("NotEqual"), &["i-0a1"]),
                filter("Status", Some("Equal"), &["running"]),
                filter("AccountID", Some("NotEqual"), &["222222222222"]),
            ],
            narrowing: true,
        },
    ]
}

/// The fleet the conjunction rows select over: several accounts, AMIs, instance types,
/// Regions, usage operations and instance states, one subscription name licensed two
/// different ways, a record carrying two product codes and one carrying none, and
/// `DualSubscription` set on some records and unset on others — so each row's filters select
/// overlapping but different parts of it.
const FLEET: &[Seed] = &[
    Seed {
        instance_id: "i-0a1",
        account_id: "000000000000",
        ami_id: "ami-0001",
        instance_type: "t3.micro",
        status: "running",
        region: REGION,
        usage_operation: "RunInstances:0010",
        product_codes: &["pc-rhel"],
        dual_subscription: Some("No"),
        subscription_name: "RHEL",
        subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
    },
    Seed {
        instance_id: "i-0b2",
        account_id: "000000000000",
        ami_id: "ami-0001",
        instance_type: "t3.small",
        status: "stopped",
        region: REGION,
        usage_operation: "RunInstances:0010",
        product_codes: &["pc-rhel"],
        dual_subscription: Some("Yes"),
        subscription_name: "RHEL",
        subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
    },
    Seed {
        instance_id: "i-0c3",
        account_id: "000000000000",
        ami_id: "ami-0002",
        instance_type: "t3.micro",
        status: "running",
        region: REGION,
        usage_operation: "RunInstances:0010",
        product_codes: &["pc-rhel-sap", "pc-extra"],
        dual_subscription: Some("Yes"),
        subscription_name: "RHEL for SAP",
        subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
    },
    Seed {
        instance_id: "i-0d4",
        account_id: "111111111111",
        ami_id: "ami-0003",
        instance_type: "m5.large",
        status: "running",
        region: OTHER_REGION,
        usage_operation: "RunInstances",
        product_codes: &[],
        dual_subscription: None,
        subscription_name: "RHEL",
        subscription_type: SUBSCRIPTION_TYPE_BYOS,
    },
    Seed {
        instance_id: "i-0e5",
        account_id: "222222222222",
        ami_id: "ami-0004",
        instance_type: "c6g.medium",
        status: "running",
        region: OTHER_REGION,
        usage_operation: "RunInstances:000g",
        product_codes: &["pc-suse"],
        dual_subscription: None,
        subscription_name: "SUSE Linux Enterprise Server",
        subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
    },
    Seed {
        instance_id: "i-0f6",
        account_id: "222222222222",
        ami_id: "ami-0005",
        instance_type: "t4g.nano",
        status: "terminated",
        region: REGION,
        usage_operation: "RunInstances:0g00",
        product_codes: &["pc-ubuntu"],
        dual_subscription: Some("No"),
        subscription_name: "Ubuntu Pro",
        subscription_type: SUBSCRIPTION_TYPE_MARKETPLACE,
    },
];

#[tokio::test]
async fn several_filters_select_what_satisfies_all_of_them_in_both_discovery_lists() {
    for list in LISTS {
        let h = harness();
        discover(&h, FLEET).await;

        // What the list reports when nothing is filtered: the set every row's filters select
        // out of, and what a row carrying no filter must select whole.
        let unfiltered = page(&h, list, &json!({}), list.label).await.0;

        for row in (list.conjunctions)() {
            let case = &format!("{}: {}", list.label, row.label);

            // The conjunction the rule states: what every one of the row's filters selects
            // on its own, intersected — computed from the list's OWN answers to the
            // single-filter requests, so nothing here restates how one filter selects.
            let mut expected = unfiltered.clone();
            let mut alone: Vec<Vec<String>> = Vec::new();
            for filter in &row.filters {
                let selected = page(
                    &h,
                    list,
                    &json!({ "Filters": [filter.clone()] }),
                    &format!("{case} (the filter {filter} on its own)"),
                )
                .await
                .0;
                expected.retain(|identity| selected.contains(identity));
                alone.push(selected);
            }

            let together = page(&h, list, &json!({ "Filters": row.filters }), case).await.0;
            assert_eq!(
                together, expected,
                "case {case:?}: the filters together must select what satisfies all of them, \
                 {expected:?}, got {together:?}"
            );

            if row.narrowing {
                assert!(
                    !together.is_empty(),
                    "case {case:?}: this row's filters must select something together, got \
                     {together:?}"
                );
                for (index, selected) in alone.iter().enumerate() {
                    assert!(
                        together.len() < selected.len(),
                        "case {case:?}: filter {index} selects {selected:?} on its own, which \
                         the conjunction {together:?} must be strictly smaller than"
                    );
                }
            }
        }
    }
}
