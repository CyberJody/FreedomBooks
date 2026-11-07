'use strict';
// Explicit fallback required by the worker brief. No installer is emitted and
// installers.cjs refuses to wrap this incomplete shell into a release artifact.
const path = require('node:path');
const {build} = require('../package.cjs');
build({platform: 'win32', native: path.resolve(__dirname, '../../electron/native/win32-x64/fb-node.node'),
  allowMissingNative: true, version: process.env.FB_RELEASE_VERSION}).then(builds => {
  for (const directory of builds) console.log(`INCOMPLETE WINDOWS SHELL ${directory}; native fb-node pending; NOT RUNNABLE; NOT A RELEASE`);
}).catch(error => {console.error(error); process.exitCode = 1;});
