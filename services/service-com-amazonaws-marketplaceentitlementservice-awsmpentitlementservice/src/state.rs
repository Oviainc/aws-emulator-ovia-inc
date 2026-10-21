// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplaceentitlementservice-awsmpentitlementservice service.
//!
//! The only resource this service models is the `Entitlement`, and the model declares
//! no operation that creates one: in AWS an entitlement appears when a buyer subscribes
//! to a listing, entirely out of band of this API. So the store below is populated
//! through [`seed_entitlements`] — the emulator's fixture/admin seam standing in for
//! that out-of-band creation — and `GetEntitlements` only ever reads it.

/// The capacity an entitlement grants, mirroring the modeled `EntitlementValue`
/// structure: AWS populates exactly ONE of the four members, the one matching the
/// entitlement's type, and leaves the rest unset.
///
/// The fields stay public so a fixture can read them back and `GetEntitlements` can
/// project them onto the wire; the four constructors below are the supported way to
/// build one and each sets exactly one member. A fixture that writes the fields directly
/// can check the invariant with [`StoredEntitlementValue::populated_count`] \u2014 nothing
/// here enforces it structurally.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoredEntitlementValue {
    pub integer_value: Option<i32>,
    pub double_value: Option<f64>,
    pub boolean_value: Option<bool>,
    pub string_value: Option<String>,
}

impl StoredEntitlementValue {
    /// An integer-typed entitlement value.
    pub fn integer(value: i32) -> Self {
        Self {
            integer_value: Some(value),
            ..Self::default()
        }
    }

    /// A double-typed entitlement value.
    pub fn double(value: f64) -> Self {
        Self {
            double_value: Some(value),
            ..Self::default()
        }
    }

    /// A boolean-typed entitlement value.
    pub fn boolean(value: bool) -> Self {
        Self {
            boolean_value: Some(value),
            ..Self::default()
        }
    }

    /// A string-typed entitlement value.
    pub fn string(value: impl Into<String>) -> Self {
        Self {
            string_value: Some(value.into()),
            ..Self::default()
        }
    }

    /// How many of the four members are set. Exactly one, for any value the
    /// constructors above built; a fixture that writes the struct fields directly can
    /// assert on this.
    pub fn populated_count(&self) -> usize {
        usize::from(self.integer_value.is_some())
            + usize::from(self.double_value.is_some())
            + usize::from(self.boolean_value.is_some())
            + usize::from(self.string_value.is_some())
    }
}

/// One stored entitlement. `product_code` is not optional: it is the key
/// `GetEntitlements` selects on, so a record without one could never be returned.
/// Every other member mirrors the modeled `Entitlement` and is optional there.
#[derive(Debug, Clone, Default)]
pub struct StoredEntitlement {
    pub product_code: String,
    pub dimension: Option<String>,
    pub customer_identifier: Option<String>,
    pub customer_aws_account_id: Option<String>,
    pub value: Option<StoredEntitlementValue>,
    pub expiration_date: Option<::aws_smithy_types::DateTime>,
    pub license_arn: Option<String>,
}

impl StoredEntitlement {
    /// A record for `product_code` with every optional member unset.
    pub fn new(product_code: impl Into<String>) -> Self {
        Self {
            product_code: product_code.into(),
            ..Self::default()
        }
    }

    /// Set the dimension (the product's capacity category).
    #[must_use]
    pub fn with_dimension(mut self, dimension: impl Into<String>) -> Self {
        self.dimension = Some(dimension.into());
        self
    }

    /// Set the customer identifier handle.
    #[must_use]
    pub fn with_customer_identifier(mut self, customer_identifier: impl Into<String>) -> Self {
        self.customer_identifier = Some(customer_identifier.into());
        self
    }

    /// Set the buyer's AWS account id.
    #[must_use]
    pub fn with_customer_aws_account_id(mut self, account_id: impl Into<String>) -> Self {
        self.customer_aws_account_id = Some(account_id.into());
        self
    }

    /// Set the entitled capacity.
    #[must_use]
    pub fn with_value(mut self, value: StoredEntitlementValue) -> Self {
        self.value = Some(value);
        self
    }

    /// Set the expiration date.
    #[must_use]
    pub fn with_expiration_date(mut self, expiration_date: ::aws_smithy_types::DateTime) -> Self {
        self.expiration_date = Some(expiration_date);
        self
    }

    /// Set the granted license's ARN.
    #[must_use]
    pub fn with_license_arn(mut self, license_arn: impl Into<String>) -> Self {
        self.license_arn = Some(license_arn.into());
        self
    }
}

/// Per-service state for com-amazonaws-marketplaceentitlementservice-awsmpentitlementservice. A handler reaches it through [`service_state`].
///
/// `entitlements` is a `Vec` rather than a map keyed by product code because the index
/// of a record in it IS the pagination cursor: insertion order is the service's stable
/// ordering, so a `NextToken` issued for one page resumes at the same record on the
/// next call.
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplaceentitlementserviceAwsmpentitlementserviceState {
    pub entitlements: Vec<StoredEntitlement>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplaceentitlementserviceAwsmpentitlementserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplaceentitlementserviceAwsmpentitlementserviceState>)
}

/// Append entitlements to this service's store, in the order given.
///
/// This is the out-of-band creation seam: AWS Marketplace grants an entitlement when a
/// buyer subscribes, and this API models no operation that does so, so a fixture (or an
/// admin path) hands the records to the emulator here instead. Appending keeps existing
/// cursors valid — nothing already stored moves.
///
/// Answers `None` when `engine` is not the shared [`::engine::InMemoryEngine`] (so there
/// is no store to seed), otherwise the number of records appended.
pub fn seed_entitlements<Eng, I>(engine: &Eng, records: I) -> Option<usize>
where
    Eng: Send + Sync + 'static,
    I: IntoIterator<Item = StoredEntitlement>,
{
    let state = service_state(engine)?;
    let mut store = state.lock();
    let before = store.entitlements.len();
    store.entitlements.extend(records);
    Some(store.entitlements.len() - before)
}
