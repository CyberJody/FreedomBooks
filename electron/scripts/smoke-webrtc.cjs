'use strict';
// Regression from staging/r-desktop-d1/rtc_check.cjs (rtc.log: 4 packets before the fix).
// Use the actual page world and real Chromium ICE, without mocking or hiding RTCPeerConnection.
const dgram = require('node:dgram');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');

module.exports = async ({page, shots, pass}) => {
  const socket = dgram.createSocket('udp4');
  const packets = [];
  socket.on('message', data => packets.push(data.length));
  try {
    await new Promise(resolve => socket.bind(0, '127.0.0.1', resolve));
    const port = socket.address().port;
    // Prove the listener actually receives UDP before using a zero count as evidence.
    const received = new Promise(resolve => socket.once('message', resolve));
    socket.send(Buffer.from('listener-check'), port, '127.0.0.1');
    await received;
    packets.length = 0;
    const result = await page.evaluate(async port => {
      let fetchBlocked = false;
      try { await fetch(`http://127.0.0.1:${port}/`); } catch { fetchBlocked = true; }
      const rtc = new RTCPeerConnection({iceServers: [{urls: `stun:127.0.0.1:${port}`}]});
      try {
        rtc.createDataChannel('local-review');
        await rtc.setLocalDescription(await rtc.createOffer());
        await new Promise(resolve => setTimeout(resolve, 3000));
        return {url: location.href, fetchBlocked};
      } finally { rtc.close(); }
    }, port);
    const evidence = {...result, udpPackets: packets.length, packetLengths: packets};
    fs.writeFileSync(path.join(shots, 'rtc.log'), JSON.stringify(evidence) + '\n');
    console.log(JSON.stringify(evidence));
    assert.equal(result.fetchBlocked, true);
    assert.equal(packets.length, 0, 'renderer WebRTC must send zero UDP/STUN packets');
    pass('WebRTC loopback STUN reproduction: listener verified, 0 UDP packets');
  } finally { socket.close(); }
};
