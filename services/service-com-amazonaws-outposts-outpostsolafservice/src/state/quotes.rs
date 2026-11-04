// SPDX-License-Identifier: CC0-1.0

//! What this service QUOTES: the pricing options a renewal and a quote are answered
//! with, and the checks that must pass before an order can be submitted from a quote.
//!
//! One file because every figure here comes from the one subscription-pricing rule
//! applied to capacity the store already holds, so a renewal's price and the price of
//! a quote for the same capacity can never disagree. The store itself is
//! [`super::store`]; the capacity these figures are applied to is
//! [`super::capacity`].

use super::*;

impl ComAmazonawsOutpostsOutpostsolafserviceState {
    /// How many renewal contracts this store holds, across every Outpost of every
    /// region.
    ///
    /// A renewal is never released — the Outpost it belongs to is what takes it away
    /// — so what `CreateRenewal` grows is capped by the store's own entry ceiling
    /// ([`MAX_RESOURCES`]), and that cap is on the whole store rather than on one
    /// Outpost's share of it.
    pub fn renewals_held(&self) -> usize {
        self.outposts
            .values()
            .map(|record| record.renewals.len())
            .sum()
    }

    /// Every renewal pricing option for the Outpost `outpost_id` names in `region`:
    /// one priced SUBSCRIPTION option per (`PaymentOption`, `PaymentTerm`)
    /// combination the model declares — three of each, so the nine the modelled
    /// `PricingOptionList` maximum (`length(0..9)`) allows — or none at all for an
    /// Outpost with no subscribed hardware to renew.
    ///
    /// Each option is the one subscription on the Outpost's own hardware, quoted
    /// under that combination by [`Self::renewal_price`] — so every figure follows
    /// from what is persisted for the Outpost, and a read answers the same figures
    /// as the next one.
    pub fn renewal_pricing(&self, region: &str, outpost_id: &str) -> Vec<PricingOption> {
        // The tables may only ever name values the model declares, and must name
        // every one of them, so a model that adds or renames a payment option or
        // term trips this in every debug build instead of letting a renewal be
        // priced under fewer combinations than the service offers.
        debug_assert!(
            PAYMENT_OPTION_UPFRONT_SHARE.len() == crate::ops::types::PAYMENT_OPTION_VALUES.len()
                && PAYMENT_OPTION_UPFRONT_SHARE.iter().all(|(option, _)| {
                    crate::ops::types::PAYMENT_OPTION_VALUES.contains(option)
                })
                && PAYMENT_TERM_MONTHS.len() == crate::ops::types::PAYMENT_TERM_VALUES.len()
                && PAYMENT_TERM_MONTHS
                    .iter()
                    .all(|(term, _)| crate::ops::types::PAYMENT_TERM_VALUES.contains(term)),
            "every modelled PaymentOption has an upfront share and every modelled PaymentTerm a \
             length in months"
        );
        // The combinations are walked in the order the model declares the two value
        // sets in, so one Outpost's options are reported in the same order on every
        // read and in every process. Each one is priced by [`Self::renewal_price`],
        // the ONE rule this service prices a renewal by — so what the pricing read
        // reports and what a renewal is actually bought at cannot disagree.
        let options: Vec<PricingOption> = crate::ops::types::PAYMENT_OPTION_VALUES
            .iter()
            .flat_map(|payment_option| {
                crate::ops::types::PAYMENT_TERM_VALUES
                    .iter()
                    .map(move |payment_term| (payment_option, payment_term))
            })
            .filter_map(|(payment_option, payment_term)| {
                Some(PricingOption {
                    pricing_type: Some(QUOTE_PRICING_TYPE_SUBSCRIPTION.to_string()),
                    subscription_pricing_details: Some(self.renewal_price(
                        region,
                        outpost_id,
                        payment_option,
                        payment_term,
                    )?),
                })
            })
            .collect();
        debug_assert!(
            options.len() <= PRICING_OPTION_MAX,
            "a renewal is priced under at most the options PricingOptionList allows"
        );
        options
    }

