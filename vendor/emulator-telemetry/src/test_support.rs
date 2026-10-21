// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! A real local HTTP server for integration tests: a std `TcpListener` with a
//! deliberately small HTTP/1.1 implementation (Content-Length bodies,
//! `Connection: close`). No mocks — the client under test speaks real TCP to
//! it via reqwest. Also hosts the deterministic generator the property tests
//! share.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Deterministic xorshift-style generator for the closed-vocabulary property
/// tests. The fixed seed makes every run identical; change it only alongside
/// the printed reproduction line.
pub struct FuzzRng(pub u64);

impl FuzzRng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }

    pub fn chance(&mut self, one_in: u64) -> bool {
        self.below(one_in) == 0
    }

    pub fn pick<'a, T>(&mut self, options: &'a [T]) -> &'a T {
        &options[self.below(options.len() as u64) as usize]
    }

    /// A short string over a pool that mixes dimension-legal bytes with the
    /// separators, quotes, whitespace and non-ASCII that identifiers, ARNs,
    /// e-mail addresses and free text contain.
    pub fn string(&mut self) -> String {
        const POOL: &[char] = &[
            'a',
            'b',
            'c',
            'z',
            'A',
            'Z',
            '0',
            '1',
            '9',
            '-',
            '.',
            '_',
            ':',
            '/',
            '@',
            ' ',
            '"',
            '\\',
            '\u{e9}',
            '\u{4e16}',
            '\u{1f600}',
            '\n',
            '\t',
        ];
        let len = self.below(40) as usize;
        (0..len).map(|_| *self.pick(POOL)).collect()
    }

    pub fn region_candidate(&mut self) -> String {
        const TEMPLATES: &[&str] = &[
            "us-west-2",
            "us-gov-west-1",
            "us-iso-east-1",
            "eu-isoe-west-1",
            "zz-west-1",
            "ap-southeast-7",
            "unknown",
        ];
        match self.below(3) {
            0 => self.string(),
            1 => self.pick(TEMPLATES).to_string(),
            _ => {
                let mut mutated: Vec<char> = self.pick(TEMPLATES).chars().collect();
                if !mutated.is_empty() {
                    let at = self.below(mutated.len() as u64) as usize;
                    mutated[at] = *self.pick(&['q', 'Q', '9', '-', '_']);
                }
                mutated.into_iter().collect()
            }
        }
    }
}

