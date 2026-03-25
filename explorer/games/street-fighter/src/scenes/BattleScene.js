import {
	SCENE_WIDTH,
	STAGE_MID_POINT,
	STAGE_PADDING,
} from '../constants/Stage.js';
import {
	FighterAttackBaseData,
	FighterAttackStrength,
	FighterId,
	FighterState,
	FighterStruckDelay,
} from '../constants/fighter.js';
import { FRAME_TIME, GAME_SPEED } from '../constants/game.js';
import { Camera } from '../engine/Camera.js';
import { EntityList } from '../engine/EntityList.js';
import {
	setPvPMode,
	getLocalHeldControls,
	updateRemoteInputs,
} from '../engine/InputHandler.js';
import { Ken, Ryu } from '../entitites/fighters/index.js';
import {
	HeavyHitSplash,
	LightHitSplash,
	MediumHitSplash,
	Shadow,
} from '../entitites/fighters/shared/index.js';
import { Fireball } from '../entitites/fighters/special/Fireball.js';
import { FpsCounter } from '../entitites/overlays/FpsCounter.js';
import { StatusBar } from '../entitites/overlays/StatusBar.js';
import { KenStage } from '../entitites/stage/KenStage.js';
import { gameState, resetGameState } from '../states/gameState.js';
import { StartScene } from './StartScene.js';

export class BattleScene {
	image = document.getElementById('Winner');
	fighters = [];
	camera = undefined;
	shadows = [];
	FighterDrawOrder = [0, 1];
	hurtTimer = 0;
	battleEnded = false;
	winnerId = undefined;

	// PvP state
	network = null;
	localPlayerId = 0;
	isPvP = false;
	isHost = false; // P1 is authoritative host
	healthSyncTimer = 0;
	inputSendTimer = 0;
	_remotePos = [null, null]; // latest authoritative positions from host

	constructor(changeScene, pvpConfig) {
		this.changeScene = changeScene;
		this.stage = new KenStage();
		this.entities = new EntityList();
		this.overlays = [
			new StatusBar(this.fighters, this.onTimeEnd),
			new FpsCounter(),
		];
		resetGameState();

		if (pvpConfig) {
			this.isPvP = true;
			this.network = pvpConfig.network;
			this.localPlayerId = pvpConfig.localPlayerId;
			this.isHost = this.localPlayerId === 0; // P1 is host/authority
			this.onPvPEnd = pvpConfig.onGameEnd;
			setPvPMode(true, this.localPlayerId);

			// Listen for remote inputs (P2 also reads piggybacked positions+states from P1)
			this._onRemoteInput = (msg) => {
				updateRemoteInputs(msg.h || []);
				if (!this.isHost && msg.p) {
					for (let fi = 0; fi < 2; fi++) {
						if (msg.p[fi]) this._remotePos[fi] = msg.p[fi];
					}
				}
				// P2: apply remote fighter state from host's input messages
				if (!this.isHost && Array.isArray(msg.st) && this.fighters.length === 2) {
					const remoteIdx = 1 - this.localPlayerId;
					const newSt = msg.st[remoteIdx];
					if (newSt && this.fighters[remoteIdx].currentState !== newSt) {
						this._forceState(this.fighters[remoteIdx], newSt);
					}
				}
			};
			this.network.on('ri', this._onRemoteInput);

			// P2 listens for authoritative health sync from P1
			if (!this.isHost) {
				this._onHealthSync = (msg) => {
					if (Array.isArray(msg.hp) && msg.hp.length === 2) {
						gameState.fighters[0].hitPoints = msg.hp[0];
						gameState.fighters[1].hitPoints = msg.hp[1];
					}
					if (Array.isArray(msg.pos) && msg.pos.length === 2) {
						for (var fi = 0; fi < 2; fi++) {
							if (msg.pos[fi] && typeof msg.pos[fi].x === 'number') {
								this._remotePos[fi] = msg.pos[fi];
							}
						}
					}
					// Apply authoritative animation states from host
					if (Array.isArray(msg.st) && msg.st.length === 2 && this.fighters.length === 2) {
						const remoteIdx = 1 - this.localPlayerId;
						const newSt = msg.st[remoteIdx];
						if (newSt && this.fighters[remoteIdx].currentState !== newSt) {
							this._forceState(this.fighters[remoteIdx], newSt);
						}
					}
				};
				this.network.on('hs', this._onHealthSync);
			}

			// P2 listens for authoritative game result from P1
			if (!this.isHost) {
				this._onGameResult = (msg) => {
					if (this.battleEnded) return;
					const w = typeof msg.winner === 'number' ? msg.winner : -1;
					if (w === 0 || w === 1) {
						this.winnerId = w;
						this.battleEnded = true;
						this.fighters[w].victory = true;
						this.fighters[1 - w].changeState(FighterState.KO, { previous: performance.now() });
						gameState.fighters[1 - w].hitPoints = 0;
						this.goToStartScene();
					}
				};
				this.network.on('gr', this._onGameResult);
			}

			// Listen for disconnect
			this._onDisconnect = () => {
				if (!this.battleEnded) {
					this.battleEnded = true;
					this.cleanup();
					if (this.onPvPEnd) this.onPvPEnd('disconnect');
				}
			};
			this.network.on('opponent_disconnected', this._onDisconnect);
		}

		this.startRound();

		// In autostart/embedded mode, browser blocks audio until user interaction.
		// Resume music on first keypress or click.
		if (window.__autostart && this.stage.backgroundMusic) {
			const resumeAudio = () => {
				this.stage.backgroundMusic.play().catch(() => {});
				window.removeEventListener('keydown', resumeAudio);
				window.removeEventListener('click', resumeAudio);
				window.removeEventListener('mousedown', resumeAudio);
			};
			window.addEventListener('keydown', resumeAudio, { once: false });
			window.addEventListener('click', resumeAudio, { once: false });
			window.addEventListener('mousedown', resumeAudio, { once: false });
		}
	}

