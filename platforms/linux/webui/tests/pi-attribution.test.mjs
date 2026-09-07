import test from 'node:test';
import assert from 'node:assert/strict';
import { piAttributionState } from '../src/utils/piAttribution.js';
import { clientKindLabel } from '../src/utils/helpers.js';

test('pi labels and attribution states distinguish missing traffic from incomplete attribution', () => {
  assert.equal(clientKindLabel('pi'), 'pi');
  assert.equal(piAttributionState(), 'unknown');
  const known = { name: 'project', source: 'workspace_local', clientKinds: ['pi'], requests: 2 };
  const missing = { name: 'unidentified_project', source: 'missing_workspace_metadata', clientKinds: ['pi'], requests: 1 };
  assert.equal(piAttributionState([known]), 'observed');
  assert.equal(piAttributionState([known, missing]), 'unattributed');
  assert.equal(piAttributionState([{ ...missing, clientKinds: ['codex'] }]), 'unknown');
  assert.equal(piAttributionState([{ ...known, requests: 0 }]), 'unknown');
});
