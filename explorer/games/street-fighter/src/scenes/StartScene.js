import { SCENE_WIDTH } from '../constants/Stage.js';
import { LOGO_FLASH_DELAY } from '../constants/battle.js';
import { BattleScene } from './BattleScene.js';

export class StartScene {
	image = document.getElementById('Controls');
	logoImg = document.getElementById('Logo');

	text = 'STREET FIGHTER 2 - CLICK TO START';
	repeatTime = 3;
	position = 10;
	logoFlash = false;
	flashTimer = 0;
	brightness = 0;
	contrast = 3;
	sceneEnded = false;

	// Mode selection state
	showModeSelect = false;
	selectedMode = null; // 'pvp' or 'practice'
	hoverMode = null;

	// Button definitions (canvas coordinates)
	pvpButton = { x: 110, y: 120, w: 160, h: 24 };
	practiceButton = { x: 110, y: 152, w: 160, h: 24 };

	endStartScene = () => {
		if (!this.showModeSelect) {
			this.showModeSelect = true;
			return;
		}
	};

	handleModeClick = (event) => {
		if (!this.showModeSelect) return;

		const canvas = document.querySelector('canvas');
		const rect = canvas.getBoundingClientRect();
		const scaleX = 382 / rect.width;
		const scaleY = 224 / rect.height;
		const x = (event.clientX - rect.left) * scaleX;
		const y = (event.clientY - rect.top) * scaleY;

		if (this.isInButton(x, y, this.pvpButton)) {
			this.selectedMode = 'pvp';
			window.removeEventListener('click', this.endStartScene);
			window.removeEventListener('click', this.handleModeClick);
			window.removeEventListener('mousemove', this.handleMouseMove);
			this.changeScene(BattleScene, 'pvp');
		} else if (this.isInButton(x, y, this.practiceButton)) {
			this.selectedMode = 'practice';
			window.removeEventListener('click', this.endStartScene);
			window.removeEventListener('click', this.handleModeClick);
			window.removeEventListener('mousemove', this.handleMouseMove);
			this.changeScene(BattleScene, 'practice');
		}
	};

	handleMouseMove = (event) => {
		if (!this.showModeSelect) return;
		const canvas = document.querySelector('canvas');
		const rect = canvas.getBoundingClientRect();
		const scaleX = 382 / rect.width;
		const scaleY = 224 / rect.height;
		const x = (event.clientX - rect.left) * scaleX;
		const y = (event.clientY - rect.top) * scaleY;

		if (this.isInButton(x, y, this.pvpButton)) {
			this.hoverMode = 'pvp';
		} else if (this.isInButton(x, y, this.practiceButton)) {
			this.hoverMode = 'practice';
		} else {
			this.hoverMode = null;
		}
	};

	isInButton = (x, y, btn) => {
		return x >= btn.x && x <= btn.x + btn.w && y >= btn.y && y <= btn.y + btn.h;
	};

	constructor(changeScene) {
		this.changeScene = changeScene;
		window.removeEventListener('click', this.endStartScene);
		window.addEventListener('click', this.endStartScene);
		window.addEventListener('click', this.handleModeClick);
		window.addEventListener('mousemove', this.handleMouseMove);
	}

	updateLogo = (time) => {
		if (this.flashTimer > time.previous) return;
		this.flashTimer = time.previous + LOGO_FLASH_DELAY[Number(!this.logoFlash)];
		this.logoFlash = !this.logoFlash;
	};

	updateTextPosition = (time) => {
		this.position -= time.secondsPassed * 100;
	};

	update = (time) => {
		this.updateLogo(time);
		if (!this.showModeSelect) this.updateTextPosition(time);
	};

	drawText = (context) => {
		context.fillStyle = 'white';
		context.font = '12px Arial';
		const textWidth = context.measureText(this.text).width;

		for (let i = 0; i < this.repeatTime; i++) {
			context.fillText(this.text, this.position + i * (textWidth + 30), 18);
		}

		if (this.position < (-textWidth + -30) * this.repeatTime) {
			this.position = SCENE_WIDTH;
		}
	};

	drawLogo = (context) => {
		if (this.logoFlash) {
			context.fillStyle = 'black';
			context.fillRect(112, 22, 170, 80);
			return;
		}
		context.drawImage(
			this.logoImg,
			0,
			0,
			this.logoImg.width,
			this.logoImg.height,
			112,
			22,
			170,
			80
		);
	};

	drawButton = (context, btn, text, isHovered) => {
		// Button background
		context.fillStyle = isHovered ? '#DE0000' : '#333';
		context.fillRect(btn.x, btn.y, btn.w, btn.h);

		// Button border
		context.strokeStyle = isHovered ? '#FF4444' : '#666';
		context.lineWidth = 1;
		context.strokeRect(btn.x, btn.y, btn.w, btn.h);

		// Button text
		context.fillStyle = isHovered ? 'white' : '#CCC';
		context.font = '8px Arial';
		const textWidth = context.measureText(text).width;
		context.fillText(
			text,
			btn.x + (btn.w - textWidth) / 2,
			btn.y + btn.h / 2 + 3
		);
	};

	drawModeSelect = (context) => {
		// Semi-transparent background
		context.fillStyle = 'rgba(0, 0, 0, 0.7)';
		context.fillRect(90, 105, 200, 85);

		// Title
		context.fillStyle = '#FFD700';
		context.font = '8px Arial';
		context.fillText('SELECT MODE', 150, 116);

		// Buttons
		this.drawButton(context, this.pvpButton, 'PvP FIGHT', this.hoverMode === 'pvp');
		this.drawButton(context, this.practiceButton, 'PRACTICE', this.hoverMode === 'practice');
	};

	draw = (context) => {
		context.drawImage(this.image, 0, 0);
		this.drawLogo(context);
		if (this.showModeSelect) {
			this.drawModeSelect(context);
		} else {
			this.drawText(context);
		}
	};
}
