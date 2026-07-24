'use strict';

const { readFile } = require('node:fs/promises');
const path = require('node:path');

async function loadConfig(root, requestedPath) {
  const target = path.resolve(root, requestedPath);
  return JSON.parse(await readFile(target, 'utf8'));
}

module.exports = { loadConfig };
