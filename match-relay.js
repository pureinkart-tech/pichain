import { createServer } from 'http';
import { WebSocketServer } from 'ws';
import { readFileSync, existsSync } from 'fs';
import { Connection, Keypair, PublicKey, SystemProgram, Transaction, sendAndConfirmTransaction, LAMPORTS_PER_SOL } from '@solana/web3.js';

const PORT = Number(process.env.PORT || 8403);

// --- Solana escrow ---
const SOL_RPC = 'https://solana-rpc.publicnode.com';
const solConn = new Connection(SOL_RPC, 'confirmed');
let escrowKeypair = null;
let ESCROW_PUBKEY = null;
const SOL_ESCROW_PATH = './live-data/sol-escrow-keypair.json';
if (existsSync(SOL_ESCROW_PATH)) {
  try {
    escrowKeypair = Keypair.fromSecretKey(new Uint8Array(JSON.parse(readFileSync(SOL_ESCROW_PATH, 'utf8'))));
    ESCROW_PUBKEY = escrowKeypair.publicKey.toBase58();
    console.log('SOL escrow loaded:', ESCROW_PUBKEY);
  } catch (e) {
    console.error('Failed to load SOL escrow keypair:', e.message);
  }
}
const SOL_HOUSE_FEE = 0.02; // 2%
const HOUSE_WALLET = '6RDecDCXVPrg6T2QHicbWnS3LBhUxMumY9D4wE3EPyhX';

// Accumulate fees and forward to house wallet weekly
let accumulatedFeeLamports = 0;
const WEEKLY_MS = 7 * 24 * 60 * 60 * 1000;

function forwardFeeToHouse(feeLamports) {
  if (feeLamports <= 0) return;
  accumulatedFeeLamports += feeLamports;
  console.log(`Fee accumulated: +${feeLamports} lamports (total pending: ${accumulatedFeeLamports})`);
}

async function flushFeesToHouse() {
  if (!escrowKeypair || accumulatedFeeLamports <= 0) return;
  const amount = accumulatedFeeLamports;
  try {
    const tx = new Transaction().add(
      SystemProgram.transfer({
        fromPubkey: escrowKeypair.publicKey,
        toPubkey: new PublicKey(HOUSE_WALLET),
        lamports: amount,
      })
    );
    const sig = await sendAndConfirmTransaction(solConn, tx, [escrowKeypair]);
    accumulatedFeeLamports = 0;
    console.log(`Weekly fee flush: ${amount} lamports to house wallet, sig: ${sig}`);
  } catch (e) {
    console.error('Weekly fee flush failed:', e.message);
  }
}

// Flush fees once a week
setInterval(flushFeesToHouse, WEEKLY_MS);

// SOL match tracking
const solMatches = new Map();      // match_id -> { wager_lamports, players: Map<pubkey, {deposited,addr}>, state, winner, dispute_votes: Map<addr,proposed_winner>, acceptors: Set<addr>, dispute_deadline, creator }
const solPayoutPending = new Map(); // match_id -> timeout handle

// SOL poker chip balances
const solPokerBalances = new Map(); // pubkey -> { chips: number, totalDeposited: number }
const LAMPORTS_PER_CHIP = 10_000_000; // 1 SOL = 100 chips → 10M lamports per chip
const processedBuyInSigs = new Set(); // prevent double-crediting same tx signature
const processedMatchDepositSigs = new Set(); // prevent reusing one deposit signature across multiple SOL matches

// Reconstruct chip balances from on-chain escrow history on startup
async function reconstructChipBalances() {
  if (!ESCROW_PUBKEY) return;
  console.log('Reconstructing chip balances from on-chain history...');
  try {
    // Fetch all recent signatures for the escrow address
    let allSigs = [];
    let before = undefined;
    for (let page = 0; page < 10; page++) { // up to 10 pages of 1000
      const opts = { limit: 1000 };
      if (before) opts.before = before;
      const sigs = await solConn.getSignaturesForAddress(new PublicKey(ESCROW_PUBKEY), opts);
      if (!sigs || sigs.length === 0) break;
      allSigs = allSigs.concat(sigs);
      before = sigs[sigs.length - 1].signature;
      if (sigs.length < 1000) break; // no more pages
    }
    console.log(`Found ${allSigs.length} escrow transactions to scan`);

    // Track per-pubkey: deposits in, cashouts out
    const deposits = new Map(); // pubkey -> total lamports deposited
    const cashouts = new Map(); // pubkey -> total lamports cashed out

    // Process in chronological order (oldest first)
    for (const sigInfo of allSigs.reverse()) {
      if (sigInfo.err) continue; // skip failed txs
      try {
        const tx = await solConn.getTransaction(sigInfo.signature, {
          commitment: 'confirmed',
          maxSupportedTransactionVersion: 0
        });
        if (!tx || !tx.meta || tx.meta.err) continue;
        const pre = tx.meta.preBalances;
        const post = tx.meta.postBalances;
        const keys = tx.transaction.message.staticAccountKeys || tx.transaction.message.accountKeys;
        const escrowIdx = keys.findIndex(k => k.toBase58() === ESCROW_PUBKEY);
        if (escrowIdx < 0) continue;
        const escrowDiff = post[escrowIdx] - pre[escrowIdx];

        if (escrowDiff > 0) {
          // Deposit TO escrow — find who sent it (account with negative diff, not fee payer system)
          for (let i = 0; i < keys.length; i++) {
            if (i === escrowIdx) continue;
            const diff = post[i] - pre[i];
            // Sender loses more than just fee (they transferred funds)
            if (diff < -5000) { // more than just tx fee
              const senderPubkey = keys[i].toBase58();
              deposits.set(senderPubkey, (deposits.get(senderPubkey) || 0) + escrowDiff);
              processedBuyInSigs.add(sigInfo.signature); // mark as processed to prevent double-credit
              break;
            }
          }
        } else if (escrowDiff < 0) {
          // Cashout FROM escrow — find who received it
          for (let i = 0; i < keys.length; i++) {
            if (i === escrowIdx) continue;
            const diff = post[i] - pre[i];
            if (diff > 0) {
              const receiverPubkey = keys[i].toBase58();
              // Skip house wallet — that's fee forwarding, not cashout
              if (receiverPubkey === HOUSE_WALLET) continue;
              cashouts.set(receiverPubkey, (cashouts.get(receiverPubkey) || 0) + diff);
              break;
            }
          }
        }
      } catch (e) {
        // Skip individual tx errors, continue scanning
      }
    }

    // Calculate net chip balance per pubkey
    let restored = 0;
    for (const [pubkey, totalDeposited] of deposits) {
      const totalCashedOut = cashouts.get(pubkey) || 0;
      const netLamports = totalDeposited - totalCashedOut;
      if (netLamports > 0) {
        const chips = Math.floor(netLamports / LAMPORTS_PER_CHIP);
        if (chips > 0) {
          const existing = solPokerBalances.get(pubkey);
          if (!existing || existing.chips < chips) {
            solPokerBalances.set(pubkey, { chips, totalDeposited });
            restored++;
            console.log(`  Restored ${pubkey.slice(0,8)}.. → ${chips} chips (deposited: ${totalDeposited/1e9} SOL, cashed out: ${totalCashedOut/1e9} SOL)`);
          }
        }
      }
    }
    console.log(`Chip balance reconstruction complete: ${restored} accounts restored`);
  } catch (e) {
    console.error('Chip balance reconstruction failed:', e.message);
  }
}

// Run reconstruction on startup (non-blocking)
reconstructChipBalances();

// In-memory match store
const matches = new Map();
const codeToId = new Map(); // room_code -> match_id for private matches
const clients = new Set();
const sfRooms = new Map(); // key -> { key, p1, p2, rematch:Set<WebSocket> }
const sfPeer = new Map();  // ws -> { key, slot: 'p1'|'p2' }

// Presence tracking for betting matches
const matchPresence = new Map();  // match_id -> Set<addr> of connected players
const cancelVotes = new Map();    // match_id -> Set<addr> of cancel voters
const wsPlayerMap = new Map();    // ws -> { addr, mid } for disconnect cleanup
const wsMatchMap = new Map();     // ws -> { addr, mid } for lobby disconnect cleanup

// Poker table store (in-memory, shared across all clients)
const pokerTablesRelay = new Map(); // table_id -> table object (no passcode)
const wsPokerMap = new Map();      // ws -> { table_id, addr } for disconnect cleanup

// Arcade queue board
const arcadeQueue = new Map();   // queue_id -> { id, game, wager_sol, addr, pubkey, currency, created, ws }
const wsQueueMap = new Map();    // ws -> Set<queue_id> for disconnect cleanup
const QUEUE_GAMES = new Set(['pong','connect4','reversi','battleship','checkers','dots','mines','bomberman','tanks','artillery','tron','airhockey']);

function genQueueId() {
  return Array.from({length:32}, () => Math.floor(Math.random()*16).toString(16)).join('');
}

function serializeQueueEntry(entry) {
  return { id:entry.id, game:entry.game, wager_sol:entry.wager_sol, addr:entry.addr, pubkey:entry.pubkey, currency:entry.currency, created:entry.created };
}

