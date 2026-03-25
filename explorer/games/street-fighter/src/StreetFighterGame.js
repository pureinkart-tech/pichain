import {
	registerGamepadEvents,
	registerKeyboardEvents,
	updateGamePads,
} from './engine/InputHandler.js';
import { getContext } from './utils/context.js';
import { BattleScene } from './scenes/BattleScene.js';
import { GAME_SPEED } from './constants/game.js';
import { StartScene } from './scenes/StartScene.js';
import { ContextHandler } from './engine/ContextHandler.js';
import { Network } from './network/Network.js';

export class StreetFighterGame {
	context = getContext();

	frameTime = {
		secondsPassed: 0,
		previous: 0,
	};

	timeStarted = 0;
	_lastFrameTime = 0;
	sceneStarted = false;
	nextScene = undefined;
	nextMode = null;

	// PvP state
	network = null;
	pvpActive = false;

	contextHandler = new ContextHandler(this.context);

	// DOM elements
	lobbyOverlay = document.getElementById('lobby-overlay');
	resultOverlay = document.getElementById('result-overlay');
	pvpIndicator = document.getElementById('pvp-indicator');

	changeScene = (SceneClass, mode) => {
		this.nextMode = mode || null;

		if (mode === 'pvp') {
			// PvP: show lobby overlay and connect
			this.showLobby();
			return;
		}

		// Practice or normal transition
		this.contextHandler.startDimDown();
		this.sceneStarted = false;
		this.nextScene = SceneClass;
	};

	startScene = (SceneClass, pvpConfig) => {
		this.contextHandler.startGlowUp();
		if (pvpConfig) {
			this.scene = new SceneClass(this.changeScene, pvpConfig);
		} else {
			this.scene = new SceneClass(this.changeScene);
		}
		this.sceneStarted = true;
	};

	constructor() {
		// Check server-injected flag first, then URL params as fallback
		const autostart = window.__autostart || new URLSearchParams(window.location.search).get('autostart');

		if (autostart === 'practice') {
			// Skip start screen, go directly to battle
			this.startScene(BattleScene);
		} else {
			this.startScene(StartScene);
		}
		this.initLobbyEvents();
		this.initResultEvents();
		this.updatePvPIndicator('PVP: READY', 'warning');

		// Embedded PvP: skip start screen, go directly to lobby + auto-join
		if (autostart === 'pvp' && (window.__sfMatchId || window.__sfRoomCode)) {
			this.showLobby();
		}
	}

	updatePvPIndicator = (text, level = 'warning') => {
		if (!this.pvpIndicator) return;
		this.pvpIndicator.classList.remove('hidden', 'connected', 'warning', 'error');
		this.pvpIndicator.classList.add(level);
		this.pvpIndicator.textContent = text;
	};

	// ============================================
	// LOBBY
	// ============================================
	showLobby = () => {
		this.lobbyOverlay.classList.remove('hidden');
		document.querySelector('main').style.display = 'none';
		this.connectAndSetupLobby();
	};

	hideLobby = () => {
		this.lobbyOverlay.classList.add('hidden');
		document.querySelector('main').style.display = '';
	};

