// SPDX-License-Identifier: CC0-1.0

//! This account's EXTENSION PUBLISHER registration: the record `RegisterPublisher` writes
//! and `DescribePublisher` reads back.
//!
//! Registering is what lets an account publish public extensions — "registers your account
//! as a publisher of public extensions in the CloudFormation registry" — and it produces
//! ONE identifier for the account: "this publisher ID applies to your account in all Amazon
//! Web Services Regions". So the store holds at most one of these records, outside the
//! per-Region namespacing everything else in this service is read through, and a read of it
//! answers the same record whichever Region the request was signed for.
//!
//! ## What a registration is verified by
//!
//! A registration states two things. The first is that the caller accepts the terms and
//! conditions ("you must accept the terms and conditions in order to register to publish
//! public extensions to the CloudFormation registry"), which is the request's own
//! `AcceptTermsAndConditions` and is not part of the record: a registration that does not
//! accept them is refused, so a record only ever exists for one that did.
//!
//! The second is the identity the publisher was verified by, and that IS part of the
//! record: a request carrying a `ConnectionArn` was verified through the Bitbucket or
//! GitHub account that connection is to ("if you are using a Bitbucket or GitHub account
//! for identity verification, the ARN for your connection to that account"), and one
//! carrying none was verified through the account's own Amazon Web Services Marketplace
//! account, the other identity an account can be verified by. [`StoredPublisher::connection_arn`]
//! is therefore what [`StoredPublisher::identity_provider`] is computed from.
//!
//! ## What an account has to BE before it can register
//!
//! Accepting the terms is a property of the REQUEST; being allowed to publish at all is a
//! property of the ACCOUNT. CloudFormation registers a publisher only for an account that is
//! already enrolled as an Amazon Web Services Marketplace seller, and that enrollment is
//! made outside this API — no operation performs it and none reports it — so it is a property
//! of the emulated account rather than a record a call could write
//! ([`MarketplaceSellerEnrollment`]). An account that is not enrolled is refused with the
//! registry's own modelled `CFNRegistryException` and [`NOT_A_MARKETPLACE_SELLER`], writing
//! nothing: that refusal is the only signal the API gives that the enrollment is missing, and
//! it is what keeps every operation asking "is this account a publisher"
//! (`DescribePublisher`, `TestType`, `PublishType`) answering the not-a-publisher refusal.

/// The sentence a publisher registration from an account that is not an enrolled Amazon Web
/// Services Marketplace seller is refused with.
///
/// Entirely FIXED text: no part of it comes from the request or from the store, like the
/// registry's other account-level refusals.
pub const NOT_A_MARKETPLACE_SELLER: &str =
    "You must be a valid AWS Marketplace Seller to be registered as a publisher";

/// The environment variable an operator uses to say that this emulated account IS enrolled
/// as an Amazon Web Services Marketplace seller, read once when this service's store is
/// created.
///
/// The enrollment is a console-only step outside this API, so there is no operation that
/// could grant it and no request member that could carry it. Unset means NOT enrolled,
/// which is what an account that never enrolled has: a publisher registration from it is
/// refused with [`NOT_A_MARKETPLACE_SELLER`].
pub const MARKETPLACE_SELLER_ENV: &str = "CLOUDFORMATION_MARKETPLACE_SELLER";

/// Whether `setting` says this account is an enrolled seller: `1`, `true`, `yes` or `on` in
/// any case says so, and anything else — including an empty value or a typo — says nothing
/// rather than being guessed at.
pub fn parse_marketplace_seller(setting: &str) -> bool {
    matches!(
        setting.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Whether the account this emulator stands for is enrolled as an Amazon Web Services
/// Marketplace seller.
///
/// A type of its own rather than a bare `bool`, so the state struct keeps its derived
/// `Default` while this one field defaults to what [`MARKETPLACE_SELLER_ENV`] says — and so
/// a caller cannot pass the flag to the gate by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketplaceSellerEnrollment(bool);

impl MarketplaceSellerEnrollment {
    /// An account the operator has said is an enrolled seller.
    pub const fn enrolled() -> Self {
        Self(true)
    }

    /// An account nobody has said anything about, which is an account that never enrolled.
    pub const fn not_enrolled() -> Self {
        Self(false)
    }

    /// Whether this account may be registered as an extension publisher at all.
    pub const fn is_enrolled(self) -> bool {
        self.0
    }
}

impl Default for MarketplaceSellerEnrollment {
    /// Not enrolled unless [`MARKETPLACE_SELLER_ENV`] says otherwise.
    fn default() -> Self {
        Self(
            ::std::env::var(MARKETPLACE_SELLER_ENV)
                .is_ok_and(|setting| parse_marketplace_seller(&setting)),
        )
    }
}

/// The model's `PublisherStatus` enum, "whether the publisher is verified": `VERIFIED |
/// UNVERIFIED`. A registered publisher is `VERIFIED` — "currently, all registered
/// publishers are verified" (`DescribePublisherOutput$PublisherStatus`).
pub mod publisher_status {
    pub const VERIFIED: &str = "VERIFIED";
    pub const UNVERIFIED: &str = "UNVERIFIED";
}

/// The model's `IdentityProvider` enum, "the type of account used as the identity provider
/// when registering this publisher with CloudFormation"
/// (`smithy_query(query="shape", shape="IdentityProvider")`: `AWS_Marketplace | GitHub |
/// Bitbucket`).
///
/// Only the Marketplace value is named here, because it is the only one a registration
/// DETERMINES: the other two are the two source-control accounts a `ConnectionArn` can be a
/// connection to, and which of them a given connection is to is a property of the
/// connection rather than of its ARN — see [`StoredPublisher::identity_provider`].
pub mod identity_provider {
    pub const AWS_MARKETPLACE: &str = "AWS_Marketplace";
}

/// The account's publisher registration.
#[derive(Debug, Clone)]
pub struct StoredPublisher {
    /// `PublisherId`, "the ID assigned this account by CloudFormation for publishing
    /// extensions" — minted once ([`crate::state::new_publisher_id`]) and stable from then
    /// on, since it is the account's identifier in every Region.
    pub publisher_id: String,
    /// [`publisher_status`].
    pub status: String,
    /// The `ConnectionArn` the registration was verified by, where it supplied one.
    pub connection_arn: Option<String>,
}

impl StoredPublisher {
    /// The record a registration that accepted the terms and conditions writes: a verified
    /// publisher, carrying whatever identity verification the request supplied.
    pub fn new(publisher_id: String, connection_arn: Option<String>) -> Self {
        Self {
            publisher_id,
            // "Currently, all registered publishers are verified."
            status: publisher_status::VERIFIED.to_string(),
            connection_arn,
        }
    }

    /// `IdentityProvider`: the type of account this publisher was verified by.
    ///
    /// A registration that supplied no connection was verified through the account's Amazon
    /// Web Services Marketplace account, so that is what it reports. One that supplied a
    /// connection was verified through a Bitbucket or GitHub account — but WHICH of the two
    /// is a property of the connection, which a connection ARN does not carry, so the
    /// member is withheld rather than guessed at.
    pub fn identity_provider(&self) -> Option<&'static str> {
        match self.connection_arn {
            Some(_) => None,
            None => Some(identity_provider::AWS_MARKETPLACE),
        }
    }
}
