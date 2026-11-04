// SPDX-License-Identifier: CC0-1.0

//! What HOLDS A CHANGE UP: the work this store still owes a resource, and the
//! resources still associated with one — the facts a refusal is answered on.
//!
//! One file because every operation that refuses a change refuses it on one of these
//! facts, and two of them must never disagree about what "an order is still
//! outstanding on the Outpost" or "the site still has an Outpost" means. The
//! transitions that FINISH that work are [`super::lifecycle`]; the wording each
//! refusal is phrased in lives with the other refusal wordings.

use super::*;

impl ComAmazonawsOutpostsOutpostsolafserviceState {
    /// The capacity task that is still ACTIVE on `outpost_id`, if any.
    ///
    /// "You can have one active capacity task for each order and each Outpost", so
    /// this is what `StartCapacityTask` refuses a second one against. Callers settle
    /// first: a task the clock has already completed holds nothing up.
    pub fn active_capacity_task_on_outpost(&self, outpost_id: &str) -> Option<&CapacityTaskRecord> {
        self.capacity_tasks
            .values()
            .find(|task| task.outpost_id == outpost_id && task.is_active())
    }

    /// The capacity task of `region` that is still ACTIVE on `order_id`, if any. See
    /// [`Self::active_capacity_task_on_outpost`] — the same rule, stated for the
    /// other resource a task is one-at-a-time per.
    ///
    /// Scoped to `region` for the reason [`RegionalRecord`] gives: an order belongs
    /// to the region it was placed in, so only that region's tasks can be running on
    /// it, and a conflict is never reported about a task of an inventory the caller's
    /// endpoint does not serve. A task takes its region from its Outpost, which is
    /// what [`Self::capacity_tasks_in`] scopes by.
    pub fn active_capacity_task_on_order(
        &self,
        region: &str,
        order_id: &str,
    ) -> Option<&CapacityTaskRecord> {
        self.capacity_tasks_in(region)
            .find(|task| task.order_id.as_deref() == Some(order_id) && task.is_active())
    }

    /// The order of `region` still OUTSTANDING on `outpost_id`, if this store holds
    /// one.
    ///
    /// An order is outstanding until it is finished
    /// ([`OrderRecord::is_outstanding`]), so this is what the service still owes the
    /// Outpost. Stated once here so the two rules that read it — what blocks a
    /// decommission ([`Self::blocking_resource_types`]) and what blocks a delete
    /// ([`Self::unfinished_work_on_outpost`]) — cannot disagree about what "an order
    /// is still outstanding on the Outpost" means. Scoped to `region` for the reason
    /// [`RegionalRecord`] gives; callers settle first (see [`Self::settle_orders`]),
    /// so the statuses read here are the ones the orders hold at that instant.
    pub fn outstanding_order_on_outpost(
        &self,
        region: &str,
        outpost_id: &str,
    ) -> Option<&OrderRecord> {
        all_in_region(&self.orders, region).find(|record| {
            record.order.outpost_id.as_deref() == Some(outpost_id) && record.is_outstanding()
        })
    }

    /// The order of `region` still OUTSTANDING at `site_id`, if this store holds
    /// one.
    ///
    /// An order is placed at a site through an Outpost of that site
    /// ([`OrderRecord::site_id`]) and is outstanding until it is finished
    /// ([`OrderRecord::is_outstanding`]), so this is what the service still owes the
    /// SITE: the fact a change to that site is held for (see
    /// [`Self::site_order_hold`]). Stated beside
    /// [`Self::outstanding_order_on_outpost`], which is the same question asked of
    /// the other resource an order belongs to. Scoped to `region` for the reason
    /// [`RegionalRecord`] gives; callers settle first (see [`Self::settle_orders`]),
    /// so the statuses read here are the ones the orders hold at that instant.
    pub fn outstanding_order_at_site(&self, region: &str, site_id: &str) -> Option<&OrderRecord> {
        all_in_region(&self.orders, region)
            .find(|record| record.site_id.as_deref() == Some(site_id) && record.is_outstanding())
    }

    /// The order of `region` that has been PLACED at `site_id`, whatever status it
    /// now holds, if this store holds one.
    ///
    /// An order is placed at a site through an Outpost of that site
    /// ([`OrderRecord::site_id`]), and placing it is what happened whether it is
    /// still outstanding or has since completed or been cancelled — so unlike
    /// [`Self::outstanding_order_at_site`], which asks what the service still OWES
    /// the site, this asks whether an order was ever placed there at all: the fact
    /// that closes the "before you place an order at the site" window
    /// ([`Self::site_address_block`]). Scoped to `region` for the reason
    /// [`RegionalRecord`] gives.
    pub fn order_placed_at_site(&self, region: &str, site_id: &str) -> Option<&OrderRecord> {
        all_in_region(&self.orders, region)
            .find(|record| record.site_id.as_deref() == Some(site_id))
    }