    /// What a renewal of the subscription on the Outpost `outpost_id` names in
    /// `region`, bought under `payment_option` and `payment_term`, costs — or `None`
    /// when there is nothing to price: an Outpost with no subscribed hardware, or a
    /// payment option or term this service holds no rule for.
    ///
    /// The capacity the Outpost's orders subscribe to
    /// ([`Self::subscribed_capacity_vcpus`]) is what a renewal covers and the only
    /// thing this store knows about its size, so that capacity is what the one
    /// pricing rule ([`subscription_price`]) is applied to — which is why the pricing
    /// read ([`Self::renewal_pricing`]) and the renewal a caller actually buys quote
    /// the same figures.
    pub fn renewal_price(
        &self,
        region: &str,
        outpost_id: &str,
        payment_option: &str,
        payment_term: &str,
    ) -> Option<SubscriptionPricingDetails> {
        subscription_price(
            self.subscribed_capacity_vcpus(region, outpost_id) as f64,
            payment_option,
            payment_term,
        )
    }

    /// The configuration-and-pricing options this service answers a quote for
    /// `capacities` with, priced under the payment options and terms that quote
    /// requests.
    ///
    /// "A quote provides pricing and configuration options based on the requested
    /// capacity", and `CreateOrder`'s `QuoteOptionIdentifier` can only ever be
    /// obtained from one of them, so a quote this service answers carries an option a
    /// caller can order from. The request describes ONE configuration — the capacity
    /// it asks for — so one option is derived for it, well within what the modelled
    /// `QuoteOptionList` ([`QUOTE_OPTION_MAX`]) allows, and it reports the members
    /// that follow from the quote:
    ///
    /// * `QuoteOptionIdentifier` (`^oqo-[a-f0-9]{17}$`), minted here and persisted
    ///   WITH the quote, so every later read of that quote names the same option and
    ///   an order can be submitted against it.
    /// * `Capacities`, "the capacities included in this quote option": the
    ///   configuration is the one the quote asks for, so it provides exactly that
    ///   capacity.
    /// * `PricingOptions`, one per payment combination the quote is priced under
    ///   ([`Self::quote_pricing`]).
    ///
    /// `CapacitySummary` and `Specifications` are absent. The first is a summary of
    /// the capacity that EXISTS against the capacity the option would leave, and this
    /// store holds no record of an Outpost's capacity per capacity unit — EC2
    /// instances per instance type, EBS and S3 volume — so a summary of it would be
    /// the request read back as if it were the Outpost's. The second is the physical
    /// rack or server a configuration is deployed on, which AWS picks when it builds
    /// the order and this store records for no quote.
    pub fn quote_options(
        &mut self,
        capacities: &[QuoteCapacity],
        payment_options: Option<&[String]>,
        payment_terms: Option<&[String]>,
    ) -> Vec<QuoteOption> {
        let pricing_options = self.quote_pricing(capacities, payment_options, payment_terms);
        let (quote_option_identifier, _) = self.mint_id(QUOTE_OPTION_ID_PREFIX);
        let options = vec![QuoteOption {
            quote_option_identifier: Some(quote_option_identifier),
            capacities: Some(capacities.to_vec()),
            capacity_summary: None,
            specifications: None,
            pricing_options: Some(pricing_options),
        }];
        debug_assert!(
            options.len() <= QUOTE_OPTION_MAX,
            "a quote reports at most the options QuoteOptionList allows"
        );
        options
    }

    /// Every pricing option a quote option providing `capacities` is reported under:
    /// one priced SUBSCRIPTION option per combination of the payment options and
    /// payment terms the quote requests ([`requested_or_modelled`], so a quote that
    /// requested neither is priced under every modelled value).
    ///
    /// The combinations are walked in the order the two value sets name them, so one
    /// configuration's options are reported in the same order on every read and in
    /// every process. Each is priced by [`subscription_price`] — the ONE rule this
    /// service prices a subscription by — on the EC2 capacity the option provides
    /// ([`Self::quoted_capacity_vcpus`]), so what a quote quotes and what a renewal
    /// of the same capacity costs cannot disagree. A combination there is nothing to
    /// price under is left out rather than reported at a figure the emulator invented
    /// — including one whose figures are beyond what a modelled price member can
    /// carry, for the reason [`subscription_price`] states.
    fn quote_pricing(
        &self,
        capacities: &[QuoteCapacity],
        payment_options: Option<&[String]>,
        payment_terms: Option<&[String]>,
    ) -> Vec<PricingOption> {
        let vcpus = self.quoted_capacity_vcpus(capacities);
        let options =
            requested_or_modelled(payment_options, crate::ops::types::PAYMENT_OPTION_VALUES);
        let terms = requested_or_modelled(payment_terms, crate::ops::types::PAYMENT_TERM_VALUES);
        let priced: Vec<PricingOption> = options
            .iter()
            .flat_map(|payment_option| {
                terms
                    .iter()
                    .map(move |payment_term| (*payment_option, *payment_term))
            })
            .filter_map(|(payment_option, payment_term)| {
                Some(PricingOption {
                    pricing_type: Some(QUOTE_PRICING_TYPE_SUBSCRIPTION.to_string()),
                    subscription_pricing_details: Some(subscription_price(
                        vcpus,
                        payment_option,
                        payment_term,
                    )?),
                })
            })
            .collect();
        debug_assert!(
            priced.len() <= PRICING_OPTION_MAX,
            "a quote option is priced under at most the options PricingOptionList allows"
        );
        priced
    }

