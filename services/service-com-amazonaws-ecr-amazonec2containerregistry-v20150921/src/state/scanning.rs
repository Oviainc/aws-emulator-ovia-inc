// SPDX-License-Identifier: CC0-1.0

//! The registry's scanning configuration: the record `PutRegistryScanningConfiguration`
//! stores and `GetRegistryScanningConfiguration` reports.
//!
//! It lives beside [`super`]'s repository state rather than inside it: there is one
//! scanning configuration per registry, and this emulator serves one registry, so the
//! record is a single optional field on the service state
//! ([`super::ComAmazonawsEcrAmazonec2containerregistryV20150921State::registry_scanning`])
//! and everything that derives from it is here.
//!
//! The scan frequencies a scan type accepts are the model's own, not this emulator's
//! invention — see [`supported_scan_frequencies`]. `CONTINUOUS_SCAN` is therefore
//! reachable now: the `ENHANCED` scan type it belongs to is what a put may set.

use crate::ops::types::{RegistryScanningConfiguration, RegistryScanningRule};

use super::{Fault, MANUAL_SCAN_FREQUENCY, SCAN_ON_PUSH_FREQUENCY};

/// The scan type a registry reports while no scanning configuration has been stored for
/// it: "When a registry scanning configuration is not defined, by default the BASIC scan
/// type is used" (`PutRegistryScanningConfigurationRequest$scanType`).
pub const DEFAULT_SCAN_TYPE: &str = BASIC_SCAN_TYPE;

/// The scan type Amazon ECR performs itself; see [`supported_scan_frequencies`].
pub const BASIC_SCAN_TYPE: &str = "BASIC";

/// The scan type Amazon Inspector performs; see [`supported_scan_frequencies`].
pub const ENHANCED_SCAN_TYPE: &str = "ENHANCED";

/// The `ScanFrequency` that belongs to [`ENHANCED_SCAN_TYPE`] scanning, where Amazon
/// Inspector scans continuously rather than on a push; see
/// [`supported_scan_frequencies`].
pub const CONTINUOUS_SCAN_FREQUENCY: &str = "CONTINUOUS_SCAN";

impl super::ComAmazonawsEcrAmazonec2containerregistryV20150921State {
    /// The registry's scanning configuration as every reader of it reports it: what the
    /// last `PutRegistryScanningConfiguration` stored, or — while nothing is stored — the
    /// documented default, the `BASIC` scan type with no scanning rules.
    pub fn registry_scanning_configuration(&self) -> RegistryScanningConfiguration {
        self.registry_scanning
            .clone()
            .unwrap_or_else(|| RegistryScanningConfiguration {
                scan_type: Some(DEFAULT_SCAN_TYPE.to_string()),
                rules: Some(Vec::new()),
            })
    }
}

/// The scan frequencies a scan type supports, from
/// `RegistryScanningRule$scanFrequency`: "When the `ENHANCED` scan type is specified, the
/// supported scan frequencies are `CONTINUOUS_SCAN` and `SCAN_ON_PUSH`. When the `BASIC`
/// scan type is specified, the `SCAN_ON_PUSH` scan frequency is supported. If scan on
/// push is not specified, then the `MANUAL` scan frequency is set by default." — so
/// `MANUAL` belongs to basic scanning, which is the type that has it as its default, and
/// `CONTINUOUS_SCAN` only to enhanced scanning.
///
/// A scan type the model does not declare cannot reach here: `ScanType` is an enum, so
/// the boundary refuses any other value before a handler runs.
pub fn supported_scan_frequencies(scan_type: &str) -> &'static [&'static str] {
    match scan_type {
        ENHANCED_SCAN_TYPE => &[CONTINUOUS_SCAN_FREQUENCY, SCAN_ON_PUSH_FREQUENCY],
        // BASIC, and anything the `ScanType` enum is widened with later.
        _ => &[MANUAL_SCAN_FREQUENCY, SCAN_ON_PUSH_FREQUENCY],
    }
}

/// The configuration a `PutRegistryScanningConfiguration` request stores: its `scanType`
/// (defaulted to [`DEFAULT_SCAN_TYPE`], since a request that names none leaves the
/// registry on the default scan type) and its rules, refused with
/// [`Fault::InvalidParameter`] when a rule asks for a frequency the scan type does not
/// support.
///
/// Every per-member constraint — the two-rule maximum, the hundred-filter maximum, the
/// `filter` pattern and length, the `filterType` and `scanFrequency` enums — is modelled,
/// so the boundary has already refused a request that breaks one. What is left is the
/// pairing of `scanType` with `scanFrequency`, which the model cannot express.
pub fn scanning_configuration_from_request(
    scan_type: Option<String>,
    rules: Option<Vec<RegistryScanningRule>>,
) -> Result<RegistryScanningConfiguration, Fault> {
    let scan_type = scan_type.unwrap_or_else(|| DEFAULT_SCAN_TYPE.to_string());
    let rules = rules.unwrap_or_default();
    let supported = supported_scan_frequencies(&scan_type);
    for rule in &rules {
        if !supported.contains(&rule.scan_frequency.as_str()) {
            return Err(Fault::InvalidParameter(format!(
                "scanFrequency '{}' is not supported with the '{scan_type}' scan type; \
                 the supported scan frequencies are {}",
                rule.scan_frequency,
                supported.join(", ")
            )));
        }
    }
    Ok(RegistryScanningConfiguration {
        scan_type: Some(scan_type),
        rules: Some(rules),
    })
}
