import assert from 'node:assert/strict';
import test from 'node:test';
import { formatOutputTokensPerSecond } from '../src/utils/outputSpeed.js';

test('输出速度按一位小数显示，空值是破折号', () => {
  assert.equal(formatOutputTokensPerSecond(12.34), '12.3 tok/s');
  assert.equal(formatOutputTokensPerSecond(0), '0.0 tok/s');
  assert.equal(formatOutputTokensPerSecond(null), '—');
  assert.equal(formatOutputTokensPerSecond(undefined), '—');
});