	connectAndSetupLobby = async () => {
		const statusEl = document.getElementById('lobby-status');
		const roomCodeDisplay = document.getElementById('room-code-display');
		const roomCodeEl = document.getElementById('room-code');
		const autoRoomCode = (window.__sfRoomCode || '').toString().trim().toUpperCase();
		const autoMatchId = (window.__sfMatchId || '').toString().trim();

		// Reset lobby UI
		roomCodeDisplay.classList.add('hidden');
		document.getElementById('join-section').classList.remove('hidden');
		document.getElementById('btn-create').classList.remove('hidden');
		statusEl.textContent = '';

		if (!this.network) {
			this.network = new Network();
		}

		if (!this.network.connected) {
			try {
				statusEl.textContent = 'Connecting...';
				this.updatePvPIndicator('PVP: CONNECTING...', 'warning');
				await this.network.connect();
				statusEl.textContent = 'Connected! Create or join a room.';
				this.updatePvPIndicator('PVP: CONNECTED', 'connected');
			} catch {
				statusEl.textContent = 'Connection failed. Try again.';
				this.updatePvPIndicator('PVP: CONNECTION FAILED', 'error');
				return;
			}
		} else {
			statusEl.textContent = 'Create or join a room.';
			this.updatePvPIndicator('PVP: CONNECTED', 'connected');
		}

		// Room created
		this.network.on('room_created', (msg) => {
			roomCodeDisplay.classList.remove('hidden');
			roomCodeEl.textContent = msg.code;
			statusEl.textContent = 'Waiting for opponent...';
			document.getElementById('join-section').classList.add('hidden');
			document.getElementById('btn-create').classList.add('hidden');
			this.updatePvPIndicator(`PVP: ROOM ${msg.code} (P1)`, 'connected');
		});

		// Opponent joined (we are player 1)
		this.network.on('opponent_joined', () => {
			statusEl.textContent = 'Opponent found! Starting...';
			this.updatePvPIndicator('PVP: OPPONENT FOUND', 'connected');
		});

		// We joined a room (we are player 2)
		this.network.on('room_joined', () => {
			statusEl.textContent = 'Joined! Starting...';
			this.updatePvPIndicator('PVP: JOINED ROOM (P2)', 'connected');
		});

		// Countdown
		this.network.on('countdown', (msg) => {
			statusEl.textContent = `Starting in ${msg.c}...`;
			this.updatePvPIndicator(`PVP: STARTING IN ${msg.c}`, 'connected');
		});

		// Game start
		this.network.on('game_start', (msg) => {
			const localPlayerId = msg.slot === 'p1' ? 0 : 1;
			const role = localPlayerId === 0 ? 'P1' : 'P2';
			this.hideLobby();
			this.pvpActive = true;
			this.updatePvPIndicator(`PVP LIVE: ${role}`, 'connected');

			// Clear lobby event listeners
			this.network.off('room_created', null);
			this.network.off('opponent_joined', null);
			this.network.off('room_joined', null);
			this.network.off('countdown', null);
			this.network.off('game_start', null);
			this.network.off('error', null);

			// Start battle scene with PvP config
			this.contextHandler.startDimDown();
			this.sceneStarted = false;
			this.nextScene = BattleScene;
			this._pvpConfig = {
				network: this.network,
				localPlayerId,
				onGameEnd: this.handlePvPGameEnd,
			};
		});

		this.network.on('error', (msg) => {
			statusEl.textContent = msg.msg || 'Error occurred';
			this.updatePvPIndicator(`PVP ERROR: ${msg.msg || 'UNKNOWN'}`, 'error');
		});

		this.network.on('disconnected', () => {
			this.updatePvPIndicator('PVP: DISCONNECTED', 'error');
		});

		// Embedded mode: auto-join the match room so both PVP entrants
		// land in the same live networked fight without manual room steps.
		if (autoMatchId) {
			document.getElementById('join-section').classList.add('hidden');
			document.getElementById('btn-create').classList.add('hidden');
			statusEl.textContent = 'Joining match room...';
			this.updatePvPIndicator('PVP: JOINING MATCH ROOM', 'warning');
			this.network.joinMatch(autoMatchId);
			return;
		}

		if (autoRoomCode) {
			document.getElementById('join-section').classList.add('hidden');
			document.getElementById('btn-create').classList.add('hidden');
			roomCodeDisplay.classList.remove('hidden');
			roomCodeEl.textContent = autoRoomCode;
			statusEl.textContent = 'Joining room...';
			this.updatePvPIndicator('PVP: JOINING ROOM', 'warning');
			this.network.createRoom(autoRoomCode);
			return;
		}
	};

