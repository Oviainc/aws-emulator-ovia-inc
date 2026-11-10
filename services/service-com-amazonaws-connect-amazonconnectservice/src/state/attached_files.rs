// SPDX-License-Identifier: CC0-1.0

//! The per-instance attached-files configuration: what an instance allows to be
//! attached, for one attachment scope.
//!
//! One configuration per `AttachmentScope` per instance, stored in
//! [`super::InstanceRecord::attached_files_configurations`] beside the instance's other
//! resources so deleting the instance drops them with it.
//!
//! A scope nobody has configured is NOT stored: it reports
//! [`AttachedFilesConfigurationRecord::defaults_for`], which is also the record an
//! update starts from, so the members an update leaves out keep the values a caller
//! would have read before it.

use std::time::SystemTime;

use super::{to_date_time, InstanceRecord};
use crate::ops::types::{AllowedExtension, AttachedFilesConfiguration, ExtensionConfiguration};

/// The maximum attached-file size a scope reports before anyone configures one.
///
/// The model bounds `MaximumSizeLimitInBytes` at 1..104857600 ("the maximum value is
/// 104857600 (100 MB)") and documents no default for it, so the emulator's own choice
/// is that bound: an unconfigured scope accepts every size the member can express, and
/// an update is what narrows it.
pub const DEFAULT_MAXIMUM_SIZE_LIMIT_IN_BYTES: i64 = 104_857_600;

/// One instance's attached-files configuration for one attachment scope.
///
/// `extension_configuration` is absent while no update has named one: the member is
/// optional in the modelled `AttachedFilesConfiguration`, and absent is what "no
/// extension restriction for this scope" reads as on the wire.
#[derive(Debug, Clone)]
pub struct AttachedFilesConfigurationRecord {
    /// The `AttachmentScope` this configuration governs, which is also its key.
    pub attachment_scope: String,
    /// The largest attached file the scope accepts, in bytes.
    pub maximum_size_limit_in_bytes: Option<i64>,
    /// The file extensions the scope allows, in the order they were configured, or
    /// `None` while no update has named a set.
    pub allowed_extensions: Option<Vec<String>>,
    /// When an update last changed this configuration, or `None` for the defaults,
    /// which nothing has modified.
    pub last_modified_time: Option<SystemTime>,
}

impl AttachedFilesConfigurationRecord {
    /// The configuration `attachment_scope` has while nobody has updated it: the
    /// default size limit, no extension restriction, and no modification instant.
    pub fn defaults_for(attachment_scope: &str) -> Self {
        Self {
            attachment_scope: attachment_scope.to_string(),
            maximum_size_limit_in_bytes: Some(DEFAULT_MAXIMUM_SIZE_LIMIT_IN_BYTES),
            allowed_extensions: None,
            last_modified_time: None,
        }
    }

    /// Apply one update to this configuration, at `now`.
    ///
    /// A member the request carries replaces the stored value; a member it leaves out
    /// keeps it. Either way the configuration counts as modified at `now`, because the
    /// call is what makes this scope's configuration an instance-specific one.
    pub fn apply(
        &mut self,
        maximum_size_limit_in_bytes: Option<i64>,
        allowed_extensions: Option<Vec<String>>,
        now: SystemTime,
    ) {
        if let Some(limit) = maximum_size_limit_in_bytes {
            self.maximum_size_limit_in_bytes = Some(limit);
        }
        if let Some(extensions) = allowed_extensions {
            self.allowed_extensions = Some(extensions);
        }
        self.last_modified_time = Some(now);
    }

    /// The stored extension set as the modelled `ExtensionConfiguration`.
    pub fn to_extension_configuration(&self) -> Option<ExtensionConfiguration> {
        self.allowed_extensions
            .as_ref()
            .map(|extensions| ExtensionConfiguration {
                allowed_extensions: extensions
                    .iter()
                    .map(|extension| AllowedExtension {
                        extension: extension.clone(),
                    })
                    .collect(),
            })
    }

    /// The modelled `AttachedFilesConfiguration` shape
    /// `DescribeAttachedFilesConfiguration` returns, for the instance it is stored on.
    pub fn to_configuration(&self, instance_id: &str) -> AttachedFilesConfiguration {
        AttachedFilesConfiguration {
            instance_id: instance_id.to_string(),
            attachment_scope: self.attachment_scope.clone(),
            maximum_size_limit_in_bytes: self.maximum_size_limit_in_bytes,
            extension_configuration: self.to_extension_configuration(),
            last_modified_time: self.last_modified_time.map(to_date_time),
        }
    }
}

impl InstanceRecord {
    /// This instance's configuration for `attachment_scope`: the stored one where an
    /// update made one, and otherwise that scope's defaults.
    ///
    /// Reading one stores nothing — the defaults are derived, so a scope nobody has
    /// updated still has no instance-specific configuration after it is read.
    pub fn attached_files_configuration(
        &self,
        attachment_scope: &str,
    ) -> AttachedFilesConfigurationRecord {
        self.attached_files_configurations
            .get(attachment_scope)
            .cloned()
            .unwrap_or_else(|| AttachedFilesConfigurationRecord::defaults_for(attachment_scope))
    }

    /// This instance's configuration for `attachment_scope`, ready to be updated:
    /// the stored one, or a freshly inserted copy of that scope's defaults, which is
    /// what makes an update that names only some members create a configuration
    /// carrying the defaults for the rest.
    pub fn attached_files_configuration_mut(
        &mut self,
        attachment_scope: &str,
    ) -> &mut AttachedFilesConfigurationRecord {
        self.attached_files_configurations
            .entry(attachment_scope.to_string())
            .or_insert_with(|| AttachedFilesConfigurationRecord::defaults_for(attachment_scope))
    }
}

/// The extensions an `ExtensionConfiguration` on the wire names, in the order it names
/// them. `None` when the request carried no configuration at all, which is what makes
/// an update leave the stored set alone.
pub fn requested_extensions(configuration: Option<&ExtensionConfiguration>) -> Option<Vec<String>> {
    configuration.map(|configuration| {
        configuration
            .allowed_extensions
            .iter()
            .map(|allowed| allowed.extension.clone())
            .collect()
    })
}
