// SPDX-License-Identifier: CC0-1.0

//! The discovery pass that feeds the discovered-instance store the two discovery lists
//! report from.
//!
//! This service DISCOVERS: "lists the Linux subscriptions that have been discovered", from
//! "the Regions in which to discover data for Linux subscriptions"
//! (`LinuxSubscriptionsDiscoverySettings$SourceRegions`), once the account's
//! `LinuxSubscriptionsDiscovery` is enabled. Nothing in this service's API registers an
//! instance or a subscription, so the only place the data can come from is the service that
//! OWNS the instances and the AMIs they were launched from: Amazon EC2, asked through the
//! AWS SDK over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which routes inside this
//! emulator — so this crate names no other service's crate and nothing leaves the process.
//! An instance a client launched with `ec2:RunInstances` in a configured source Region is
//! therefore one both discovery lists report, exactly as in the real service.
//!
//! What the pass establishes per instance:
//!
//! * the inventory facts, as the owner reports them: the instance's id, type, state, the
//!   account that owns it, the AMI it was launched from and the Region it runs in;
//! * what that AMI BILLS, which is what decides the subscription: `Image$PlatformDetails`
//!   is "the platform details associated with the billing code of the AMI" and
//!   `Image$UsageOperation` is that billing code, so the subscription an instance uses is
//!   the one its AMI's billing platform names ([`subscription_of_image`]).
//!
//! A pass is MATERIALIZED by the operations that report discovered data rather than by a
//! background thread: discovery runs whether or not anybody calls, so a report asks the
//! owner what the inventory holds at the instant it runs and records that — which is the
//! same answer a caller would have been shown had nobody called at all. Re-running it over
//! an unchanged inventory records the same records with the same
//! `Instance$LastUpdatedTime`, so a read never changes what a later caller sees.
//!
//! WHEN a report runs a pass is this module's own rule, stated once in
//! [`materialize_for_report`]: every report reads the one store a pass fills, so a report
//! cannot decide for itself without two of them disagreeing about the inventory they
//! answer from — and every report runs one, because the pass is owed by the CLOCK and not
//! by the shape of the request that happened to arrive.

use std::collections::{BTreeSet, HashMap};
use std::time::SystemTime;

use crate::state::DiscoveredInstanceRecord;

/// The `Subscription$Type` of a subscription whose licensing comes WITH Amazon EC2.
///
/// Each of the three values below is the model's OWN wording for that way of licensing a
/// subscription — "the type can be subscription-included with Amazon EC2, Bring Your Own
/// Subscription model (BYOS), or from the Amazon Web Services Marketplace"
/// (`Subscription$Type`, which is an unconstrained string, so its documentation is the only
/// thing that states its values) — quoted rather than reworded, so a client reads the words
/// the member is documented with.
pub const SUBSCRIPTION_TYPE_INCLUDED: &str = "subscription-included with Amazon EC2";

/// The `Subscription$Type` of a subscription the account brings itself.
pub const SUBSCRIPTION_TYPE_BYOS: &str = "Bring Your Own Subscription model (BYOS)";

/// The `Subscription$Type` of a subscription licensed through the Marketplace.
pub const SUBSCRIPTION_TYPE_MARKETPLACE: &str = "Amazon Web Services Marketplace";

/// The billing platform of an AMI that bills NO operating-system subscription: the plain
/// Linux billing platform, whose usage operation carries no billing code
/// (`Image$PlatformDetails` / `Image$UsageOperation`, "Understand AMI billing
/// information"). An instance launched from such an AMI runs no Linux subscription, so it
/// is not something a list of Linux subscriptions reports.
const PLATFORM_DETAILS_WITHOUT_SUBSCRIPTION: &str = "Linux/UNIX";

/// The billing platforms of a WINDOWS AMI all begin with this word (`Windows`, `Windows
/// with SQL Server …`), and such an AMI also declares `Image$Platform` — "this value is set
/// to `windows` for Windows AMIs; otherwise, it is blank". Either mark makes the instance
/// one this service does not report: these operations list LINUX subscriptions.
const PLATFORM_DETAILS_WINDOWS_PREFIX: &str = "Windows";

