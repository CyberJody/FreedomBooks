'use strict';
const path = require('node:path');
const os = require('node:os');
const ID = 'app.freedombooks.FreedomBooks';
const NAME = 'FreedomBooks';

function configure(app, platform = process.platform) {
  app.setName(NAME);
  if (platform === 'win32') app.setAppUserModelId(ID);
  if (platform === 'linux') {
    app.setDesktopName(`${ID}.desktop`);
    app.commandLine.appendSwitch('class', ID);
  }
}

function settingsDir(app) {
  return path.join(app.getPath('appData'), ID);
}

function dataDir(app, platform = process.platform, env = process.env) {
  if (platform !== 'linux') return settingsDir(app);
  const base = env.XDG_DATA_HOME && path.isAbsolute(env.XDG_DATA_HOME)
    ? env.XDG_DATA_HOME : path.join(os.homedir(), '.local/share');
  const destination = path.join(base, NAME);
  require('./migrate-data.cjs').migrate(settingsDir(app), destination);
  return destination;
}

// Keep the legacy lock namespace: unmodified installed builds claim it before setting userData.
// Moving only this lock lets both versions run. Settings/cache still use the permanent profile;
// the legacy directory is a required compatibility lock directory, not abandoned app data.
function claimInstance(app, settings) {
  app.setPath('userData', path.join(app.getPath('appData'), NAME));
  const claimed = app.requestSingleInstanceLock();
  app.setPath('userData', settings);
  return claimed;
}

module.exports = { ID, NAME, claimInstance, configure, dataDir, settingsDir };