	getFighterClass = (id) => {
		switch (id) {
			case FighterId.KEN:
				return Ken;
			case FighterId.RYU:
				return Ryu;
			default:
				return new Error('Invalid Fighter Id');
		}
	};

	getFighterEntitiy = (id, index) => {
		const FighterClass = this.getFighterClass(id);
		return new FighterClass(index, this.handleAttackHit, this.entities);
	};

	getFighterEntities = () => {
		const fighterEntities = gameState.fighters.map(({ id }, index) => {
			const fighterEntity = this.getFighterEntitiy(id, index);
			gameState.fighters[index].instance = fighterEntity;
			return fighterEntity;
		});

		fighterEntities[0].opponent = fighterEntities[1];
		fighterEntities[1].opponent = fighterEntities[0];

		// PvP non-host: skip hit detection for the remote fighter to prevent phantom hits
		// Only the host runs authoritative hit detection; P2 only detects its own attacks
		if (this.isPvP && !this.isHost) {
			const remoteIdx = 1 - this.localPlayerId; // remote fighter index
			fighterEntities[remoteIdx].skipHitDetection = true;
		}

		return fighterEntities;
	};

	updateFighters = (time, context) => {
		for (let i = 0; i < this.fighters.length; i++) {
			const fighter = this.fighters[i];
			if (this.hurtTimer > time.previous) {
				fighter.updateHurtShake(time, this.hurtTimer);
			} else fighter.update(time, this.camera);
		}
	};

	getHitSplashClass = (strength) => {
		switch (strength) {
			case FighterAttackStrength.LIGHT:
				return LightHitSplash;
			case FighterAttackStrength.MEDIUM:
				return MediumHitSplash;
			case FighterAttackStrength.HEAVY:
				return HeavyHitSplash;
			default:
				return new Error('Invalid Strength Splash requested');
		}
	};

	handleAttackHit = (time, playerId, opponentId, position, strength) => {
		this.FighterDrawOrder = [opponentId, playerId];

		// In PvP, only the host (P1) modifies HP — P2 gets HP via health sync
		if (!this.isPvP || this.isHost) {
			gameState.fighters[playerId].score += FighterAttackBaseData[strength].score;
			gameState.fighters[opponentId].hitPoints -=
				FighterAttackBaseData[strength].damage;
		}

		const HitSplashClass = this.getHitSplashClass(strength);

		// Only host triggers KO state change
		if ((!this.isPvP || this.isHost) && gameState.fighters[opponentId].hitPoints <= 0) {
			this.fighters[opponentId].changeState(FighterState.KO, time);
		}

		this.fighters[opponentId].direction =
			this.fighters[playerId].direction * -1;

		position &&
			this.entities.add(HitSplashClass, position.x, position.y, playerId);

		this.hurtTimer = time.previous + FighterStruckDelay * FRAME_TIME;
	};

	updateShadows = (time) => {
		for (let i = 0; i < this.shadows.length; i++) this.shadows[i].update(time);
	};

	startRound = () => {
		this.fighters = this.getFighterEntities();
		this.camera = new Camera(
			STAGE_PADDING + STAGE_MID_POINT - SCENE_WIDTH / 2,
			16,
			this.fighters
		);

		this.shadows = this.fighters.map((fighter) => new Shadow(fighter));
	};

	goToStartScene = () => {
		// PvP host: send authoritative game result to P2
		if (this.isPvP && this.isHost && this.network && this.winnerId !== undefined) {
			this.network.sendGameResult(this.winnerId);
		}

		setTimeout(() => {
			if (this.isPvP) {
				const result = this.winnerId === this.localPlayerId ? 'win' : 'lose';
				this.cleanup();
				if (this.onPvPEnd) this.onPvPEnd(result);
			} else if (!window.__autostart) {
				this.changeScene(StartScene);
			}
			// else: non-PvP embedded mode, stay on KO screen
		}, 3000);
	};

