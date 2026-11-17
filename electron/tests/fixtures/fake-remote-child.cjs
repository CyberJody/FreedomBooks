'use strict';
// A stand-in for `fb-remote client` in unit tests: reads frames, answers from a script of canned
// behaviours chosen by the request. Synthetic values only.
const {encode, decoder} = require('../../main/remote-client.cjs');
const out = value => process.stdout.write(encode(value));
process.stdin.on('data', decoder(m => {
  if (m.op === 'pair') {
    if (m.code === 'WRONG') return out({kind: 'error', id: m.id, code: 'pairing', message: 'That pairing code did not work.'});
    return out({kind: 'paired', id: m.id, identity: {cert: 'aa', key: 'bb'}, host_pin: 'c'.repeat(64), device_id: 'dev1', host_name: 'Office PC', fingerprint: 'AB12 CD34'});
  }
  if (m.op === 'connect') return out({kind: 'answer', id: m.id, response: {type: 'Session', session: 's', signed_in: null, host_name: 'Office PC', resumed: false}});
  if (m.op === 'call') {
    const r = m.request;
    if (r.type === 'Render' && r.target === '/hang/') return; // never answered
    if (r.type === 'Render' && r.target === '/exit/') return process.exit(0);
    if (r.type === 'Render') {
      out({kind: 'link', state: 'reconnecting'});
      return out({kind: 'answer', id: m.id, response: {type: 'Page', html: `<html><head></head><body>${r.target}</body></html>`, status: 200, route: 'x'}});
    }
    return out({kind: 'answer', id: m.id, response: {type: 'Outcome', outcome: JSON.stringify({outcome: 'done', data: {echo: r.request_id}})}});
  }
}, () => process.exit(3)));
