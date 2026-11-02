// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Calls to an emulator over TLS trusting only what the test says, and as raw bytes on the wire.

use super::emulator_process::Emulator;
use super::request_signing::{signed, Signer};
use super::workspace_models::SweptOperation;
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, StreamOwned, SupportedProtocolVersion as Version};
use std::io::{Read, Result, Write};
use std::{net::Ipv4Addr, net::TcpStream, path::Path, sync::Arc, time::Duration};

pub const HEALTH_PATH: &str = "/__admin/health";

/// The `supported_versions` extension offering TLS 1.2 alone.
const TLS_1_2_ALONE: [u8; 7] = [0x00, 0x2b, 0x00, 0x03, 0x02, 0x03, 0x03];

/// The status, headers and body of an answer.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: http::HeaderMap,
    pub body: String,
}

pub type Tls = StreamOwned<ClientConnection, TcpStream>;

impl Reply {
    /// The error the answer names in `x-amzn-errortype`, or `""` when it names none.
    pub fn error_type(&self) -> &str {
        let header = self.headers.get("x-amzn-errortype");
        header.and_then(|value| value.to_str().ok()).unwrap_or("")
    }
}

impl Emulator {
    /// The answer to `request`, its `host` header as written, from a client trusting only `ca.pem`.
    pub fn send(&self, request: http::Request<Vec<u8>>) -> Reply {
        let client = self.client.get_or_init(|| {
            let tls = Arc::unwrap_or_clone(self.trusting_client());
            let client = reqwest::blocking::Client::builder();
            client.use_preconfigured_tls(tls).build().unwrap()
        });
        let response = client.execute(request.try_into().unwrap()).unwrap();
        let (status, headers) = (response.status().as_u16(), response.headers().clone());
        let body = response.text().unwrap();
        Reply {
            status,
            headers,
            body,
        }
    }

    /// An unsigned call to `operation` with the body `{}` and its required headers set to `x`.
    pub fn request_for(&self, operation: &SweptOperation) -> http::Request<Vec<u8>> {
        let uri = format!("https://127.0.0.1:{}{}", self.port, operation.path);
        let mut request = http::Request::builder().uri(uri);
        request = request.method(operation.method.as_str());
        for name in &operation.required_headers {
            request = request.header(name, "x");
        }
        let request = request.header("host", &operation.host);
        let request = request.header("content-type", "application/json");
        request.body(b"{}".to_vec()).unwrap()
    }

    pub fn signed_call(&self, operation: &SweptOperation, signer: &Signer) -> Reply {
        self.send(signed(self.request_for(operation), signer))
    }

    /// The published key pair, signing for `operation`'s service.
    pub fn published_signer(&self, operation: &SweptOperation) -> Signer {
        let (key, secret) = self.published_key();
        Signer::new(key, secret, &operation.signing_name)
    }

    /// Health still answers `ok`: the emulator survived whatever the test sent it.
    pub fn assert_healthy(&self) {
        let uri = format!("https://127.0.0.1:{}{HEALTH_PATH}", self.port);
        let reply = self.send(http::Request::get(uri).body(Vec::new()).unwrap());
        let answer = (reply.status, reply.body.as_str());
        assert_eq!(answer, (200, "ok"), "{}", self.log());
    }

    /// A TLS client that trusts only this start's `ca.pem`, speaking every TLS version.
    pub fn trusting_client(&self) -> Arc<ClientConfig> {
        client_trusting(&self.file("ca.pem"), rustls::ALL_VERSIONS)
    }

    /// A connection naming the emulator `name`, trusting what `client` does; it handshakes lazily.
    pub fn tls(&self, name: &str, client: Arc<ClientConfig>) -> Result<Tls> {
        let name = ServerName::try_from(name.to_string()).unwrap();
        let connection = ClientConnection::new(client, name).unwrap();
        Ok(StreamOwned::new(connection, self.tcp(60)?))
    }

    pub fn handshake(&self, name: &str, client: Arc<ClientConfig>) -> Result<()> {
        let mut stream = self.tls(name, client)?;
        stream.conn.complete_io(&mut stream.sock).map(drop)
    }

    /// What the emulator answers over TLS to `parts`, written until it stops reading part way.
    pub fn exchange(&self, parts: impl IntoIterator<Item = Vec<u8>>) -> Result<Vec<u8>> {
        written_and_read(self.tls("127.0.0.1", self.trusting_client())?, parts)
    }

    /// What `bytes`, written below any TLS or HTTP library, get before a close or 5 s of quiet.
    pub fn raw(&self, bytes: &[u8]) -> Vec<u8> {
        let tcp = self.tcp(5).unwrap();
        written_and_read(tcp, [bytes.to_vec()]).unwrap_or_default()
    }

    /// A TLS 1.2 ClientHello offering only `version`, in its legacy field and `supported_versions`.
    pub fn hello_offering_only(&self, version: [u8; 2]) -> Vec<u8> {
        let tls_1_2 = client_trusting(&self.file("ca.pem"), &[&rustls::version::TLS12]);
        let connection = ClientConnection::new(tls_1_2, "localhost".try_into().unwrap());
        let mut hello = Vec::new();
        connection.unwrap().write_tls(&mut hello).unwrap();
        hello[9..11].copy_from_slice(&version);
        let extension = hello.windows(7).position(|bytes| bytes == TLS_1_2_ALONE);
        let offered = extension.expect("no supported_versions extension") + 5;
        hello[offered..offered + 2].copy_from_slice(&version);
        hello
    }

    /// The DER `SubjectPublicKeyInfo` of this start's certificate.
    pub fn certificate_key(&self) -> Vec<u8> {
        let der = CertificateDer::from_pem_file(self.file("ca.pem")).unwrap();
        let certificate = webpki::EndEntityCert::try_from(&der).unwrap();
        certificate.subject_public_key_info().to_vec()
    }

    fn tcp(&self, read_timeout_secs: u64) -> Result<TcpStream> {
        let tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port))?;
        let read_timeout = Duration::from_secs(read_timeout_secs);
        tcp.set_read_timeout(Some(read_timeout)).map(|()| tcp)
    }
}

/// A TLS client that trusts only the certificate in `ca_pem` and speaks only `versions`.
pub fn client_trusting(ca_pem: &Path, versions: &[&'static Version]) -> Arc<ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    let certificate = CertificateDer::from_pem_file(ca_pem).unwrap();
    roots.add(certificate).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = ClientConfig::builder_with_provider(provider);
    let builder = builder.with_protocol_versions(versions).unwrap();
    Arc::new(builder.with_root_certificates(roots).with_no_client_auth())
}

/// The status code an HTTP/1.1 answer starts with.
pub fn status_of(answer: &[u8]) -> Option<u16> {
    std::str::from_utf8(answer.get(9..12)?).ok()?.parse().ok()
}

fn written_and_read(
    mut stream: impl Read + Write,
    parts: impl IntoIterator<Item = Vec<u8>>,
) -> Result<Vec<u8>> {
    let mut write = |part: Vec<u8>| stream.write_all(&part).and_then(|()| stream.flush());
    let _refused_part_way = parts.into_iter().try_for_each(&mut write);
    let mut received = Vec::new();
    match stream.read_to_end(&mut received) {
        Err(error) if received.is_empty() => Err(error),
        _ => Ok(received),
    }
}
