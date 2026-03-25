/**
 * Bomberman Colyseus Game Server (standalone)
 * Run: node server.js
 * Port: 8406
 */

const { createServer } = require("http");
const express = require("../bomberman-src/node_modules/express");
const cors = require("../bomberman-src/node_modules/cors");
const { Server, LobbyRoom } = require("../bomberman-src/node_modules/@colyseus/core");
const { WebSocketTransport } = require("../bomberman-src/node_modules/@colyseus/ws-transport");
const { Schema, MapSchema, type, defineTypes } = require("../bomberman-src/node_modules/@colyseus/schema");
const { Room } = require("../bomberman-src/node_modules/@colyseus/core");

// ============================================================
// Shared Data (inlined from shared/)
// ============================================================

const tiles = {
    ground: { id: " ", name: "ground", width: 1, height: 0.1, isWalkable: true, offset_y: -0.1 },
    spawnpoint: { id: "S", name: "spawnpoint", width: 1, height: 1, isWalkable: true, offset_y: -0.1 },
    wall: { id: "W", name: "wall", width: 1, height: 1, offset_y: 0, isWalkable: false },
    breakable_wall: { id: "B", name: "breakable_wall", width: 1, height: 1, isWalkable: false },
    bomb: { id: "T", name: "bomb", width: 1, height: 1, isWalkable: false },
    player: { id: "P", name: "player", width: 1, height: 1, offset_y: 0, isWalkable: false },
    explosion: { id: "E", name: "explosion", width: 1, height: 1, offset_y: 0, isWalkable: false },
};

const maps = require("../bomberman-src/packages/shared/Data/maps.json");

const CellType = {
    GROUND: "ground",
    SPAWNPOINT: "spawnpoint",
    WALL: "wall",
    BREAKABLE_WALL: "breakable_wall",
    EXPLOSION: "explosion",
    PLAYER: "player",
    BOMB: "bomb",
    POWER_UP: "power_up",
};

const PowerUpTypes = { HEALTH: 0, BOMB: 1, SPEED: 2 };

const ServerMsg = {
    PING: 1,
    PONG: 2,
    START_GAME: 3,
    START_MAP_UPDATE: 4,
    START_GAME_REQUESTED: 5,
    PLAYER_MOVE: 6,
    PLACE_BOMB: 7,
    DO_EXPLOSION: 8,
    PLAYER_ELIMINATED: 9,
    GAME_OVER: 10,
};

// ============================================================
// Utility functions
// ============================================================

function generateRoomId() {
    const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let result = "";
    for (let i = 0; i < 4; i++) result += chars.charAt(Math.floor(Math.random() * chars.length));
    return result;
}

function getRandomInt(min, max) {
    min = Math.ceil(min);
    max = Math.floor(max);
    return Math.floor(Math.random() * (max - min + 1)) + min;
}

// ============================================================
// MapHelper
// ============================================================

class MapHelper {
    constructor(mapName = "map_01") {
        this.spawnPoints = [];
        this.baseCells = [];
        this.cells = [];
        this.shadow_map = [];
        this.mapData = maps[mapName].data;
        this.tiles = tiles;
        this.generate(mapName);
    }

    setSpawnPoint(sessionId) {
        for (let i = 0; i < this.spawnPoints.length; i++) {
            let spawn = this.spawnPoints[i];
            if (!spawn.player) {
                this.spawnPoints[i].player = sessionId;
                return spawn;
            }
        }
    }

    isCellAvailable(cells, row, col) {
        for (let [key, element] of cells) {
            if (element.row === row && element.col === col && element.cellInfo && !element.cellInfo.isWalkable) {
                return false;
            }
        }
        return true;
    }

    generate(map) {
        this.mapData.forEach((col, colId) => {
            col.forEach((tileID, rowId) => {
                this.processTile(tileID, colId, rowId);
            });
        });
        this.baseCells = [...this.cells];
    }

    findTile(tileID, id = "id") {
        let found;
        for (let tID in tiles) {
            let tile = tiles[tID];
            if (tile && tile[id] === tileID) {
                found = tile;
            }
        }
        return found;
    }

