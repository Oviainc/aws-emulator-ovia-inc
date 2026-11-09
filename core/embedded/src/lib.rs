// SPDX-License-Identifier: CC0-1.0

#![forbid(unsafe_code)]
//! In-process entry point for the aws-emulator emulator.
//!
//! A host that wants to exercise this emulator inside its own test process —
//! without binding a socket or running the standalone binary — links this crate
//! and builds an emulator with [`AwsEmulatorEmbedded::new`]. Its
//! [`AwsEmulatorEmbedded::http_client`] is an
//! `aws_smithy_runtime_api::client::http::HttpClient`, so it drops straight into an AWS
//! SDK config's `http_client(..)` in place of a network connector, and
//! [`AwsEmulatorEmbedded::aws_credentials_provider`] signs for it: SDK calls are
//! served from the in-memory engine with no I/O.
//!
//! Each emulator owns a FRESH engine: clients from one emulator share state, and two
//! emulators never do.

pub use emulator_server::embedded::{Authentication, EmbeddedOptions, Telemetry};
pub use emulator_server::{TlsCertificateAuthority, VendedCredentials};
use emulator_telemetry::{TelemetryClient, TelemetryOptions};
use smithy_server_runtime::EmbeddedEmulator;
use std::sync::Once;

/// The project id this emulator reports to telemetry.
const TELEMETRY_PROJECT_ID: &str = "proj-a85d506b35244f71850223e92be29acb";

/// The telemetry configuration endpoint. `None` leaves telemetry off.
const TELEMETRY_CONFIG_ENDPOINT: Option<&str> = Some("https://aws-emulator-telemetry.us-west-2.api.aws/v1/config");

/// The emulator's folder in the user's home (config file, telemetry state).
const EMULATOR_HOME_DIR: &str = ".aws/emulator";

/// One in-process emulator: a fresh in-memory engine with its options. Every
/// [`Self::http_client`] serves the same engine, so SDK clients built from one emulator
/// share state and two emulators never do.
#[derive(Debug, Clone)]
pub struct AwsEmulatorEmbedded {
    emulator: EmbeddedEmulator,
}

impl AwsEmulatorEmbedded {
    /// An emulator with `options`. Authentication applies to this emulator; telemetry is
    /// process-wide, and [`Telemetry::Off`] from any emulator turns it off for the process.
    pub fn new(options: EmbeddedOptions) -> Self {
        if let Err(error) = emulator_server::set_emulator_home_dir(EMULATOR_HOME_DIR) {
            eprintln!("{error}");
        }
        start_telemetry(options.telemetry);
        let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = aws_emulator_router::build_router(engine);
        if let Some(telemetry) = emulator_telemetry::global() {
            telemetry.mark_ready();
        }
        router.set_request_authentication(options.authentication == Authentication::On);
        // The emulator is the one in-process surface: it serves both the SDK
        // `HttpClient` a host hands to a config's `.http_client(..)` (the router
        // is itself the client) and the credential accessors, all reading this
        // one router.
        Self {
            emulator: EmbeddedEmulator::from_router(router),
        }
    }

    /// The SDK `HttpClient` for this emulator, to pass to a config's `.http_client(..)`.
    pub fn http_client(
        &self,
    ) -> emulator_server::aws_smithy_runtime_api::client::http::SharedHttpClient {
        self.emulator.http_client()
    }

    /// The SDK `HttpConnector` for this emulator, for driving it in-process
    /// directly (`connector.call(request)`) without building a full SDK client.
    pub fn http_connector(
        &self,
    ) -> emulator_server::aws_smithy_runtime_api::client::http::SharedHttpConnector {
        self.emulator.http_connector()
    }

    /// What a request must carry to pass this emulator's authentication; empty when it is off.
    /// The key pair is minted once per process, so every emulator with authentication on
    /// accepts the same keys.
    pub fn credentials(&self) -> Vec<VendedCredentials> {
        self.emulator.credentials()
    }

    /// An AWS SDK credentials provider signing with [`Self::credentials`], or with
    /// placeholders when authentication is off.
    pub fn aws_credentials_provider(
        &self,
    ) -> emulator_server::embedded::SharedCredentialsProvider {
        self.emulator.aws_credentials_provider()
    }

    /// The TLS certificate authority behind every listener this emulator opens, such as the
    /// one a signed URL points at. Trust it to fetch those URLs: `pem()` for `AWS_CA_BUNDLE`
    /// or `curl --cacert`, `der()` for a rustls root store.
    pub fn certificate_authority(&self) -> &TlsCertificateAuthority {
        self.emulator.certificate_authority()
    }
}

impl Default for AwsEmulatorEmbedded {
    /// [`EmbeddedOptions::default`]: authentication off, usage telemetry on.
    fn default() -> Self {
        Self::new(EmbeddedOptions::default())
    }
}

/// Starts the process's telemetry client once, unless no endpoint is compiled in. The client
/// records for the whole process and installs no panic hook; its final flush runs when the
/// process exits.
///
/// [`Telemetry::Off`] wins for the whole process, whichever emulator says it and whenever, and
/// deletes the stored installation id and outbox. If the first emulator says it, the client
/// starts opted out: it records and sends nothing, and a later default emulator cannot turn
/// telemetry on. If a client is already running, it opts out at once and sends nothing more.
fn start_telemetry(telemetry: Telemetry) {
    static START: Once = Once::new();
    START.call_once(|| {
        if let Some(mut options) = telemetry_options() {
            options.embedding_opt_out = telemetry == Telemetry::Off;
            emulator_telemetry::init_global(TelemetryClient::start(options));
            emulator_telemetry::flush_at_process_exit();
        }
    });
    if telemetry == Telemetry::Off {
        if let Some(client) = emulator_telemetry::global() {
            client.opt_out();
        }
    }
}

/// The version embedded telemetry reports: the release version the release build stamps
/// as `EMULATOR_VERSION`, and the package version with `-dev` appended for any other build.
const EMBEDDED_VERSION: &str = match option_env!("EMULATOR_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

fn telemetry_options() -> Option<TelemetryOptions> {
    let project_id = emulator_telemetry::ProjectId::new(TELEMETRY_PROJECT_ID).ok()?;
    let version = emulator_telemetry::ProductVersion::new(EMBEDDED_VERSION).ok()?;
    let endpoint = emulator_telemetry::Endpoint::parse(TELEMETRY_CONFIG_ENDPOINT?).ok()?;
    let mut options = TelemetryOptions::new(project_id, version, endpoint);
    options.run_mode = emulator_telemetry::RunMode::Embedded;
    Some(options)
}