    /// The order placed at `site_id` in `region` that a change to that site has to
    /// wait for, or `None` when no order there is still outstanding.
    ///
    /// Several of this service's operations document that a change to a site waits
    /// for an order in progress at it — a change to either of the site's addresses
    /// (`UpdateSiteAddress`) and a change to the rack properties of the site
    /// (`UpdateSiteRackPhysicalProperties`) — so what holds such a change up is
    /// stated ONCE here: the order still outstanding at the site
    /// ([`Self::outstanding_order_at_site`]), together with the resource the refusal
    /// names it by. Each operation then words its own message around
    /// [`SiteOrderHold::order`], and none of them can disagree about whether an order
    /// at the site holds a change up.
    ///
    /// Callers settle first (see [`Self::settle_orders`]), so the order statuses read
    /// here are the ones the orders hold at that instant.
    pub fn site_order_hold(&self, region: &str, site_id: &str) -> Option<SiteOrderHold> {
        let order = self.outstanding_order_at_site(region, site_id)?;
        // The order itself is a resource the modelled `ResourceType` names, so the
        // conflict an order causes names the order. An order record the store holds
        // is keyed by the id it reports, so the fallback is for a record that names
        // no order at all: the refusal still states the limit, naming no resource
        // rather than one it cannot identify.
        Some(match order.order.order_id.as_deref() {
            Some(order_id) => SiteOrderHold {
                resource: Some((RESOURCE_TYPE_ORDER, order_id.to_string())),
                order: format!("order {order_id}"),
            },
            None => SiteOrderHold {
                resource: None,
                order: "an order placed at it".to_string(),
            },
        })
    }

    /// The quote of `outpost_id` an order WAS submitted from, if this region's
    /// inventory holds one: its SECOND record of an order outstanding on that
    /// Outpost.
    ///
    /// A quote reaches [`QUOTE_STATUS_ORDER_SUBMITTED`] by an order being submitted
    /// from it, and its `SubmittedOrderId` names that order. See
    /// [`Self::outstanding_order_on_outpost`], which this stands beside; callers
    /// settle first (see [`Self::settle_quotes`]), so the quote statuses read here
    /// are the ones the quotes hold at that instant.
    ///
    /// A quote is only the SECOND record of the order: where this store holds the
    /// order the quote names, that order's own status is what says whether anything is
    /// still outstanding — an order that has finished
    /// ([`OrderRecord::is_outstanding`]) is finished on both records, and the quote it
    /// was submitted from stays `ORDER_SUBMITTED` because an order WAS submitted for
    /// it. Only a submitted order this store has no record of is answered for by the
    /// quote alone.
    pub fn quote_ordered_from_on_outpost(
        &self,
        region: &str,
        outpost_id: &str,
    ) -> Option<&QuoteRecord> {
        all_in_region(&self.quotes, region).find(|record| {
            if record.associated_outpost_id() != Some(outpost_id)
                || !record.records_submitted_order()
            {
                return false;
            }
            record
                .quote
                .submitted_order_id
                .as_deref()
                .and_then(|order_id| self.orders.get(order_id))
                .is_none_or(OrderRecord::is_outstanding)
        })
    }

