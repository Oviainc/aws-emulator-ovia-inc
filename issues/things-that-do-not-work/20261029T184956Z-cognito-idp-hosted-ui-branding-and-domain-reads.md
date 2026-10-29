Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-ui-customization-and-risk-configuration
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-ui-customization-and-risk-configuration cognito-idp-ui-customization-crud cognito-idp-ui-customization-global-and-client cognito-idp-ui-customization-lifecycle cognito-idp-ui-customization-round-trip cognito-idp-user-pool-domain-lifecycle cognito-idp-managed-login-branding-crud cognito-idp-managed-login-branding-error-paths cognito-idp-managed-login-branding-errors cognito-idp-managed-login-branding-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|GetUICustomization|2xx->2xx|$.UICustomization.CSSUrl
Issue-Key: cognito-idp|SetUICustomization|2xx->2xx|$.UICustomization.CSSUrl
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.CloudFrontDistribution
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.S3Bucket
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.Status
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.Version
Issue-Key: cognito-idp|CreateManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Assets
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Assets
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Assets
Issue-Key: cognito-idp|UpdateManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Assets
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.categories.signUp.acceptanceElements[*]
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.divider.lightMode.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.dropDown
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.input.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.input.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.inputDescription
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.inputLabel.darkMode.textColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.inputLabel.lightMode.textColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.link.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.link.darkMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.link.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.link.lightMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.optionControls.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.optionControls.darkMode.selected
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.optionControls.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.optionControls.lightMode.selected
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.error
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.pending
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.success
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.warning
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.error
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.pending
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.success
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.warning
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.alert.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.alert.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.form.backgroundImage
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.form.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.form.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageBackground.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageBackground.image.enabled
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageBackground.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.backgroundImage
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.logo.location
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.backgroundImage
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.logo.location
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.darkMode.bodyColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.darkMode.descriptionColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.darkMode.headingColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.lightMode.bodyColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.lightMode.descriptionColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.lightMode.headingColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.disabled
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.disabled
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.darkMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.darkMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.lightMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.lightMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.categories.signUp.acceptanceElements[*]
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.divider.lightMode.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.dropDown
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.input.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.input.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.inputDescription
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.inputLabel.darkMode.textColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.inputLabel.lightMode.textColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.link.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.link.darkMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.link.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.link.lightMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.optionControls.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.optionControls.darkMode.selected
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.optionControls.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.optionControls.lightMode.selected
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.error
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.pending
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.success
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.warning
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.error
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.pending
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.success
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.warning
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.alert.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.alert.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.form.backgroundImage
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.form.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.form.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageBackground.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageBackground.image.enabled
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageBackground.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.backgroundImage
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.logo.location
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.backgroundImage
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.darkMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.lightMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.logo.location
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.darkMode.bodyColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.darkMode.descriptionColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.darkMode.headingColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.lightMode.bodyColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.lightMode.descriptionColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageText.lightMode.headingColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.disabled
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.disabled
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.darkMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.darkMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.darkMode.hover
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.lightMode.active
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.lightMode.defaults
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.secondaryButton.lightMode.hover
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: hosted-UI, branding and domain reads omit server-generated fields

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUICustomization

## Description
Nine recorded causes in which a hosted-UI, managed-login-branding or domain read omits a field the service generates server-side. Generated hostnames, bucket names and version stamps differ per environment and are NOT claimed here as literal values — the requirement is that the member is present and carries the shape the service emits.

