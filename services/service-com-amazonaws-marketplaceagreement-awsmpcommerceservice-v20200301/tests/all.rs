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
#[path = "all/accepted_term_payloads.rs"]
mod accepted_term_payloads;
#[path = "all/agreement_cancellation_requests.rs"]
mod agreement_cancellation_requests;
#[path = "all/agreement_charges.rs"]
mod agreement_charges;
#[path = "all/agreement_entitlements.rs"]
mod agreement_entitlements;
#[path = "all/agreement_lifecycle.rs"]
mod agreement_lifecycle;
#[path = "all/agreement_parties.rs"]
mod agreement_parties;
#[path = "all/billing_adjustments.rs"]
mod billing_adjustments;
#[path = "all/cancellation_request_decisions.rs"]
mod cancellation_request_decisions;
#[path = "all/charge_amounts.rs"]
mod charge_amounts;
#[path = "all/error_bodies.rs"]
mod error_bodies;
#[path = "all/idempotency_token_conflicts.rs"]
mod idempotency_token_conflicts;
#[path = "all/invoice_adjustments.rs"]
mod invoice_adjustments;
#[path = "all/payment_request_cancellation.rs"]
mod payment_request_cancellation;
#[path = "all/payment_request_rejection.rs"]
mod payment_request_rejection;
#[path = "all/payment_request_selector_resolution.rs"]
mod payment_request_selector_resolution;
#[path = "all/payment_requests.rs"]
mod payment_requests;
#[path = "all/purchase_order_associations.rs"]
mod purchase_order_associations;
#[path = "all/purchase_order_updates.rs"]
mod purchase_order_updates;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/selector_resolution.rs"]
mod selector_resolution;
#[path = "all/selector_values.rs"]
mod selector_values;
#[path = "all/seller_submitted_request_views.rs"]
mod seller_submitted_request_views;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unknown_ids.rs"]
mod unknown_ids;
#[path = "all/validation_envelope.rs"]
mod validation_envelope;
#[path = "all/validation_messages.rs"]
mod validation_messages;
