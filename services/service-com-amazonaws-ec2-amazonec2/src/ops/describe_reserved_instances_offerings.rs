// SPDX-License-Identifier: CC0-1.0

//! @op DescribeReservedInstancesOfferings
//!
//! Reports the Reserved Instance offerings that are available for purchase in the Region the
//! request was signed for.
//!
//! An offering is a published price-list entry, not a resource a caller creates: no EC2
//! operation adds or withdraws one, so the catalog is DERIVED — every instance type the
//! modelled `InstanceType` enum declares, in each place it can be used (each Availability
//! Zone of the Region, and the Region itself), on each modelled `RIProductDescription`, each
//! modelled `OfferingClassType`, each payment option Reserved Instances are sold on and each
//! term the `duration` filter documents — and this handler reads and writes NO state. Two
//! calls therefore report the same offerings under the same ids, which is what lets a later
//! call purchase or exchange into an offering an earlier call reported: an offering's id
//! CARRIES its terms (`crate::state::encode_reserved_instances_offering_id`), so the terms
//! travel with the id rather than in a store a read would have to write.
//!
//! Which of those offerings a request gets:
//!
//! * `AvailabilityZone` / `AvailabilityZoneId` name the one zone the Reserved Instance is to
//!   be used in — "Either AvailabilityZone or AvailabilityZoneId can be specified, but not
//!   both" — and a request naming neither gets the Regional offerings beside the zonal ones.
//!   A zonal offering exists only where that zone offers the instance type
//!   (`crate::state::zone_offers_instance_type`), the same fact
//!   `DescribeInstanceTypeOfferings` reports and a launch is held to.
//! * `InstanceType`, `ProductDescription`, `OfferingClass` and `OfferingType` each name one
//!   value of their modelled enum and select the offerings carrying it; `MinDuration` and
//!   `MaxDuration` bound the term in seconds, inclusively, with the defaults those members
//!   document.
//! * `InstanceTenancy` selects the tenancy of the instances the reservation covers. Every
//!   Reserved Instance this emulator can hold is of `default` tenancy (the one writer,
//!   `crate::state::ComAmazonawsEc2Amazonec2State::insert_reserved_instances`, derives it),
//!   so `default` — the member's own documented default — is the tenancy published, and
//!   `host` "cannot be used with this parameter".
//! * `Filters` are ANDed with each other and ORed over each filter's values, with EC2's `*`
//!   and `?` wildcards; a filter this operation does not document is refused. A filter
//!   naming a field of the catalog narrows that field BEFORE the product is enumerated, and
//!   the three that name a DERIVED value (both prices and the offering id) are matched
//!   against the offerings themselves. Every comparison is charged to one matching budget, so
//!   a search whose derived filter would have to be compared against the whole product — an
//!   offering id, which is long, with nothing else narrowing the search — is refused with
//!   `RequestLimitExceeded` rather than answered from a scan that stopped half way;
//!   `ReservedInstancesOfferingIds` is the exact way to name offerings by id.
//! * `IncludeMarketplace` and `MaxInstanceCount` select among Reserved Instance Marketplace
//!   offerings, of which this service publishes none: a Marketplace offering is another
//!   account's listing, "If you have listed your own Reserved Instances for sale in the
//!   Reserved Instance Marketplace, they will be excluded from these results", and the only
//!   listings this emulator holds are this account's own. So every offering reported carries
//!   `marketplace` false and no `pricingDetails`.
//! * `ReservedInstancesOfferingIds` names offerings directly: each id carries the terms of
//!   the offering it names, and a value this service never minted names no offering at all.
//! * `MaxResults` and `NextToken` page the selected offerings in a stable order; the token
//!   carries the fingerprint of the search it was minted for, so one presented back against a
//!   different search is refused rather than resuming inside a set it never described.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DescribeReservedInstancesOfferings::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeReservedInstancesOfferings`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Describes Reserved Instance offerings that are available for purchase. With Reserved
//! Instances, you purchase the right to launch instances for a period of time. During that
//! time period, you do not receive insufficient capacity errors, and you pay a lower usage
//! rate than the rate charged for On-Demand instances for the actual time used. If you have
//! listed your own Reserved Instances for sale in the Reserved Instance Marketplace, they
//! will be excluded from these results. This is to ensure that you do not purchase your own
//! Reserved Instances. For more information, see Sell in the Reserved Instance Marketplace
//! in the Amazon EC2 User Guide. The order of the elements in the response, including those
//! within nested structures, might vary. Applications should not assume the elements appear
//! in a particular order.
//!
//! ## Input
//!
//! * `AvailabilityZone` (String) — The Availability Zone in which the Reserved Instance can
//!   be used. Either AvailabilityZone or AvailabilityZoneId can be specified, but not both.
//! * `AvailabilityZoneId` (AvailabilityZoneId) — The ID of the Availability Zone. Either
//!   AvailabilityZone or AvailabilityZoneId can be specified, but not both.
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the action,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `Filters` (FilterList) — One or more filters. availability-zone - The Availability Zone
//!   where the Reserved Instance can be used. availability-zone-id - The ID of the
//!   Availability Zone where the Reserved Instance can be used. duration - The duration of
//!   the Reserved Instance (for example, one year or three years), in seconds (31536000 |
//!   94608000). fixed-price - The purchase price of the Reserved Instance (for example,
//!   9800.0). instance-type - The instance type that is covered by the reservation.
//!   marketplace - Set to true to show only Reserved Instance Marketplace offerings. When
//!   this filter is not used, which is the default behavior, all offerings from both Amazon
//!   Web Services and the Reserved Instance Marketplace …
//! * `IncludeMarketplace` (Boolean) — Include Reserved Instance Marketplace offerings in the
//!   response.
//! * `InstanceTenancy` (Tenancy, one of: dedicated | default | host) — The tenancy of the
//!   instances covered by the reservation. A Reserved Instance with a tenancy of dedicated is
//!   applied to instances that run in a VPC on single-tenant hardware (i.e., Dedicated
//!   Instances). Important: The host value cannot be used with this parameter. Use the
//!   default or dedicated values only. Default: default
//! * `InstanceType` (InstanceType, one of: a1.2xlarge | a1.4xlarge | a1.large | a1.medium |
//!   a1.metal | a1.xlarge | c1.medium | c1.xlarge | c3.2xlarge | c3.4xlarge | c3.8xlarge |
//!   c3.large | c3.xlarge | c4.2xlarge | c4.4xlarge | c4.8xlarge | c4.large | c4.xlarge |
//!   c5.12xlarge | …) — The instance type that the reservation will cover (for example,
//!   m1.small). For more information, see Amazon EC2 instance types in the Amazon EC2 User
//!   Guide.
//! * `MaxDuration` (Long) — The maximum duration (in seconds) to filter when searching for
//!   offerings. Default: 94608000 (3 years)
//! * `MaxInstanceCount` (Integer) — The maximum number of instances to filter when searching
//!   for offerings. Default: 20
//! * `MaxResults` (Integer) — The maximum number of results to return for the request in a
//!   single page. The remaining results of the initial request can be seen by sending another
//!   request with the returned NextToken value. The maximum is 100. Default: 100
//! * `MinDuration` (Long) — The minimum duration (in seconds) to filter when searching for
//!   offerings. Default: 2592000 (1 month)
//! * `NextToken` (String) — The token to retrieve the next page of results.
//! * `OfferingClass` (OfferingClassType, one of: convertible | standard) — The offering class
//!   of the Reserved Instance. Can be standard or convertible.
//! * `OfferingType` (OfferingTypeValues, one of: All Upfront | Heavy Utilization | Light
//!   Utilization | Medium Utilization | No Upfront | Partial Upfront) — The Reserved Instance
//!   offering type. If you are using tools that predate the 2011-11-01 API version, you only
//!   have access to the Medium Utilization Reserved Instance offering type.
//! * `ProductDescription` (RIProductDescription, one of: Linux/UNIX | Linux/UNIX (Amazon VPC)
//!   | Windows | Windows (Amazon VPC)) — The Reserved Instance product platform description.
//!   Instances that include (Amazon VPC) in the description are for use with Amazon VPC.
//! * `ReservedInstancesOfferingIds` (ReservedInstancesOfferingIdStringList) — One or more
//!   Reserved Instances offering IDs.
//!
//! ## Output
//!
//! * `NextToken` (String) — The token to use to retrieve the next page of results. This value
//!   is null when there are no more results to return.
//! * `ReservedInstancesOfferings` (ReservedInstancesOfferingList) — A list of Reserved
//!   Instances offerings.