    processTile(tileID, colId, rowId) {
        let foundTile = this.findTile(tileID);
        if (!foundTile) console.error("Tile: " + tileID + " does not exist");

        if (foundTile && foundTile.id === "S") {
            this.spawnPoints.push({
                player: false,
                col: rowId,
                row: colId,
            });
        }

        if (!this.cells[colId]) {
            this.cells[colId] = [];
        }
        this.cells[colId][rowId] = foundTile;
    }

    generateServerMap(room) {
        this.mapData.forEach((row, rowId) => {
            row.forEach((tileID, colId) => {
                let foundTile = this.findTile(tileID);
                if (!foundTile) console.error("Tile: " + tileID + " does not exist");

                let sessionId = "" + rowId + "-" + colId;
                let cellType = CellType.GROUND;

                let rand = Math.random();
                if (tileID === " " && rand < 0.4) {
                    cellType = CellType.BREAKABLE_WALL;
                } else if (tileID === " " && rand < 0.5) {
                    cellType = CellType.WALL;
                }

                if (this.isNearSpawnPoint(rowId, colId)) {
                    cellType = CellType.GROUND;
                }

                if (tileID === "W") {
                    cellType = CellType.WALL;
                }

                let cell = new Cell().assign({
                    sessionId: sessionId,
                    row: rowId,
                    col: colId,
                    type: cellType,
                });
                cell.cellInfo = this.findTile(cellType, "name");

                room.state.cells.set(sessionId, cell);

                if (!this.shadow_map[rowId]) {
                    this.shadow_map[rowId] = [];
                }
                this.shadow_map[rowId][colId] = cellType;
            });
        });
    }

    isNearSpawnPoint(rowId, colId, radius = 1) {
        let found = false;
        this.spawnPoints.forEach((spawn) => {
            if (Math.abs(colId - spawn.col) <= radius && Math.abs(rowId - spawn.row) <= radius) {
                found = true;
            }
        });
        return found;
    }

    findAvailableColor(players, colors) {
        let randomColor = colors[Math.floor(Math.random() * colors.length)];
        let found = false;
        players.forEach((p) => {
            if (randomColor === p.color) {
                found = true;
            }
        });
        if (found) {
            return this.findAvailableColor(players, colors);
        }
        return randomColor;
    }
}

// ============================================================
// Schema Classes
// ============================================================

class Cell extends Schema {
    constructor() {
        super();
        this.sessionId = "";
        this.type = CellType.GROUND;
        this.col = 0;
        this.row = 0;
        this.playerId = "";
        this.bombId = "";
        this.cellInfo = null;
    }
}
defineTypes(Cell, {
    sessionId: "string",
    type: "string",
    col: "int8",
    row: "int8",
    playerId: "string",
    bombId: "string",
});

class PowerUp extends Schema {
    constructor() {
        super();
        this.sessionId = "";
        this.type = CellType.POWER_UP;
        this.col = 0;
        this.row = 0;
        this.power = 0;
    }
}
defineTypes(PowerUp, {
    sessionId: "string",
    type: "string",
    col: "int8",
    row: "int8",
    power: "int8",
});

class Bomb extends Schema {
    constructor() {
        super();
        this.sessionId = "";
        this.type = CellType.BOMB;
        this.col = 0;
        this.row = 0;
        this.size = 3;
        this.owner = "";
        this.timeoutTimer = null;
    }
}
defineTypes(Bomb, {
    sessionId: "string",
    type: "string",
    col: "int8",
    row: "int8",
    size: "int8",
});

class Player extends Schema {
    constructor() {
        super();
        this.sessionId = "";
        this.type = CellType.PLAYER;
        this.col = 0;
        this.row = 0;
        this.name = "";
        this.ready = false;
        this.autoReady = false;
        this.disconnected = false;
        this.admin = false;
        this.eliminated = false;
        this.score = 0;
        this.bombs = 1;
        this.health = 1;
        this.speed = 1;
        this.color = "";
        this.sequence = 0;
        this.rot = 0;
        this.explosion_size = 3;
        this.spawnPoint = null;
        this._room = null;
    }
}
defineTypes(Player, {
    sessionId: "string",
    type: "string",
    col: "int8",
    row: "int8",
    name: "string",
    ready: "boolean",
    autoReady: "boolean",
    disconnected: "boolean",
    admin: "boolean",
    eliminated: "boolean",
    score: "int8",
    bombs: "int8",
    health: "int8",
    speed: "int8",
    color: "string",
    sequence: "int16",
    rot: "float32",
});

