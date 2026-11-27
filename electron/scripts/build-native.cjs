'use strict';
const path = require('node:path');
const {buildNative} = require('../../packaging/build-native.cjs');
const root = path.resolve(__dirname, '../..');
buildNative(root, {output: path.join(root, 'electron/native')});