/// The word a BYOL billing platform carries (`Red Hat BYOL Linux`, `Windows BYOL` —
/// "Understand AMI billing information"): the account brought the subscription itself,
/// which is the Bring Your Own Subscription model.
const PLATFORM_DETAILS_BYOL_WORD: &str = "BYOL";

/// The prefix of a COMPOSITE billing platform whose OPERATING SYSTEM is the plain Linux
/// one, so only the software beside it is licensed: EC2 publishes `Linux with SQL Server
/// Standard`, `Linux with SQL Server Enterprise` and `Linux with SQL Server Web`
/// (`smithy_query(query="shape",shape="CapacityReservationInstancePlatform")` on EC2's
/// model, beside the `RHEL with …` platforms whose OS part DOES name a subscription). An
/// instance of such a platform runs no Linux subscription, exactly as one billing
/// [`PLATFORM_DETAILS_WITHOUT_SUBSCRIPTION`] does.
const PLATFORM_DETAILS_LINUX_WITH_PREFIX: &str = "Linux with ";

/// `ProductCode$ProductCodeType` of a product code bought through the Marketplace
/// (`ProductCodeValues`: devpay | marketplace, EC2's model): an AMI carrying one is
/// licensed through it.
const PRODUCT_CODE_TYPE_MARKETPLACE: &str = "marketplace";

/// The `InstanceState$Name` of an instance that is GONE. EC2 keeps reporting a terminated
/// instance for a while after it is gone and it runs no subscription, so a pass does not
/// record one — the same rule the other service that resolves instances against EC2 holds.
const INSTANCE_STATE_TERMINATED: &str = "terminated";

/// The EC2 `Filters` name that selects AMIs BY ID (`DescribeImagesRequest$Filters`:
/// "`image-id` - The ID of the image").
///
/// A filter is what asks the question rather than `ImageIds`: an id the account can see no
/// image for is then an absent row instead of a failure of the whole call, so one
/// unresolvable AMI cannot make a whole pass discover nothing.
const IMAGE_ID_FILTER: &str = "image-id";

/// The most AMI ids ONE hop asks about, as values of one `image-id` filter: the per-filter
/// value count EC2's own describes refuse above, so one hop costs a constant times what the
/// account stores rather than something this pass chooses.
const IMAGE_IDS_PER_HOP: usize = 64;

/// The most pages of the EC2 inventory ONE refresh reads, over all of its Regions.
///
/// `DescribeInstances` answers a page of reservations and a token for the next, so without
/// a cap one report could walk an unbounded inventory. A pass that spends the budget
/// records what it read and leaves the rest to the next one — which the next one then
/// reads, because the point this one stopped at is recorded
/// ([`crate::state::DiscoveryResume`]) and its plan begins there.
const MAX_INVENTORY_PAGES: usize = 8;

/// The most instances of the account's inventory ONE refresh reads, over all of its
/// Regions.
///
/// A budget on the work one report does, not a ceiling on the store: what the store holds
/// is what the owner's inventory in the discovered Regions holds, which is why a Region a
/// pass read to its end REPLACES what was discovered there and a Region the settings no
/// longer name is dropped entirely.
const MAX_DISCOVERED_INSTANCES: usize = 1_000;

