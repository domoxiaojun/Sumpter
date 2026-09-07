// The Help page deliberately derives one onboarding state from the same
// status/config/runtime snapshots used by the rest of the WebUI.  Keeping the
// classifier pure makes the Linux and macOS copies easy to compare and keeps
// diagnostics out of the first-run path.
import { hasRoutableModel } from './modelGroups.js';

export const ONBOARDING_STATES = Object.freeze({
  NOT_STARTED: 'not_started',
  NOT_CONFIGURED: 'not_configured',
  NO_MAPPING: 'no_mapping',
  CLIENT_NOT_CONNECTED: 'client_not_connected',
  FIRST_FAILURE: 'first_failure',
  FIRST_SUCCESS: 'first_success',
});

function number(value) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

function countersFrom(runtime) {
  const summary = runtime?.summary || runtime || {};
  return summary.counters || summary;
}

/**
 * Return exactly one state from enabled group bindings, or legacy endpoint
 * mappings when modelGroups is absent.
 */
export function getOnboardingState({ status, config, runtime } = {}) {
  if (!status?.running) return ONBOARDING_STATES.NOT_STARTED;

  const endpoints = Array.isArray(config?.endpoints) ? config.endpoints : null;
  if (!config || endpoints === null || endpoints.length === 0) {
    return ONBOARDING_STATES.NOT_CONFIGURED;
  }

  const hasMapping = hasRoutableModel(config);
  if (!hasMapping) return ONBOARDING_STATES.NO_MAPPING;

  const counters = countersFrom(runtime);
  const requests = number(counters.clientRequests ?? counters.client_requests);
  const successes = number(counters.clientSuccesses ?? counters.client_successes);
  const failures = number(counters.clientFailures ?? counters.client_failures);
  if (requests <= 0) return ONBOARDING_STATES.CLIENT_NOT_CONNECTED;
  if (successes > 0) return ONBOARDING_STATES.FIRST_SUCCESS;
  if (failures > 0) return ONBOARDING_STATES.FIRST_FAILURE;

  // A request that is still in flight (or was cancelled before completion)
  // has not reached either terminal onboarding state yet.
  return ONBOARDING_STATES.CLIENT_NOT_CONNECTED;
}
