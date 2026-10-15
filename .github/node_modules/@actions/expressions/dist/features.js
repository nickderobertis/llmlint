/**
 * All known experimental feature keys.
 * This list must be kept in sync with the ExperimentalFeatures interface.
 */
const allFeatureKeys = [
    "missingInputsQuickfix",
    "blockScalarChompingWarning",
    "allowCaseFunction",
    "allowCopilotRequestsPermission",
    "allowDrivesPermissionAutocomplete",
    "allowBackgroundSteps"
];
export class FeatureFlags {
    constructor(features) {
        this.features = features ?? {};
    }
    /**
     * Check if an experimental feature is enabled.
     *
     * Resolution order:
     * 1. Explicit feature flag (if set)
     * 2. `all` flag (if set)
     * 3. false (default)
     */
    isEnabled(feature) {
        const explicit = this.features[feature];
        if (explicit !== undefined) {
            return explicit;
        }
        return this.features.all ?? false;
    }
    /**
     * Returns list of all enabled experimental features.
     */
    getEnabledFeatures() {
        return allFeatureKeys.filter(key => this.isEnabled(key));
    }
}
//# sourceMappingURL=features.js.map