// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/account_subscriptions_listing.rs"]
mod account_subscriptions_listing;

#[path = "all/data_protection_policy.rs"]
mod data_protection_policy;

#[path = "all/error_envelope.rs"]
mod error_envelope;

#[path = "all/phone_number_opt_in.rs"]
mod phone_number_opt_in;

#[path = "all/platform_applications.rs"]
mod platform_applications;

#[path = "all/platform_endpoint_deletion.rs"]
mod platform_endpoint_deletion;

#[path = "all/publish_batch.rs"]
mod publish_batch;

#[path = "all/put_data_protection_policy.rs"]
mod put_data_protection_policy;

#[path = "all/refusal_sentences.rs"]
mod refusal_sentences;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/set_subscription_attributes.rs"]
mod set_subscription_attributes;

#[path = "all/sms_sandbox_phone_numbers.rs"]
mod sms_sandbox_phone_numbers;

#[path = "all/sms_sandbox_verification.rs"]
mod sms_sandbox_verification;

#[path = "all/sqs_delivery_authorization.rs"]
mod sqs_delivery_authorization;

#[path = "all/sqs_subscription_delivery.rs"]
mod sqs_subscription_delivery;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/subscription_attributes.rs"]
mod subscription_attributes;

#[path = "all/subscription_pending_confirmation.rs"]
mod subscription_pending_confirmation;

#[path = "all/topic_attributes.rs"]
mod topic_attributes;

#[path = "all/topics_subscriptions_publish.rs"]
mod topics_subscriptions_publish;
