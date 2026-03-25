// Micro-Racing server launcher with @napi-rs/canvas support
// The webpack bundle (dist/api/server.js) includes its own Express + WS server.
const path = require('path');
const Module = require('module');

// Patch canvas module resolution to use @napi-rs/canvas (or mock fallback)
const origResolve = Module._resolveFilename;
Module._resolveFilename = function(request, parent, isMain, options) {
  if (request === 'canvas') {
    return require.resolve('./canvas-shim.cjs');
  }
  return origResolve.call(this, request, parent, isMain, options);
};

// Set port before loading the bundle
process.env.PORT = process.env.PORT || process.env.APP_PORT || '8409';
process.env.APP_PORT = process.env.PORT;
process.env.NODE_ENV = process.env.NODE_ENV || 'production';

// Load the webpack-bundled server (Express static + WS game server)
require(path.join(__dirname, '../micro-racing-src/dist/api/server.js'));
console.log(`[Micro-Racing] Server loading on port ${process.env.PORT}`);