// Mk48 winner determination — called after each result and on timeout
function decideMk48Winner(mk48Match) {
  if (mk48Match.winner) return; // already decided
  const mk48Players = mk48Match.players || [];
  const mk48RealPlayers = mk48Players.filter(p => !p.simulated);
  const mk48Results = mk48Match._mk48Results || {};
  const mk48ReportedCount = Object.keys(mk48Results).length;
  const mk48DeadCount = Object.values(mk48Results).filter(r => !r.alive).length;
  const mk48RealCount = mk48RealPlayers.length || mk48Players.length;

  let mk48Winner = null;

  if (mk48DeadCount >= mk48RealCount - 1 && mk48RealCount > 1) {
    // All real players but one eliminated — last alive wins
    for (const p of mk48RealPlayers) {
      const r = mk48Results[p.addr];
      if (!r || r.alive) { mk48Winner = p.addr; break; }
    }
  } else if (mk48ReportedCount >= mk48RealCount) {
    // Time up — all reported. Winner: highest score (kills/damage). Alive beats dead.
    let bestScore = -1;
    for (const p of mk48Players) {
      const r = mk48Results[p.addr];
      if (!r) continue;
      // Alive players beat dead ones; among same alive status, highest game score wins
      const combinedScore = (r.alive ? 1000000 : 0) + (r.score || 0);
      if (combinedScore > bestScore) { bestScore = combinedScore; mk48Winner = p.addr; }
    }
  } else if (mk48ReportedCount > 0 && mk48ReportedCount < mk48RealCount) {
    // Partial results — decide if 15 min has passed
    const elapsed = Date.now() - (mk48Match.created || 0);
    if (elapsed > 900000) { // 15 min — match should be done
      console.log(`[MK48] Timeout: only ${mk48ReportedCount}/${mk48RealCount} reported, deciding with partial data`);
      let bestScore = -1;
      for (const p of mk48RealPlayers) {
        const r = mk48Results[p.addr];
        // Unreported players assumed alive with score 0
        const alive = !r || r.alive;
        const gameScore = r ? (r.score || 0) : 0;
        const combinedScore = (alive ? 1000000 : 0) + gameScore;
        if (combinedScore > bestScore) { bestScore = combinedScore; mk48Winner = p.addr; }
      }
    }
  }

  if (mk48Winner) {
    mk48Match.state = 'completed';
    mk48Match.winner = mk48Winner;
    matches.set(mk48Match.id, mk48Match);
    if (mk48Match._mk48Timeout) { clearTimeout(mk48Match._mk48Timeout); mk48Match._mk48Timeout = null; }
    const mk48WinMsg = JSON.stringify({ t: 'mk48_winner', mid: mk48Match.id, winner: mk48Winner });
    for (const [peer, peerInfo] of wsMatchMap) {
      if (peerInfo.mid === mk48Match.id && peer.readyState === 1) {
        try { peer.send(mk48WinMsg); } catch(_e){}
      }
    }
    broadcast({ t: 'match_updated', match: mk48Match });
    console.log(`[MK48] Match ${mk48Match.id} winner: ${mk48Winner} (dead=${mk48DeadCount} reported=${mk48ReportedCount}/${mk48Players.length})`);
  }
}

function removePlayerFromAllQueues(addr) {
  const removedIds = [];
  for (const [qid, entry] of arcadeQueue) {
    if (entry.addr === addr) {
      arcadeQueue.delete(qid);
      removedIds.push(qid);
      const qSet = wsQueueMap.get(entry.ws);
      if (qSet) qSet.delete(qid);
    }
  }
  for (const qid of removedIds) {
    broadcast({ t:'queue_removed', queue_id:qid });
  }
  return removedIds;
}

function genCode() {
  const chars = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
  let code;
  do {
    code = '';
    for (let i = 0; i < 5; i++) code += chars[Math.floor(Math.random() * chars.length)];
  } while (codeToId.has(code));
  return code;
}

// SECURITY (H2): Cap max message size at the WebSocket layer — rejects before parsing
const wss = new WebSocketServer({ noServer: true, maxPayload: 65536 });

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
    const age = now - m.created;
    // Open matches: expire after 10 minutes (no one joined)
    const isStale = (m.state === 'open' && age > 10 * 60 * 1000) ||
                    // Active/playing matches: expire after 30 minutes
                    ((m.state === 'active' || m.state === 'playing') && age > 30 * 60 * 1000) ||
                    // All matches: hard expire after 2 hours
                    (age > 2 * 60 * 60 * 1000);
    if (isStale) {
      if (m.room_code) codeToId.delete(m.room_code);
      matches.delete(id);
      matchPresence.delete(id);
      cancelVotes.delete(id);
      broadcast({ t:'match_expired', mid:id });
    }
  }
  // Clean up stale queue entries (older than 30 minutes)
  for (const [qid, entry] of arcadeQueue) {
    if (now - entry.created > 30 * 60 * 1000) {
      arcadeQueue.delete(qid);
      const qSet = wsQueueMap.get(entry.ws);
      if (qSet) qSet.delete(qid);
      broadcast({ t:'queue_removed', queue_id:qid });
    }
  }
  // Clean up stale poker tables (older than 4 hours)
  for (const [id, tbl] of pokerTablesRelay) {
    if (tbl.created && now - tbl.created > 4 * 60 * 60 * 1000) {
      pokerTablesRelay.delete(id);
    }
  }
  // Clean up stale SOL matches (paid or 2+ hours old with deadline passed)
  for (const [id, sm] of solMatches) {
    if (sm.state === 'paid' && sm.dispute_deadline && now - sm.dispute_deadline > 60 * 60 * 1000) {
      solMatches.delete(id);
      const timer = solPayoutPending.get(id);
      if (timer) { clearTimeout(timer); solPayoutPending.delete(id); }
    }
  }
}

function genSfCode() {
  const chars = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';
  let out = '';
  for (let i = 0; i < 4; i++) out += chars[Math.floor(Math.random() * chars.length)];
  return out;
}

function sfRoomFor(ws) {
  const peer = sfPeer.get(ws);
  if (!peer) return null;
  const room = sfRooms.get(peer.key);
  return room || null;
}

function sfStartCountdown(room) {
  const players = [room.p1, room.p2].filter(Boolean);
  if (players.length < 2) return;
  [3, 2, 1].forEach((c, idx) => {
    setTimeout(() => {
      players.forEach((p) => {
        if (p.readyState === 1) p.send(JSON.stringify({ t: 'countdown', c }));
      });
      if (c === 1) {
        const p1 = room.p1;
        const p2 = room.p2;
        if (p1 && p1.readyState === 1) p1.send(JSON.stringify({ t: 'game_start', slot: 'p1' }));
        if (p2 && p2.readyState === 1) p2.send(JSON.stringify({ t: 'game_start', slot: 'p2' }));
      }
    }, idx * 800);
  });
}

function sfLeave(ws) {
  const peer = sfPeer.get(ws);
  if (!peer) return;
  const room = sfRooms.get(peer.key);
  sfPeer.delete(ws);
  if (!room) return;

  const opp = peer.slot === 'p1' ? room.p2 : room.p1;
  if (peer.slot === 'p1') room.p1 = null;
  else room.p2 = null;
  room.rematch.delete(ws);

  if (opp && opp.readyState === 1) {
    opp.send(JSON.stringify({ t: 'opponent_disconnected' }));
  }

  if (!room.p1 && !room.p2) {
    sfRooms.delete(peer.key);
  } else {
    sfRooms.set(peer.key, room);
  }
}

function sfJoin(ws, keyRaw) {
  const key = String(keyRaw || '').trim().toUpperCase().replace(/[^A-Z0-9_-]/g, '').slice(0, 64);
  if (!key) {
    ws.send(JSON.stringify({ t: 'error', msg: 'Invalid room key' }));
    return;
  }

  // Ensure one room membership per socket.
  sfLeave(ws);

  let room = sfRooms.get(key);
  if (!room) {
    room = { key, p1: ws, p2: null, rematch: new Set() };
    sfRooms.set(key, room);
    sfPeer.set(ws, { key, slot: 'p1' });
    ws.send(JSON.stringify({ t: 'room_created', code: key.slice(0, 4) || genSfCode() }));
    return;
  }

  if (!room.p1) {
    room.p1 = ws;
    sfPeer.set(ws, { key, slot: 'p1' });
    ws.send(JSON.stringify({ t: 'room_created', code: key.slice(0, 4) || genSfCode() }));
    sfRooms.set(key, room);
    return;
  }

  if (!room.p2) {
    room.p2 = ws;
    sfPeer.set(ws, { key, slot: 'p2' });
    ws.send(JSON.stringify({ t: 'room_joined' }));
    if (room.p1.readyState === 1) room.p1.send(JSON.stringify({ t: 'opponent_joined' }));
    sfRooms.set(key, room);
    sfStartCountdown(room);
    return;
  }

  ws.send(JSON.stringify({ t: 'error', msg: 'Room is full' }));
}

// --- Solana helper functions ---
async function verifySolDeposit(signature, expectedLamports, fromPubkey) {
  try {
    const tx = await solConn.getTransaction(signature, { commitment: 'confirmed', maxSupportedTransactionVersion: 0 });
    if (!tx || !tx.meta || tx.meta.err) return false;
    const preBalances = tx.meta.preBalances;
    const postBalances = tx.meta.postBalances;
    const keys = tx.transaction.message.staticAccountKeys || tx.transaction.message.accountKeys;
    const escrowIdx = keys.findIndex(k => k.toBase58() === ESCROW_PUBKEY);
    if (escrowIdx < 0) return false;
    const received = postBalances[escrowIdx] - preBalances[escrowIdx];
    if (received < expectedLamports) return false;

    // SECURITY: Verify the sender matches the claimed pubkey
    // The fee payer (index 0) or a signer must match fromPubkey
    if (fromPubkey) {
      const senderIdx = keys.findIndex(k => k.toBase58() === fromPubkey);
      if (senderIdx < 0) {
        console.warn(`verifySolDeposit: claimed sender ${fromPubkey.slice(0,8)}.. not found in tx signers`);
        return false;
      }
      // Verify the sender actually lost funds (sent SOL)
      const senderDiff = postBalances[senderIdx] - preBalances[senderIdx];
      if (senderDiff >= 0) {
        console.warn(`verifySolDeposit: claimed sender ${fromPubkey.slice(0,8)}.. did not lose funds (diff: ${senderDiff})`);
        return false;
      }
    }
    return true;
  } catch (e) {
    console.error('verifySolDeposit error:', e.message);
    return false;
  }
}

async function sendSolPayout(toPubkey, lamports) {
  if (!escrowKeypair) throw new Error('No escrow keypair loaded');
  const tx = new Transaction().add(
    SystemProgram.transfer({
      fromPubkey: escrowKeypair.publicKey,
      toPubkey: new PublicKey(toPubkey),
      lamports,
    })
  );
  const sig = await sendAndConfirmTransaction(solConn, tx, [escrowKeypair]);
  return sig;
}

