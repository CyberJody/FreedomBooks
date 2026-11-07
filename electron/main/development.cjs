'use strict';
// All shell testing overrides pass this single gate. Packaged code always uses bundled resources.
const path = require('node:path');
const override = (app, name, env = process.env) => app.isPackaged ? undefined : env[name];
module.exports = {
  nativeModule: (app, directory, env) => override(app, 'FB_NATIVE_MODULE', env) || path.join(directory, '..', 'native', 'fb-node.node'),
  userData: (app, env) => override(app, 'FB_DESKTOP_USER_DATA', env),
  devTools: (app, env) => override(app, 'FB_DEVTOOLS', env) === '1',
  // The fb-net network helper, packaged next to the native module. FB_NET_HELPER (dev/smoke only)
  // may point at a fake-network build; a packaged app ignores it.
  netHelper: (app, directory, env) => override(app, 'FB_NET_HELPER', env)
    || path.join(app.isPackaged ? app.getAppPath() + '.unpacked' : path.join(directory, '..'), 'native', process.platform === 'win32' ? 'fb-net.exe' : 'fb-net'),
};