#[allow(unused_imports)]
use super::types::*;

use ::std::borrow::Cow;

use crate::state::{
    AvailabilityZoneRecord, CompiledFilter, FilterPattern, MAX_FILTER_MATCH_STEPS,
    MAX_FILTER_VALUE_BYTES, MAX_FILTER_VALUES, MAX_FILTERS, MAX_RESOURCE_IDS, MatchBudget,
    RESERVED_INSTANCE_SCOPE_REGION, RESERVED_INSTANCE_SCOPE_ZONE, ReservedInstancesOfferingTerms,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeReservedInstancesOfferingsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeReservedInstancesOfferingsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DescribeReservedInstancesOfferings")]
#[derive(Debug)]
pub enum DescribeReservedInstancesOfferingsErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_reserved_instances_offerings::DescribeReservedInstancesOfferingsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeReservedInstancesOfferingsError = crate::scaffolded::Scaffolded<DescribeReservedInstancesOfferingsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeReservedInstancesOfferingsErrorModeled> for DescribeReservedInstancesOfferingsError {
    fn from(modeled: DescribeReservedInstancesOfferingsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeReservedInstancesOfferings` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeReservedInstancesOfferings<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeReservedInstancesOfferings<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeReservedInstancesOfferings<Eng>
{
    type Input = DescribeReservedInstancesOfferingsInput;
    type Output = DescribeReservedInstancesOfferingsOutput;
    type Error = DescribeReservedInstancesOfferingsError;

    const NAME: &'static str = "DescribeReservedInstancesOfferings";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_RESERVED_INSTANCES_OFFERINGS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_RESERVED_INSTANCES_OFFERINGS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::scaffolded::Scaffolded;
        use crate::state;

        // An offering is a published price-list entry rather than a resource a caller
        // creates — see the module docs — so this operation reads and writes no state and
        // never reaches the engine on `self`.
        let _ = &self.engine;

        // Matching cost is filters x values x value length x candidates, and the model bounds
        // none of them (`FilterList` and `Filter$Values` carry no `@length`), so every
        // request-controlled factor is capped HERE, before any work proportional to it is
        // done, and their PRODUCT is charged to one [`MatchBudget`] below. The same ceilings
        // every sibling describe of this crate applies.
        let filters: &[Filter] = input.filters.as_deref().unwrap_or_default();
        if filters.len() > MAX_FILTERS {
            return Err(state::invalid_parameter_value(format!(
                "The request carries {supplied} filters, above the maximum of {MAX_FILTERS}",
                supplied = filters.len(),
            )));
        }
        // A filter name this operation does not document is refused whether or not any
        // offering would have matched it, so an unsupported filter can never read as "no
        // results".
        for filter in filters {
            let name = filter.name.as_deref().unwrap_or_default();
            if !RESERVED_INSTANCES_OFFERING_FILTERS.contains(&name) {
                return Err(state::invalid_parameter_value(format!(
                    "The filter '{name}' is invalid"
                )));
            }
            let values: &[String] = filter.values.as_deref().unwrap_or_default();
            if values.len() > MAX_FILTER_VALUES {
                return Err(state::invalid_parameter_value(format!(
                    "The filter '{name}' carries {supplied} values, above the maximum of \
                     {MAX_FILTER_VALUES}",
                    supplied = values.len(),
                )));
            }
            if let Some(oversize) = values
                .iter()
                .find(|value| value.len() > MAX_FILTER_VALUE_BYTES)
            {
                return Err(state::invalid_parameter_value(format!(
                    "A value of the filter '{name}' carries {supplied} bytes, above the \
                     maximum of {MAX_FILTER_VALUE_BYTES}",
                    supplied = oversize.len(),
                )));
            }
        }

        // "One or more Reserved Instances offering IDs": the model bounds the list at
        // nothing and every entry is resolved, so the number of ids one request may name is
        // capped the way every other EC2 describe of this crate caps a resource-id list.
        let named_ids: &[String] = input
            .reserved_instances_offering_ids
            .as_deref()
            .unwrap_or_default();
        if named_ids.len() > MAX_RESOURCE_IDS {
            return Err(state::invalid_parameter_value(format!(
                "ReservedInstancesOfferingIds carries {supplied} ids, above the maximum of \
                 {MAX_RESOURCE_IDS}",
                supplied = named_ids.len(),
            )));
        }

        // "Either AvailabilityZone or AvailabilityZoneId can be specified, but not both"
        // (`$AvailabilityZone`): two spellings of one zone, so a request carrying both names
        // no single place for the reservation to be used.
        if input.availability_zone.is_some() && input.availability_zone_id.is_some() {
            return Err(state::invalid_parameter_combination(
                "Either availabilityZone or availabilityZoneId can be specified, but not both",
            ));
        }

        // "The host value cannot be used with this parameter. Use the default or dedicated
        // values only" (`$InstanceTenancy`). The modelled `Tenancy` enum declares `host`, so
        // the boundary validator admits it and this operation is the one that refuses it.
        if input.instance_tenancy.as_deref() == Some(TENANCY_HOST) {
            return Err(state::invalid_parameter_value(format!(
                "Invalid value '{TENANCY_HOST}' for InstanceTenancy. \
                 Use the default or dedicated values only"
            )));
        }

        // "Checks whether you have the required permissions for the action, without actually
        // making the request, and provides an error response. If you have the required
        // permissions, the error response is DryRunOperation": the flag asks about
        // PERMISSION, so it is answered once the request's own form has been checked and
        // before anything is selected; every caller of this emulator is permitted.
        if input.dry_run.unwrap_or(false) {
            return Err(state::dry_run_operation());
        }

        // EC2 is a regional service reached through a regional endpoint: the Region the
        // caller signed for is the Region whose offerings it is asking about.
        let region = state::request_region(ctx.parts());
        let zones = state::availability_zones_in_region(&region);

        // "The token to retrieve the next page of results" — anything else is refused rather
        // than read as a cursor. The token carries the fingerprint of the search it was
        // minted for ([`search_fingerprint`]) beside the position inside it, so a token
        // presented back against a DIFFERENT search is refused instead of resuming at a
        // position in a set it never described.
        let fingerprint = search_fingerprint(&input, &region);
        let start_at = match input.next_token.as_deref() {
            None => 0,
            Some(token) => state::decode_scoped_page_token(token, &fingerprint)
                .and_then(|position| position.parse::<usize>().ok())
                .ok_or_else(|| state::invalid_pagination_token_for(token))?,
        };

        // The zone the request named, which must be one this Region has: both members name a
        // zone, so both are resolved through the one lookup of a Region's zones.
        let named_zone: Option<&AvailabilityZoneRecord> = match (
            input.availability_zone.as_deref(),
            input.availability_zone_id.as_deref(),
        ) {
            (Some(zone_name), _) => Some(state::availability_zone_of_region(
                &zones,
                &region,
                "AvailabilityZone",
                zone_name,
                |zone| zone.zone_name.as_str(),
            )?),
            (_, Some(zone_id)) => Some(state::availability_zone_of_region(
                &zones,
                &region,
                "AvailabilityZoneId",
                zone_id,
                |zone| zone.zone_id.as_str(),
            )?),
            _ => None,
        };

        // Every comparison the request's filter VALUES make is charged to one budget, so the
        // product of filters, values, value length and candidates is bounded however the
        // caller combines them. Each value is compiled into its pattern ONCE.
        let mut budget = MatchBudget::new(MAX_FILTER_MATCH_STEPS);
        let compiled: Vec<(&str, Vec<FilterPattern>)> = filters
            .iter()
            .map(|filter| {
                let patterns = filter
                    .values
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|wanted| FilterPattern::compile(wanted))
                    .collect();
                (filter.name.as_deref().unwrap_or_default(), patterns)
            })
            .collect();
        // Does every filter named `field` admit a candidate holding `held`? "If you specify
        // multiple filters, the filters are joined with an AND" (`Filter`), and within one
        // filter "if you specify multiple values for a filter, the values are joined with an
        // OR" (`Filter$Values`). A candidate holding NO value for the field — an offering of
        // Regional scope has no Availability Zone — matches nothing rather than everything.
        let admits = |field: &str, held: &[&str], budget: &mut MatchBudget| -> bool {
            compiled
                .iter()
                .filter(|(name, _)| *name == field)
                .all(|(_, patterns)| {
                    patterns.iter().any(|pattern| {
                        held.iter()
                            .any(|value| pattern.matches_within(value, budget))
                    })
                })
        };

        // Each filter that names a field of the catalog narrows that FIELD before the product
        // of the fields is enumerated: the same answer as testing every built offering, at a
        // cost proportional to the sum of the fields rather than to their product.
        let scopes: Vec<OfferingScope> = offering_scopes(&zones, named_zone)
            .into_iter()
            .filter(|scope| {
                admits(
                    "availability-zone",
                    &scope.availability_zone.as_deref().into_iter().collect::<Vec<_>>(),
                    &mut budget,
                ) && admits(
                    "availability-zone-id",
                    &scope
                        .availability_zone_id
                        .as_deref()
                        .into_iter()
                        .collect::<Vec<_>>(),
                    &mut budget,
                ) && admits("scope", &[scope.scope()], &mut budget)
            })
            .collect();
        let instance_types: Vec<&'static str> = state::offered_instance_types()
            .iter()
            .copied()
            .filter(|instance_type| {
                selects(input.instance_type.as_deref(), instance_type)
                    && admits("instance-type", &[instance_type], &mut budget)
            })
            .collect();
        let products: Vec<&'static str> = R_I_PRODUCT_DESCRIPTION_VALUES
            .iter()
            .copied()
            .filter(|product_description| {
                selects(input.product_description.as_deref(), product_description)
                    && admits("product-description", &[product_description], &mut budget)
            })
            .collect();
        let classes: Vec<&'static str> = OFFERING_CLASS_TYPE_VALUES
            .iter()
            .copied()
            .filter(|offering_class| selects(input.offering_class.as_deref(), offering_class))
            .collect();
        let payment_options: Vec<&'static str> = RESERVED_INSTANCES_PAYMENT_OPTIONS
            .iter()
            .copied()
            .filter(|offering_type| selects(input.offering_type.as_deref(), offering_type))
            .collect();
        // "The minimum duration (in seconds) to filter when searching for offerings. Default:
        // 2592000 (1 month)" and "The maximum duration (in seconds) ... Default: 94608000 (3
        // years)": bounds on the term, inclusive at both ends.
        let min_duration = input.min_duration.unwrap_or(DEFAULT_MIN_DURATION_SECONDS);
        let max_duration = input.max_duration.unwrap_or(DEFAULT_MAX_DURATION_SECONDS);
        let durations: Vec<i64> = RESERVED_INSTANCES_OFFERED_DURATIONS
            .iter()
            .copied()
            .filter(|duration| {
                (min_duration..=max_duration).contains(duration)
                    && admits("duration", &[&duration.to_string()], &mut budget)
            })
            .collect();
        // Every offering this service publishes is an Amazon Web Services offering rather
        // than a Marketplace one (see the module docs), so the `marketplace` filter is
        // answered once for the whole catalog: a request asking only for Marketplace
        // offerings is asking for none of these.
        let marketplace_admitted = admits(
            "marketplace",
            &[&OFFERING_MARKETPLACE.to_string()],
            &mut budget,
        );
        // "The tenancy of the instances covered by the reservation ... Default: default"
        // (`$InstanceTenancy`), likewise answered once: every offering published is of
        // `default` tenancy, because every Reserved Instance this emulator can hold is (see
        // the module docs), so a search for another tenancy is a search for none of these.
        let tenancy_admitted = selects(
            input.instance_tenancy.as_deref(),
            crate::state::DEFAULT_INSTANCE_TENANCY,
        );
        // `IncludeMarketplace` and `MaxInstanceCount` select among those Marketplace
        // offerings too, so they are read and change nothing rather than being refused.
        let _ = (input.include_marketplace, input.max_instance_count);

        // The filters that name a DERIVED value of an offering — its two prices, which come
        // out of the term and the payment option, and its id, which carries the terms — are
        // matched against the offerings themselves rather than against a field of the
        // catalog.
        let derived: Vec<CompiledFilter> = filters
            .iter()
            .filter(|filter| {
                DERIVED_OFFERING_FILTERS.contains(&filter.name.as_deref().unwrap_or_default())
            })
            .map(|filter| {
                CompiledFilter::new(
                    filter.name.clone().unwrap_or_default(),
                    filter.values.clone().unwrap_or_default(),
                )
            })
            .collect();

        // `MaxResults` bounds one page: "The maximum number of results to return for the
        // request in a single page ... The maximum is 100. Default: 100", so a request naming
        // no page size gets the largest page this operation serves and one naming a larger
        // value is served that same maximum.
        let limit = input
            .max_results
            .and_then(|max_results| usize::try_from(max_results).ok())
            .unwrap_or(MAX_RESULTS)
            .clamp(1, MAX_RESULTS);

        let mut page: Vec<ReservedInstancesOffering> = Vec::new();
        let mut next_position: Option<usize> = None;
        if marketplace_admitted && tenancy_admitted {
            match named_ids.is_empty() {
                // "One or more Reserved Instances offering IDs": the ids name the offerings
                // to report. An id carries its terms, so what it names is the offering THIS
                // Region publishes for those terms ([`published_offering`]) — prices and all,
                // derived here exactly as the catalog branch below derives them, never read
                // out of the id — and an id no such offering carries names no offering at
                // all. The rest of the request then narrows what is left exactly as it
                // narrows the catalog.
                false => {
                    let mut named: Vec<(OfferingScope, ReservedInstancesOfferingTerms)> =
                        Vec::new();
                    for offering_id in state::deduplicate_ids(named_ids) {
                        let published = state::decode_reserved_instances_offering_id(&offering_id)
                            .and_then(|terms| published_offering(&terms, &zones))
                            .filter(|(_, published)| {
                                state::encode_reserved_instances_offering_id(published)
                                    == offering_id
                            })
                            .ok_or_else(|| {
                                state::invalid_reserved_instances_offering_id(&offering_id)
                            })?;
                        named.push(published);
                    }
                    for (position, (scope, terms)) in named.into_iter().enumerate().skip(start_at)
                    {
                        if !scopes
                            .iter()
                            .any(|narrowed| narrowed.availability_zone == terms.availability_zone)
                        {
                            continue;
                        }
                        if !instance_types.contains(&terms.instance_type.as_str())
                            || !products.contains(&terms.product_description.as_str())
                            || !classes.contains(&terms.offering_class.as_str())
                            || !payment_options.contains(&terms.offering_type.as_str())
                            || !durations.contains(&terms.duration_seconds)
                        {
                            continue;
                        }
                        if !budget.spend() {
                            break;
                        }
                        let offering = offering_view(&terms, &scope);
                        match take_matching(offering, &derived, &mut budget, limit, &mut page) {
                            Taken::Full => {
                                next_position = Some(position);
                                break;
                            }
                            Taken::Pushed | Taken::Skipped => {}
                        }
                    }
                }
                // The published catalog: the product of the fields narrowed above, walked
                // from the position the token named. Only the offerings of ONE page are
                // built, so a parameterless call costs a page rather than the whole product.
                true => {
                    let axes = CatalogAxes {
                        scopes: &scopes,
                        instance_types: &instance_types,
                        products: &products,
                        classes: &classes,
                        payment_options: &payment_options,
                        durations: &durations,
                    };
                    // The facts each offering's price is derived from, read once per instance
                    // type the search covers rather than once per offering of it: every
                    // offering of a type is a share of the same On-Demand hour.
                    let records: Vec<Option<crate::state::InstanceTypeRecord>> = instance_types
                        .iter()
                        .map(|instance_type| crate::state::instance_type_record(instance_type))
                        .collect();
                    for position in start_at..axes.total() {
                        if !budget.spend() {
                            break;
                        }
                        let [scope_at, type_at, product_at, class_at, payment_at, duration_at] =
                            axes.position_of(position);
                        // Read by index rather than indexed into: a position is inside the
                        // product of the axes' lengths, so each digit of it names an entry of
                        // its own axis, and nothing here can be out of bounds even if that
                        // ever stopped holding.
                        let (
                            Some(scope),
                            Some(instance_type),
                            Some(record),
                            Some(product_description),
                            Some(offering_class),
                            Some(offering_type),
                            Some(duration_seconds),
                        ) = (
                            scopes.get(scope_at),
                            instance_types.get(type_at).copied(),
                            records.get(type_at).and_then(Option::as_ref),
                            products.get(product_at).copied(),
                            classes.get(class_at).copied(),
                            payment_options.get(payment_at).copied(),
                            durations.get(duration_at).copied(),
                        )
                        else {
                            continue;
                        };
                        // A zonal offering exists only where that zone offers the instance
                        // type at all, the same fact `DescribeInstanceTypeOfferings` reports
                        // and a launch into the zone is held to.
                        if let Some(zone_name) = scope.availability_zone.as_deref() {
                            if !state::zone_offers_instance_type(zone_name, instance_type) {
                                continue;
                            }
                        }
                        let Some((fixed_price, usage_price)) = offering_prices(
                            record,
                            product_description,
                            offering_class,
                            offering_type,
                            duration_seconds,
                        ) else {
                            continue;
                        };
                        let terms = ReservedInstancesOfferingTerms {
                            instance_type: instance_type.to_string(),
                            availability_zone: scope.availability_zone.clone(),
                            product_description: product_description.to_string(),
                            offering_class: offering_class.to_string(),
                            offering_type: offering_type.to_string(),
                            duration_seconds,
                            fixed_price,
                            usage_price,
                        };
                        let offering = offering_view(&terms, scope);
                        match take_matching(offering, &derived, &mut budget, limit, &mut page) {
                            Taken::Full => {
                                next_position = Some(position);
                                break;
                            }
                            Taken::Pushed | Taken::Skipped => {}
                        }
                    }
                }
            }
        }

        // A spent budget means the answer is INCOMPLETE, which is not something to report as
        // a result: the request asked for more filtering than one call will do, and is told
        // so rather than given a silently truncated report.
        if budget.exhausted() {
            return Err(Scaffolded::request_limit_exceeded(format!(
                "The request's filters would take more than {MAX_FILTER_MATCH_STEPS} \
                 comparisons to evaluate; narrow them or page with MaxResults"
            )));
        }

        // "This value is null when there are no more results to return"
        // (`DescribeReservedInstancesOfferingsResult$NextToken`): a token only when an
        // offering follows this page.
        let next_token = next_position
            .map(|position| state::encode_scoped_page_token(&fingerprint, &position.to_string()));

        Ok(DescribeReservedInstancesOfferingsOutput {
            next_token,
            reserved_instances_offerings: Some(page),
        })
    }
}

// ---------------------------------------------------------------------------
// The published offering catalog
// ---------------------------------------------------------------------------
//
// An offering is a price-list entry a caller purchases, never a resource it creates, so the
// catalog is DERIVED here — beside the operation that reports it — rather than stored in
// `crate::state`, which holds the resources callers create. What `crate::state` does own is
// the TERMS of one offering and the id that carries them
// (`crate::state::ReservedInstancesOfferingTerms`), because the exchange operations resolve
// an id a caller hands back; this module builds the offerings those terms describe, so there
// is one definition of an offering's id and one of what the Region offers.

/// Every filter `DescribeReservedInstancesOfferings` documents, as its `Filters` member lists
/// them:
///
/// ```text
/// smithy_query(query="doc",shape="DescribeReservedInstancesOfferingsRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// A filter outside this set is refused. Every entry is answered either by narrowing the
/// field it names before the catalog's product is enumerated, or — for the entries of
/// [`DERIVED_OFFERING_FILTERS`] — by [`offering_filter_values`].
const RESERVED_INSTANCES_OFFERING_FILTERS: [&str; 10] = [
    "availability-zone",
    "availability-zone-id",
    "duration",
    "fixed-price",
    "instance-type",
    "marketplace",
    "product-description",
    "reserved-instances-offering-id",
    "scope",
    "usage-price",
];

/// The filters of [`RESERVED_INSTANCES_OFFERING_FILTERS`] that name a value DERIVED from an
/// offering's terms rather than a field the catalog is built from: the purchase price and the
/// hourly usage price, which come out of the term and the payment option, and the offering id,
/// which carries the terms. They are matched against the built offerings
/// ([`offering_filter_values`]) because there is no field of the catalog to narrow for them.
const DERIVED_OFFERING_FILTERS: [&str; 3] = [
    "fixed-price",
    "reserved-instances-offering-id",
    "usage-price",
];

/// The `Tenancy` value `$InstanceTenancy` refuses: "The host value cannot be used with this
/// parameter. Use the default or dedicated values only" — a member of the modelled `Tenancy`
/// enum (`super::types::TENANCY_VALUES`), so only this operation can refuse it.
const TENANCY_HOST: &str = "host";

/// Whether an offering this service publishes is a Reserved Instance Marketplace offering:
/// never, for the reason the module docs give — a Marketplace offering is another account's
/// listing, and the only listings this emulator holds are this account's own, which "will be
/// excluded from these results".
const OFFERING_MARKETPLACE: bool = false;

/// The seconds in a one-year term: `31536000`, one of the two values the `duration` filter
/// documents ("the duration of the Reserved Instance (for example, one year or three years),
/// in seconds (31536000 | 94608000)").
const ONE_YEAR_SECONDS: i64 = 31_536_000;

/// The seconds in a three-year term, the other of those two values — which is also what
/// `$MaxDuration` documents as its default.
const THREE_YEAR_SECONDS: i64 = 94_608_000;

/// The terms Reserved Instances are offered for, ascending: the two the `duration` filter
/// documents.
const RESERVED_INSTANCES_OFFERED_DURATIONS: [i64; 2] = [ONE_YEAR_SECONDS, THREE_YEAR_SECONDS];

/// "The minimum duration (in seconds) to filter when searching for offerings. Default:
/// 2592000 (1 month)" (`$MinDuration`).
const DEFAULT_MIN_DURATION_SECONDS: i64 = 2_592_000;

/// "The maximum duration (in seconds) to filter when searching for offerings. Default:
/// 94608000 (3 years)" (`$MaxDuration`).
const DEFAULT_MAX_DURATION_SECONDS: i64 = THREE_YEAR_SECONDS;

/// The payment options Reserved Instances are sold on: the three members of the modelled
/// `OfferingTypeValues` enum (`super::types::OFFERING_TYPE_VALUES_VALUES`) that name WHEN a
/// term is paid for.
///
/// The enum's other three members — `Heavy Utilization`, `Light Utilization` and `Medium
/// Utilization` — are the offering types of the API versions before `2011-11-01` ("If you are
/// using tools that predate the 2011-11-01 API version, you only have access to the Medium
/// Utilization Reserved Instance offering type", `$OfferingType`), so no current offering is
/// published on one and a search for one reports none.
const RESERVED_INSTANCES_PAYMENT_OPTIONS: [&str; 3] =
    [ALL_UPFRONT, PARTIAL_UPFRONT, NO_UPFRONT];

/// The `OfferingTypeValues` member whose whole term is paid at purchase.
const ALL_UPFRONT: &str = "All Upfront";

/// The member that pays part at purchase and the rest per hour.
const PARTIAL_UPFRONT: &str = "Partial Upfront";

/// The member that pays nothing at purchase and the whole term per hour.
const NO_UPFRONT: &str = "No Upfront";

/// The offering class that may be exchanged for another of the same or higher value: the
/// `OfferingClassType` enum's `convertible` member, as `crate::state` already names it for the
/// exchange operations.
const OFFERING_CLASS_CONVERTIBLE: &str = crate::state::RESERVED_INSTANCE_OFFERING_CLASS_CONVERTIBLE;

/// The most offerings one page carries, and the page size a request that names no
/// `MaxResults` gets: "The maximum number of results to return for the request in a single
/// page ... The maximum is 100. Default: 100" (`$MaxResults`).
const MAX_RESULTS: usize = 100;

/// The seconds in an hour, for turning a term into the hours its hourly price is charged for.
const SECONDS_PER_HOUR: u64 = 3_600;

/// The micro-dollars in one cent, for rounding a purchase price to the cent: the hour this
/// catalog discounts is priced in micro-dollars (`crate::state::MICROS_PER_DOLLAR`).
const MICROS_PER_CENT: u64 = crate::state::MICROS_PER_DOLLAR / 100;

/// The platform whose price carries an operating-system licence premium: the `Windows` member
/// of the modelled `RIProductDescription` enum, which its `(Amazon VPC)` form starts with too.
const WINDOWS_PLATFORM: &str = "Windows";

/// The share of an On-Demand hour a one-year Reserved Instance of the `standard` offering
/// class is charged, as a percentage.
///
/// CHOSEN rate, documented RULE: "you pay a lower usage rate than the rate charged for
/// On-Demand instances" (this operation's own documentation), a longer term is discounted
/// further, and a `convertible` offering — which "can be exchanged for Reserved Instances of
/// the same or higher monetary value" — is discounted less than a `standard` one of the same
/// term by [`CONVERTIBLE_PREMIUM_PERCENTAGE_POINTS`].
const ONE_YEAR_PERCENTAGE_OF_ON_DEMAND: u64 = 60;

/// The same share for a term of three years or more: a longer commitment is discounted
/// further.
const THREE_YEAR_PERCENTAGE_OF_ON_DEMAND: u64 = 40;

/// What the flexibility of a `convertible` offering costs, in percentage points of the
/// On-Demand hour, on top of the share a `standard` offering of the same term is charged.
const CONVERTIBLE_PREMIUM_PERCENTAGE_POINTS: u64 = 10;

/// What one place a Reserved Instance offering can be used is: one Availability Zone of the
/// Region, or the Region itself.
///
/// "The scope of the Reserved Instance" (`ReservedInstancesOffering$Scope`) is one of the two
/// members of the modelled `scope` enum, and which one follows from whether the offering names
/// a zone — the same derivation
/// `crate::state::ComAmazonawsEc2Amazonec2State::insert_reserved_instances` makes for a
/// purchase, so an offering and the Reserved Instance a purchase of it mints cannot disagree
/// about their scope.
#[derive(Debug, Clone)]
struct OfferingScope {
    /// The Availability Zone the Reserved Instance can be used in, or `None` for an offering
    /// of Regional scope.
    availability_zone: Option<String>,
    /// That zone's id, or `None` for a Regional offering.
    availability_zone_id: Option<String>,
}

impl OfferingScope {
    /// "The scope of the Reserved Instance", read off whether this place is one zone.
    fn scope(&self) -> &'static str {
        if self.availability_zone.is_some() {
            RESERVED_INSTANCE_SCOPE_ZONE
        } else {
            RESERVED_INSTANCE_SCOPE_REGION
        }
    }
}

/// The places the offerings of one Region are published for: the Region itself, then each of
/// its Availability Zones — or the ONE zone the request named, since a request that names a
/// zone is asking where a Reserved Instance is to be used.
fn offering_scopes(
    zones: &[AvailabilityZoneRecord],
    named_zone: Option<&AvailabilityZoneRecord>,
) -> Vec<OfferingScope> {
    let zonal = |zone: &AvailabilityZoneRecord| OfferingScope {
        availability_zone: Some(zone.zone_name.clone()),
        availability_zone_id: Some(zone.zone_id.clone()),
    };
    match named_zone {
        Some(zone) => vec![zonal(zone)],
        None => ::std::iter::once(OfferingScope {
            availability_zone: None,
            availability_zone_id: None,
        })
        .chain(zones.iter().map(zonal))
        .collect(),
    }
}

/// The fields the published catalog is the product of, as one request narrowed them.
///
/// The product is walked LAZILY rather than materialised: a Region offers every instance type
/// in every place on every product description, class, payment option and term, which is
/// hundreds of thousands of offerings, while one page carries at most [`MAX_RESULTS`] of them.
/// So a position in the product is turned into the one offering it names
/// ([`Self::position_of`]) and only a page of them is ever built.
struct CatalogAxes<'a> {
    /// Where the Reserved Instance can be used.
    scopes: &'a [OfferingScope],
    /// The instance types the reservation can cover.
    instance_types: &'a [&'static str],
    /// The product platform descriptions.
    products: &'a [&'static str],
    /// The offering classes.
    classes: &'a [&'static str],
    /// The payment options.
    payment_options: &'a [&'static str],
    /// The terms, in seconds.
    durations: &'a [i64],
}