async function executeSolPayout(matchId) {
  const sm = solMatches.get(matchId);
  if (!sm || sm.state === 'paid') return;
  sm.state = 'paid';
  solMatches.set(matchId, sm);

  const playerCount = sm.players.size;
  const totalPot = sm.wager_lamports * playerCount;
  const fee = Math.floor(totalPot * SOL_HOUSE_FEE);
  const payout = totalPot - fee;

  // Find winner's pubkey
  let winnerPubkey = null;
  for (const [pubkey, info] of sm.players) {
    if (info.addr === sm.winner) { winnerPubkey = pubkey; break; }
  }
  if (!winnerPubkey) {
    console.error('executeSolPayout: winner pubkey not found for', sm.winner);
    broadcast({ t: 'sol_payout_error', mid: matchId, error: 'Winner pubkey not found' });
    return;
  }

  try {
    const sig = await sendSolPayout(winnerPubkey, payout);
    broadcast({ t: 'sol_payout_complete', mid: matchId, winner: sm.winner, amount: payout, fee, signature: sig });
    console.log(`SOL payout: ${payout} lamports to ${sm.winner}, fee: ${fee}, sig: ${sig}`);
    if (fee > 0) forwardFeeToHouse(fee);
  } catch (e) {
    console.error('SOL payout failed:', e.message);
    broadcast({ t: 'sol_payout_error', mid: matchId, error: e.message });
  }

  // Clear dispute timer
  const timer = solPayoutPending.get(matchId);
  if (timer) { clearTimeout(timer); solPayoutPending.delete(matchId); }
}

// --- Security: rate limiting & identity binding ---
const wsIdentity = new WeakMap(); // ws -> { pubkey, lastBuyIn, lastCashOut, buyInCount, cashOutCount }
const RATE_LIMIT_BUYIN_MS = 10_000;    // min 10s between buy-ins
const RATE_LIMIT_CASHOUT_MS = 30_000;  // min 30s between cashouts
const MAX_BUYIN_PER_HOUR = 20;
const MAX_CASHOUT_PER_HOUR = 10;
const cashOutLocks = new Set(); // pubkeys currently processing a cashout

// SECURITY: WebSocket-to-address binding — once a ws sends a message with an addr,
// all subsequent messages from that ws must use the same addr. Prevents identity spoofing.
const wsAddrBinding = new WeakMap(); // ws -> bound addr (string)

// Bind or validate ws<->addr. Returns true if valid, false if mismatch.
function validateWsAddr(ws, addr) {
  if (!addr || typeof addr !== 'string') return false;
  const bound = wsAddrBinding.get(ws);
  if (!bound) {
    wsAddrBinding.set(ws, addr);
    return true;
  }
  return bound === addr;
}

// Get the bound addr for a ws (null if not yet bound)
function getWsBoundAddr(ws) {
  return wsAddrBinding.get(ws) || null;
}

// SECURITY: Track SOL poker table buy-ins per pubkey per table
// Prevents sol_poker_return from crediting more chips than were bought in
const pokerTableStakes = new Map(); // "pubkey:table_id" -> chips bought in

function recordTableBuyIn(pubkey, tableId, chips) {
  const key = pubkey + ':' + tableId;
  pokerTableStakes.set(key, (pokerTableStakes.get(key) || 0) + chips);
}

function getTableStake(pubkey, tableId) {
  return pokerTableStakes.get(pubkey + ':' + tableId) || 0;
}

function consumeTableStake(pubkey, tableId, chips) {
  const key = pubkey + ':' + tableId;
  const current = pokerTableStakes.get(key) || 0;
  const allowed = Math.min(chips, current);
  if (allowed <= 0) return 0;
  const remaining = current - allowed;
  if (remaining <= 0) pokerTableStakes.delete(key);
  else pokerTableStakes.set(key, remaining);
  return allowed;
}

// SECURITY: Track which ws created which poker table (ws -> table_id)
const wsPokerCreatorMap = new WeakMap(); // ws -> table_id (for verifying settle requests)

// SECURITY: Rate limit poker table creation per address
const pokerTableCreateTimes = new Map(); // addr -> [timestamp, timestamp, ...]
const MAX_TABLES_PER_HOUR = 10;

function getWsIdentity(ws) {
  if (!wsIdentity.has(ws)) wsIdentity.set(ws, { pubkey: null, lastBuyIn: 0, lastCashOut: 0, buyInCount: 0, cashOutCount: 0, hourReset: Date.now() });
  const id = wsIdentity.get(ws);
  if (Date.now() - id.hourReset > 3600_000) { id.buyInCount = 0; id.cashOutCount = 0; id.hourReset = Date.now(); }
  return id;
}