/// The EC2 client this service asks the account's inventory of.
///
/// `region` is the source Region being discovered, so the question is asked in the Region
/// whose data the settings named. The client is given the callback's own HTTP client, so
/// the endpoint URL below is only the address that transport routes on IN PROCESS: no
/// socket is opened, no name is resolved and nothing leaves this process. The credentials
/// are the callback's, which sign this in-process hop and never appear in a response.
fn ec2(ctx: &smithy_server_runtime::OperationContext<'_>, region: &str) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials the
        // callback hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// Materialize, for ONE report of discovered data, the discovery pass the account's
/// settings ask for, and record what it found.
///
/// EVERY report runs one, whatever cursor its request carried. Discovery reads the
/// inventory whether or not anybody calls, so the pass a report materializes is owed at the
/// instant that report is served — by the clock, not by the shape of the request — and a
/// request continuing a walk is served from the inventory of the instant IT runs exactly as
/// the request that began the walk was. Which also makes the store's content independent of
/// the CALL PATTERN that reached it: a walk cannot be shown one inventory or another
/// depending on whether something else was called between two of its pages, and no report
/// is answered from an inventory of an earlier instant than its own.
///
/// What keeps such a walk from skipping or repeating an element is its CURSOR and not a
/// frozen store: [`crate::state::paginate`] resumes after the element the previous page
/// reported LAST, named by that element's own identity in the list's order, so a pass that
/// adds or removes a record between two pages can carry nothing across the cursor.
///
/// The rule lives HERE rather than in the reports, because every operation that reports
/// discovered data reports from the one store this pass fills: were each to decide for
/// itself, two of them could be shown inventories of different instants.
///
/// What the store holds afterwards is what the account's CURRENT settings say is discovered:
/// the inventory of the source Regions they name while `LinuxSubscriptionsDiscovery` is
/// enabled, and nothing at all while it is not. No hop is made for an account that did not
/// ask for its inventory to be discovered, so the store is never FILLED from one — and
/// nothing a pass made while discovery was enabled stays behind either, because no later
/// pass refreshes it and a report of it would answer from history rather than from the
/// setting.
///
/// One hop per configured source Region (the modelled `SourceRegions` list bounds that at
/// 100 entries), plus the AMI hops the instances found there need, all bounded by the
/// constants above — and the store's lock is never held across any of them. A plan whose
/// Regions hold more inventory than those constants let one pass read is read over
/// SUCCESSIVE passes: the point a pass stopped at is recorded
/// ([`crate::state::DiscoveryResume`]) and the next pass's plan begins there
/// ([`crate::state::ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState::discovery_plan`]),
/// so every Region the settings name is asked about rather than only the first few of them.
///
/// The settings are therefore re-read from the store at every point the pass needs them —
/// before each Region's hop and under the lock each write takes — rather than snapshotted
/// once before the first hop: the pass awaits repeatedly, the settings singleton can be
/// rewritten while it does, and what the store may hold is decided by the settings AT THE
/// INSTANT of each hop and each write.
pub async fn materialize_for_report<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    let Some(state) = crate::state::service_state(engine) else {
        return;
    };
    // The Regions to ask about and where in them to start, read with the lock taken and
    // released HERE — the store is never held across an await. This is the pass's PLAN and
    // not its authority: each Region is checked against the settings again below, because
    // they can be rewritten while the pass is reading another Region.
    let plan = state.lock().discovery_plan();

    let mut budget = Budget {
        pages: MAX_INVENTORY_PAGES,
        records: MAX_DISCOVERED_INSTANCES,
    };
    // Where this pass got to, recorded at the end so the NEXT one resumes there.
    let mut stopped: Option<crate::state::DiscoveryResume> = None;
    // Whether the pass left part of its plan unread: a plan it walked to the end holds
    // nothing for a later pass to pick up, so that pass starts the plan afresh.
    let mut unread = false;
    for (position, region) in plan.regions.iter().enumerate() {
        if budget.spent() {
            unread = true;
            break;
        }
        // Asked again right before the hop: an account does not discover a Region its
        // settings have stopped naming — nor any Region at all once
        // `LinuxSubscriptionsDiscovery` is no longer enabled — so no hop is made for one,
        // exactly as none is made for an account that never asked for discovery.
        let discovered_now = state.lock().discovers_region(region);
        if !discovered_now {
            continue;
        }
        // An inventory cursor means something only in the Region it was reached in, which
        // is the one the plan BEGINS at: every other Region of the plan is read from the
        // beginning of its own inventory, including when the first one was skipped above.
        let from = match position {
            0 => plan.resume_cursor.clone(),
            _ => None,
        };
        let Some(read) = discovered_in_region(ctx, region, &mut budget, from).await else {
            // EC2 could not be asked about this Region at all: that is evidence of
            // NOTHING, so what an earlier pass discovered there stays as it was rather
            // than being dropped. The Region was still asked about, so the next pass goes
            // on past it instead of stopping on the same unanswerable hop.
            stopped = Some(crate::state::DiscoveryResume::After(region.clone()));
            continue;
        };
        let now = SystemTime::now();
        // Both writes settle against the settings under the lock they take
        // (`record_discovery_pass` / `merge_discovery_pass`), so what this pass found in a
        // Region the account stopped discovering while the pass was reading is dropped
        // instead of recorded.
        match read.pass {
            // The Region's whole inventory was read, so these records are what it holds.
            RegionPass::Complete(found) => {
                state.lock().record_discovery_pass(region, found, now);
            }
            // Only part of it was read, so the rest of what an earlier pass found there is
            // still the best evidence about it and is kept.
            RegionPass::Truncated(found) => {
                state.lock().merge_discovery_pass(found, now);
            }
        }
        stopped = Some(read.resume);
    }
    // A Region whose inventory this pass stopped INSIDE is unread work too, even when it
    // was the last Region of the plan and the loop above ended on its own.
    let unread = unread || matches!(stopped, Some(crate::state::DiscoveryResume::Inside(_, _)));
    // Discovery holds data for the Regions it discovers and no others, so a Region the
    // settings no longer name is dropped — including one dropped while this pass was
    // reading another, and every Region at once for an account that discovers none. The
    // Regions kept are the ones the store's settings name when this write takes the lock,
    // which is what lets it drop a Region this pass itself still had in its plan.
    let mut store = state.lock();
    store.retain_discovered_regions();
    // What is left for the next pass to read, which is what decides where its plan begins.
    store.record_discovery_resume(if unread { stopped } else { None });
}

