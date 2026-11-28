#!/usr/bin/env node
// From npm (ms), and run as the program itself (require.main), as `node cli.js` would.
const ms = require("ms");

function main() {
  const who = process.argv[2] || "world";
  console.log(`node-hello 1.0.0: hello, ${who} (Node ${process.versions.node.split(".")[0]}, a minute is ${ms(60000)})`);
}

if (require.main === module) main();