impl CatalogAxes<'_> {
    /// The lengths of the axes, in the order a position counts them — the last varying
    /// fastest.
    fn lengths(&self) -> [usize; 6] {
        [
            self.scopes.len(),
            self.instance_types.len(),
            self.products.len(),
            self.classes.len(),
            self.payment_options.len(),
            self.durations.len(),
        ]
    }

    /// How many offerings the product holds: zero when any axis was narrowed to nothing.
    fn total(&self) -> usize {
        self.lengths()
            .into_iter()
            .fold(1usize, |total, length| total.saturating_mul(length))
    }

    /// Where `position` sits on each axis: the mixed-radix digits of the position, so a
    /// position names exactly one offering and consecutive positions walk the product in a
    /// stable order (the last axis varying fastest).
    fn position_of(&self, position: usize) -> [usize; 6] {
        let lengths = self.lengths();
        let mut at = [0usize; 6];
        let mut rest = position;
        for axis in (0..lengths.len()).rev() {
            let length = lengths[axis].max(1);
            at[axis] = rest % length;
            rest /= length;
        }
        at
    }
}

/// The offering THIS Region publishes for the terms an offering id carries: the place it can
/// be used and the terms the catalog itself reports for it, with both prices DERIVED here the
/// way the catalog derives them rather than read out of the id. `None` when the Region
/// publishes no such offering — a zone it does not have, a zone that does not offer the
/// instance type, or a value outside one of the catalog's own axes.
///
/// This is what makes the two halves of this operation agree about what EXISTS: an id names an
/// offering exactly when the offering published for its terms carries that same id, which the
/// caller checks by re-encoding what this answers.
fn published_offering(
    terms: &ReservedInstancesOfferingTerms,
    zones: &[AvailabilityZoneRecord],
) -> Option<(OfferingScope, ReservedInstancesOfferingTerms)> {
    let scope = match terms.availability_zone.as_deref() {
        None => OfferingScope {
            availability_zone: None,
            availability_zone_id: None,
        },
        Some(zone_name) => {
            let zone = zones.iter().find(|zone| zone.zone_name == zone_name)?;
            // A zonal offering exists only where that zone offers the instance type, the same
            // check the catalog walk makes.
            if !crate::state::zone_offers_instance_type(zone_name, &terms.instance_type) {
                return None;
            }
            OfferingScope {
                availability_zone: Some(zone.zone_name.clone()),
                availability_zone_id: Some(zone.zone_id.clone()),
            }
        }
    };
    if !crate::state::offered_instance_types().contains(&terms.instance_type.as_str())
        || !R_I_PRODUCT_DESCRIPTION_VALUES.contains(&terms.product_description.as_str())
        || !OFFERING_CLASS_TYPE_VALUES.contains(&terms.offering_class.as_str())
        || !RESERVED_INSTANCES_PAYMENT_OPTIONS.contains(&terms.offering_type.as_str())
        || !RESERVED_INSTANCES_OFFERED_DURATIONS.contains(&terms.duration_seconds)
    {
        return None;
    }
    let (fixed_price, usage_price) = offering_prices(
        &crate::state::instance_type_record(&terms.instance_type)?,
        &terms.product_description,
        &terms.offering_class,
        &terms.offering_type,
        terms.duration_seconds,
    )?;
    Some((
        scope,
        ReservedInstancesOfferingTerms {
            fixed_price,
            usage_price,
            ..terms.clone()
        },
    ))
}