/// What ONE refresh may still spend: pages of the owner's inventory, and records it may
/// record. Shared across the Regions of one pass, so the work a report does is bounded by
/// constants however many Regions the settings name.
#[derive(Debug)]
struct Budget {
    pages: usize,
    records: usize,
}

impl Budget {
    /// Whether there is nothing left to spend.
    fn spent(&self) -> bool {
        self.pages == 0 || self.records == 0
    }
}

/// What one Region's pass found.
#[derive(Debug)]
enum RegionPass {
    /// The whole inventory of the Region was read, from its beginning, so these records ARE
    /// what the account has discovered there.
    Complete(Vec<DiscoveredInstanceRecord>),
    /// The pass read only part of that inventory — it spent its budget before the inventory
    /// ran out, or it resumed one an earlier pass had stopped inside — so these records are
    /// only part of it: what an earlier pass found there is still evidence, and is merged
    /// with rather than replaced by them.
    Truncated(Vec<DiscoveredInstanceRecord>),
}

/// What one Region's read left behind: the records it found, and where a later pass picks
/// that Region up ([`crate::state::DiscoveryResume`]).
#[derive(Debug)]
struct RegionRead {
    pass: RegionPass,
    resume: crate::state::DiscoveryResume,
}

/// The instances this pass discovered in `region`, or `None` when EC2 could not be asked
/// about it at all.
///
/// `from` is the owner's inventory cursor to begin at — `None` being the beginning of that
/// inventory, and `Some` the point an earlier pass stopped inside it, so the remainder it
/// left is what this read covers.
///
/// An instance is recorded only when the AMI it was launched from bills a Linux
/// subscription ([`subscription_of_image`]); everything else the account runs there is
/// inventory this service does not report.
async fn discovered_in_region(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    budget: &mut Budget,
    from: Option<String>,
) -> Option<RegionRead> {
    let client = ec2(ctx, region);
    let mut inventory: Vec<InventoryInstance> = Vec::new();
    // Where the inventory this read has NOT covered begins: the cursor the next page is
    // asked for, which starts at the point an earlier pass stopped at.
    let mut cursor: Option<String> = from.clone();
    // Whether the Region's inventory ran out, or the budget ran out first.
    let mut reached_end = false;
    loop {
        if budget.spent() {
            break;
        }
        let answer = client
            .describe_instances()
            // A page no larger than what this pass may still RECORD, so a page it asked for
            // can always be read to its end and a resumed read always moves past the page
            // it starts at: `MaxResults` is the `@paginated` page size of
            // `DescribeInstances` — "the maximum number of items to return for this
            // request" — on which that model declares no range.
            .set_max_results(i32::try_from(budget.records).ok())
            .set_next_token(cursor.clone())
            .send()
            .await;
        let page = match answer {
            Ok(page) => page,
            Err(error) => {
                // At `debug`: an account with no instances in a Region, and a process that
                // mounts this service with no EC2 beside it, are both ordinary — but a
                // failed hop IS why a pass discovered nothing, so it is traceable.
                ::tracing::debug!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the EC2 inventory of a source Region could not be read"
                );
                return None;
            }
        };
        budget.pages -= 1;
        let mut whole_page = true;
        'page: for reservation in page.reservations() {
            let account_id = reservation.owner_id().unwrap_or_default().to_string();
            for instance in reservation.instances() {
                if budget.records == 0 {
                    // Nothing more may be recorded, so the rest of this PAGE is not read
                    // either. The page was asked for no more items than the record budget
                    // held, so this is reached only when the owner answered a larger page
                    // than it was asked for.
                    whole_page = false;
                    break 'page;
                }
                if let Some(held) = inventory_instance(instance, &account_id, region) {
                    budget.records -= 1;
                    inventory.push(held);
                }
            }
        }
        if !whole_page {
            // Part of THIS page was never read, so the unread remainder begins at the page
            // itself — which is the cursor this read asked it for, left as it is.
            break;
        }
        cursor = page.next_token().map(str::to_string);
        if cursor.is_none() {
            reached_end = true;
            break;
        }
    }

    // What each distinct AMI of those instances BILLS, asked of the owner of AMIs: that is
    // what decides the subscription, and it is read off the image rather than copied onto
    // the instance, where the two could disagree.
    let image_ids: Vec<String> = inventory
        .iter()
        .map(|held| held.image_id.clone())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    // A hop that was not answered leaves this pass without the billing facts of some of the
    // instances it read, so it has not established that those are gone: it records as a
    // TRUNCATED pass, the same treatment an unanswered inventory hop gets. So does a read
    // that RESUMED this Region's inventory: the part an earlier pass covered is not part of
    // what this one established, so it is kept rather than replaced.
    let (billing, answered) = image_billing(&client, region, &image_ids).await;
    let complete = reached_end && answered && from.is_none();

    let found: Vec<DiscoveredInstanceRecord> = inventory
        .into_iter()
        .filter_map(|held| {
            let image = billing.get(&held.image_id)?;
            let (name, subscription_type) = subscription_of_image(image)?;
            Some(DiscoveredInstanceRecord {
                instance_id: held.instance_id,
                account_id: held.account_id,
                ami_id: held.image_id,
                instance_type: held.instance_type,
                status: held.status,
                region: held.region,
                usage_operation: image.usage_operation.clone(),
                product_codes: image.product_codes.clone(),
                // Set by the pass that records the instance
                // ([`crate::state::ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState::record_discovery_pass`]),
                // which is where "the time in which the last discovery updated the
                // instance details" is decided.
                last_updated_time: None,
                subscription_name: name,
                subscription_type,
                // `OsVersion` is "the operating system software version that runs on
                // your instance", which is a fact about the running guest: no EC2
                // inventory fact states it, so none is invented.
                os_version: None,
                // The two `SubscriptionProvider*Time` members report on a subscription
                // retrieved from a registered third-party provider, and
                // `DualSubscription` on two subscriptions for the same software;
                // nothing EC2 reports establishes either, so neither is invented.
                subscription_provider_create_time: None,
                subscription_provider_update_time: None,
                dual_subscription: None,
                registered_with_subscription_provider: None,
            })
        })
        .collect();
    Some(RegionRead {
        pass: if complete {
            RegionPass::Complete(found)
        } else {
            RegionPass::Truncated(found)
        },
        // A Region whose inventory ran out is behind the plan; one this read stopped inside
        // is where the next pass resumes, at the cursor the unread remainder begins at.
        resume: if reached_end {
            crate::state::DiscoveryResume::After(region.to_string())
        } else {
            crate::state::DiscoveryResume::Inside(region.to_string(), cursor)
        },
    })
}