    /// The unfinished work this store still holds on `outpost_id`, or `None` when
    /// the Outpost has none and can be removed.
    ///
    /// Work is unfinished while the service is still to carry it out: a capacity
    /// task of the Outpost in a non-terminal status
    /// ([`CapacityTaskRecord::is_active`]), and an order of the Outpost that is
    /// still outstanding — either the order record itself
    /// ([`Self::outstanding_order_on_outpost`]) or a quote an order was submitted
    /// from ([`Self::quote_ordered_from_on_outpost`]), the two records this store
    /// keeps of one. Removing the Outpost would destroy the first and orphan the
    /// second, neither of which anything cancelled or completed — the inconsistent
    /// state the modelled `ConflictException` exists for, so a delete answers it
    /// rather than reporting a success that lost the work.
    ///
    /// What is NOT unfinished work, and so holds nothing up:
    ///
    /// * work in a TERMINAL status — a completed, failed or cancelled capacity task
    ///   ([`CAPACITY_TASK_TERMINAL`]), an order that has finished
    ///   ([`ORDER_STATUS_SETTLED`]), a quote no order was submitted from or one
    ///   that has expired: there is nothing left for the service to carry out.
    /// * the members of the Outpost's OWN record, which are attributes of the
    ///   Outpost rather than work on it and go away with it: its private
    ///   connectivity configuration, its renewal pricing, its installed assets and
    ///   the instances recorded as running on it. None has an identity outside the
    ///   Outpost, so removing the Outpost leaves nothing of them behind to dangle.
    /// * an installation connection started for one of those assets, which is a
    ///   tunnel to hardware of this Outpost rather than work the service owes on it,
    ///   and which the removal takes with the asset it installs
    ///   ([`Self::remove_outpost`]).
    ///
    /// ONE piece of work, since the modelled `ConflictException` names one
    /// resource: an Outpost that holds both a running task and an outstanding order
    /// reports the task, so a caller that lets the task finish and retries then
    /// meets the order. Which is reported first is this emulator's order of
    /// looking, not a statement about the work.
    pub fn unfinished_work_on_outpost(
        &self,
        region: &str,
        outpost_id: &str,
    ) -> Option<UnfinishedWork> {
        if let Some(active) = self.active_capacity_task_on_outpost(outpost_id) {
            // The modelled `ResourceType` names only `OUTPOST` and `ORDER`, so the
            // resource a caller is told the conflict is about is the Outpost the
            // task is running on; the message names the task itself, which is what
            // the caller polls.
            return Some(UnfinishedWork {
                resource_type: RESOURCE_TYPE_OUTPOST,
                resource_id: outpost_id.to_string(),
                work: format!("capacity task {}", active.capacity_task_id),
            });
        }
        if let Some(outstanding) = self.outstanding_order_on_outpost(region, outpost_id) {
            // The order itself is a resource the modelled `ResourceType` names, so
            // a conflict an order causes names the order.
            return Some(match outstanding.order.order_id.as_deref() {
                Some(order_id) => UnfinishedWork {
                    resource_type: RESOURCE_TYPE_ORDER,
                    resource_id: order_id.to_string(),
                    work: format!("order {order_id}"),
                },
                None => UnfinishedWork {
                    resource_type: RESOURCE_TYPE_OUTPOST,
                    resource_id: outpost_id.to_string(),
                    work: "an outstanding order".to_string(),
                },
            });
        }
        let order = self.quote_ordered_from_on_outpost(region, outpost_id)?;
        Some(match order.quote.submitted_order_id.as_deref() {
            // As above: the order the quote was ordered from is what the conflict is
            // about, and the quote reports its id.
            Some(order_id) => UnfinishedWork {
                resource_type: RESOURCE_TYPE_ORDER,
                resource_id: order_id.to_string(),
                work: format!("order {order_id}"),
            },
            // A submitted order with no recorded id leaves the Outpost as the one
            // resource the caller can be given, and the quote as the only name the
            // outstanding order has.
            None => UnfinishedWork {
                resource_type: RESOURCE_TYPE_OUTPOST,
                resource_id: outpost_id.to_string(),
                work: match order.quote.quote_id.as_deref() {
                    Some(quote_id) => format!("order submitted from quote {quote_id}"),
                    None => "a submitted order".to_string(),
                },
            },
        })
    }

    /// The Outpost that still belongs to `site_id` in `region`, if the store holds
    /// one.
    ///
    /// An Outpost names the site it lives at, so what this answers is whether that
    /// site still has hardware at it: the fact `DeleteSite` refuses a delete on, and
    /// the one a change to the site's OPERATING address waits on once an order has
    /// been placed at the site ([`Self::site_address_block`]). Stated once here so
    /// the two cannot disagree about what "the site still has an Outpost" means.
    pub fn outpost_at_site(&self, region: &str, site_id: &str) -> Option<&OutpostRecord> {
        self.outposts_in(region)
            .find(|record| record.outpost.site_id.as_deref() == Some(site_id))
    }