    /// How many vCPUs of EC2 capacity the configuration `capacities` describes
    /// carries, as this service's published catalog figures it: for every EC2
    /// capacity, the default vCPUs published for the instance type its `Unit` names
    /// ([`Self::published_vcpus`]) times the quantity of instances asked for.
    ///
    /// "For Amazon EC2, this is the instance type (for example, `c5.24xlarge`)", as
    /// the model documents `QuoteCapacity$Unit`, and "for Amazon EC2, the quantity is
    /// the number of additional instances to add to the Outpost" — so this is the
    /// capacity of a quoted configuration, read off the same catalog
    /// `ListOrderableInstanceTypes` publishes. A capacity measured in storage rather
    /// than in instances (the modelled `EBS` and `S3` types), one naming a unit the
    /// catalog publishes no instance type for, and one asking for no instances at all
    /// contribute nothing: capacity this service publishes no figure for is not
    /// capacity the emulator can price, and never one it invents.
    ///
    /// Totalled in double precision, which is wide enough for every total the
    /// modelled request can describe — the largest `QuoteCapacity$Quantity` its
    /// single-precision type carries, times the largest vCPU figure the catalog
    /// publishes, over the whole `QuoteCapacityList` — so the capacity a quote is
    /// priced on is the capacity it asked for, and never a value the arithmetic
    /// saturated to.
    pub fn quoted_capacity_vcpus(&self, capacities: &[QuoteCapacity]) -> f64 {
        capacities
            .iter()
            .filter(|capacity| {
                capacity.quote_capacity_type.as_deref() == Some(QUOTE_CAPACITY_TYPE_EC2)
            })
            .filter_map(|capacity| {
                let vcpus = self.published_vcpus(capacity.unit.as_deref()?)?;
                let quantity = capacity
                    .quantity
                    .filter(|quantity| quantity.is_finite() && *quantity > 0.0)?;
                Some(f64::from(quantity) * f64::from(vcpus))
            })
            .sum()
    }