/// One instance of the account's EC2 inventory, as the owner reports it.
#[derive(Debug, Clone)]
struct InventoryInstance {
    instance_id: String,
    account_id: String,
    image_id: String,
    instance_type: String,
    status: String,
    region: String,
}

/// The inventory facts of one instance EC2 reported, or `None` when it is not an instance
/// of `region` this pass can place a subscription on: a terminated one (gone), one EC2
/// reports no id, AMI or Availability Zone for (nothing to discover a subscription from),
/// and one running in another Region (whose own pass reports it).
fn inventory_instance(
    instance: &::aws_sdk_ec2::types::Instance,
    account_id: &str,
    region: &str,
) -> Option<InventoryInstance> {
    let status = instance
        .state()
        .and_then(::aws_sdk_ec2::types::InstanceState::name)
        .map(|name| name.as_str().to_string())?;
    if status == INSTANCE_STATE_TERMINATED {
        return None;
    }
    let zone = instance
        .placement()
        .and_then(::aws_sdk_ec2::types::Placement::availability_zone)?;
    if !zone_is_in_region(zone, region) {
        return None;
    }
    Some(InventoryInstance {
        instance_id: instance.instance_id()?.to_string(),
        // An instance EC2 reports without an owning account or a type is not one a
        // discovered record could state those members of, and the real service reports no
        // instance with neither, so it is dropped rather than recorded with an empty value
        // a client could filter or group on.
        account_id: Some(account_id)
            .filter(|account_id| !account_id.is_empty())?
            .to_string(),
        image_id: instance.image_id()?.to_string(),
        instance_type: instance
            .instance_type()
            .map(|instance_type| instance_type.as_str().to_string())?,
        status,
        region: region.to_string(),
    })
}

