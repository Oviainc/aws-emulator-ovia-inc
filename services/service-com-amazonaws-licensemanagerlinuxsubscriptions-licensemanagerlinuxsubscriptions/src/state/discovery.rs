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

use std::collections::{BTreeSet, HashMap};
use std::time::SystemTime;

use crate::state::{DiscoveredInstanceRecord, DISCOVERY_ENABLED};

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
/// records what it read and leaves the rest to the next one.
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

/// Run the discovery pass this account's settings ask for, and record what it found.
///
/// Nothing happens for an account whose `LinuxSubscriptionsDiscovery` is not enabled: there
/// is no discovery to run, so the store is left exactly as it is rather than being filled
/// from an inventory the account did not ask to have discovered.
///
/// One hop per configured source Region (the modelled `SourceRegions` list bounds that at
/// 100 entries), plus the AMI hops the instances found there need, all bounded by the
/// constants above — and the store's lock is never held across any of them.
pub async fn refresh_discovered_instances<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) {
    let Some(state) = crate::state::service_state(engine) else {
        return;
    };
    // The lock is taken and released HERE, before the first hop: what the pass needs from
    // the store is the account's settings, and the store is never held across an await.
    let settings = {
        let store = state.lock();
        store.service_settings.as_ref().map(|settings| {
            (
                settings.linux_subscriptions_discovery.clone(),
                settings.discovery_settings.source_regions.clone(),
            )
        })
    };
    let Some((discovery, source_regions)) = settings else {
        return;
    };
    if discovery != DISCOVERY_ENABLED {
        return;
    }

    let mut budget = Budget {
        pages: MAX_INVENTORY_PAGES,
        records: MAX_DISCOVERED_INSTANCES,
    };
    // A Region named twice is one Region: the pass asks about each distinct source Region
    // once, in a stable order.
    let discovered_regions: BTreeSet<&String> = source_regions.iter().collect();
    for region in &discovered_regions {
        if budget.spent() {
            break;
        }
        let Some(pass) = discovered_in_region(ctx, region, &mut budget).await else {
            // EC2 could not be asked about this Region at all: that is evidence of
            // NOTHING, so what an earlier pass discovered there stays as it was rather
            // than being dropped.
            continue;
        };
        let now = SystemTime::now();
        match pass {
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
    }
    // Discovery holds data for the Regions it discovers and no others, so a Region the
    // settings no longer name is dropped — including one dropped while this pass was
    // reading another.
    let retained: Vec<&str> = discovered_regions
        .iter()
        .map(|region| region.as_str())
        .collect();
    state.lock().retain_discovered_regions(&retained);
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
    /// The whole inventory of the Region was read, so these records ARE what the account
    /// has discovered there.
    Complete(Vec<DiscoveredInstanceRecord>),
    /// The pass spent its budget before the Region's inventory ran out, so these records
    /// are only part of it: what an earlier pass found there is still evidence, and is
    /// merged with rather than replaced by them.
    Truncated(Vec<DiscoveredInstanceRecord>),
}

/// The instances this pass discovered in `region`, or `None` when EC2 could not be asked
/// about it at all.
///
/// An instance is recorded only when the AMI it was launched from bills a Linux
/// subscription ([`subscription_of_image`]); everything else the account runs there is
/// inventory this service does not report.
async fn discovered_in_region(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    budget: &mut Budget,
) -> Option<RegionPass> {
    let client = ec2(ctx, region);
    let mut inventory: Vec<InventoryInstance> = Vec::new();
    let mut next_token: Option<String> = None;
    // Whether the Region's inventory was read to its end, or the budget ran out first.
    let mut complete = true;
    loop {
        if budget.spent() {
            complete = false;
            break;
        }
        let answer = client
            .describe_instances()
            .set_next_token(next_token.clone())
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
        'page: for reservation in page.reservations() {
            let account_id = reservation.owner_id().unwrap_or_default().to_string();
            for instance in reservation.instances() {
                if budget.records == 0 {
                    // Nothing more may be recorded, so the rest of this PAGE is not read
                    // either: the pass is truncated and the loop above ends on its next
                    // turn.
                    complete = false;
                    break 'page;
                }
                if let Some(held) = inventory_instance(instance, &account_id, region) {
                    budget.records -= 1;
                    inventory.push(held);
                }
            }
        }
        next_token = page.next_token().map(str::to_string);
        if next_token.is_none() {
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
    // TRUNCATED pass, the same treatment an unanswered inventory hop gets.
    let (billing, answered) = image_billing(&client, region, &image_ids).await;
    let complete = complete && answered;

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
    Some(if complete {
        RegionPass::Complete(found)
    } else {
        RegionPass::Truncated(found)
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
