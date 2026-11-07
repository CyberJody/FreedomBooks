'use strict';
const test = require('node:test');
const path = require('node:path');
const { build } = require('./package.cjs');
const { verify } = require('./fuses.cjs');
test('packaged executable has the required fuse wire', {timeout: 300000}, async () => {
  const builds = await build();
  for (const build of builds) {
    await verify(path.join(build, process.platform === 'darwin' ? 'FreedomBooks.app' : process.platform === 'win32' ? 'FreedomBooks.exe' : 'freedombooks'));
  }
});