Cause 1 of 9: GetUICustomization omits UICustomization.CSSUrl. AWS returns the URL at which the uploaded CSS is served — path shape `https://<distribution-host>/<userPoolId>/<clientId>/<version>/assets/CSS/custom-css.css`, with the pool id, client id and version from the service's state. Emulator returns None. Why: a client renders the hosted UI by fetching this URL; without it the customization appears not to have been applied. Fix: SetUICustomization state must record the asset version and GetUICustomization must project the URL. Fixture cognito-idp-ui-customization-and-risk-configuration, step task#6 (8 recorded steps).
Cause 2 of 9: SetUICustomization's own response omits the same CSSUrl. Fixture cognito-idp-ui-customization-crud, step task#4 (6 recorded steps).
Cause 3 of 9: DescribeUserPoolDomain omits CloudFrontDistribution, S3Bucket and Version, and reports Status `CREATING` forever. AWS returns all three and the domain reaches `ACTIVE`. Why: clients poll Status to know when the domain is usable and read CloudFrontDistribution to create the DNS alias; neither is possible. Fix: the domain lifecycle in the user-pool-domain state must transition to ACTIVE and populate the generated distribution/bucket/version fields. Fixture cognito-idp-user-pool-domain-lifecycle, step task#3 (2 recorded steps).
Cause 4 of 9: CreateManagedLoginBranding omits ManagedLoginBranding.Assets; AWS returns the member as an empty list `[]` when no assets were supplied. Fix: the branding serializer must always emit Assets. Fixture cognito-idp-managed-login-branding-crud, step task#3 (4 recorded steps).
Cause 5 of 9: DescribeManagedLoginBranding omits the same empty Assets list. Fixture cognito-idp-managed-login-branding-crud, step task#4 (4 recorded steps).
Cause 6 of 9: DescribeManagedLoginBrandingByClient omits the same empty Assets list. Fixture cognito-idp-managed-login-branding-crud, step task#5 (2 recorded steps).
Cause 7 of 9: UpdateManagedLoginBranding omits the same empty Assets list. Fixture cognito-idp-managed-login-branding-crud, step task#7 (2 recorded steps).
Cause 8 of 9: DescribeManagedLoginBranding with UseCognitoProvidedValues omits both the service-provided Assets (the default SVG asset set, one entry per Category/ColorMode pair with Extension `SVG`) and the whole default Settings document (componentClasses, components, categories sub-trees). Emulator returns neither. Fix: the branding state must materialise the service-provided default asset set and settings document. Fixture cognito-idp-managed-login-branding-crud, step task#6.
Cause 9 of 9: DescribeManagedLoginBrandingByClient has the same omission of the service-provided Assets and default Settings document. Fixture cognito-idp-managed-login-branding-lifecycle, step task#6.

## Evidence
GetUICustomization (cognito-idp-ui-customization-and-risk-configuration#task#6, 200 both sides): $.UICustomization.CSSUrl: expected a URL of the form https://<host>/<poolId>/<clientId>/<version>/assets/CSS/custom-css.css, got None.
DescribeUserPoolDomain (cognito-idp-user-pool-domain-lifecycle#task#3): $.DomainDescription.CloudFrontDistribution expected a distribution hostname got None; $.DomainDescription.S3Bucket expected the service asset bucket got None; $.DomainDescription.Version expected a version stamp got None; $.DomainDescription.Status expected 'ACTIVE' got 'CREATING'.
CreateManagedLoginBranding (cognito-idp-managed-login-branding-crud#task#3): $.ManagedLoginBranding.Assets expected [] got None.
DescribeManagedLoginBranding (cognito-idp-managed-login-branding-crud#task#6): $.ManagedLoginBranding.Assets expected the default SVG asset set (Category/ColorMode/Extension per entry) got None, plus ~65 missing Settings sub-members (componentClasses.*, components.*, categories.signUp.acceptanceElements[0]).

## Steps to Reproduce
1. Replay cognito-idp-ui-customization-and-risk-configuration step task#6 and cognito-idp-ui-customization-crud step task#4; check UICustomization.CSSUrl.
2. Replay cognito-idp-user-pool-domain-lifecycle step task#3; check DomainDescription.Status and the generated fields.
3. Replay cognito-idp-managed-login-branding-crud steps task#3, task#4, task#5, task#6, task#7 and cognito-idp-managed-login-branding-lifecycle step task#6; check ManagedLoginBranding.Assets and Settings.