class GameState extends Schema {
    constructor() {
        super();
        this.status = "CREATED";
        this.map = "map_01";
        this.players = new MapSchema();
        this.cells = new MapSchema();
        this.bombs = new MapSchema();
        this.powers = new MapSchema();
    }
}
defineTypes(GameState, {
    status: "string",
    map: "string",
    players: { map: Player },
    cells: { map: Cell },
    bombs: { map: Bomb },
    powers: { map: PowerUp },
});

// ============================================================
// Player logic methods (attached to plain objects via room ref)
// ============================================================

function playerUpdate(player, dt, room) {
    if (player.health < 1 && !player.eliminated) {
        player.eliminated = true;
        player.row = -99;
        player.col = -99;
        room.broadcast(ServerMsg.PLAYER_ELIMINATED, {
            sessionId: player.sessionId,
            name: player.name,
        });
        // Remove eliminated player from state after a short delay
        // so the client entity gets cleaned up via onRemove
        setTimeout(() => {
            room.state.players.delete(player.sessionId);
        }, 1500);
    }
}

function playerHasPowerUp(room, row, col) {
    let found = null;
    room.state.powers.forEach((power) => {
        if (power.row === row && power.col === col) {
            found = power;
        }
    });
    return found;
}

function playerConsumePowerUp(player, powerup, room) {
    if (powerup.power === PowerUpTypes.HEALTH) {
        player.health += 1;
    } else if (powerup.power === PowerUpTypes.BOMB) {
        player.bombs += 1;
    } else if (powerup.power === PowerUpTypes.SPEED) {
        player.speed += 1;
    }
    room.state.powers.delete(powerup.sessionId);
}

function playerMove(player, playerInput, room, mapHelper) {
    let previousCell = room.state.cells.get(player.row + "-" + player.col);

    let speed = 1;
    let newCol = player.col - playerInput.h * speed;
    let newRow = player.row - playerInput.v * speed;
    const newRotY = Math.atan2(playerInput.h, playerInput.v);

    if (mapHelper.isCellAvailable(room.state.cells, newRow, newCol)) {
        player.col = newCol;
        player.row = newRow;
        player.rot = player.rot + (newRotY - player.rot);
        player.sequence = playerInput.seq;

        let powerUpFound = playerHasPowerUp(room, newRow, newCol);
        if (powerUpFound) {
            playerConsumePowerUp(player, powerUpFound, room);
        }

        let cell = room.state.cells.get(player.row + "-" + player.col);
        if (cell) {
            cell.playerId = player.sessionId;
        }
        if (previousCell) {
            previousCell.playerId = "";
        }
    }
}

function playerPlaceBomb(player, data, room) {
    if (player.bombs > 0) {
        let sessionId = "bomb-" + player.row + "-" + player.col;

        if (room.state.bombs.get(sessionId)) {
            return false;
        }

        let bomb = new Bomb();
        bomb.sessionId = sessionId;
        bomb.owner = player.sessionId;
        bomb.col = player.col;
        bomb.row = player.row;
        bomb.size = player.explosion_size;
        bomb.type = CellType.BOMB;

        bomb.timeoutTimer = setTimeout(() => {
            triggerBomb(bomb, room);
        }, 3000);

        room.state.bombs.set(bomb.sessionId, bomb);

        let cell = room.state.cells.get(player.row + "-" + player.col);
        if (cell) {
            cell.bombId = sessionId;
        }

        player.bombs--;
    }
}

// ============================================================
// Bomb logic
// ============================================================