	drawWinnerText = (context, id) => {
		context.drawImage(this.image, 0, 11 * id, 70, 9, 120, 60, 140, 30);
	};

	onTimeEnd = (time) => {
		if (gameState.fighters[0].hitPoints >= gameState.fighters[1].hitPoints) {
			this.fighters[0].victory = true;
			this.fighters[1].changeState(FighterState.KO, time);
			this.winnerId = 0;
		} else {
			this.fighters[1].victory = true;
			this.fighters[0].changeState(FighterState.KO, time);
			this.winnerId = 1;
		}
		this.goToStartScene();
	};

	updateOverlays = (time) => {
		for (let i = 0; i < this.overlays.length; i++) this.overlays[i].update(time);
	};

	updateFighterHP = (time) => {
		// In PvP, only the host (P1) determines KO. P2 waits for authoritative result.
		if (this.isPvP && !this.isHost) return;

		for (let index = 0; index < gameState.fighters.length; index++) {
			const fighter = gameState.fighters[index];
			if (fighter.hitPoints <= 0 && !this.battleEnded) {
				this.fighters[index].opponent.victory = true;
				this.winnerId = 1 - index;
				this.battleEnded = true;
				this.goToStartScene();
			}
		}
	};

	update = (time) => {
		// PvP: send local inputs at ~30hz (every 33ms) for tighter sync
		if (this.isPvP && this.network) {
			if (time.previous - this.inputSendTimer > 33) {
				this.inputSendTimer = time.previous;
				if (this.isHost && this.fighters.length === 2) {
					// Host piggybacks positions + states onto input messages for 30Hz sync
					this.network.send({
						t: 'input',
						h: getLocalHeldControls(),
						p: [
							{ x: this.fighters[0].position.x, y: this.fighters[0].position.y },
							{ x: this.fighters[1].position.x, y: this.fighters[1].position.y },
						],
						st: [this.fighters[0].currentState, this.fighters[1].currentState],
					});
				} else {
					this.network.sendInput(getLocalHeldControls());
				}
			}
		}

		// PvP host: send health + position sync every ~100ms
		if (this.isPvP && this.isHost && this.network && !this.battleEnded) {
			if (time.previous - this.healthSyncTimer > 100) {
				this.healthSyncTimer = time.previous;
				this.network.send({
					t: 'hs',
					hp: [gameState.fighters[0].hitPoints, gameState.fighters[1].hitPoints],
					pos: [
						{ x: this.fighters[0].position.x, y: this.fighters[0].position.y },
						{ x: this.fighters[1].position.x, y: this.fighters[1].position.y },
					],
					st: [this.fighters[0].currentState, this.fighters[1].currentState],
				});
			}
		}

		this.updateFighters(time);

		// P2: override REMOTE fighter position with P1's authoritative data
		// Local fighter keeps its own simulation (responsive to local input)
		if (this.isPvP && !this.isHost && this.fighters.length === 2) {
			const remoteIdx = 1 - this.localPlayerId;
			const rp = this._remotePos[remoteIdx];
			if (rp) {
				this.fighters[remoteIdx].position.x = rp.x;
				this.fighters[remoteIdx].position.y = rp.y;
			}
		}

		this.updateShadows(time);
		this.stage.update(time);
		this.entities.update(time, this.camera);
		this.camera.update(time);
		this.updateOverlays(time);
		this.updateFighterHP(time);
	};

	drawFighters(context) {
		for (let i = 0; i < this.FighterDrawOrder.length; i++) {
			this.fighters[this.FighterDrawOrder[i]].draw(context, this.camera);
		}
	}

	drawShadows(context) {
		for (let i = 0; i < this.shadows.length; i++) this.shadows[i].draw(context, this.camera);
	}

	drawOverlays(context) {
		for (let i = 0; i < this.overlays.length; i++) this.overlays[i].draw(context, this.camera);
		if (this.winnerId !== undefined) {
			this.drawWinnerText(context, this.winnerId);
		}
	}

	draw = (context) => {
		this.stage.drawBackground(context, this.camera);
		this.drawShadows(context);
		this.drawFighters(context);
		this.entities.draw(context, this.camera);
		this.stage.drawForeground(context, this.camera);
		this.drawOverlays(context);
	};

	// Force a fighter into a specific animation state, bypassing validFrom checks.
	// Used by P2 to sync remote fighter state from P1's authoritative data.
	_forceState(fighter, newState) {
		if (!fighter.states[newState]) return;
		fighter.currentState = newState;
		fighter.animationFrame = 0;
	}

	cleanup = () => {
		if (this.isPvP) {
			setPvPMode(false);
			if (this.network) {
				if (this._onRemoteInput) this.network.off('ri', this._onRemoteInput);
				if (this._onDisconnect) this.network.off('opponent_disconnected', this._onDisconnect);
				if (this._onHealthSync) this.network.off('hs', this._onHealthSync);
				if (this._onGameResult) this.network.off('gr', this._onGameResult);
			}
		}
	};
}
