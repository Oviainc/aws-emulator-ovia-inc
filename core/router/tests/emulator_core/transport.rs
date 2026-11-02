// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The data port serves only HTTPS over TLS 1.2 and 1.3 for its own names, and outlives junk.

use super::emulator_process::Emulator;
use super::https_exchange::client_trusting;
use rustls::version::{TLS12, TLS13};

const TLS_HANDSHAKE_RECORD: u8 = 0x16;
const TLS_1_0: [u8; 2] = [0x03, 0x01];
const TLS_1_1: [u8; 2] = [0x03, 0x02];
const JUNK: [&[u8]; 5] = [
    b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n",
    &[0xff; 4096],
    b"\x16\x03\x01\xff\xff",
    b"\x16\x03\x03\x00\x05hello",
    b"\r\n\r\n",
];
const COVERED: [&str; 2] = ["127.0.0.1", "localhost"];
const UNCOVERED: [&str; 3] = ["not-the-emulator.example", "localhost.example", "10.0.0.1"];

#[test]
fn plain_http_old_tls_hellos_and_junk_get_no_answer_and_do_not_stop_the_listener() {
    let emulator = Emulator::start(&[]);
    let hellos = [TLS_1_0, TLS_1_1].map(|version| emulator.hello_offering_only(version));
    for bytes in JUNK.into_iter().chain(hellos.iter().map(Vec::as_slice)) {
        let reply = emulator.raw(bytes);
        let answered = reply.starts_with(b"HTTP/") || reply.first() == Some(&TLS_HANDSHAKE_RECORD);
        assert!(!answered, "{} got {reply:?}", bytes.escape_ascii());
        emulator.assert_healthy();
    }
}

#[test]
fn only_covered_names_verify_over_tls_1_2_and_1_3() {
    let emulator = Emulator::start(&[]);
    for version in [&TLS12, &TLS13] {
        let client = client_trusting(&emulator.file("ca.pem"), &[version]);
        for name in COVERED.iter().chain(&UNCOVERED) {
            let verified = emulator.handshake(name, client.clone()).is_ok();
            assert_eq!(verified, COVERED.contains(name), "{name} over {version:?}");
        }
    }
}
