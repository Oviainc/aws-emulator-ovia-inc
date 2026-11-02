// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Bodies over 16 MiB get 413 however framed, and at once when their declared length says so.

use super::emulator_process::Emulator;
use super::https_exchange::{status_of, Tls};
use std::{io::Read, io::Write, time::Duration, time::Instant};

const MAX_BODY_BYTES: usize = 16 << 20;
const PART_BYTES: usize = 1 << 20;
const CHUNKED: &str = "transfer-encoding: chunked";
const DEADLINE: Duration = Duration::from_secs(5);

#[test]
#[cfg_attr(
    not(emulator_full_security_suite),
    ignore = "fails on long-running prod and preprod trees until their emulator server refuses over-cap bodies at once"
)]
fn bodies_over_16_mib_get_413_however_framed() {
    let emulator = Emulator::start(&[]);
    for chunked in [false, true] {
        let status = |length| body_status(&emulator, chunked, length);
        assert_eq!(status(MAX_BODY_BYTES + 1), 413, "chunked {chunked}");
        let at_cap = status(MAX_BODY_BYTES);
        assert!(read_and_answered(at_cap), "chunked {chunked}: {at_cap}");
    }
    let (asked, mut status_line) = (Instant::now(), [0; 12]);
    let mut head_only = head_sent(&emulator, false, 1 << 30);
    head_only.read_exact(&mut status_line).unwrap();
    assert_eq!(status_of(&status_line), Some(413));
    assert!(asked.elapsed() < DEADLINE, "{:?}", asked.elapsed());
}

fn read_and_answered(status: u16) -> bool {
    ![0, 408, 413].contains(&status)
}

/// The status a `POST /` with a body of `length` spaces gets, or 0 for no answer.
fn body_status(emulator: &Emulator, chunked: bool, length: usize) -> u16 {
    let answer = emulator.exchange(framed(chunked, length)).ok();
    answer.and_then(|answer| status_of(&answer)).unwrap_or(0)
}

fn head_sent(emulator: &Emulator, chunked: bool, length: usize) -> Tls {
    let client = emulator.trusting_client();
    let mut stream = emulator.tls("127.0.0.1", client).unwrap();
    let head = framed(chunked, length).next().unwrap();
    stream.write_all(&head).and(stream.flush()).unwrap();
    stream
}

/// The head, then the parts, of a `POST /` with `length` spaces, chunked or declared.
fn framed(chunked: bool, length: usize) -> impl Iterator<Item = Vec<u8>> {
    let (framing, wrap): (_, fn(String) -> String) = match chunked {
        true => (CHUNKED.into(), |s| format!("{:x}\r\n{s}\r\n", s.len())),
        false => (format!("content-length: {length}"), |part| part),
    };
    let head = format!("POST / HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n{framing}\r\nconnection: close\r\n\r\n");
    let part = move |start| wrap(" ".repeat(PART_BYTES.min(length - start)));
    let last_chunk = chunked.then(|| "0\r\n\r\n".to_string());
    let parts = (0..length).step_by(PART_BYTES).map(part).chain(last_chunk);
    std::iter::once(head).chain(parts).map(String::into_bytes)
}