	initLobbyEvents = () => {
		// Create room
		document.getElementById('btn-create').addEventListener('click', () => {
			if (this.network && this.network.connected) {
				this.network.createRoom();
			}
		});

		// Join room
		document.getElementById('btn-join').addEventListener('click', () => {
			const code = document.getElementById('input-room-code').value.trim().toUpperCase();
			if (code.length !== 4) {
				document.getElementById('lobby-status').textContent = 'Enter a 4-character code';
				return;
			}
			if (this.network && this.network.connected) {
				this.network.joinRoom(code);
			}
		});

		// Back button
		document.getElementById('btn-lobby-back').addEventListener('click', () => {
			this.hideLobby();
			if (this.network) {
				this.network.disconnect();
				this.network = null;
			}
			this.pvpActive = false;
			this.updatePvPIndicator('PVP: READY', 'warning');
			// Reset to start scene
			this.timeStarted = 0;
			this.startScene(StartScene);
		});
	};

	// ============================================
	// PVP RESULT
	// ============================================
	handlePvPGameEnd = (result) => {
		this.pvpActive = false;

		// In embedded mode, send result to parent frame and let betting page handle it
		if (window.__autostart === 'pvp') {
			try {
				window.parent.postMessage({
					type: 'sfPvPResult',
					result: result, // 'win', 'lose', or 'disconnect'
				}, '*');
			} catch (_e) {}

			// Disconnect network
			if (this.network) {
				this.network.disconnect();
				this.network = null;
			}
			return;
		}

		// Non-embedded: show result overlay
		const resultText = document.getElementById('pvp-result-text');
		if (result === 'win') {
			resultText.textContent = 'YOU WIN!';
			resultText.style.color = '#50E661';
		} else if (result === 'lose') {
			resultText.textContent = 'YOU LOSE';
			resultText.style.color = '#DE0000';
		} else if (result === 'disconnect') {
			resultText.textContent = 'OPPONENT LEFT';
			resultText.style.color = '#FFD700';
		}
		this.resultOverlay.classList.remove('hidden');
		document.querySelector('main').style.display = 'none';
	};

	initResultEvents = () => {
		// Main menu (non-embedded only)
		document.getElementById('btn-main-menu').addEventListener('click', () => {
			this.resultOverlay.classList.add('hidden');
			document.querySelector('main').style.display = '';
			if (this.network) {
				this.network.disconnect();
				this.network = null;
			}
			this.pvpActive = false;
			this.updatePvPIndicator('PVP: READY', 'warning');
			this.timeStarted = 0;
			this.startScene(StartScene);
		});
	};

	// ============================================
	// GAME LOOP
	// ============================================
	updateScenes = () => {
		this.scene.draw(this.context);
		if (this.contextHandler.dimDown) return;
		if (!this.sceneStarted) {
			this.startScene(this.nextScene, this._pvpConfig || null);
			this._pvpConfig = null;
		}
		this.scene.update(this.frameTime);
	};

	frame = (time) => {
		window.requestAnimationFrame(this._boundFrame);

		if (this.timeStarted === 0) {
			this.timeStarted = time;
		}

		// Cap at 60fps — skip rendering on high-refresh displays (120Hz/144Hz)
		if (time - this._lastFrameTime < 15) return;
		this._lastFrameTime = time;

		time -= this.timeStarted;
		time = time * GAME_SPEED;

		this.frameTime.secondsPassed = (time - this.frameTime.previous) / 1000;
		this.frameTime.previous = time;
		updateGamePads();
		this.contextHandler.update(this.frameTime);
		// Only apply expensive CSS filter when brightness/contrast are not default (1.0)
		// Canvas filters force software rendering and kill performance when set every frame
		if (this.contextHandler.brightness !== 1 || this.contextHandler.contrast !== 1) {
			this.context.filter = `brightness(${this.contextHandler.brightness}) contrast(${this.contextHandler.contrast})`;
		} else if (this.context.filter !== 'none') {
			this.context.filter = 'none';
		}
		this.updateScenes();
	};

	start() {
		if (!this._boundFrame) this._boundFrame = this.frame.bind(this);
		registerKeyboardEvents();
		registerGamepadEvents();
		window.requestAnimationFrame(this._boundFrame);
	}
}