/// Whether the Availability Zone `zone` is in Region `region`.
///
/// An Availability Zone is named by "the Region code, followed by a letter identifier"
/// (Amazon EC2 User Guide, "Regions and Zones"), which is also how the owner of zones names
/// them, so the Region an instance runs in is the one its `Placement$AvailabilityZone`
/// carries. A zone name of any other form — a Local Zone's, say — names no Region here and
/// so belongs to none of the source Regions.
fn zone_is_in_region(zone: &str, region: &str) -> bool {
    zone.strip_prefix(region).is_some_and(|identifier| {
        identifier.len() == 1 && identifier.bytes().all(|byte| byte.is_ascii_lowercase())
    })
}

/// What the AMI an instance was launched from bills.
#[derive(Debug, Clone)]
struct ImageBilling {
    /// `Image$PlatformDetails` — "the platform details associated with the billing code of
    /// the AMI".
    platform_details: String,
    /// `Image$UsageOperation` — that billing code.
    usage_operation: String,
    /// Whether the AMI declares `Image$Platform`, which is set only for a Windows AMI.
    windows: bool,
    /// "Any product codes associated with the AMI" (`Image$ProductCodes`), by id.
    product_codes: Vec<String>,
    /// Whether any of them was bought through the Marketplace.
    marketplace: bool,
}

