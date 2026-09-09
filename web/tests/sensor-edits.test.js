import test from 'node:test';
import assert from 'node:assert/strict';
import { applySensorEdits } from '../src/sensor-edits.js';

test('confirming many sensors uses one acknowledged batch', async () => {
  let calls = 0;
  const edits = [100, 101, 102].map(address => ({address, fields: {confirmed: true, status: 'confirmed'}}));
  const api = {bulkSensors: async (addresses, op) => {
    calls++; assert.deepEqual(addresses, [100,101,102]); assert.equal(op, 'confirm');
    return {updated: 3};
  }};
  const result = await applySensorEdits(api, edits);
  assert.equal(calls, 1); assert.equal(result.size, 3);
});

test('failed or incomplete batch does not report success', async () => {
  const edits = [{address:100,fields:{confirmed:true}}];
  await assert.rejects(applySensorEdits({bulkSensors: async () => ({updated:0})}, edits));
  await assert.rejects(applySensorEdits({bulkSensors: async () => {throw Error('offline');}}, edits), /offline/);
});

test('individual edits are sequential and return the accepted device values', async () => {
  let pending = false;
  const result = await applySensorEdits({patchSensor: async (address, fields) => {
    assert.equal(pending, false); pending = true;
    await new Promise(resolve => setTimeout(resolve, 2)); pending = false;
    assert.equal(fields.status, undefined);
    return {address, name:'accepted'};
  }}, [100,101].map(address => ({address,fields:{name:'requested',status:'confirmed'}})));
  assert.equal(result.get(101).name,'accepted');
});
