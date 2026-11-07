'use strict';
// WAVE4: only an allowed origin opens in the system browser for Outcome::External.
const test = require('node:test');
const assert = require('node:assert');
const guards = require('../main/guards.cjs');

const production = ['https://auth.openai.com'];

test('the OpenAI issuer over https is allowed', () => {
  const url = 'https://auth.openai.com/api/accounts/authorize?client_id=x&state=s';
  assert.strictEqual(guards.signIn(url, production), url);
});

test('other hosts, http, credentials, fragments and look-alikes are refused', () => {
  for (const url of [
    'http://auth.openai.com/api/accounts/authorize',
    'https://auth.openai.com.evil.example/authorize',
    'https://evil.example/?https://auth.openai.com',
    'https://user:pw@auth.openai.com/authorize',
    'https://auth.openai.com/authorize#frag',
    'https://auth.openai.com:8443/authorize',
    'http://127.0.0.1:4000/authorize',
    'file:///etc/passwd',
    'javascript:alert(1)',
    'fb://app/',
    '',
    42,
  ]) assert.throws(() => guards.signIn(url, production), /Invalid external address/, String(url));
});

test('a fake-network build allows only its exact loopback origin', () => {
  const origins = [...production, 'http://127.0.0.1:4000'];
  assert.strictEqual(guards.signIn('http://127.0.0.1:4000/authorize?x=1', origins), 'http://127.0.0.1:4000/authorize?x=1');
  assert.throws(() => guards.signIn('http://127.0.0.1:4001/authorize', origins), /Invalid external address/);
  assert.throws(() => guards.signIn('http://localhost:4000/authorize', origins), /Invalid external address/);
  // An http origin that is not numeric loopback is never honoured, even if listed.
  assert.throws(() => guards.signIn('http://example.com/', [...origins, 'http://example.com']), /Invalid external address/);
});
