import { Control } from '../constants/controls.js';

const PSControls = {
	X: 0,
	O: 1,
	SQ: 2,
	TR: 3,
	L1: 4,
	R1: 5,
	L2: 6,
	R2: 7,
	L3: 10,
	R3: 11,
	UP: 12,
	DOWN: 13,
	LEFT: 14,
	RIGHT: 15,
};

// Both players use same controls (PvP is on separate machines)
// Arrow keys = movement, Space = jump, 1-6 = attacks
// Hadouken combo: Down, Down-Forward, Forward + 1/2/3
const pvpKeyboard = {
	[Control.LEFT]: 'ArrowLeft',
	[Control.RIGHT]: 'ArrowRight',
	[Control.UP]: 'ArrowUp',
	[Control.DOWN]: 'ArrowDown',
	[Control.LIGHT_PUNCH]: 'Digit1',
	[Control.MEDIUM_PUNCH]: 'Digit2',
	[Control.HEAVY_PUNCH]: 'Digit3',
	[Control.LIGHT_KICK]: 'Digit4',
	[Control.MEDIUM_KICK]: 'Digit5',
	[Control.HEAVY_KICK]: 'Digit6',
};

const pvpGamepad = {
	[Control.LEFT]: PSControls.LEFT,
	[Control.RIGHT]: PSControls.RIGHT,
	[Control.UP]: PSControls.UP,
	[Control.DOWN]: PSControls.DOWN,
	[Control.LIGHT_PUNCH]: PSControls.X,
	[Control.MEDIUM_PUNCH]: PSControls.SQ,
	[Control.HEAVY_PUNCH]: PSControls.L1,
	[Control.LIGHT_KICK]: PSControls.O,
	[Control.MEDIUM_KICK]: PSControls.TR,
	[Control.HEAVY_KICK]: PSControls.R1,
};

export const controls = [
	{ gamepad: pvpGamepad, keyboard: pvpKeyboard },
	{ gamepad: pvpGamepad, keyboard: pvpKeyboard },
];

// Space bar also triggers jump (alias for ArrowUp)
export const JUMP_ALIAS = 'Space';

export const CONTROLLER_DEADZONE = 0.4;