/// Whether a product description is a Windows platform, which carries an operating-system
/// licence on top of the hardware: the two Windows members of the modelled
/// `RIProductDescription` enum, one for use in a VPC and one outside it — the `(Amazon VPC)`
/// part says WHERE the reservation applies, not what it runs, so both are priced alike.
fn product_is_windows(product_description: &str) -> bool {
    product_description.starts_with(WINDOWS_PLATFORM)
}

/// The share of the On-Demand hour a Reserved Instance of `offering_class` held for
/// `duration_seconds` is charged, as a percentage: the term's discount, with a `convertible`
/// offering discounted less than a `standard` one of the same term.
fn percentage_of_on_demand(offering_class: &str, duration_seconds: i64) -> u64 {
    let term = if duration_seconds >= THREE_YEAR_SECONDS {
        THREE_YEAR_PERCENTAGE_OF_ON_DEMAND
    } else {
        ONE_YEAR_PERCENTAGE_OF_ON_DEMAND
    };
    if offering_class == OFFERING_CLASS_CONVERTIBLE {
        term + CONVERTIBLE_PREMIUM_PERCENTAGE_POINTS
    } else {
        term
    }
}

/// What share of a term's hourly price is charged PER HOUR rather than at purchase, as a
/// percentage, or `None` for an offering type that is not one of
/// [`RESERVED_INSTANCES_PAYMENT_OPTIONS`].
///
/// This is the whole of what a payment option says about a price: the three options split the
/// SAME total — `All Upfront` pays it all at purchase, `No Upfront` all of it per hour, and
/// `Partial Upfront` half each way.
fn hourly_percentage_of_term(offering_type: &str) -> Option<u64> {
    match offering_type {
        ALL_UPFRONT => Some(0),
        PARTIAL_UPFRONT => Some(50),
        NO_UPFRONT => Some(100),
        _ => None,
    }
}

