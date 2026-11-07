'use strict';
// Use the same checksum-verifying downloader as the packager, never execute the runtime.
(async () => {
  const {downloadArtifact} = await import('@electron/get');
  console.log(await downloadArtifact({version: '45.2.0', artifactName: 'electron', platform: process.argv[2], arch: 'x64'}));
})().catch(error => {console.error(error); process.exitCode = 1;});