function triggerBomb(bomb, room) {
    if (bomb.timeoutTimer) {
        clearTimeout(bomb.timeoutTimer);
        bomb.timeoutTimer = null;
    }

    // Clear bomb from cell
    let currentCell = room.state.cells.get(bomb.row + "-" + bomb.col);
    if (currentCell) {
        currentCell.bombId = "";
    }

    const dirs = [
        { col: -1, row: 0 },
        { col: 1, row: 0 },
        { col: 0, row: -1 },
        { col: 0, row: 1 },
    ];

    let positions = new Map();
    let players = new Map();

    dirs.forEach((dir) => {
        for (let i = 0; i <= bomb.size; i++) {
            const col = bomb.col + dir.col * i;
            const row = bomb.row + dir.row * i;

            let key = row + "-" + col;
            const cell = room.state.cells.get(row + "-" + col);

            if (!cell) return;

            if (cell.type === CellType.WALL) {
                return;
            }

            if (cell.type === CellType.BREAKABLE_WALL) {
                room.state.cells.delete(cell.sessionId);

                let newCell = new Cell();
                newCell.sessionId = row + "-" + col;
                newCell.row = row;
                newCell.col = col;
                newCell.type = CellType.GROUND;
                newCell.cellInfo = { isWalkable: true, name: "ground" };
                room.state.cells.set(newCell.sessionId, newCell);
                positions.set(key, { row: row, col: col });

                // 50% chance for powerup
                if (Math.random() <= 0.5) {
                    let powerUp = new PowerUp();
                    powerUp.sessionId = row + "-" + col;
                    powerUp.row = row;
                    powerUp.col = col;
                    powerUp.type = CellType.POWER_UP;
                    powerUp.power = getRandomInt(0, 2);
                    room.state.powers.set(powerUp.sessionId, powerUp);
                }
                return;
            }

            // check if player is hit
            if (cell.playerId !== "" && cell.playerId) {
                players.set(cell.playerId, cell.playerId);
            }

            positions.set(key, { row: row, col: col });

            // chain reaction
            if (cell.bombId) {
                let otherBomb = room.state.bombs.get(cell.bombId);
                if (otherBomb) {
                    setTimeout(() => triggerBomb(otherBomb, room), 400);
                }
            }
        }
    });

    // damage players
    players.forEach((sessionId) => {
        const player = room.state.players.get(sessionId);
        if (player) {
            player.health--;
        }
    });

    // increase player available bombs
    const playerState = room.state.players.get(bomb.owner);
    if (playerState) {
        playerState.bombs++;
    }

    // remove bomb
    room.state.bombs.delete(bomb.sessionId);

    // broadcast explosion
    room.broadcast(ServerMsg.DO_EXPLOSION, {
        row: bomb.row,
        col: bomb.col,
        size: bomb.size,
        cells: Object.fromEntries(positions),
    });
}

// ============================================================
// Pre-registered rooms
// ============================================================

const preRegisteredRooms = new Map();

// ============================================================
// GameRoom
// ============================================================

class GameRoom extends Room {
    onCreate(options) {
        this.maxClients = 10;
        this.autoDispose = true;

        if (options.roomId) {
            this.roomId = options.roomId;
        }

        this.expectedPlayers = options.maxClient || 2;
        // Set maxClients higher than expectedPlayers to allow spectators
        // Colyseus auto-locks when maxClients is reached
        this.maxClients = this.expectedPlayers + 50; // Room for spectators
        this.gameStarted = false;
        this.gameEnded = false;

        this.colors = [
            "#eb4034", "#58eb34", "#2e84e6", "#d332db", "#ffa500",
            "#00ced1", "#ff69b4", "#ffff00", "#8b4513", "#00ff7f",
        ];

        this.setState(new GameState());
        this.clock.start();

        const mapName = "map_arena";

        this.setMetadata({ map: mapName }).then(() => {
            this.state.map = mapName;
        });

        this.mapHelper = new MapHelper(mapName);
        console.log("Creating Room", this.roomId, mapName, "expecting", this.expectedPlayers, "players");

        this.setSimulationInterval((dt) => {
            this.state.players.forEach((player) => {
                playerUpdate(player, dt, this);
            });
            if (this.gameStarted && !this.gameEnded) {
                this.checkForWinner();
            }
        }, 100);

        this.processMessages();
    }

    onAuth(client, auth) {
        // Spectators bypass room-full check
        if (auth && auth.spectator) return auth;
        if (this.state.players.size >= this.maxClients) {
            throw new Error("room is full");
        }
        return auth;
    }