/// The two prices of one offering: "The purchase price of the Reserved Instance"
/// (`ReservedInstancesOffering$FixedPrice`) and "The usage price of the Reserved Instance, per
/// hour" (`$UsagePrice`), in the offering's currency.
///
/// Both come out of ONE total — the crate's On-Demand hour for this instance type and platform
/// (`crate::state::on_demand_hour_micros`, the same basis a Spot price is a share of),
/// discounted for the term and the class ([`percentage_of_on_demand`]) and charged for every
/// hour of the term — split by the payment option ([`hourly_percentage_of_term`]): the hourly
/// part is the usage price, and what is left of the total is due at purchase, rounded to the
/// cent. `None` for an offering type no offering is published on.
fn offering_prices(
    record: &crate::state::InstanceTypeRecord,
    product_description: &str,
    offering_class: &str,
    offering_type: &str,
    duration_seconds: i64,
) -> Option<(f32, f32)> {
    let on_demand_micros =
        crate::state::on_demand_hour_micros(record, product_is_windows(product_description));
    let reserved_micros = on_demand_micros
        .saturating_mul(percentage_of_on_demand(offering_class, duration_seconds))
        / 100;
    let hourly_micros =
        reserved_micros.saturating_mul(hourly_percentage_of_term(offering_type)?) / 100;
    let term_hours = u64::try_from(duration_seconds.max(0)).unwrap_or(0) / SECONDS_PER_HOUR;
    let upfront_micros = reserved_micros
        .saturating_sub(hourly_micros)
        .saturating_mul(term_hours);
    Some((upfront_dollars(upfront_micros), hourly_dollars(hourly_micros)))
}

