'use strict';

function watch({powerMonitor, window, remote, native, busy, lockNow, idleSeconds, setTimer = setInterval}) {
  // Power and timer callbacks must not take errors to the app-wide error dialog.
  const safely = work => () => {
    try { void Promise.resolve(work()).catch(() => {}); } catch { /* try again on the next event */ }
  };
  const lock = safely(() => lockNow('idle'));
  const refresh = safely(() => {
    if (!window.isDestroyed()) return remote.refresh();
  });

  powerMonitor.on('suspend', lock);
  powerMonitor.on('lock-screen', lock);
  powerMonitor.on('resume', refresh);
  powerMonitor.on('unlock-screen', refresh);
  window.on('show', refresh);
  window.on('focus', refresh);

  setTimer(safely(() => {
    if (window.isDestroyed() || powerMonitor.getSystemIdleTime() < idleSeconds) return;
    // Hosting records a lock without borrowing the books, even during a desktop operation.
    if (remote.active() || native.hostingActive() || (!busy() && JSON.parse(native.appState()).open)) return lockNow('idle');
  }), 30_000).unref();
  remote.watch();
}

module.exports = {watch};
