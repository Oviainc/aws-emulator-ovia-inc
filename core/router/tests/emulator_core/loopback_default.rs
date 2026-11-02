// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Loopback by default; a `-b` or `EMULATOR_BIND_ADDRESS` that is not an IP stops startup.

use super::emulator_process::Emulator;
use std::net::{TcpStream, UdpSocket};
use std::time::Duration;

#[test]
fn the_default_bind_is_not_reachable_from_a_non_loopback_address() {
    let udp = UdpSocket::bind("0.0.0.0:0").unwrap();
    let source = udp.connect("192.0.2.1:9").and_then(|()| udp.local_addr());
    let routable = |a: &std::net::SocketAddr| !a.ip().is_loopback() && !a.ip().is_unspecified();
    let Some(mut address) = source.ok().filter(routable) else {
        return eprintln!("nothing to check: this host has no non-loopback address");
    };
    let emulator = Emulator::start(&[]);
    emulator.assert_healthy();
    address.set_port(emulator.port);
    let connected = TcpStream::connect_timeout(&address, Duration::from_secs(2));
    assert!(connected.is_err(), "{address} accepted a connection");
}

#[test]
fn a_bind_address_that_is_not_an_ip_address_stops_startup() {
    for value in ["", "localhost", "garbage", "0.0.0.0.0"] {
        let env = [("EMULATOR_BIND_ADDRESS", value)];
        for (args, env) in [(&["-b", value][..], &[][..]), (&[], &env[..])] {
            let (exit, log) = Emulator::exit_of(args, env);
            let refused = exit.is_some_and(|s| !s.success()) && log.contains(&format!("{value:?}"));
            assert!(refused, "{args:?} {env:?}: {exit:?}\n{log}");
        }
    }
}
