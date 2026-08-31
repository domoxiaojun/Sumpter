import assert from 'node:assert/strict';
import test from 'node:test';

const { ONBOARDING_STATES, getOnboardingState } = await import('../src/utils/onboarding.js');

const mappedConfig = {
  endpoints: [{ enabled: true, mappings: [{ clientPattern: 'gpt-*' }] }],
};

test('help onboarding follows the six-state contract and uses existing snapshots', () => {
  assert.equal(getOnboardingState(), ONBOARDING_STATES.NOT_STARTED);
  assert.equal(getOnboardingState({ status: { running: true } }), ONBOARDING_STATES.NOT_CONFIGURED);
  assert.equal(
    getOnboardingState({ status: { running: true }, config: { endpoints: [{ enabled: true, mappings: [] }] } }),
    ONBOARDING_STATES.NO_MAPPING,
  );
  assert.equal(
    getOnboardingState({ status: { running: true }, config: mappedConfig, runtime: { clientRequests: 0 } }),
    ONBOARDING_STATES.CLIENT_NOT_CONNECTED,
  );
  assert.equal(
    getOnboardingState({ status: { running: true }, config: mappedConfig, runtime: { clientRequests: 1, clientFailures: 1 } }),
    ONBOARDING_STATES.FIRST_FAILURE,
  );
  assert.equal(
    getOnboardingState({ status: { running: true }, config: mappedConfig, runtime: { counters: { clientRequests: 2, clientSuccesses: 1 } } }),
    ONBOARDING_STATES.FIRST_SUCCESS,
  );
});

