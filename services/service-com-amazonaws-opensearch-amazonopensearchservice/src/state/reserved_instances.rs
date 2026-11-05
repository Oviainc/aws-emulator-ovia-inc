// SPDX-License-Identifier: CC0-1.0

//! Reserved Instances, split out of [`crate::state`] unchanged: what an account bought
//! from one published offering, and how its payment and its term settle.

use super::*;

/// The values a new reservation carries that do NOT come from the offering it is
/// bought from — one argument, so [`ReservedInstanceRecord::from_offering`] does
/// not take a row of positional values.
#[derive(Debug, Clone)]
pub struct NewReservation {
    /// The `GUID` the reservation is addressed by, from
    /// [`new_reserved_instance_id`].
    pub reserved_instance_id: String,
    /// `ReservationName`: "the customer-specified identifier to track this
    /// reservation".
    pub reservation_name: String,
    /// `InstanceCount`: how many instances the purchase reserved.
    pub instance_count: i32,
    /// `BillingSubscriptionId`, from
    /// [`ComAmazonawsOpensearchAmazonopensearchserviceState::mint_billing_subscription_id`].
    pub billing_subscription_id: i64,
    /// The instant of the purchase, which is the reservation's `StartTime`.
    pub now: SystemTime,
}

/// One Reserved Instance, as this emulator persists it.
///
/// Every priced member is COPIED out of the catalog entry the reservation was
/// bought from, which is what makes a reservation a record of what WAS bought
/// rather than a second view of the live catalog.
#[derive(Debug, Clone)]
pub struct ReservedInstanceRecord {
    pub reservation_name: String,
    /// "The unique identifier for the reservation", which is this record's
    /// identity in the store.
    pub reserved_instance_id: String,
    pub billing_subscription_id: i64,
    /// The offering it was bought from.
    pub reserved_instance_offering_id: String,
    pub instance_type: Option<String>,
    /// "The date and time when the reservation was purchased."
    pub start_time: SystemTime,
    /// "The duration, in seconds, for which the OpenSearch instance is
    /// reserved", from the offering's term.
    pub duration_seconds: i32,
    pub fixed_price: Option<f64>,
    pub usage_price: Option<f64>,
    pub currency_code: Option<String>,
    /// "The number of OpenSearch instances that have been reserved."
    pub instance_count: i32,
    /// One of [`RESERVATION_STATE_PAYMENT_PENDING`],
    /// [`RESERVATION_STATE_ACTIVE`] or [`RESERVATION_STATE_RETIRED`].
    pub state: String,
    /// When the clock owes this reservation its move out of `payment-pending`;
    /// `None` once that move has been materialised.
    pub active_due: Option<SystemTime>,
    pub payment_option: Option<String>,
    /// "The recurring charge to your account, regardless of whether you create
    /// any domains using the Reserved Instance offering", from the offering.
    pub recurring_charges: Vec<RecurringCharge>,
}

impl ReservedInstanceRecord {
    /// The reservation `new` buys from `offering`.
    pub fn from_offering(offering: &ReservedInstanceOffering, new: NewReservation) -> Self {
        Self {
            reservation_name: new.reservation_name,
            reserved_instance_id: new.reserved_instance_id,
            billing_subscription_id: new.billing_subscription_id,
            reserved_instance_offering_id: offering
                .reserved_instance_offering_id
                .clone()
                .unwrap_or_default(),
            instance_type: offering.instance_type.clone(),
            start_time: new.now,
            duration_seconds: offering.duration,
            // The offering's prices are per reserved instance — "the upfront
            // fixed charge you will paid to purchase the specific Reserved
            // Instance offering" and "the hourly rate at which you're charged
            // for the domain using this Reserved Instance" — so a multi-instance
            // reservation reports the same per-instance prices beside its
            // `InstanceCount` rather than a total no member of this shape
            // documents.
            fixed_price: offering.fixed_price,
            usage_price: offering.usage_price,
            currency_code: offering.currency_code.clone(),
            instance_count: new.instance_count,
            state: RESERVATION_STATE_PAYMENT_PENDING.to_string(),
            active_due: Some(new.now + RESERVATION_SETTLEMENT),
            payment_option: offering.payment_option.clone(),
            recurring_charges: offering.recurring_charges.clone().unwrap_or_default(),
        }
    }

    /// Materialise every state change the clock owes this reservation — the only
    /// write a reporting operation makes, and only what was already owed at this
    /// instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        self.settle_payment(now);
        self.settle_term(now);
    }

    /// `payment-pending` becomes `active` once the payment's due instant has
    /// passed.
    pub(super) fn settle_payment(&mut self, now: SystemTime) {
        let Some(due) = self.active_due else {
            return;
        };
        if now < due {
            return;
        }
        self.state = RESERVATION_STATE_ACTIVE.to_string();
        self.active_due = None;
    }

    /// An `active` reservation becomes `retired` once the term it was bought for
    /// has elapsed: it runs for its offering's `Duration` from its `StartTime`,
    /// and that span is the offering's own rather than one the emulator chose.
    pub(super) fn settle_term(&mut self, now: SystemTime) {
        if self.state != RESERVATION_STATE_ACTIVE {
            return;
        }
        let Ok(seconds) = u64::try_from(self.duration_seconds) else {
            return;
        };
        if now < self.start_time + Duration::from_secs(seconds) {
            return;
        }
        self.state = RESERVATION_STATE_RETIRED.to_string();
    }

    /// This record as the modelled `ReservedInstance`.
    pub fn to_modeled(&self) -> ReservedInstance {
        ReservedInstance {
            reservation_name: Some(self.reservation_name.clone()),
            reserved_instance_id: Some(self.reserved_instance_id.clone()),
            billing_subscription_id: Some(self.billing_subscription_id),
            reserved_instance_offering_id: Some(self.reserved_instance_offering_id.clone()),
            instance_type: self.instance_type.clone(),
            start_time: Some(datetime(self.start_time)),
            duration: self.duration_seconds,
            fixed_price: self.fixed_price,
            usage_price: self.usage_price,
            currency_code: self.currency_code.clone(),
            instance_count: self.instance_count,
            state: Some(self.state.clone()),
            payment_option: self.payment_option.clone(),
            recurring_charges: Some(self.recurring_charges.clone()),
        }
    }
}

/// The refusal of a `GUID` that names no stored Reserved Instance.
///
/// A well-formed identifier that addresses no record names no resource, so this is
/// the modelled `ResourceNotFoundException` (HTTP 409) — the same refusal the
/// offering catalog answers an id it publishes nothing for with
/// ([`offering_not_found`]) — in the service's own wording for a reservation id that
/// resolves to nothing.
pub fn reserved_instance_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Reservation id not found".to_string()),
    }
}
