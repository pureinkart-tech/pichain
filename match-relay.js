import { createServer } from 'http';
import { WebSocketServer } from 'ws';
import { readFileSync } from 'fs';

const PORT = 8003;

// In-memory match store
const matches = new Map();
const codeToId = new Map(); // room_code -> match_id for private matches
const clients = new Set();

function genCode() {
  const chars = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
  let code;
  do {
    code = '';
    for (let i = 0; i < 5; i++) code += chars[Math.floor(Math.random() * chars.length)];
  } while (codeToId.has(code));
  return code;
}

const wss = new WebSocketServer({ noServer: true });

function broadcast(msg, exclude = null) {
  const data = JSON.stringify(msg);
  for (const ws of clients) {
    if (ws !== exclude && ws.readyState === 1) {
      ws.send(data);
    }
  }
}

function cleanupStaleMatches() {
  const now = Date.now();
  for (const [id, m] of matches) {
    // Remove matches older than 2 hours
    if (now - m.created > 2 * 60 * 60 * 1000) {
      if (m.room_code) codeToId.delete(m.room_code);
      matches.delete(id);
    }
  }
}

wss.on('connection', (ws) => {
  clients.add(ws);

  // Send all public (non-private) matches to the new client
  const publicMatches = Array.from(matches.values()).filter(m => !m.private);
  ws.send(JSON.stringify({ t: 'sync', matches: publicMatches }));

  ws.on('message', (raw) => {
    let msg;
    try { msg = JSON.parse(raw); } catch { return; }

    switch (msg.t) {
      case 'create_match': {
        if (!msg.match || !msg.match.id) break;
        if (msg.match.private) {
          // Private match — assign a room code, don't broadcast to others
          const code = genCode();
          msg.match.room_code = code;
          codeToId.set(code, msg.match.id);
          matches.set(msg.match.id, msg.match);
          ws.send(JSON.stringify({ t: 'match_created_private', match: msg.match, code }));
        } else {
          matches.set(msg.match.id, msg.match);
          broadcast({ t: 'match_created', match: msg.match }, ws);
        }
        break;
      }
      case 'join_code': {
        const matchId = codeToId.get(msg.code?.toUpperCase());
        if (!matchId) {
          ws.send(JSON.stringify({ t: 'error', msg: 'Invalid room code' }));
          break;
        }
        const m = matches.get(matchId);
        if (!m || m.state !== 'open') {
          ws.send(JSON.stringify({ t: 'error', msg: 'Match not found or not open' }));
          break;
        }
        if (m.players.length >= m.max_players) {
          ws.send(JSON.stringify({ t: 'error', msg: 'Match is full' }));
          break;
        }
        if (m.players.find(p => p.addr === msg.addr)) {
          ws.send(JSON.stringify({ t: 'error', msg: 'Already in match' }));
          break;
        }
        m.players.push({ addr: msg.addr, simulated: false, ready: false });
        if (m.players.length >= m.max_players) m.state = 'active';
        matches.set(m.id, m);
        // Send the full match to the joiner
        ws.send(JSON.stringify({ t: 'joined_private', match: m }));
        // Notify all clients who have this match
        broadcast({ t: 'match_updated', match: m });
        break;
      }
      case 'join_match': {
        const m = matches.get(msg.id);
        if (!m || m.state !== 'open') {
          ws.send(JSON.stringify({ t: 'error', msg: 'Match not found or not open' }));
          break;
        }
        if (m.players.length >= m.max_players) {
          ws.send(JSON.stringify({ t: 'error', msg: 'Match is full' }));
          break;
        }
        if (m.players.find(p => p.addr === msg.addr)) {
          ws.send(JSON.stringify({ t: 'error', msg: 'Already in match' }));
          break;
        }
        m.players.push({ addr: msg.addr, simulated: false, ready: false });
        if (m.players.length >= m.max_players) {
          m.state = 'active';
        }
        matches.set(m.id, m);
        broadcast({ t: 'match_updated', match: m });
        break;
      }
      case 'cancel_match': {
        const m = matches.get(msg.id);
        if (!m) break;
        m.state = 'cancelled';
        matches.set(m.id, m);
        broadcast({ t: 'match_updated', match: m });
        break;
      }
      case 'update_match': {
        if (!msg.match || !msg.match.id) break;
        matches.set(msg.match.id, msg.match);
        broadcast({ t: 'match_updated', match: msg.match }, ws);
        break;
      }
      case 'ready': {
        const m = matches.get(msg.id);
        if (!m) break;
        const player = m.players.find(p => p.addr === msg.addr);
        if (player) player.ready = true;
        matches.set(m.id, m);
        broadcast({ t: 'match_updated', match: m });
        break;
      }
    }
  });

  ws.on('close', () => {
    clients.delete(ws);
  });
});

// HTTP server for health checks
const server = createServer((req, res) => {
  if (req.url === '/health') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({
      matches: matches.size,
      clients: clients.size,
      uptime: process.uptime()
    }));
    return;
  }
  res.writeHead(404);
  res.end();
});

server.on('upgrade', (req, socket, head) => {
  wss.handleUpgrade(req, socket, head, (ws) => {
    wss.emit('connection', ws, req);
  });
});

// Cleanup stale matches every 10 minutes
setInterval(cleanupStaleMatches, 10 * 60 * 1000);

server.listen(PORT, () => {
  console.log(`Match relay server running on port ${PORT}`);
});