/// A purchase price in micro-dollars as the `Float` member carries it: dollars, rounded to the
/// nearest cent, which is how a one-off currency amount is quoted.
fn upfront_dollars(micros: u64) -> f32 {
    let cents = micros.saturating_add(MICROS_PER_CENT / 2) / MICROS_PER_CENT;
    cents as f32 / 100.0
}

/// An hourly price in micro-dollars as the `Float` member carries it: dollars, keeping the
/// places an hourly Reserved Instance rate is quoted to.
fn hourly_dollars(micros: u64) -> f32 {
    micros as f32 / crate::state::MICROS_PER_DOLLAR as f32
}

/// One offering of the catalog as the modelled `ReservedInstancesOffering` shape.
///
/// Every member the service itself owns is derived rather than taken from a request — the id
/// from the terms, the currency, the tenancy, the scope and the zone id from the place, and
/// the Marketplace members from the fact that this service publishes no Marketplace offering —
/// so two callers cannot be shown the same offering differently.
fn offering_view(
    terms: &ReservedInstancesOfferingTerms,
    scope: &OfferingScope,
) -> ReservedInstancesOffering {
    ReservedInstancesOffering {
        // Derived from the terms, so the same offering always carries the same id and a
        // purchase or an exchange can read the terms back out of it.
        reserved_instances_offering_id: Some(
            crate::state::encode_reserved_instances_offering_id(terms),
        ),
        instance_type: Some(terms.instance_type.clone()),
        availability_zone: terms.availability_zone.clone(),
        availability_zone_id: scope.availability_zone_id.clone(),
        duration: Some(terms.duration_seconds),
        fixed_price: Some(terms.fixed_price),
        usage_price: Some(terms.usage_price),
        product_description: Some(terms.product_description.clone()),
        offering_class: Some(terms.offering_class.clone()),
        offering_type: Some(terms.offering_type.clone()),
        scope: Some(scope.scope().to_string()),
        currency_code: Some(crate::state::RESERVED_INSTANCE_CURRENCY_CODE.to_string()),
        instance_tenancy: Some(crate::state::DEFAULT_INSTANCE_TENANCY.to_string()),
        marketplace: Some(OFFERING_MARKETPLACE),
        // "The pricing details of the Reserved Instance offering"
        // (`ReservedInstancesOffering$PricingDetails`) are the remaining instances and their
        // prices of a MARKETPLACE offering, of which this service publishes none.
        pricing_details: Some(Vec::new()),
        // "The recurring charge tag assigned to the resource"
        // (`ReservedInstancesOffering$RecurringCharges`): the hourly part of this term's price
        // is its `UsagePrice` — "the usage price of the Reserved Instance, per hour" — so no
        // separate recurring charge is assigned to it, exactly as the Reserved Instance a
        // purchase of this offering mints carries none.
        recurring_charges: Some(Vec::new()),
    }
}

