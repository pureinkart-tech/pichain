#!/usr/bin/env node

/**
 * TOSIOS Game Server - Standalone entry point
 * Runs on port 8407 (configurable via PORT env var)
 *
 * This wraps the built server bundle and resolves its dependencies
 * from the tosios-src workspace's node_modules.
 */

const path = require('path');
const Module = require('module');

// Set port
process.env.PORT = process.env.PORT || '8407';

// Add the tosios-src node_modules to the module search path
// so that external dependencies (express, cors, compression, colyseus) can be found
const srcRoot = path.join(__dirname, '..', 'tosios-src');
const srcNodeModules = path.join(srcRoot, 'node_modules');
const serverNodeModules = path.join(srcRoot, 'packages', 'server', 'node_modules');

// Prepend our node_modules paths
const originalResolveFilename = Module._resolveFilename;
Module._resolveFilename = function(request, parent, isMain, options) {
    // For external dependencies, try our paths first
    if (!request.startsWith('.') && !request.startsWith('/')) {
        for (const nmPath of [serverNodeModules, srcNodeModules]) {
            try {
                const resolved = path.join(nmPath, request);
                return originalResolveFilename.call(this, resolved, parent, isMain, options);
            } catch (e) {
                // Try next path
            }
        }
    }
    return originalResolveFilename.call(this, request, parent, isMain, options);
};

// The built server bundle serves static files from ../../client/public relative to itself
// We need to run the bundle from a location where that path resolves to our static files
// OR we can override __dirname. Let's just require the bundle directly - it uses __dirname + '../../client/public'
// We need to make that path point to our tosios/ directory.

// The bundle's PUBLIC_DIR = join(__dirname, '../../client/public')
// If we run from tosios-src/packages/server/dist/, it resolves to tosios-src/packages/client/public/
// Instead, let's create a symlink approach or patch the path.

// Actually, let's just run the original bundle from its original location
// since we've already copied client files to the right place in the source tree.

// Add /add-bot endpoint for spawning AI bots
const http = require('http');
const origListen = http.Server.prototype.listen;
http.Server.prototype.listen = function() {
    // After the server starts, add our custom route
    this.on('request', (req, res) => {
        if (req.method === 'POST' && req.url === '/add-bot') {
            let body = '';
            req.on('data', chunk => body += chunk);
            req.on('end', () => {
                try {
                    const { roomId, count } = JSON.parse(body);
                    if (!roomId) { res.writeHead(400); res.end('{"error":"roomId required"}'); return; }
                    const numBots = Math.min(parseInt(count) || 1, 10);
                    const { spawn } = require('child_process');
                    const botScript = path.resolve(__dirname, '../../test-bots/tosios-bot.js');
                    const bot = spawn('node', [botScript, roomId, String(numBots)], { stdio: ['ignore', 'pipe', 'pipe'] });
                    bot.stdout.on('data', d => console.log('[BOT]', d.toString().trim()));
                    bot.stderr.on('data', d => console.error('[BOT-ERR]', d.toString().trim()));
                    console.log(`[TOSIOS] Spawned ${numBots} bot(s) for room ${roomId} (pid: ${bot.pid})`);
                    res.writeHead(200, {'Content-Type': 'application/json'});
                    res.end(JSON.stringify({ ok: true, roomId, bots: numBots, pid: bot.pid }));
                } catch (err) {
                    res.writeHead(500);
                    res.end(JSON.stringify({ error: err.message }));
                }
            });
            return;
        }
    });
    return origListen.apply(this, arguments);
};
require(path.join(srcRoot, 'packages', 'server', 'dist', 'index.js'));