    /// The requirements that must be met before an order can be submitted for a quote
    /// of `country_code` associated with the Outpost `outpost_id` names in `region`
    /// (or with no Outpost at all).
    ///
    /// "The requirements that must be met before an order can be submitted for the
    /// quote", and the modelled `OrderingRequirementType` names the checks AWS runs
    /// (see [`crate::ops::types::ORDERING_REQUIREMENT_TYPE_VALUES`]). A check is
    /// reported only where this store holds the records to decide it, and a reported
    /// one carries the outcome those records give at the instant the quote is priced:
    ///
    /// * [`ORDERING_REQUIREMENT_OUTPOST_ID_MISSING`] — whether the quote names an
    ///   Outpost, which is the association `CreateQuote` and `UpdateQuote` record.
    /// * [`ORDERING_REQUIREMENT_MAXIMUM_ALLOWED_ORDERS`] — whether this store can
    ///   still take an order, which is the ceiling `CreateOrder` refuses at.
    /// * [`ORDERING_REQUIREMENT_OUTPOST_ACTIVE`] — whether that Outpost has settled
    ///   in its terminal status. Callers materialise the lifecycle transition the
    ///   clock owes first (see [`Self::settle_outposts`]), so the status read here is
    ///   the one the Outpost holds at that same instant.
    /// * [`ORDERING_REQUIREMENT_OPERATING_ADDRESS`] and
    ///   [`ORDERING_REQUIREMENT_SHIPPING_ADDRESS`] — whether the site that Outpost
    ///   stands on records each of the two addresses an order is placed against.
    /// * [`ORDERING_REQUIREMENT_COUNTRY_CODE_MISMATCH`] — whether the country the
    ///   quote is for is the country of that site, which only a site with a recorded
    ///   operating address answers.
    ///
    /// Every outcome follows from a record this store holds, so a check is never
    /// reported as passed because nothing was looked at — and the checks that depend
    /// on an Outpost are reported only for a quote that names one, because a quote
    /// associated with no Outpost gives them nothing to look at.
    pub fn ordering_requirements(
        &self,
        region: &str,
        country_code: &str,
        outpost_id: Option<&str>,
    ) -> Vec<OrderingRequirement> {
        let mut requirements = vec![match outpost_id {
            Some(outpost_id) => ordering_requirement(
                ORDERING_REQUIREMENT_OUTPOST_ID_MISSING,
                ORDERING_REQUIREMENT_PASS,
                format!("the quote is associated with Outpost {outpost_id}"),
            ),
            None => ordering_requirement(
                ORDERING_REQUIREMENT_OUTPOST_ID_MISSING,
                ORDERING_REQUIREMENT_FAIL,
                "the quote is associated with no Outpost".to_string(),
            ),
        }];
        let placed = self.orders.len();
        requirements.push(ordering_requirement(
            ORDERING_REQUIREMENT_MAXIMUM_ALLOWED_ORDERS,
            pass_or_fail(placed < MAX_RESOURCES),
            format!("{placed} of the {MAX_RESOURCES} orders this service holds have been placed"),
        ));
        if let Some(record) = outpost_id.and_then(|outpost_id| self.outpost(region, outpost_id)) {
            if let (Some(outpost_id), Some(status)) = (
                record.outpost.outpost_id.as_deref(),
                record.outpost.life_cycle_status.as_deref(),
            ) {
                requirements.push(ordering_requirement(
                    ORDERING_REQUIREMENT_OUTPOST_ACTIVE,
                    pass_or_fail(status == LIFE_CYCLE_STATUS_ACTIVE),
                    format!("Outpost {outpost_id} is {status}"),
                ));
            }
            let site = record
                .outpost
                .site_id
                .as_deref()
                .and_then(|site_id| Some((site_id, self.site(region, site_id)?)));
            if let Some((site_id, site)) = site {
                for (check, kind, address) in [
                    (
                        ORDERING_REQUIREMENT_OPERATING_ADDRESS,
                        SiteAddressKind::Operating,
                        "operating address",
                    ),
                    (
                        ORDERING_REQUIREMENT_SHIPPING_ADDRESS,
                        SiteAddressKind::Shipping,
                        "shipping address",
                    ),
                ] {
                    let recorded = site.address(kind).is_some();
                    requirements.push(ordering_requirement(
                        check,
                        pass_or_fail(recorded),
                        match recorded {
                            true => format!("site {site_id} has a recorded {address}"),
                            false => format!("site {site_id} has no recorded {address}"),
                        },
                    ));
                }
                if let Some(country) = site
                    .address(SiteAddressKind::Operating)
                    .map(|address| address.country_code.as_str())
                {
                    requirements.push(ordering_requirement(
                        ORDERING_REQUIREMENT_COUNTRY_CODE_MISMATCH,
                        pass_or_fail(country == country_code),
                        match country == country_code {
                            true => format!(
                                "the quote's country code {country_code} is the country of site \
                                 {site_id}"
                            ),
                            false => format!(
                                "the quote's country code {country_code} is not {country}, the \
                                 country of site {site_id}"
                            ),
                        },
                    ));
                }
            }
        }
        debug_assert!(
            requirements.len() <= ORDERING_REQUIREMENT_MAX
                && requirements.iter().all(|requirement| {
                    requirement
                        .ordering_requirement_type
                        .as_deref()
                        .is_some_and(|check| {
                            crate::ops::types::ORDERING_REQUIREMENT_TYPE_VALUES.contains(&check)
                        })
                        && requirement.status.as_deref().is_some_and(|status| {
                            crate::ops::types::ORDERING_REQUIREMENT_STATUS_VALUES.contains(&status)
                        })
                }),
            "every requirement reported names a modelled check with a modelled status, and a \
             quote reports at most the requirements OrderingRequirementList allows"
        );
        requirements
    }
}