/// Does a request member that names ONE value of an enum select a candidate carrying `held`?
///
/// A member the request omits selects every candidate, which is what makes the narrowing
/// members of this request independent of each other.
fn selects(asked: Option<&str>, held: &str) -> bool {
    asked.is_none_or(|asked| asked == held)
}

/// What [`take_matching`] did with one candidate offering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Taken {
    /// A derived filter excluded it.
    Skipped,
    /// It is on the page.
    Pushed,
    /// The page was already full, so it is where the next page starts.
    Full,
}

/// Put `offering` on `page` unless a derived filter excludes it, answering whether the page
/// was already full — in which case `offering` is the one the next page starts at and nothing
/// was added.
fn take_matching(
    offering: ReservedInstancesOffering,
    derived: &[CompiledFilter],
    budget: &mut MatchBudget,
    limit: usize,
    page: &mut Vec<ReservedInstancesOffering>,
) -> Taken {
    if !crate::state::matches_every_filter(derived, budget, &|name| {
        offering_filter_values(&offering, name)
    }) {
        return Taken::Skipped;
    }
    if page.len() >= limit {
        return Taken::Full;
    }
    page.push(offering);
    Taken::Pushed
}

/// The values the EC2 filter `name` compares against this offering, for the filters that name
/// a DERIVED value ([`DERIVED_OFFERING_FILTERS`]).
///
/// A filter naming anything else compares against NO values here, because the field it names
/// was narrowed before the offering was built — every such filter has already held.
fn offering_filter_values<'a>(
    offering: &'a ReservedInstancesOffering,
    name: &str,
) -> Vec<Cow<'a, str>> {
    match name {
        "fixed-price" => crate::state::price_filter_values(offering.fixed_price),
        "usage-price" => crate::state::price_filter_values(offering.usage_price),
        "reserved-instances-offering-id" => offering
            .reserved_instances_offering_id
            .as_deref()
            .map(Cow::Borrowed)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// The fingerprint of the search `input` describes, in `region`: every member that SELECTS
/// what is reported, as the request gives them.
///
/// A `NextToken` is SCOPED to it (`crate::state::encode_scoped_page_token`), so a token minted
/// by one search is refused when it is presented back against another, the way the real
/// service refuses a pagination token whose request parameters changed, instead of resuming at
/// a position inside a set it never described. The Region is part of it because a Region's
/// zones are what the places of its offerings are.
fn search_fingerprint(input: &DescribeReservedInstancesOfferingsInput, region: &str) -> String {
    use ::std::hash::{Hash, Hasher};

    let mut hasher = ::std::hash::DefaultHasher::new();
    region.hash(&mut hasher);
    input.availability_zone.hash(&mut hasher);
    input.availability_zone_id.hash(&mut hasher);
    input.instance_type.hash(&mut hasher);
    input.product_description.hash(&mut hasher);
    input.offering_class.hash(&mut hasher);
    input.offering_type.hash(&mut hasher);
    input.instance_tenancy.hash(&mut hasher);
    input.min_duration.hash(&mut hasher);
    input.max_duration.hash(&mut hasher);
    input.include_marketplace.hash(&mut hasher);
    input.max_instance_count.hash(&mut hasher);
    input.reserved_instances_offering_ids.hash(&mut hasher);
    for filter in input.filters.iter().flatten() {
        filter.name.hash(&mut hasher);
        filter.values.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}