/// What each AMI of `image_ids` bills, by AMI id, and whether EVERY hop was answered — an
/// id the account can see no image for is simply absent, and so is every id of a hop EC2
/// did not answer (then nothing is discovered for those instances rather than a subscription
/// being invented for an AMI nobody described).
///
/// Hops of at most [`IMAGE_IDS_PER_HOP`] ids each, over the ids of the instances one pass
/// already bounded, so the work is bounded by constants too. `client` is the one already
/// addressed at this Region, so no second client is built per hop.
async fn image_billing(
    client: &::aws_sdk_ec2::Client,
    region: &str,
    image_ids: &[String],
) -> (HashMap<String, ImageBilling>, bool) {
    let mut billing: HashMap<String, ImageBilling> = HashMap::new();
    let mut answered = true;
    for chunk in image_ids.chunks(IMAGE_IDS_PER_HOP) {
        let by_id = ::aws_sdk_ec2::types::Filter::builder()
            .name(IMAGE_ID_FILTER)
            .set_values(Some(chunk.to_vec()))
            .build();
        let answer = client.describe_images().filters(by_id).send().await;
        match answer {
            Ok(output) => {
                for image in output.images() {
                    let Some(image_id) = image.image_id() else {
                        continue;
                    };
                    billing.insert(
                        image_id.to_string(),
                        ImageBilling {
                            platform_details: image
                                .platform_details()
                                .unwrap_or_default()
                                .to_string(),
                            usage_operation: image.usage_operation().unwrap_or_default().to_string(),
                            windows: image.platform().is_some(),
                            product_codes: image
                                .product_codes()
                                .iter()
                                .filter_map(|code| {
                                    code.product_code_id().map(str::to_string)
                                })
                                .collect(),
                            marketplace: image.product_codes().iter().any(|code| {
                                code.product_code_type().is_some_and(|product_code_type| {
                                    product_code_type.as_str() == PRODUCT_CODE_TYPE_MARKETPLACE
                                })
                            }),
                        },
                    );
                }
            }
            Err(error) => {
                // The pass did not establish what these AMIs bill, so it is not a pass that
                // can decide the Region holds nothing for them: the caller is told so.
                answered = false;
                ::tracing::debug!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the AMIs the discovered instances were launched from could not be read"
                );
            }
        }
    }
    (billing, answered)
}

/// The `(Subscription$Name, Subscription$Type)` an instance launched from this AMI uses, or
/// `None` when the AMI bills no Linux subscription at all.
///
/// The subscription is the one the AMI's BILLING PLATFORM names — `Image$PlatformDetails`
/// is "the platform details associated with the billing code of the AMI", so an AMI that
/// bills as Red Hat Enterprise Linux, SUSE Linux or Ubuntu Pro runs that subscription —
/// which leaves the AMIs whose billing platform names no Linux subscription: a Windows one
/// (these operations list LINUX subscriptions), one billing the plain Linux platform, whose
/// usage operation carries no operating-system billing code, and a composite platform whose
/// operating system IS that plain one ([`PLATFORM_DETAILS_LINUX_WITH_PREFIX`]), where only
/// the software beside it is licensed.
///
/// The TYPE is how that subscription is licensed, which the same billing facts state: a
/// BYOL billing platform is the account's own subscription, an AMI carrying a Marketplace
/// product code is licensed through the Marketplace, and anything else is included with
/// Amazon EC2.
fn subscription_of_image(image: &ImageBilling) -> Option<(String, String)> {
    let platform = image.platform_details.trim();
    if platform.is_empty()
        || platform == PLATFORM_DETAILS_WITHOUT_SUBSCRIPTION
        || platform.starts_with(PLATFORM_DETAILS_LINUX_WITH_PREFIX)
        || image.windows
        || platform.starts_with(PLATFORM_DETAILS_WINDOWS_PREFIX)
    {
        return None;
    }
    let subscription_type = if platform.contains(PLATFORM_DETAILS_BYOL_WORD) {
        SUBSCRIPTION_TYPE_BYOS
    } else if image.marketplace {
        SUBSCRIPTION_TYPE_MARKETPLACE
    } else {
        SUBSCRIPTION_TYPE_INCLUDED
    };
    Some((platform.to_string(), subscription_type.to_string()))
}