wss.on('connection', (ws) => {
  clients.add(ws);

  // Send all public (non-private) matches to the new client
  const publicMatches = Array.from(matches.values()).filter(m => !m.private);
  ws.send(JSON.stringify({ t: 'sync', matches: publicMatches }));

  // Send current arcade queue board
  const queueEntries = Array.from(arcadeQueue.values()).map(serializeQueueEntry);
  if (queueEntries.length > 0) ws.send(JSON.stringify({ t:'queue_sync', entries:queueEntries }));

  // Send current poker tables
  if (pokerTablesRelay.size > 0) {
    ws.send(JSON.stringify({ t: 'poker_tables_sync', tables: Array.from(pokerTablesRelay.values()) }));
  }

  ws.on('message', async (raw) => {
    let msg;
    try { msg = JSON.parse(raw); } catch { return; }

    // Note: maxPayload on WebSocketServer enforces 64KB limit before parsing

    // SECURITY (Critical #4): WebSocket address binding
    // Once a ws sends msg.addr, all future messages must use the same addr.
    // This prevents identity spoofing where an attacker sends messages as another player.
    if (msg.addr && typeof msg.addr === 'string') {
      if (!validateWsAddr(ws, msg.addr)) {
        ws.send(JSON.stringify({ t: 'error', error: 'Address mismatch — this connection is bound to a different address' }));
        return;
      }
    }

    switch (msg.t) {
      case 'sf_join_match': {
        sfJoin(ws, msg.match_id);
        break;
      }
      case 'create_room': {
        // Street Fighter room create/join flow.
        // If code is supplied, create-or-join that deterministic room.
        // If not supplied, create random room code.
        const code = msg.code ? String(msg.code).toUpperCase() : genSfCode();
        sfJoin(ws, code);
        break;
      }
      case 'join_room': {
        sfJoin(ws, msg.code);
        break;
      }
      case 'input': {
        const room = sfRoomFor(ws);
        if (!room) break;
        const peer = sfPeer.get(ws);
        if (!peer) break;
        const opp = peer.slot === 'p1' ? room.p2 : room.p1;
        if (opp && opp.readyState === 1) {
          const fwd = { t: 'ri', h: Array.isArray(msg.h) ? msg.h : [] };
          if (msg.p) fwd.p = msg.p;
          opp.send(JSON.stringify(fwd));
        }
        break;
      }
      case 'hs': {
        // Health + position sync: P1 sends authoritative state to P2
        const room = sfRoomFor(ws);
        if (!room) break;
        const peer = sfPeer.get(ws);
        if (!peer || peer.slot !== 'p1') break; // Only P1 is authoritative
        if (room.p2 && room.p2.readyState === 1) {
          const fwd = { t: 'hs', hp: msg.hp };
          if (msg.pos) fwd.pos = msg.pos;
          if (msg.st) fwd.st = msg.st;
          room.p2.send(JSON.stringify(fwd));
        }
        break;
      }
      case 'gr': {
        // Game result: P1 sends authoritative result to P2
        const room = sfRoomFor(ws);
        if (!room) break;
        const peer = sfPeer.get(ws);
        if (!peer || peer.slot !== 'p1') break; // Only P1 is authoritative
        const players = [room.p1, room.p2].filter(Boolean);
        players.forEach(p => {
          if (p.readyState === 1) p.send(JSON.stringify({ t: 'gr', winner: msg.winner }));
        });
        break;
      }
      case 'rematch_request': {
        const room = sfRoomFor(ws);
        if (!room) break;
        room.rematch.add(ws);
        const players = [room.p1, room.p2].filter(Boolean);
        if (players.length === 2) {
          players.forEach((p) => {
            if (p.readyState === 1) p.send(JSON.stringify({ t: 'rematch_request' }));
          });
        }
        if (room.rematch.size >= 2 && room.p1 && room.p2) {
          room.rematch.clear();
          [room.p1, room.p2].forEach((p) => {
            if (p.readyState === 1) p.send(JSON.stringify({ t: 'rematch_accepted' }));
          });
          sfStartCountdown(room);
        }
        sfRooms.set(room.key, room);
        break;
      }
      case 'create_match': {
        if (!msg.match || !msg.match.id) break;
        // SECURITY (H4): Prevent overwriting existing matches via ID collision
        if (matches.has(msg.match.id)) {
          ws.send(JSON.stringify({ t: 'error', error: 'Match ID already exists' })); break;
        }
        // Track creator ws for lobby disconnect cleanup
        if (msg.match.creator) wsMatchMap.set(ws, { addr: msg.match.creator, mid: msg.match.id });
        if (msg.match.private) {
          // Use custom room code from client if provided, otherwise generate one
          const code = (msg.match.room_code && msg.match.room_code.length >= 3) ? msg.match.room_code.toUpperCase() : genCode();
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
        if (!m.unlimited && m.players.length >= m.max_players) m.state = 'active';
        matches.set(m.id, m);
        // Track joiner ws for lobby disconnect cleanup
        wsMatchMap.set(ws, { addr: msg.addr, mid: m.id });
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
        // Unlimited matches don't auto-activate — they wait for the countdown timer
        if (!m.unlimited && m.players.length >= m.max_players) {
          m.state = 'active';
        }
        matches.set(m.id, m);
        // Track joiner ws for lobby disconnect cleanup
        wsMatchMap.set(ws, { addr: msg.addr, mid: m.id });
        broadcast({ t: 'match_updated', match: m });
        break;
      }
      case 'unlimited_start': {
        const m = matches.get(msg.mid);
        if (!m || !m.unlimited || m.state !== 'open') break;
        if (m.players.length < 2) break;
        m.state = 'active';
        m.players.forEach(p => { if (p.simulated) p.ready = true; });
        matches.set(m.id, m);
        broadcast({ t: 'match_updated', match: m });
        break;
      }
      case 'cancel_match': {
        const m = matches.get(msg.id);
        if (!m) break;
        // Allow cancel if: creator, OR if reason is disconnect (any participant)
        const cancelAddr = getWsBoundAddr(ws);
        const isCancelCreator = m.creator && m.creator === cancelAddr;
        const isParticipant = m.players && m.players.some(p => p.addr === cancelAddr);
        const isDisconnectCancel = msg.reason === 'disconnect';
        if (!cancelAddr || (!isCancelCreator && !(isDisconnectCancel && isParticipant))) {
          ws.send(JSON.stringify({ t: 'error', error: 'Only the match creator can cancel' }));
          break;
        }
        if (m.state === 'completed') break; // Can't cancel a completed match
        m.state = 'cancelled';
        matches.set(m.id, m);
        broadcast({ t: 'match_updated', match: m });
        break;
      }
      case 'match_completed': {
        const m = matches.get(msg.mid);
        if (!m) break;
        if (m.state === 'cancelled') break; // Already cancelled
        m.state = 'completed';
        m.winner = msg.winner;
        matches.set(m.mid, m);
        broadcast({ t: 'match_updated', match: m });
        break;
      }
      case 'race_pos': {
        // Racing: relay player position to all other clients (filtered by match_id client-side)
        if (!msg.mid) break;
        broadcast({ t: 'race_pos', mid: msg.mid, addr: msg.addr, z: msg.z, x: msg.x, lap: msg.lap, spd: msg.spd, fin: msg.fin }, ws);
        break;
      }
      case 'race_item': {
        if (!msg.mid) break;
        broadcast({ t: 'race_item', mid: msg.mid, addr: msg.addr, item: msg.item, z: msg.z, x: msg.x, seg: msg.seg, target: msg.target }, ws);
        break;
      }
      // Arcade 1v1: forward moves between players
      case 'arcade_move': {
        if (!msg.mid || !msg.move) break;
        broadcast({ t: 'arcade_move', mid: msg.mid, addr: msg.addr, move: msg.move }, ws);
        break;
      }
      case 'pvp_player_ready': {
        if (!msg.mid || !msg.addr) break;
        // Persist ready state in the match so re-syncs have correct status
        const readyMatch = matches.get(msg.mid);
        if (readyMatch && Array.isArray(readyMatch.players)) {
          const rp = readyMatch.players.find(p => p.addr === msg.addr);
          if (rp) rp.ready = !!msg.ready;
          matches.set(msg.mid, readyMatch);
        }
        broadcast({ t: 'pvp_player_ready', mid: msg.mid, addr: msg.addr, ready: msg.ready }, ws);
        break;
      }
      case 'arcade_player_ready': {
        if (!msg.mid) break;
        broadcast({ t: 'arcade_player_ready', mid: msg.mid, addr: msg.addr }, ws);
        break;
      }
      case 'arcade_rematch_ready': {
        if (!msg.mid) break;
        broadcast({ t: 'arcade_rematch_ready', mid: msg.mid, addr: msg.addr }, ws);
        break;
      }
      case 'arcade_result': {
        if (!msg.mid) break;
        broadcast({ t: 'arcade_result', mid: msg.mid, addr: msg.addr, winner: msg.winner, score: msg.score }, ws);
        break;
      }
      case 'arcade_leave': {
        if (!msg.mid) break;
        broadcast({ t: 'arcade_leave', mid: msg.mid, addr: msg.addr }, ws);
        break;
      }

      // ── Arcade Queue Board ──────────────────────────────────
      case 'queue_join': {
        if (!msg.game || !msg.addr || !QUEUE_GAMES.has(msg.game)) break;
        const wagerSol = Number(msg.wager_sol) || 0;
        // Prevent duplicate: same addr + game + wager
        let queueDuplicate = false;
        for (const [, qe] of arcadeQueue) {
          if (qe.addr === msg.addr && qe.game === msg.game && qe.wager_sol === wagerSol) {
            ws.send(JSON.stringify({ t:'queue_error', error:'Already queued for this game at this wager' }));
            queueDuplicate = true;
            break;
          }
        }
        if (queueDuplicate) break;
        const qid = genQueueId();
        const entry = { id:qid, game:msg.game, wager_sol:wagerSol, addr:msg.addr, pubkey:msg.pubkey||null, currency:msg.currency||'PI', created:Date.now(), ws };
        arcadeQueue.set(qid, entry);
        if (!wsQueueMap.has(ws)) wsQueueMap.set(ws, new Set());
        wsQueueMap.get(ws).add(qid);
        broadcast({ t:'queue_added', entry:serializeQueueEntry(entry) });
        break;
      }
      case 'queue_cancel': {
        if (!msg.queue_id || !msg.addr) break;
        const entry = arcadeQueue.get(msg.queue_id);
        if (!entry || entry.addr !== msg.addr) break;
        arcadeQueue.delete(msg.queue_id);
        const qSet = wsQueueMap.get(ws);
        if (qSet) qSet.delete(msg.queue_id);
        broadcast({ t:'queue_removed', queue_id:msg.queue_id });
        break;
      }
      case 'queue_accept': {
        if (!msg.queue_id || !msg.addr) break;
        const entry = arcadeQueue.get(msg.queue_id);
        if (!entry) { ws.send(JSON.stringify({ t:'queue_error', error:'Queue entry no longer available' })); break; }
        if (entry.addr === msg.addr) { ws.send(JSON.stringify({ t:'queue_error', error:'Cannot join your own queue' })); break; }

        // Create match from queue entry
        const matchId = genQueueId();
        const roomCode = genCode();
        const match = {
          id:matchId, game_type:'arcade_'+entry.game, wager:entry.wager_sol, currency:entry.currency,
          max_players:2, players:[{addr:entry.addr,simulated:false,ready:false},{addr:msg.addr,simulated:false,ready:false}],
          creator:entry.addr, state:'active', created:Date.now(), private:true, room_code:roomCode, from_queue:true
        };
        matches.set(matchId, match);
        codeToId.set(roomCode, matchId);

        // SOL escrow if needed
        if (entry.currency === 'SOL' && entry.wager_sol > 0) {
          const players = new Map();
          if (entry.pubkey) players.set(entry.pubkey, { deposited:false, addr:entry.addr });
          if (msg.pubkey) players.set(msg.pubkey, { deposited:false, addr:msg.addr });
          solMatches.set(matchId, { wager_lamports:Math.round(entry.wager_sol*1e9), players, state:'awaiting_deposits', winner:null, dispute_votes:new Map(), acceptors:new Set(), dispute_deadline:null, creator:entry.addr });
        }

        // Remove both players from all queues
        removePlayerFromAllQueues(entry.addr);
        removePlayerFromAllQueues(msg.addr);

        // Notify both parties
        if (entry.ws && entry.ws.readyState === 1) {
          entry.ws.send(JSON.stringify({ t:'queue_matched', match, role:1 }));
        }
        ws.send(JSON.stringify({ t:'queue_matched', match, role:2 }));
        break;
      }

      case 'update_match': {
        if (!msg.match || !msg.match.id) break;
        const existing = matches.get(msg.match.id);
        if (!existing) break;

        // SECURITY (Critical #3): Only match creator or participants can update
        const senderAddr = getWsBoundAddr(ws);
        const isCreator = existing.creator && existing.creator === senderAddr;
        const isParticipant = Array.isArray(existing.players) && existing.players.some(p => p.addr === senderAddr);
        if (!senderAddr || (!isCreator && !isParticipant)) {
          ws.send(JSON.stringify({ t: 'error', error: 'Not authorized to update this match' }));
          break;
        }

        // SECURITY: Only allow updating safe fields — never overwrite id, creator, players list, wager, or currency
        const safeFields = ['state', 'ready', 'score', 'winner', 'round', 'turn', 'game_data'];
        for (const field of safeFields) {
          if (msg.match[field] !== undefined) existing[field] = msg.match[field];
        }
        // Allow player ready/score updates within existing players array
        if (Array.isArray(msg.match.players)) {
          for (const up of msg.match.players) {
            const ep = existing.players.find(p => p.addr === up.addr);
            if (ep) {
              if (up.ready !== undefined) ep.ready = up.ready;
              if (up.score !== undefined) ep.score = up.score;
            }
          }
        }
        matches.set(existing.id, existing);
        broadcast({ t: 'match_updated', match: existing }, ws);
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

      // --- Presence tracking ---
      case 'game_connected': {
        if (!msg.mid || !msg.addr) break;
        if (!matchPresence.has(msg.mid)) matchPresence.set(msg.mid, new Set());
        matchPresence.get(msg.mid).add(msg.addr);
        wsPlayerMap.set(ws, { addr: msg.addr, mid: msg.mid });
        broadcast({ t: 'presence_update', mid: msg.mid, connected: Array.from(matchPresence.get(msg.mid)) });
        break;
      }
      case 'game_disconnected': {
        if (!msg.mid || !msg.addr) break;
        const pres = matchPresence.get(msg.mid);
        if (pres) {
          pres.delete(msg.addr);
          broadcast({ t: 'presence_update', mid: msg.mid, connected: Array.from(pres), left: msg.addr });
        }
        break;
      }

      // --- Cancel voting ---
      case 'vote_cancel': {
        if (!msg.mid || !msg.addr) break;
        // SECURITY (H6): Only match participants can vote to cancel
        const vcMatch = matches.get(msg.mid);
        if (!vcMatch) break;
        if (!Array.isArray(vcMatch.players) || !vcMatch.players.some(p => p.addr === msg.addr)) {
          ws.send(JSON.stringify({ t: 'error', error: 'Only match participants can vote to cancel' })); break;
        }
        if (!cancelVotes.has(msg.mid)) cancelVotes.set(msg.mid, new Set());
        cancelVotes.get(msg.mid).add(msg.addr);
        const votes = cancelVotes.get(msg.mid);
        const total = vcMatch.players.length;
        const unanimous = total > 0 && votes.size >= total;
        broadcast({ t: 'cancel_vote_update', mid: msg.mid, votes: votes.size, total, unanimous, voters: Array.from(votes) });
        break;
      }
      case 'unvote_cancel': {
        if (!msg.mid || !msg.addr) break;
        // SECURITY (H6): Only match participants can unvote
        const uvMatch = matches.get(msg.mid);
        if (!uvMatch) break;
        if (!Array.isArray(uvMatch.players) || !uvMatch.players.some(p => p.addr === msg.addr)) break;
        const vv = cancelVotes.get(msg.mid);
        if (vv) vv.delete(msg.addr);
        const voteCount = vv ? vv.size : 0;
        const total2 = uvMatch.players.length;
        broadcast({ t: 'cancel_vote_update', mid: msg.mid, votes: voteCount, total: total2, unanimous: false, voters: vv ? Array.from(vv) : [] });
        break;
      }

      // --- Solana betting ---
      case 'get_escrow_info': {
        if (!ESCROW_PUBKEY) {
          ws.send(JSON.stringify({ t: 'error', msg: 'SOL escrow not configured' }));
          break;
        }
        ws.send(JSON.stringify({ t: 'escrow_info', pubkey: ESCROW_PUBKEY }));
        break;
      }
      case 'sol_create_match': {
        if (!msg.mid || !msg.wager_lamports || !msg.addr || !msg.pubkey) break;
        if (!ESCROW_PUBKEY) { ws.send(JSON.stringify({ t: 'error', msg: 'SOL escrow not configured' })); break; }
        // SECURITY: Validate wager_lamports is a positive integer within bounds
        const wagerVal = Number(msg.wager_lamports);
        if (!Number.isInteger(wagerVal) || wagerVal < 1000 || wagerVal > 100 * LAMPORTS_PER_SOL) {
          ws.send(JSON.stringify({ t: 'error', error: 'Invalid wager amount (must be 1000 lamports to 100 SOL)' })); break;
        }
        // SECURITY (H5): Prevent overwriting existing SOL matches (could lose escrowed funds)
        if (solMatches.has(msg.mid)) {
          ws.send(JSON.stringify({ t: 'error', error: 'SOL match ID already exists' })); break;
        }
        const players = new Map();
        players.set(msg.pubkey, { deposited: false, addr: msg.addr });
        solMatches.set(msg.mid, {
          wager_lamports: wagerVal,
          players,
          state: 'awaiting_deposits',
          winner: null,
          dispute_votes: new Map(),
          acceptors: new Set(),
          dispute_deadline: null,
          creator: msg.addr,
        });
        break;
      }
      case 'sol_join_match': {
        if (!msg.mid || !msg.addr || !msg.pubkey) break;
        const sm = solMatches.get(msg.mid);
        if (!sm) break;
        // SECURITY: Only allow joining during awaiting_deposits phase
        if (sm.state !== 'awaiting_deposits') {
          ws.send(JSON.stringify({ t: 'error', error: 'Match is no longer accepting players' })); break;
        }
        // SECURITY: Prevent duplicate pubkey
        if (sm.players.has(msg.pubkey)) {
          ws.send(JSON.stringify({ t: 'error', error: 'Already in this SOL match' })); break;
        }
        // SECURITY: Cap at 2 players for 1v1 SOL matches — prevents payout inflation
        // (totalPot = wager * players.size, so extra players would inflate payout beyond escrow)
        if (sm.players.size >= 2) {
          ws.send(JSON.stringify({ t: 'error', error: 'SOL match is full' })); break;
        }
        // SECURITY: Verify the joiner is a participant in the corresponding regular match
        const regularMatch = matches.get(msg.mid);
        if (regularMatch && Array.isArray(regularMatch.players)) {
          if (!regularMatch.players.some(p => p.addr === msg.addr)) {
            ws.send(JSON.stringify({ t: 'error', error: 'Not a participant in this match' })); break;
          }
        }
        sm.players.set(msg.pubkey, { deposited: false, addr: msg.addr });
        solMatches.set(msg.mid, sm);
        break;
      }
      case 'sol_deposit': {
        if (!msg.mid || !msg.addr || !msg.pubkey || !msg.signature) break;
        const sm = solMatches.get(msg.mid);
        if (!sm) { ws.send(JSON.stringify({ t: 'error', msg: 'SOL match not found' })); break; }
        const playerInfo = sm.players.get(msg.pubkey);
        if (!playerInfo) { ws.send(JSON.stringify({ t: 'error', msg: 'Not in this SOL match' })); break; }
        if (playerInfo.deposited) { ws.send(JSON.stringify({ t: 'sol_deposit_verified', mid: msg.mid, addr: msg.addr, already: true })); break; }

        // SECURITY: Prevent reusing one deposit signature across multiple matches
        if (processedMatchDepositSigs.has(msg.signature)) {
          ws.send(JSON.stringify({ t: 'sol_deposit_failed', mid: msg.mid, error: 'This deposit signature has already been used' }));
          break;
        }

        // Verify on-chain
        const verified = await verifySolDeposit(msg.signature, sm.wager_lamports, msg.pubkey);
        if (!verified) {
          ws.send(JSON.stringify({ t: 'sol_deposit_failed', mid: msg.mid, error: 'Deposit verification failed' }));
          break;
        }
        processedMatchDepositSigs.add(msg.signature);
        playerInfo.deposited = true;
        sm.players.set(msg.pubkey, playerInfo);
        broadcast({ t: 'sol_deposit_verified', mid: msg.mid, addr: msg.addr });

        // Check if all deposited
        let allDeposited = true;
        for (const [, info] of sm.players) {
          if (!info.deposited) { allDeposited = false; break; }
        }
        if (allDeposited) {
          sm.state = 'deposits_confirmed';
          solMatches.set(msg.mid, sm);
          broadcast({ t: 'sol_all_deposited', mid: msg.mid });
        }
        break;
      }
      case 'sol_game_result': {
        if (!msg.mid || !msg.winner) break;
        const sm = solMatches.get(msg.mid);
        if (!sm || sm.state === 'paid' || sm.state === 'disputing') break;

        // SECURITY (Critical #2): Only match participants can report game results
        const resultSender = getWsBoundAddr(ws);
        const playerAddrsResult = Array.from(sm.players.values()).map(p => p.addr);
        if (!resultSender || !playerAddrsResult.includes(resultSender)) {
          ws.send(JSON.stringify({ t: 'error', error: 'Only match participants can report results' }));
          break;
        }
        // SECURITY: Winner must also be a participant in the match
        if (!playerAddrsResult.includes(msg.winner)) {
          ws.send(JSON.stringify({ t: 'error', error: 'Declared winner is not a match participant' }));
          break;
        }

        sm.winner = msg.winner;
        sm.state = 'disputing';
        sm.dispute_deadline = Date.now() + 5 * 60 * 1000; // 5 minutes
        sm.dispute_votes.clear();
        sm.acceptors.clear();
        solMatches.set(msg.mid, sm);

        // Start dispute timer — auto-payout after 5 min
        const timer = setTimeout(() => {
          executeSolPayout(msg.mid);
          solPayoutPending.delete(msg.mid);
        }, 5 * 60 * 1000);
        solPayoutPending.set(msg.mid, timer);

        broadcast({ t: 'sol_result_pending', mid: msg.mid, winner: msg.winner, deadline: sm.dispute_deadline });
        break;
      }
      case 'sol_dispute_vote': {
        if (!msg.mid || !msg.addr || !msg.proposed_winner) break;
        const sm = solMatches.get(msg.mid);
        if (!sm || sm.state !== 'disputing') break;
        // SECURITY (H3): Only match participants can vote on disputes
        const dvPlayerAddrs = Array.from(sm.players.values()).map(p => p.addr);
        if (!dvPlayerAddrs.includes(msg.addr)) {
          ws.send(JSON.stringify({ t: 'error', error: 'Only match participants can dispute' })); break;
        }
        // SECURITY: proposed_winner must also be a participant
        if (!dvPlayerAddrs.includes(msg.proposed_winner)) {
          ws.send(JSON.stringify({ t: 'error', error: 'Proposed winner is not a match participant' })); break;
        }
        sm.dispute_votes.set(msg.addr, msg.proposed_winner);
        sm.acceptors.delete(msg.addr); // can't accept and dispute at same time
        solMatches.set(msg.mid, sm);

        // Check if unanimous on a different winner
        const voters = Array.from(sm.dispute_votes.values());
        const allVoted = dvPlayerAddrs.every(a => sm.dispute_votes.has(a));
        const allAgree = allVoted && voters.every(v => v === voters[0]);
        if (allAgree && voters[0] !== sm.winner) {
          sm.winner = voters[0];
          sm.state = 'dispute_resolved';
          solMatches.set(msg.mid, sm);
          broadcast({ t: 'sol_dispute_resolved', mid: msg.mid, new_winner: voters[0] });
          executeSolPayout(msg.mid);
        } else {
          broadcast({ t: 'sol_dispute_update', mid: msg.mid, votes: Object.fromEntries(sm.dispute_votes), acceptors: Array.from(sm.acceptors) });
        }
        break;
      }
      case 'sol_dispute_withdraw': {
        if (!msg.mid || !msg.addr) break;
        const sm = solMatches.get(msg.mid);
        if (!sm || sm.state !== 'disputing') break;
        // SECURITY (H3): Only match participants can withdraw disputes
        const dwPlayerAddrs = Array.from(sm.players.values()).map(p => p.addr);
        if (!dwPlayerAddrs.includes(msg.addr)) break;
        sm.dispute_votes.delete(msg.addr);
        solMatches.set(msg.mid, sm);
        broadcast({ t: 'sol_dispute_update', mid: msg.mid, votes: Object.fromEntries(sm.dispute_votes), acceptors: Array.from(sm.acceptors) });
        break;
      }
      case 'sol_accept_result': {
        if (!msg.mid || !msg.addr) break;
        const sm = solMatches.get(msg.mid);
        if (!sm || sm.state !== 'disputing') break;
        // SECURITY (H3): Only match participants can accept results
        const arPlayerAddrs = Array.from(sm.players.values()).map(p => p.addr);
        if (!arPlayerAddrs.includes(msg.addr)) {
          ws.send(JSON.stringify({ t: 'error', error: 'Only match participants can accept results' })); break;
        }
        sm.acceptors.add(msg.addr);
        sm.dispute_votes.delete(msg.addr); // can't accept and dispute
        const allAccepted = arPlayerAddrs.every(a => sm.acceptors.has(a));
        if (allAccepted) {
          // Everyone accepts — skip timer, payout now
          executeSolPayout(msg.mid);
        } else {
          broadcast({ t: 'sol_dispute_update', mid: msg.mid, votes: Object.fromEntries(sm.dispute_votes), acceptors: Array.from(sm.acceptors) });
        }
        break;
      }

      // --- SOL Poker Chip Handlers ---

      case 'sol_poker_buy_in': {
        if (!msg.pubkey || !msg.signature || !msg.lamports) break;
        if (!escrowKeypair) { ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Escrow not configured' })); break; }

        // SECURITY: Validate pubkey format (base58, 32-44 chars)
        if (typeof msg.pubkey !== 'string' || msg.pubkey.length < 32 || msg.pubkey.length > 44) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Invalid pubkey format' })); break;
        }
        // SECURITY: Validate signature format (base58, 64-88 chars)
        if (typeof msg.signature !== 'string' || msg.signature.length < 64 || msg.signature.length > 88) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Invalid signature format' })); break;
        }
        // SECURITY: Validate lamports is a reasonable positive integer
        const buyLamports = Number(msg.lamports);
        if (!Number.isInteger(buyLamports) || buyLamports < 100_000_000 || buyLamports > 100_000_000_000) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Invalid deposit amount (0.1-100 SOL)' })); break;
        }

        // SECURITY: Rate limiting
        const buyId = getWsIdentity(ws);
        if (buyId.pubkey && buyId.pubkey !== msg.pubkey) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Cannot use multiple pubkeys on same connection' })); break;
        }
        buyId.pubkey = msg.pubkey;
        if (Date.now() - buyId.lastBuyIn < RATE_LIMIT_BUYIN_MS) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Too many buy-ins — wait a moment' })); break;
        }
        if (buyId.buyInCount >= MAX_BUYIN_PER_HOUR) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Hourly buy-in limit reached' })); break;
        }
        buyId.lastBuyIn = Date.now();
        buyId.buyInCount++;

        // Prevent double-crediting the same transaction
        if (processedBuyInSigs.has(msg.signature)) {
          const bal = solPokerBalances.get(msg.pubkey) || { chips: 0, totalDeposited: 0 };
          ws.send(JSON.stringify({ t: 'sol_poker_balance', pubkey: msg.pubkey, chips: bal.chips }));
          break;
        }
        try {
          // Retry verification up to 3 times with delay (tx may not be confirmed yet)
          let verified = false;
          for (let attempt = 0; attempt < 3; attempt++) {
            verified = await verifySolDeposit(msg.signature, buyLamports, msg.pubkey);
            if (verified) break;
            if (attempt < 2) await new Promise(r => setTimeout(r, 3000));
          }
          if (!verified) { ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Deposit verification failed — signature does not match sender or escrow' })); break; }
          processedBuyInSigs.add(msg.signature);
          const chips = Math.floor(buyLamports / LAMPORTS_PER_CHIP);
          if (chips <= 0) { ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Deposit too small (min 0.1 SOL = 10 chips)' })); break; }
          const bal = solPokerBalances.get(msg.pubkey) || { chips: 0, totalDeposited: 0 };
          bal.chips += chips;
          bal.totalDeposited += buyLamports;
          solPokerBalances.set(msg.pubkey, bal);
          ws.send(JSON.stringify({ t: 'sol_poker_balance', pubkey: msg.pubkey, chips: bal.chips }));
          console.log(`SOL poker buy-in: ${msg.pubkey.slice(0,8)}.. deposited ${buyLamports} lamports → ${chips} chips (total: ${bal.chips})`);
        } catch (e) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: e.message }));
        }
        break;
      }

      case 'sol_poker_cash_out': {
        if (!msg.pubkey || !msg.chips) break;
        if (!escrowKeypair) { ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Escrow not configured' })); break; }

        // SECURITY: Validate pubkey format
        if (typeof msg.pubkey !== 'string' || msg.pubkey.length < 32 || msg.pubkey.length > 44) {
          ws.send(JSON.stringify({ t: 'sol_poker_cashout_failed', error: 'Invalid pubkey' })); break;
        }
        // SECURITY: Validate chips is a positive integer
        const cashChips = Number(msg.chips);
        if (!Number.isInteger(cashChips) || cashChips <= 0 || cashChips > 1_000_000) {
          ws.send(JSON.stringify({ t: 'sol_poker_cashout_failed', error: 'Invalid chip amount' })); break;
        }

        // SECURITY: Identity binding — must match buy-in pubkey for this connection
        const cashId = getWsIdentity(ws);
        if (cashId.pubkey && cashId.pubkey !== msg.pubkey) {
          ws.send(JSON.stringify({ t: 'sol_poker_cashout_failed', error: 'Pubkey mismatch — reconnect with correct wallet' })); break;
        }
        cashId.pubkey = msg.pubkey;

        // SECURITY: Rate limiting
        if (Date.now() - cashId.lastCashOut < RATE_LIMIT_CASHOUT_MS) {
          ws.send(JSON.stringify({ t: 'sol_poker_cashout_failed', error: 'Too many cashouts — wait 30 seconds' })); break;
        }
        if (cashId.cashOutCount >= MAX_CASHOUT_PER_HOUR) {
          ws.send(JSON.stringify({ t: 'sol_poker_cashout_failed', error: 'Hourly cashout limit reached' })); break;
        }

        // SECURITY: Prevent concurrent cashout for same pubkey
        if (cashOutLocks.has(msg.pubkey)) {
          ws.send(JSON.stringify({ t: 'sol_poker_cashout_failed', error: 'Cashout already in progress' })); break;
        }

        const bal = solPokerBalances.get(msg.pubkey);
        if (!bal || bal.chips < cashChips) {
          ws.send(JSON.stringify({ t: 'sol_poker_cashout_failed', error: 'Insufficient chip balance (server: ' + (bal ? bal.chips : 0) + ' chips)' }));
          break;
        }

        // SECURITY: Deduct chips BEFORE sending payout to prevent double-spend
        cashOutLocks.add(msg.pubkey);
        bal.chips -= cashChips;
        solPokerBalances.set(msg.pubkey, bal);
        cashId.lastCashOut = Date.now();
        cashId.cashOutCount++;

        const lamports = cashChips * LAMPORTS_PER_CHIP;
        const fee = Math.floor(lamports * SOL_HOUSE_FEE);
        const payout = lamports - fee;
        try {
          const sig = await sendSolPayout(msg.pubkey, payout);
          ws.send(JSON.stringify({ t: 'sol_poker_cashout_complete', pubkey: msg.pubkey, chips_remaining: bal.chips, sol_paid: payout, fee, signature: sig }));
          console.log(`SOL poker cash-out: ${msg.pubkey.slice(0,8)}.. cashed ${cashChips} chips → ${payout} lamports (fee: ${fee}), sig: ${sig}`);
          if (fee > 0) forwardFeeToHouse(fee);
        } catch (e) {
          // SECURITY: Refund chips on payout failure
          bal.chips += cashChips;
          solPokerBalances.set(msg.pubkey, bal);
          ws.send(JSON.stringify({ t: 'sol_poker_cashout_failed', error: 'Payout failed: ' + e.message }));
          console.error(`SOL cashout FAILED, refunded ${cashChips} chips to ${msg.pubkey.slice(0,8)}..`);
        } finally {
          cashOutLocks.delete(msg.pubkey);
        }
        break;
      }

      // --- Poker Table Relay ---
      case 'poker_table_create': {
        if (!msg.table || !msg.table.id) break;
        // SECURITY: Rate limit table creation per address
        const tableCreatorAddr = getWsBoundAddr(ws) || (msg.table.creator ? String(msg.table.creator) : null);
        if (tableCreatorAddr) {
          const now = Date.now();
          const times = pokerTableCreateTimes.get(tableCreatorAddr) || [];
          const recentTimes = times.filter(t => now - t < 3600_000);
          if (recentTimes.length >= MAX_TABLES_PER_HOUR) {
            ws.send(JSON.stringify({ t: 'error', error: 'Table creation rate limit exceeded (max ' + MAX_TABLES_PER_HOUR + '/hour)' }));
            break;
          }
          recentTimes.push(now);
          pokerTableCreateTimes.set(tableCreatorAddr, recentTimes);
        }
        const pubTable = { ...msg.table };
        delete pubTable.passcode; // never relay passcode
        pubTable.created = pubTable.created || Date.now();
        pubTable.playerCount = (pubTable.players || []).length;
        pokerTablesRelay.set(pubTable.id, pubTable);
        // Track ws for disconnect cleanup
        if (pubTable.creator) {
          wsPokerMap.set(ws, { table_id: pubTable.id, addr: pubTable.creator });
          // SECURITY: Bind this ws as the table creator for settle verification
          wsPokerCreatorMap.set(ws, pubTable.id);
        }
        broadcast({ t: 'poker_table_created', table: pubTable });
        break;
      }
      case 'poker_table_join': {
        if (!msg.table_id || !msg.player || !msg.player.addr) break;
        const tbl = pokerTablesRelay.get(msg.table_id);
        if (tbl) {
          if (!tbl.players) tbl.players = [];
          if (!tbl.players.find(p => p.addr === msg.player.addr)) {
            // SECURITY: Sanitize player object — only allow safe fields, reject raw injection
            const sanitizedPlayer = {
              addr: String(msg.player.addr),
              name: typeof msg.player.name === 'string' ? msg.player.name.slice(0, 32) : 'Anon',
              stack: typeof msg.player.stack === 'number' ? Math.max(0, Math.floor(msg.player.stack)) : 0,
              sittingOut: !!msg.player.sittingOut,
            };
            tbl.players.push(sanitizedPlayer);
          }
          tbl.playerCount = tbl.players.length;
          pokerTablesRelay.set(msg.table_id, tbl);
          // Track ws for disconnect cleanup
          if (msg.player.addr) wsPokerMap.set(ws, { table_id: msg.table_id, addr: msg.player.addr });
          const joinPlayer = tbl.players.find(p => p.addr === msg.player.addr);
          broadcast({ t: 'poker_table_joined', table_id: msg.table_id, player: joinPlayer, table: tbl });
        }
        break;
      }
      case 'poker_table_leave': {
        if (!msg.table_id || !msg.addr) break;
        const tbl = pokerTablesRelay.get(msg.table_id);
        if (tbl && tbl.players) {
          tbl.players = tbl.players.filter(p => p.addr !== msg.addr);
          tbl.playerCount = tbl.players.length;
          if (tbl.players.length === 0) {
            pokerTablesRelay.delete(msg.table_id);
            broadcast({ t: 'poker_table_removed', table_id: msg.table_id });
          } else {
            pokerTablesRelay.set(msg.table_id, tbl);
            broadcast({ t: 'poker_table_updated', table: tbl });
          }
        }
        // Clear disconnect tracking since they left explicitly
        wsPokerMap.delete(ws);
        break;
      }
      case 'poker_table_remove': {
        if (!msg.table_id) break;
        // SECURITY: Only the table creator can remove a table (verified via ws binding, not msg.addr)
        const rmTbl = pokerTablesRelay.get(msg.table_id);
        const rmSender = getWsBoundAddr(ws);
        if (rmTbl && rmTbl.creator && rmTbl.creator !== rmSender) {
          ws.send(JSON.stringify({ t: 'error', error: 'Only the table creator can remove this table' }));
          break;
        }
        pokerTablesRelay.delete(msg.table_id);
        broadcast({ t: 'poker_table_removed', table_id: msg.table_id });
        break;
      }

      // --- Poker reconnection: forward state request to all (host will respond) ---
      case 'poker_request_state': {
        if (!msg.table_id || !msg.addr) break;
        broadcast({ t: 'poker_request_state', table_id: msg.table_id, addr: msg.addr }, ws);
        break;
      }

      // --- Poker multiplayer game state relay ---
      case 'poker_hand_dealt':
      case 'poker_state_update':
      case 'poker_player_action':
      case 'poker_auto_folded': {
        // Forward to all other clients (table_id filtering done client-side)
        if (!msg.table_id) break;
        // SECURITY: Construct clean relay message — only forward expected poker fields,
        // not the raw untrusted msg which could contain injected properties
        const pokerRelayAllowedFields = ['t', 'table_id', 'state', 'players', 'action', 'player', 'seat',
          'amount', 'hand', 'communityCards', 'pot', 'dealer', 'currentPlayer', 'round',
          'winners', 'sidePots', 'addr', 'cards', 'handName', 'handRank'];
        const cleanPokerMsg = {};
        for (const field of pokerRelayAllowedFields) {
          if (msg[field] !== undefined) cleanPokerMsg[field] = msg[field];
        }
        broadcast(cleanPokerMsg, ws);
        break;
      }

      // SECURITY: Server-side SOL poker chip settlement
      // Host reports chip deltas after each hand; server validates and updates balances
      case 'sol_poker_settle': {
        if (!msg.table_id || !Array.isArray(msg.deltas)) break;
        const settleTbl = pokerTablesRelay.get(msg.table_id);
        if (!settleTbl || settleTbl.currency !== 'SOL') break;

        // SECURITY (Critical #5): Verify sender IS the table creator via ws binding,
        // NOT via msg.creator which can be spoofed by any client
        const settleAddr = getWsBoundAddr(ws);
        if (!settleAddr || settleTbl.creator !== settleAddr) {
          ws.send(JSON.stringify({ t: 'error', error: 'Only table host can settle chips' })); break;
        }
        // Also verify via wsPokerCreatorMap for double-check
        const creatorTableId = wsPokerCreatorMap.get(ws);
        if (creatorTableId && creatorTableId !== msg.table_id) {
          ws.send(JSON.stringify({ t: 'error', error: 'WebSocket not bound to this table' })); break;
        }
        // Validate deltas sum to zero (zero-sum game — no chips created or destroyed)
        let deltaSum = 0;
        for (const d of msg.deltas) {
          if (!d.pubkey || typeof d.delta !== 'number') continue;
          deltaSum += d.delta;
        }
        if (Math.abs(deltaSum) > 0) {
          console.warn(`sol_poker_settle: non-zero delta sum ${deltaSum} for table ${msg.table_id} — rejecting`);
          ws.send(JSON.stringify({ t: 'error', error: 'Settlement rejected: chip totals do not balance' })); break;
        }
        // Apply deltas
        for (const d of msg.deltas) {
          if (!d.pubkey || typeof d.delta !== 'number' || d.delta === 0) continue;
          const bal = solPokerBalances.get(d.pubkey) || { chips: 0, totalDeposited: 0 };
          bal.chips += d.delta;
          if (bal.chips < 0) bal.chips = 0; // safety floor
          solPokerBalances.set(d.pubkey, bal);
          // Update table stake tracking: winners gain stake allowance, losers lose it
          if (d.delta > 0) {
            recordTableBuyIn(d.pubkey, msg.table_id, d.delta);
          } else {
            consumeTableStake(d.pubkey, msg.table_id, Math.abs(d.delta));
          }
          console.log(`  SOL settle: ${d.pubkey.slice(0,8)}.. ${d.delta > 0 ? '+' : ''}${d.delta} chips → ${bal.chips} total`);
        }
        // Broadcast updated balances to all affected players
        for (const d of msg.deltas) {
          if (!d.pubkey) continue;
          const bal = solPokerBalances.get(d.pubkey) || { chips: 0, totalDeposited: 0 };
          broadcast({ t: 'sol_poker_balance', pubkey: d.pubkey, chips: bal.chips });
        }
        break;
      }

      // Server-side chip deduction when joining SOL poker table
      case 'sol_poker_table_buyin': {
        if (!msg.pubkey || !msg.chips || !msg.table_id) break;
        const tbChips = Number(msg.chips);
        if (!Number.isInteger(tbChips) || tbChips <= 0 || tbChips > 100_000) break;
        const tbId = getWsIdentity(ws);
        if (tbId.pubkey && tbId.pubkey !== msg.pubkey) break;
        tbId.pubkey = msg.pubkey;
        const tbBal = solPokerBalances.get(msg.pubkey) || { chips: 0, totalDeposited: 0 };
        if (tbBal.chips < tbChips) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'Insufficient chips for table buy-in (server: ' + tbBal.chips + ')' }));
          break;
        }
        tbBal.chips -= tbChips;
        solPokerBalances.set(msg.pubkey, tbBal);
        // SECURITY: Record this buy-in so sol_poker_return can't exceed it
        recordTableBuyIn(msg.pubkey, msg.table_id, tbChips);
        ws.send(JSON.stringify({ t: 'sol_poker_balance', pubkey: msg.pubkey, chips: tbBal.chips }));
        console.log(`SOL poker table buy-in: ${msg.pubkey.slice(0,8)}.. spent ${tbChips} chips on table ${msg.table_id} → ${tbBal.chips} remaining`);
        break;
      }

      // Server-side chip return when leaving SOL poker table
      case 'sol_poker_return': {
        if (!msg.pubkey || !msg.chips || !msg.table_id) break;
        const returnChips = Number(msg.chips);
        if (!Number.isInteger(returnChips) || returnChips <= 0 || returnChips > 100_000) break;
        // SECURITY: Validate pubkey matches connection identity
        const retId = getWsIdentity(ws);
        if (retId.pubkey && retId.pubkey !== msg.pubkey) break;

        // SECURITY (Critical #1): Cap returned chips to what was actually staked at this table.
        // Without this, any client could credit unlimited chips by sending fake return messages.
        const maxReturn = getTableStake(msg.pubkey, msg.table_id);
        if (maxReturn <= 0) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'No chips staked at this table' }));
          console.warn(`SOL poker return BLOCKED: ${msg.pubkey.slice(0,8)}.. tried to return ${returnChips} chips but has 0 staked at table ${msg.table_id}`);
          break;
        }
        const allowedReturn = consumeTableStake(msg.pubkey, msg.table_id, returnChips);
        if (allowedReturn <= 0) {
          ws.send(JSON.stringify({ t: 'sol_poker_error', error: 'No chips to return from this table' }));
          break;
        }
        if (allowedReturn < returnChips) {
          console.warn(`SOL poker return CAPPED: ${msg.pubkey.slice(0,8)}.. requested ${returnChips} but only ${allowedReturn} staked — returning ${allowedReturn}`);
        }

        // Credit only the validated amount back to server-side balance
        const retBal = solPokerBalances.get(msg.pubkey) || { chips: 0, totalDeposited: 0 };
        retBal.chips += allowedReturn;
        solPokerBalances.set(msg.pubkey, retBal);
        ws.send(JSON.stringify({ t: 'sol_poker_balance', pubkey: msg.pubkey, chips: retBal.chips }));
        console.log(`SOL poker return: ${msg.pubkey.slice(0,8)}.. returned ${allowedReturn} chips from table ${msg.table_id} → ${retBal.chips} total`);
        break;
      }

      case 'sol_poker_get_balance': {
        if (!msg.pubkey) break;
        const bal = solPokerBalances.get(msg.pubkey) || { chips: 0, totalDeposited: 0 };
        ws.send(JSON.stringify({ t: 'sol_poker_balance', pubkey: msg.pubkey, chips: bal.chips }));
        break;
      }

      // --- Swordbattle death relay (elimination mode) ---
      case 'sb_death': {
        if (!msg.mid || !msg.addr) break;
        const sbMatch = matches.get(msg.mid);
        if (!sbMatch) break;
        if (!sbMatch._sbDeaths) sbMatch._sbDeaths = new Set();
        if (!sbMatch._sbKills) sbMatch._sbKills = {};
        sbMatch._sbDeaths.add(msg.addr);
        sbMatch._sbKills[msg.addr] = msg.kills || 0;

        // Broadcast death to all peers
        const sbMsg = JSON.stringify({ t: 'sb_death', mid: msg.mid, addr: msg.addr });
        for (const [peer, peerInfo] of wsMatchMap) {
          if (peerInfo.mid === msg.mid && peer.readyState === 1) {
            try { peer.send(sbMsg); } catch(_e){}
          }
        }

        // Check if only one REAL player remains alive (bots don't report)
        const sbPlayers = sbMatch.players || [];
        const sbRealPlayers = sbPlayers.filter(p => !p.simulated);
        const sbAlive = sbRealPlayers.filter(p => !sbMatch._sbDeaths.has(p.addr));
        if (sbAlive.length === 1 && sbRealPlayers.length > 1 && !sbMatch.winner) {
          const sbWinner = sbAlive[0].addr;
          sbMatch.state = 'completed';
          sbMatch.winner = sbWinner;
          matches.set(sbMatch.id, sbMatch);
          const sbWinMsg = JSON.stringify({ t: 'sb_winner', mid: msg.mid, winner: sbWinner });
          for (const [peer, peerInfo] of wsMatchMap) {
            if (peerInfo.mid === msg.mid && peer.readyState === 1) {
              try { peer.send(sbWinMsg); } catch(_e){}
            }
          }
          broadcast({ t: 'match_updated', match: sbMatch });
          console.log(`Swordbattle match ${sbMatch.id} winner: ${sbWinner} (last alive)`);
        }
        break;
      }

      // --- Swordbattle timeout (20 min timer expired, player still alive) ---
      case 'sb_timeout': {
        if (!msg.mid || !msg.addr) break;
        const sbTMatch = matches.get(msg.mid);
        if (!sbTMatch || sbTMatch.winner) break;
        if (!sbTMatch._sbTimeouts) sbTMatch._sbTimeouts = {};
        if (!sbTMatch._sbKills) sbTMatch._sbKills = {};
        sbTMatch._sbTimeouts[msg.addr] = { kills: msg.kills || 0, ts: Date.now() };
        sbTMatch._sbKills[msg.addr] = msg.kills || 0;

        // Check if all alive REAL players have reported timeout (bots never report)
        const sbTPlayers = sbTMatch.players || [];
        const sbTDeaths = sbTMatch._sbDeaths || new Set();
        const sbTAlive = sbTPlayers.filter(p => !p.simulated && !sbTDeaths.has(p.addr));
        const sbTTimeouts = sbTMatch._sbTimeouts;
        const allAliveReported = sbTAlive.every(p => sbTTimeouts[p.addr]);

        if (allAliveReported && sbTAlive.length > 0 && !sbTMatch.winner) {
          // All alive players timed out — winner is player with most kills
          let sbTWinner = null;
          let bestKills = -1;
          for (const p of sbTAlive) {
            const kills = sbTTimeouts[p.addr] ? sbTTimeouts[p.addr].kills : 0;
            if (kills > bestKills) { bestKills = kills; sbTWinner = p.addr; }
          }
          if (sbTWinner) {
            sbTMatch.state = 'completed';
            sbTMatch.winner = sbTWinner;
            matches.set(sbTMatch.id, sbTMatch);
            const sbTWinMsg = JSON.stringify({ t: 'sb_winner', mid: msg.mid, winner: sbTWinner });
            for (const [peer, peerInfo] of wsMatchMap) {
              if (peerInfo.mid === msg.mid && peer.readyState === 1) {
                try { peer.send(sbTWinMsg); } catch(_e){}
              }
            }
            broadcast({ t: 'match_updated', match: sbTMatch });
            console.log(`Swordbattle match ${sbTMatch.id} winner: ${sbTWinner} (timeout, most kills: ${bestKills})`);
          }
        }
        break;
      }

      // --- Bomberman death relay ---
      case 'bm_death': {
        if (!msg.mid || !msg.addr) break;
        const bmMsg = JSON.stringify({ t: 'bm_death', mid: msg.mid, addr: msg.addr });
        for (const [peer, peerInfo] of wsMatchMap) {
          if (peerInfo.mid === msg.mid && peer.readyState === 1) {
            try { peer.send(bmMsg); } catch(_e){}
          }
        }
        break;
      }

      // --- Mk48 naval elimination match result ---
      case 'mk48_match_end': {
        if (!msg.mid || !msg.addr) break;
        const mk48Match = matches.get(msg.mid);
        if (!mk48Match) break;
        if (!mk48Match._mk48Results) mk48Match._mk48Results = {};
        mk48Match._mk48Results[msg.addr] = { alive: !!msg.alive, survived: msg.survived || 0, score: msg.score || 0, ts: Date.now() };
        console.log(`[MK48] ${msg.addr} reported: alive=${msg.alive} survived=${msg.survived} score=${msg.score || 0} match=${msg.mid}`);

        // Notify other players about this elimination/survival
        const mk48StatusMsg = JSON.stringify({ t: 'mk48_status', mid: msg.mid, addr: msg.addr, alive: !!msg.alive, survived: msg.survived || 0 });
        for (const [peer, peerInfo] of wsMatchMap) {
          if (peerInfo.mid === msg.mid && peer.readyState === 1) {
            try { peer.send(mk48StatusMsg); } catch(_e){}
          }
        }

        // Start a relay-side fallback timer on first result (15 min match duration)
        if (!mk48Match._mk48Timeout) {
          mk48Match._mk48Timeout = setTimeout(() => {
            decideMk48Winner(mk48Match);
          }, 900000);
        }

        decideMk48Winner(mk48Match);
        break;
      }

      // --- Survev.io match result reporting ---
      case 'survev_death':
      case 'survev_timeout': {
        if (!msg.mid || !msg.addr) break;
        const svMatch = matches.get(msg.mid);
        if (!svMatch) break;
        // Store the player's result on the match
        if (!svMatch._survevResults) svMatch._survevResults = {};
        svMatch._survevResults[msg.addr] = {
          kills: msg.kills || 0,
          won: !!msg.won,
          type: msg.t,
          ts: Date.now()
        };
        // Relay to all players in the match so they can see each other's results
        const svResultMsg = JSON.stringify({ t: 'survev_result', mid: msg.mid, addr: msg.addr, kills: msg.kills || 0, won: !!msg.won });
        for (const [peer, peerInfo] of wsMatchMap) {
          if (peerInfo.mid === msg.mid && peer.readyState === 1) {
            try { peer.send(svResultMsg); } catch(_e){}
          }
        }
        // If all REAL players have reported, determine winner and broadcast
        const svPlayers = svMatch.players || [];
        const svRealPlayers = svPlayers.filter(p => !p.simulated);
        const svResults = svMatch._survevResults;
        if (svRealPlayers.length > 0 && Object.keys(svResults).length >= svRealPlayers.length) {
          // Determine winner: player who self-reported won=true, or most kills
          let svWinner = null;
          let svBestKills = -1;
          for (const p of svRealPlayers) {
            const r = svResults[p.addr];
            if (!r) continue;
            if (r.won) { svWinner = p.addr; break; }
            if (r.kills > svBestKills) { svBestKills = r.kills; svWinner = p.addr; }
          }
          if (svWinner) {
            svMatch.state = 'completed';
            svMatch.winner = svWinner;
            matches.set(svMatch.id, svMatch);
            broadcast({ t: 'match_updated', match: svMatch });
            console.log(`Survev match ${svMatch.id} winner: ${svWinner} (kills comparison)`);
          }
        }
        break;
      }
    }
  });

  ws.on('close', () => {
    sfLeave(ws);
    // Clean up lobby/match when a player disconnects before game starts
    const wm = wsMatchMap.get(ws);
    if (wm) {
      const m = matches.get(wm.mid);
      if (m && (m.state === 'open' || m.state === 'active')) {
        // Remove the disconnected player from the match
        m.players = m.players.filter(p => p.addr !== wm.addr);
        if (m.players.length === 0) {
          // No players left — remove the match entirely
          if (m.room_code) codeToId.delete(m.room_code);
          matches.delete(m.id);
          broadcast({ t: 'match_updated', match: { ...m, state: 'cancelled' } });
        } else {
          // Revert to open so another player can join
          m.state = 'open';
          // Reset ready flags since lobby changed
          m.players.forEach(p => { p.ready = false; });
          matches.set(m.id, m);
          broadcast({ t: 'match_updated', match: m });
        }
      }
      wsMatchMap.delete(ws);
    }
    // Clean up presence tracking
    const wp = wsPlayerMap.get(ws);
    if (wp) {
      const pres = matchPresence.get(wp.mid);
      if (pres) {
        pres.delete(wp.addr);
        broadcast({ t: 'presence_update', mid: wp.mid, connected: Array.from(pres), left: wp.addr });
      }
      wsPlayerMap.delete(ws);
    }
    // Clean up poker table membership for disconnected player
    const pokerInfo = wsPokerMap.get(ws);
    if (pokerInfo) {
      const tbl = pokerTablesRelay.get(pokerInfo.table_id);
      if (tbl && tbl.players) {
        tbl.players = tbl.players.filter(p => p.addr !== pokerInfo.addr);
        tbl.playerCount = tbl.players.length;
        if (tbl.players.length === 0) {
          pokerTablesRelay.delete(pokerInfo.table_id);
          broadcast({ t: 'poker_table_removed', table_id: pokerInfo.table_id });
        } else {
          pokerTablesRelay.set(pokerInfo.table_id, tbl);
          broadcast({ t: 'poker_table_updated', table: tbl });
        }
      }
      wsPokerMap.delete(ws);
    }
    // Clean up queue entries for disconnected player
    const queueIds = wsQueueMap.get(ws);
    if (queueIds) {
      for (const qid of queueIds) {
        if (arcadeQueue.has(qid)) {
          arcadeQueue.delete(qid);
          broadcast({ t:'queue_removed', queue_id:qid });
        }
      }
      wsQueueMap.delete(ws);
    }
    clients.delete(ws);
  });
});

// HTTP server for health checks
const server = createServer((req, res) => {
  if (req.url === '/health' || req.url === '/ws/relay/health') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({
      matches: matches.size,
      clients: clients.size,
      queue: arcadeQueue.size,
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