    onJoin(client) {
        // Spectator: receives state updates but NO player entity
        if (client.auth && client.auth.spectator) {
            if (!this._spectators) this._spectators = new Set();
            this._spectators.add(client.sessionId);
            console.log(`[Room ${this.roomId}] Spectator joined: ${client.sessionId}`);
            if (this.gameStarted) {
                client.send(ServerMsg.START_GAME, true);
            }
            return;
        }

        console.log(`[Room ${this.roomId}] Player joined: ${client.sessionId}`);

        let spawnpoint = this.mapHelper.setSpawnPoint(client.sessionId);
        if (!spawnpoint) {
            spawnpoint = { col: 1, row: 1 };
        }

        let color = this.mapHelper.findAvailableColor(this.state.players, this.colors);

        let player = new Player();
        player.sessionId = client.sessionId;
        player.name = (client.auth && client.auth.name) || "Player";
        player.admin = this.state.players.size === 0;
        player.col = spawnpoint.col;
        player.row = spawnpoint.row;
        player.color = color;
        player.spawnPoint = spawnpoint;
        player.type = CellType.PLAYER;
        player.health = 1;
        player.bombs = 1;
        player.explosion_size = 3;
        player._room = this;

        this.state.players.set(client.sessionId, player);

        // Auto-start when expected players have joined
        if (this.state.players.size >= this.expectedPlayers && !this.gameStarted) {
            this.clock.setTimeout(() => {
                if (!this.gameStarted && this.state.players.size >= 2) {
                    this.startGame();
                }
            }, 2000);
        }

        // Fallback: start after 8 seconds if at least 2 players joined
        // (handles case where not all expected players show up)
        if (this.state.players.size >= 2 && !this.gameStarted && !this._fallbackStartScheduled) {
            this._fallbackStartScheduled = true;
            this.clock.setTimeout(() => {
                if (!this.gameStarted && this.state.players.size >= 2) {
                    console.log(`[Room ${this.roomId}] Fallback start with ${this.state.players.size}/${this.expectedPlayers} players`);
                    this.startGame();
                }
            }, 8000);
        }
    }

    startGame() {
        if (this.gameStarted) return;
        this.gameStarted = true;

        // Don't lock — spectators can still join mid-game
        // this.lock();

        // Generate level
        this.mapHelper.generateServerMap(this);

        // Mark each player's spawn cell so bombs can detect them
        this.state.players.forEach((player) => {
            let spawnCell = this.state.cells.get(player.row + "-" + player.col);
            if (spawnCell) {
                spawnCell.playerId = player.sessionId;
            }
        });

        this.broadcast(ServerMsg.START_GAME, true);
        this.state.status = "PLAYING";
        console.log("Game started in room", this.roomId, "with", this.state.players.size, "players");
    }

    checkForWinner() {
        let alivePlayers = [];
        this.state.players.forEach((player) => {
            if (!player.eliminated) {
                alivePlayers.push(player);
            }
        });

        if (alivePlayers.length <= 1 && this.state.players.size >= 2) {
            this.gameEnded = true;
            this.state.status = "ENDED";

            const winner = alivePlayers.length === 1 ? alivePlayers[0] : null;
            const winnerName = winner ? winner.name : "Nobody";
            const winnerSessionId = winner ? winner.sessionId : "";

            console.log("Game over in room", this.roomId, "- Winner:", winnerName);

            this.broadcast(ServerMsg.GAME_OVER, {
                winner: winnerName,
                winnerSessionId: winnerSessionId,
            });

            // Auto-dispose after 10 seconds
            this.clock.setTimeout(() => {
                this.disconnect();
            }, 10000);
        }
    }

    async onLeave(client, consented) {
        // Spectator leaving — no game impact
        if (this._spectators && this._spectators.has(client.sessionId)) {
            this._spectators.delete(client.sessionId);
            console.log(`[Room ${this.roomId}] Spectator left: ${client.sessionId}`);
            client.leave();
            return;
        }

        console.log(`[Room ${this.roomId}] Player left: ${client.sessionId} (consented: ${consented})`);

        const player = this.state.players.get(client.sessionId);
        if (player && this.gameStarted && !this.gameEnded) {
            player.eliminated = true;
            player.health = 0;
            this.broadcast(ServerMsg.PLAYER_ELIMINATED, {
                sessionId: client.sessionId,
                name: player.name,
            });
        } else {
            this.deletePlayer(client.sessionId);
        }

        client.leave();
    }

