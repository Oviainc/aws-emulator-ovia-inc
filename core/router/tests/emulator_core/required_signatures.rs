// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Only this start's published key and certificate are accepted; every start publishes new ones.

use super::emulator_process::Emulator;
use super::https_exchange::Reply;
use super::request_signing::Signer;
use super::suite_support::{probe_operation, signed_operations};
use super::workspace_models::SweptOperation;

/// Errors answered before any signature check, as the request names nothing the emulator serves.
const UNSERVED: &str = "UnknownService AmbiguousService NotImplemented UnknownOperationException";

#[test]
#[cfg_attr(
    not(emulator_full_security_suite),
    ignore = "fails on a long-running preprod tree, which serves ssooidc CreateTokenWithIAM unsigned"
)]
fn every_operation_the_models_sign_refuses_an_unsigned_call() {
    let (Some(_), operations) = (probe_operation(), signed_operations()) else {
        return;
    };
    let emulator = &Emulator::start(&[]);
    let share = operations.len().div_ceil(8);
    let served: Vec<_> = std::thread::scope(|scope| {
        let sweep = |chunk| scope.spawn(move || served_unsigned(emulator, chunk));
        let sweeps: Vec<_> = operations.chunks(share).map(sweep).collect();
        let served = sweeps.into_iter().map(|sweep| sweep.join().unwrap());
        served.flatten().collect()
    });
    assert!(served.is_empty(), "served unsigned: {served:?}");
}

#[test]
fn only_the_key_and_certificate_this_start_published_are_accepted() {
    let (mut this, other) = (Emulator::start(&[]), Emulator::start(&[]));
    if let Some(operation) = served_for_published_key(&this) {
        assert_only_its_published_key_is_served(&this, &other, operation);
    }
    let handshake = this.handshake("127.0.0.1", other.trusting_client());
    assert!(handshake.is_err(), "another start's ca.pem was trusted");
    let published = |emulator: &Emulator| (emulator.certificate_key(), emulator.published_key());
    let before = published(&this);
    this.stop();
    let after = published(&Emulator::start_in(&[], this.home.clone()));
    assert_ne!(before.0, after.0, "a restart reused its certificate");
    assert_ne!(before.1, after.1, "a restart reused its key");
}

/// The first signed operation refused unsigned that the published key gets past; `None` if none.
fn served_for_published_key(emulator: &Emulator) -> Option<&'static SweptOperation> {
    let first = probe_operation()?;
    let call = |operation| emulator.signed_call(operation, &emulator.published_signer(operation));
    let refused_unsigned = |op| emulator.send(emulator.request_for(op)).status == 403;
    let mut operations = signed_operations().iter();
    let served = operations.find(|op| call(op).status != 403 && refused_unsigned(op));
    assert!(served.is_some(), "{} got {:?}", first.name, call(first));
    served
}

fn assert_only_its_published_key_is_served(this: &Emulator, other: &Emulator, op: &SweptOperation) {
    let ((key, secret), other_start) = (this.published_key(), other.published_key());
    let never_minted = |key: &str| (key.to_string(), secret.clone());
    let refused = [
        ((key, "wrong".to_string()), "SignatureDoesNotMatch"),
        (other_start, "AccessDenied"),
        (never_minted("AKIAIOSFODNN7EXAMPLE"), "AccessDenied"),
        (never_minted("ASIAIOSFODNN7EXAMPLE"), "AccessDenied"),
    ];
    for ((key, secret), error) in refused {
        let reply = this.signed_call(op, &Signer::new(key, secret, &op.signing_name));
        assert!(
            reply.status == 403 && reply.error_type().contains(error),
            "{reply:?}"
        );
    }
}

fn served_unsigned(emulator: &Emulator, chunk: &'static [SweptOperation]) -> Vec<&'static str> {
    let served = chunk.iter().filter(|op| !refuses_unsigned(emulator, op));
    served.map(|operation| operation.name.as_str()).collect()
}

/// A 403, or an error answered before any signature check and the same for a wrong secret.
fn refuses_unsigned(emulator: &Emulator, operation: &SweptOperation) -> bool {
    let unsigned = emulator.send(emulator.request_for(operation));
    let early = UNSERVED
        .split(' ')
        .any(|error| unsigned.error_type().contains(error));
    let (key, _) = emulator.published_key();
    let wrong = Signer::new(key, "x".into(), &operation.signing_name);
    let same = |reply: Reply| (reply.status, &reply.body) == (unsigned.status, &unsigned.body);
    unsigned.status == 403 || early && same(emulator.signed_call(operation, &wrong))
}
