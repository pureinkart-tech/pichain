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
	}

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
				await this.network.connect();
				statusEl.textContent = 'Connected! Create or join a room.';
			} catch {
				statusEl.textContent = 'Connection failed. Try again.';
				return;
			}
		} else {
			statusEl.textContent = 'Create or join a room.';
		}

		// Room created
		this.network.on('room_created', (msg) => {
			roomCodeDisplay.classList.remove('hidden');
			roomCodeEl.textContent = msg.code;
			statusEl.textContent = 'Waiting for opponent...';
			document.getElementById('join-section').classList.add('hidden');
			document.getElementById('btn-create').classList.add('hidden');
		});

		// Opponent joined (we are player 1)
		this.network.on('opponent_joined', () => {
			statusEl.textContent = 'Opponent found! Starting...';
		});

		// We joined a room (we are player 2)
		this.network.on('room_joined', () => {
			statusEl.textContent = 'Joined! Starting...';
		});

		// Countdown
		this.network.on('countdown', (msg) => {
			statusEl.textContent = `Starting in ${msg.c}...`;
		});

		// Game start
		this.network.on('game_start', (msg) => {
			const localPlayerId = msg.slot === 'p1' ? 0 : 1;
			this.hideLobby();
			this.pvpActive = true;

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
		});
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
		const resultText = document.getElementById('pvp-result-text');
		const rematchStatus = document.getElementById('rematch-status');
		rematchStatus.textContent = '';

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
		// Rematch
		document.getElementById('btn-rematch').addEventListener('click', () => {
			if (!this.network || !this.network.connected) {
				document.getElementById('rematch-status').textContent = 'Connection lost';
				return;
			}
			this.network.requestRematch();
			document.getElementById('rematch-status').textContent = 'Waiting for opponent...';

			this.network.on('rematch_request', () => {
				// Both players want rematch, accept automatically
				this.network.requestRematch();
			});

			this.network.on('rematch_accepted', () => {
				document.getElementById('rematch-status').textContent = 'Rematch starting!';
			});

			this.network.on('countdown', (msg) => {
				document.getElementById('rematch-status').textContent = `Starting in ${msg.c}...`;
			});

			this.network.on('game_start', (msg) => {
				const localPlayerId = msg.slot === 'p1' ? 0 : 1;
				this.resultOverlay.classList.add('hidden');
				document.querySelector('main').style.display = '';

				this.network.off('rematch_request', null);
				this.network.off('rematch_accepted', null);
				this.network.off('countdown', null);
				this.network.off('game_start', null);

				this.pvpActive = true;
				this.contextHandler.startDimDown();
				this.sceneStarted = false;
				this.nextScene = BattleScene;
				this._pvpConfig = {
					network: this.network,
					localPlayerId,
					onGameEnd: this.handlePvPGameEnd,
				};
			});
		});

		// Main menu
		document.getElementById('btn-main-menu').addEventListener('click', () => {
			this.resultOverlay.classList.add('hidden');
			document.querySelector('main').style.display = '';
			if (this.network) {
				this.network.disconnect();
				this.network = null;
			}
			this.pvpActive = false;
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
		window.requestAnimationFrame(this.frame.bind(this));

		if (this.timeStarted === 0) {
			this.timeStarted = time;
		}
		time -= this.timeStarted;
		time = time * GAME_SPEED;

		this.frameTime = {
			secondsPassed: (time - this.frameTime.previous) / 1000,
			previous: time,
		};
		updateGamePads();
		this.contextHandler.update(this.frameTime);
		this.context.filter = `brightness(${this.contextHandler.brightness}) contrast(${this.contextHandler.contrast})`;
		this.updateScenes();
	};

	start() {
		registerKeyboardEvents();
		registerGamepadEvents();
		window.requestAnimationFrame(this.frame.bind(this));
	}
}
