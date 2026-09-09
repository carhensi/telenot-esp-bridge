import test from 'node:test';
import assert from 'node:assert/strict';
import { saveAndReboot } from '../src/save-config.js';

function fixture(states) {
  const calls = [];
  const api = {
    commit: async (...args) => calls.push(['commit', ...args]),
    getCommit: async () => { calls.push(['poll']); return states.shift() || {state:'pending'}; },
    reboot: async () => calls.push(['reboot']),
  };
  return {api, calls};
}
test('reboot follows durable acknowledgement and verifies sensor count', async () => {
  const {api, calls} = fixture([{state:'pending'}, {state:'saved', sensors:7}]);
  await saveAndReboot(api, 7, 7, async () => {});
  assert.deepEqual(calls, [['commit',true,7,7], ['poll'], ['poll'], ['reboot']]);
});
test('failure, wrong count, lost job and timeout never reboot', async () => {
  for (const states of [[{state:'failed',message:'disk'}], [{state:'saved',sensors:0}], [{state:'idle'}], []]) {
    const {api, calls} = fixture(states);
    await assert.rejects(saveAndReboot(api, 7, 7, async () => {}));
    assert.ok(!calls.some(c => c[0] === 'reboot'));
  }
});


test('pending sensor edits prevent commit and reboot', async () => {
  const {api, calls} = fixture([]);
  api.sensorEditsPending = 1;
  await assert.rejects(saveAndReboot(api, 7, 7, async () => {}));
  assert.deepEqual(calls, []);
});