    onDispose() {
        console.log(`[Room ${this.roomId}] Disposing`);
    }

    deletePlayer(id) {
        const player = this.state.players.get(id);
        if (!player) return;

        player.ready = false;
        this.state.players.delete(id);

        if (player.admin && this.state.players.size > 0) {
            player.admin = false;
            const a = [...this.state.players.values()];
            a[Math.floor(Math.random() * a.length)].admin = true;
        }
    }

    processMessages() {
        this.onMessage("*", (client, type, data) => {
            const playerState = this.state.players.get(client.sessionId);
            if (!playerState) return false;

            if (type === ServerMsg.PING) {
                client.send(ServerMsg.PONG, data);
            }

            if (type === ServerMsg.START_MAP_UPDATE) {
                // Map changes disabled — always use arena
            }

            if (type === ServerMsg.START_GAME_REQUESTED) {
                if (!this.gameStarted) {
                    this.startGame();
                }
            }

            if (type === ServerMsg.PLAYER_MOVE) {
                if (!playerState.eliminated) {
                    playerMove(playerState, data, this, this.mapHelper);
                }
            }

            if (type === ServerMsg.PLACE_BOMB) {
                if (!playerState.eliminated) {
                    playerPlaceBomb(playerState, data, this);
                }
            }
        });
    }

    changeMap(key) {
        this.setMetadata({ map: key }).then(() => {
            this.state.map = key;
            this.mapHelper = new MapHelper(key);
        });
    }
}

// ============================================================
// Start Server
// ============================================================

const PORT = 8406;
const app = express();
app.use(cors());
app.use(express.json());

const httpServer = createServer(app);

const gameServer = new Server({
    transport: new WebSocketTransport({ server: httpServer }),
});

gameServer.define("lobby", LobbyRoom);
gameServer.define("gameroom", GameRoom).enableRealtimeListing();

// REST: POST /create-room
app.post("/create-room", async (req, res) => {
    try {
        const { roomCode, expectedPlayers } = req.body;
        if (!roomCode) {
            return res.status(400).json({ error: "roomCode is required" });
        }
        const numPlayers = expectedPlayers || 2;

        // Store pre-registration info
        preRegisteredRooms.set(roomCode, { expectedPlayers: numPlayers, created: Date.now() });

        console.log(`Pre-registered room ${roomCode} for ${numPlayers} players`);
        res.json({ ok: true, roomCode, expectedPlayers: numPlayers });
    } catch (err) {
        console.error("create-room error:", err);
        res.status(500).json({ error: err.message });
    }
});

// REST: GET /health
app.get("/health", (req, res) => {
    res.json({ status: "ok", rooms: preRegisteredRooms.size });
});

// REST: POST /add-bot — spawn an AI bot into a room using the external bomberman-bot.js
app.post("/add-bot", async (req, res) => {
    try {
        const { roomId, count } = req.body || {};
        if (!roomId) return res.status(400).json({ error: "roomId required" });
        const numBots = Math.min(parseInt(count) || 1, 10);
        const { spawn } = require("child_process");
        const path = require("path");
        const botScript = path.resolve(__dirname, "../../../test-bots/bomberman-bot.js");
        const bot = spawn("node", [botScript, roomId, String(numBots), "--verbose"], {
            stdio: ["ignore", "pipe", "pipe"],
        });
        bot.stdout.on("data", d => console.log("[BOT] " + d.toString().trim()));
        bot.stderr.on("data", d => console.error("[BOT-ERR] " + d.toString().trim()));
        bot.on("close", code => console.log(`[BOT] Process exited with code ${code}`));
        console.log(`[Bomberman] Spawned ${numBots} AI bot(s) for room ${roomId} (pid: ${bot.pid})`);
        res.json({ ok: true, roomId, bots: numBots, pid: bot.pid });
    } catch (err) {
        console.error("add-bot error:", err);
        res.status(500).json({ error: err.message });
    }
});

gameServer.listen(PORT).then(() => {
    console.log(`[Bomberman] Server listening on http://localhost:${PORT}`);
});
