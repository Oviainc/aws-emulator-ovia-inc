// SPDX-License-Identifier: CC0-1.0

//! Boot check: the in-process facade assembles a connector.
//!
//! Keep this as an integration test under `embedded/tests/`, so `cargo test`
//! exercises the facade a host actually links. [`AwsEmulatorEmbedded::http_client`]
//! builds the whole router internally, so this proves the in-process entry point
//! assembles with no socket bind.
//!
//! # Assemble on an explicitly sized thread
//!
//! Build the facade on a thread created with [`BOOT_STACK_SIZE`]: `http_client`
//! builds the full router, and running it on the default cargo-test thread stack
//! overflows and aborts once the workspace carries one large service.
use aws_emulator_embedded::AwsEmulatorEmbedded;

/// Stack for assembling the facade here. Keep this at or above the size the
/// router's own boot check uses, so a workspace that boots in production also
/// passes here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// An AWS SDK config builder's `http_client(..)` accepts only an `HttpClient`, so the
/// facade must stay one: this stops compiling if `http_client()` no longer returns it.
fn assert_sdk_http_client(_client: impl aws_smithy_runtime_api::client::http::HttpClient) {}

#[test]
fn the_in_process_facade_assembles() {
    std::thread::Builder::new()
        .name("boot-check".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = AwsEmulatorEmbedded::default();
            assert_sdk_http_client(emulator.http_client());
            assert!(
                emulator
                    .certificate_authority()
                    .pem()
                    .starts_with("-----BEGIN CERTIFICATE-----"),
                "the facade must hand a host the certificate its listeners present"
            );
        })
        .expect("spawn the boot-check thread")
        .join()
        .expect("the in-process facade must assemble a connector");
}