/// One captured request.
#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl CapturedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn body_utf8(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Behavior knobs shared with the running server.
pub struct ServerBehavior {
    /// Server config JSON body for `GET /v1/config` (the ETag is fixed).
    pub config_json: Mutex<String>,
    /// ETag served with the config; `If-None-Match` hits answer 304.
    pub etag: Mutex<String>,
    /// Ingest bodies larger than this answer `400` (the split contract).
    pub ingest_reject_over_bytes: AtomicU64,
    /// When set, every ingest POST answers `500`.
    pub ingest_fail_all: AtomicBool,
    /// When set, every ingest POST answers `429`.
    pub ingest_throttle_all: AtomicBool,
    /// When set, every ingest POST answers `408`.
    pub ingest_timeout_all: AtomicBool,
    /// When set, every ingest POST answers `403`.
    pub ingest_forbid_all: AtomicBool,
    /// When set, every `GET /v1/config` waits for one message (or the
    /// sender's drop) before answering, holding the client's collection
    /// decision pending.
    pub config_release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// When set, every `POST /v1/events` waits for one message (or the
    /// sender's drop) before answering, holding the client's drain mid-flight.
    pub ingest_release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

/// The running test server plus everything it captured.
pub struct TestServer {
    pub base_url: String,
    pub behavior: Arc<ServerBehavior>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    first_request_at: Arc<Mutex<Option<std::time::Instant>>>,
    accepted: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TestServer {
    /// Start on an ephemeral loopback port with the given config JSON.
    pub fn start(config_json: &str) -> TestServer {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .expect("bind ephemeral loopback port");
        let addr = match listener.local_addr().expect("local addr") {
            std::net::SocketAddr::V4(v4) => v4,
            other => panic!("unexpected addr family: {other}"),
        };
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");

        let behavior = Arc::new(ServerBehavior {
            config_json: Mutex::new(config_json.to_string()),
            etag: Mutex::new("\"test-etag-1\"".to_string()),
            ingest_reject_over_bytes: AtomicU64::new(u64::MAX),
            ingest_fail_all: AtomicBool::new(false),
            ingest_throttle_all: AtomicBool::new(false),
            ingest_timeout_all: AtomicBool::new(false),
            ingest_forbid_all: AtomicBool::new(false),
            config_release: Mutex::new(None),
            ingest_release: Mutex::new(None),
        });
        let requests = Arc::new(Mutex::new(Vec::new()));
        let first_request_at = Arc::new(Mutex::new(None));
        let accepted = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        let handle = {
            let behavior = behavior.clone();
            let requests = requests.clone();
            let first_request_at = first_request_at.clone();
            let accepted = accepted.clone();
            let shutdown = shutdown.clone();
            std::thread::spawn(move || {
                while !shutdown.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            first_request_at
                                .lock()
                                .unwrap()
                                .get_or_insert_with(std::time::Instant::now);
                            accepted.fetch_add(1, Ordering::SeqCst);
                            let _ = handle_connection(stream, &behavior, &requests);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })
        };

        TestServer {
            base_url: format!("http://{addr}"),
            behavior,
            requests,
            first_request_at,
            accepted,
            shutdown,
            handle: Some(handle),
        }
    }

    /// Hold every config response until the returned sender sends or drops.
    pub fn hold_config_responses(&self) -> std::sync::mpsc::Sender<()> {
        let (release, held) = std::sync::mpsc::channel();
        *self.behavior.config_release.lock().unwrap() = Some(held);
        release
    }

    /// Hold every ingest response until the returned sender sends or drops.
    pub fn hold_ingest_responses(&self) -> std::sync::mpsc::Sender<()> {
        let (release, held) = std::sync::mpsc::channel();
        *self.behavior.ingest_release.lock().unwrap() = Some(held);
        release
    }

    /// Block until the server has accepted at least one connection.
    pub fn wait_for_first_request(&self) {
        self.wait_for_accepted(1);
    }

    /// Connections accepted so far — including one currently held open,
    /// which [`Self::requests`] only lists once it has been answered.
    pub fn accepted_connections(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Block until at least `count` connections have been accepted.
    pub fn wait_for_accepted(&self, count: usize) {
        let started = std::time::Instant::now();
        while self.accepted_connections() < count {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "only {} of {count} connections reached the test server",
                self.accepted_connections()
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// When the very first connection arrived, if any has.
    pub fn first_request_at(&self) -> Option<std::time::Instant> {
        *self.first_request_at.lock().unwrap()
    }

    /// Every captured request, in arrival order.
    pub fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// Captured ingest POST bodies (NDJSON), in arrival order.
    pub fn ingest_bodies(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "POST" && r.path == "/v1/events")
            .map(|r| r.body_utf8())
            .collect()
    }

    /// Requests against `GET /v1/config`.
    pub fn config_requests(&self) -> Vec<CapturedRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "GET" && r.path == "/v1/config")
            .collect()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A loopback address whose port was bound and released: connections are
/// refused instantly, so "unreachable" never means "slow" in tests.
pub fn refused_base_url() -> String {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    format!("http://{addr}")
}

fn handle_connection(
    stream: TcpStream,
    behavior: &ServerBehavior,
    requests: &Mutex<Vec<CapturedRequest>>,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let content_length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;

    let captured = CapturedRequest {
        method: method.clone(),
        path: path.clone(),
        headers,
        body,
    };

    let (status, response_headers, response_body) = route(behavior, &captured);
    requests.lock().unwrap().push(captured);

    let mut stream = reader.into_inner();
    let mut response = format!("HTTP/1.1 {status}\r\nconnection: close\r\n");
    for (name, value) in &response_headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("content-length: {}\r\n\r\n", response_body.len()));
    stream.write_all(response.as_bytes())?;
    stream.write_all(&response_body)?;
    stream.flush()
}

fn route(
    behavior: &ServerBehavior,
    request: &CapturedRequest,
) -> (&'static str, Vec<(String, String)>, Vec<u8>) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/config") => {
            if let Some(held) = behavior.config_release.lock().unwrap().as_ref() {
                let _ = held.recv_timeout(std::time::Duration::from_secs(30));
            }
            let etag = behavior.etag.lock().unwrap().clone();
            if request.header("if-none-match") == Some(etag.as_str()) {
                return ("304 Not Modified", vec![("etag".into(), etag)], Vec::new());
            }
            let body = behavior.config_json.lock().unwrap().clone().into_bytes();
            (
                "200 OK",
                vec![
                    ("etag".into(), etag),
                    ("content-type".into(), "application/json".into()),
                ],
                body,
            )
        }
        ("POST", "/v1/events") => {
            if let Some(held) = behavior.ingest_release.lock().unwrap().as_ref() {
                let _ = held.recv_timeout(std::time::Duration::from_secs(30));
            }
            if behavior.ingest_fail_all.load(Ordering::SeqCst) {
                return ("500 Internal Server Error", Vec::new(), b"{}".to_vec());
            }
            if behavior.ingest_throttle_all.load(Ordering::SeqCst) {
                return ("429 Too Many Requests", Vec::new(), b"{}".to_vec());
            }
            if behavior.ingest_timeout_all.load(Ordering::SeqCst) {
                return ("408 Request Timeout", Vec::new(), b"{}".to_vec());
            }
            if behavior.ingest_forbid_all.load(Ordering::SeqCst) {
                return ("403 Forbidden", Vec::new(), b"{}".to_vec());
            }
            let cap = behavior.ingest_reject_over_bytes.load(Ordering::SeqCst);
            if request.body.len() as u64 > cap {
                return (
                    "400 Bad Request",
                    Vec::new(),
                    b"{\"error\":\"split\"}".to_vec(),
                );
            }
            ("200 OK", Vec::new(), b"{}".to_vec())
        }
        _ => ("404 Not Found", Vec::new(), Vec::new()),
    }
}
