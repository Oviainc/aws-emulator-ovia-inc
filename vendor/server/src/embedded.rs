// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Options and credentials for an emulator linked into a host's own process (embedded mode).
//!
//! Each setting is an enum named in the command line's words (`on` / `off`), so a setting can
//! grow a variant without breaking a host, and [`FromStr`] parses the same tokens the CLI takes.

use crate::request_authentication::VendedCredentials;
pub use aws_credential_types::provider::SharedCredentialsProvider;
use std::str::FromStr;

/// Options for one in-process emulator connector. Build from [`Default`] and the builder methods.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmbeddedOptions {
    pub authentication: Authentication,
    pub telemetry: Telemetry,
}

impl EmbeddedOptions {
    pub fn authentication(mut self, authentication: Authentication) -> Self {
        self.authentication = authentication;
        self
    }

    pub fn telemetry(mut self, telemetry: Telemetry) -> Self {
        self.telemetry = telemetry;
        self
    }
}

/// Whether the connector verifies request signatures. Off by default: an in-process emulator is
/// reachable only by its host, which usually signs with placeholder credentials.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Authentication {
    #[default]
    Off,
    /// Every request must be signed with the [`VendedCredentials`] the connector hands out.
    On,
}

/// Usage telemetry for the process. On by default, as for the emulator binary; the
/// `EMULATOR_TELEMETRY` variable or the config file turning it off still wins.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Telemetry {
    #[default]
    On,
    Off,
}

/// A value that is not one of a setting's command-line tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownSetting(pub String);

impl FromStr for Authentication {
    type Err = UnknownSetting;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            other => Err(UnknownSetting(other.to_string())),
        }
    }
}

impl FromStr for Telemetry {
    type Err = UnknownSetting;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            other => Err(UnknownSetting(other.to_string())),
        }
    }
}

/// An AWS SDK credentials provider for a connector that vended `credentials`: its access key,
/// or placeholder keys when it vends none (authentication off accepts any signature).
pub fn aws_credentials_provider(credentials: &[VendedCredentials]) -> SharedCredentialsProvider {
    let (key, secret) = signing_key_pair(credentials);
    SharedCredentialsProvider::new(aws_credential_types::Credentials::new(
        key,
        secret,
        None,
        None,
        "aws-emulator",
    ))
}

/// The SigV4 key id and secret a connector that vended `credentials` signs with: its access
/// key, or placeholder keys when it vends none (authentication off accepts any signature).
pub fn signing_key_pair(credentials: &[VendedCredentials]) -> (&str, &str) {
    credentials
        .iter()
        .find_map(|vended| match vended {
            VendedCredentials::AwsAccessKey {
                access_key_id,
                secret_access_key,
            } => Some((access_key_id.as_str(), secret_access_key.as_str())),
        })
        .unwrap_or(("EMULATORPLACEHOLDER", "placeholder"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_credential_types::provider::ProvideCredentials;

    #[test]
    fn defaults_are_authentication_off_and_telemetry_on() {
        let options = EmbeddedOptions::default();
        assert_eq!(options.authentication, Authentication::Off);
        assert_eq!(options.telemetry, Telemetry::On);
    }

    #[test]
    fn builder_methods_set_each_option() {
        let options = EmbeddedOptions::default()
            .authentication(Authentication::On)
            .telemetry(Telemetry::Off);
        assert_eq!(options.authentication, Authentication::On);
        assert_eq!(options.telemetry, Telemetry::Off);
    }

    #[test]
    fn settings_parse_the_command_line_tokens_only() {
        assert_eq!("on".parse(), Ok(Authentication::On));
        assert_eq!("off".parse(), Ok(Authentication::Off));
        assert_eq!("on".parse(), Ok(Telemetry::On));
        assert_eq!("off".parse(), Ok(Telemetry::Off));
        assert_eq!(
            "true".parse::<Authentication>(),
            Err(UnknownSetting("true".to_string()))
        );
        assert_eq!(
            "ON".parse::<Telemetry>(),
            Err(UnknownSetting("ON".to_string()))
        );
    }

    #[tokio::test]
    async fn the_provider_signs_with_the_vended_access_key() {
        let vended = [VendedCredentials::AwsAccessKey {
            access_key_id: "EMULATORKEY".to_string(),
            secret_access_key: "secret".to_string(),
        }];
        let credentials = aws_credentials_provider(&vended)
            .provide_credentials()
            .await
            .unwrap();
        assert_eq!(credentials.access_key_id(), "EMULATORKEY");
        assert_eq!(credentials.secret_access_key(), "secret");
    }

    #[tokio::test]
    async fn the_provider_falls_back_to_placeholders_when_nothing_is_vended() {
        let credentials = aws_credentials_provider(&[])
            .provide_credentials()
            .await
            .unwrap();
        assert_eq!(credentials.access_key_id(), "EMULATORPLACEHOLDER");
    }
}
