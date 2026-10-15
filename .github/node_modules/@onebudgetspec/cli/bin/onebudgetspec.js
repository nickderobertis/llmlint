#!/usr/bin/env node
// Run the platform carrier's binary with this process's arguments and streams, and exit as
// it did. lib/launcher.js holds the decisions.
const { spawnSync } = require("node:child_process");
const { finish, locate } = require("../lib/launcher.js");

const found = locate(process.platform, process.arch, require.resolve);
if ("message" in found) {
  console.error(found.message);
  process.exit(found.status);
}
const ended = finish(
  spawnSync(found.binary, process.argv.slice(2), { stdio: "inherit" }),
  found.binary,
  found.carrier,
);
if (ended.message) console.error(ended.message);
process.exit(ended.status);