    /// What holds a change to the `kind` address of `site_id` in `region` up, if
    /// anything — the fact `UpdateSiteAddress` answers its modelled
    /// `ConflictException` on.
    ///
    /// AWS documents TWO limits on when a site address may change, and this states
    /// both, in the order a caller meets them:
    ///
    /// * "You can't update a site address if there is an order in progress. You must
    ///   wait for the order to complete or cancel the order." — on an address of
    ///   EITHER kind, while an order placed at the site is still outstanding
    ///   ([`Self::outstanding_order_at_site`]). It is the narrower limit of the two
    ///   and the one with something for the caller to do, so it is reported first
    ///   where both hold.
    /// * "You can update the operating address before you place an order at the
    ///   site, or after all Outposts that belong to the site have been deactivated."
    ///   — on the OPERATING address only, and only OUTSIDE both windows that
    ///   sentence permits: an order has been placed at the site
    ///   ([`Self::order_placed_at_site`]), so the first window has closed, AND an
    ///   Outpost still belongs to the site ([`Self::outpost_at_site`]), so the
    ///   second has not opened. The operating address is where the site itself is,
    ///   so hardware ordered for it depends on it; a site no order was ever placed
    ///   at is inside the first window however many Outposts belong to it, and the
    ///   shipping address is where that hardware ships to and no Outpost at the site
    ///   holds it up.
    ///
    /// Callers settle first (see [`Self::settle_orders`]), so the order statuses read
    /// here are the ones the orders hold at that instant.
    pub fn site_address_block(
        &self,
        region: &str,
        site_id: &str,
        kind: SiteAddressKind,
    ) -> Option<SiteAddressBlock> {
        if let Some(hold) = self.site_order_hold(region, site_id) {
            return Some(SiteAddressBlock {
                resource: hold.resource,
                message: site_address_order_conflict(site_id, &hold.order),
            });
        }
        if kind != SiteAddressKind::Operating {
            return None;
        }
        // The first window is open until an order is placed AT THE SITE, so an
        // Outpost at a site nobody has ordered hardware for holds nothing up.
        self.order_placed_at_site(region, site_id)?;
        let outpost_id = self
            .outpost_at_site(region, site_id)
            .and_then(|record| record.outpost.outpost_id.clone())?;
        Some(SiteAddressBlock {
            message: site_address_conflict(site_id, &outpost_id),
            resource: Some((RESOURCE_TYPE_OUTPOST, outpost_id)),
        })
    }

    /// Every `BlockingResourceType` this store observes still associated with
    /// `outpost_id`, deduplicated and in one stable order — what a `BLOCKED`
    /// `StartOutpostDecommission` reports in `BlockingResourceTypes`.
    ///
    /// The modelled enum names seven kinds of resource (see
    /// [`crate::ops::types::shared_types::BLOCKING_RESOURCE_TYPE_VALUES`]). Two of
    /// them are resources THIS service's store can hold. The first:
    ///
    /// * [`BLOCKING_OUTPOST_ORDER_CANCELLABLE`] — an Outpost order still outstanding
    ///   on the Outpost. This store holds two records of such an order: the order
    ///   itself, which `CreateOrder` persists against the Outpost it names and which
    ///   is outstanding until it is finished ([`OrderRecord::is_outstanding`]), and a
    ///   quote of this Outpost that an order was submitted from — the modelled
    ///   `QuoteStatus` [`QUOTE_STATUS_ORDER_SUBMITTED`], whose `SubmittedOrderId`
    ///   names it. An order is outstanding until it is finished, so an order the store
    ///   still holds outstanding is one that can still be cancelled, and a
    ///   decommission has to wait for it.
    ///
    /// A second is a resource it CAN hold:
    ///
    /// * [`BLOCKING_EC2_INSTANCE`] — an Amazon EC2 instance still running on the
    ///   Outpost, which AWS requires to be gone before an Outpost can be
    ///   decommissioned. The store's record of such an instance is
    ///   [`OutpostRecord::instances`]; an Outpost nobody has launched an instance on
    ///   holds none, so it reports none. The instances a capacity task names in
    ///   `InstancesToExclude` are the caller's own input and never a record of a
    ///   running instance.
    ///
    /// The other five name resources that live nowhere in this store, so reporting one
    /// would be asserting a resource this emulator has no record of:
    ///
    /// * `OUTPOST_RAM_SHARE`, `LGW_ROUTING_DOMAIN`, `LGW_ROUTE_TABLE`,
    ///   `LGW_VIRTUAL_INTERFACE_GROUP` — resource shares and local gateway resources
    ///   belong to other services' APIs; nothing in this service creates or holds
    ///   one.
    /// * `OUTPOST_ORDER_INTERVENTION_REQUIRED` — an order condition nothing in this
    ///   slice produces: no order the store knows about ever fails or stalls, so the
    ///   emulator never claims one needs intervention.
    ///
    /// Callers settle first (see [`Self::settle_quotes`] and [`Self::settle_orders`]),
    /// so the statuses read here are the ones the quotes and orders hold at that
    /// instant.
    pub fn blocking_resource_types(&self, region: &str, outpost_id: &str) -> Vec<String> {
        let mut blocking: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        if self.outstanding_order_on_outpost(region, outpost_id).is_some()
            || self
                .quote_ordered_from_on_outpost(region, outpost_id)
                .is_some()
        {
            blocking.insert(BLOCKING_OUTPOST_ORDER_CANCELLABLE.to_string());
        }
        let instance_running = self
            .outpost(region, outpost_id)
            .is_some_and(|record| !record.instances.is_empty());
        if instance_running {
            blocking.insert(BLOCKING_EC2_INSTANCE.to_string());
        }
        blocking.into_iter().collect()
    }
}
