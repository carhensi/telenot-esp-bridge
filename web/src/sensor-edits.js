// Return changes only after the device accepted them. Batch simple identical edits.
export async function applySensorEdits(api, edits) {
  const updates = new Map();
  if (!edits.length) return updates;
  const payloads = edits.map(({fields}) => { const {status: _status, ...payload} = fields; return payload; });
  const first = payloads[0];
  const key = Object.keys(first).length === 1 ? Object.keys(first)[0] : null;
  const op = key === 'confirmed' && first.confirmed === true ? 'confirm'
    : key === 'include' ? (first.include ? 'include' : 'exclude')
    : key === 'kind' ? 'set_kind' : key === 'polarity' ? 'set_polarity' : null;
  if (op && payloads.every(p => JSON.stringify(p) === JSON.stringify(first))) {
    const result = await api.bulkSensors(edits.map(e => e.address), op, first);
    if (result.updated !== edits.length) throw new Error('Nicht alle Melder auf dem Geraet gefunden.');
    edits.forEach(e => updates.set(e.address, e.fields));
  } else {
    for (let i = 0; i < edits.length; i++) {
      const result = await api.patchSensor(edits[i].address, payloads[i]);
      updates.set(edits[i].address, result);
    }
  }
  return updates;
}
